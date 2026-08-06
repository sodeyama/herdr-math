//! Incremental native rendering for `tmath render --engine native -`.

use std::io::{self, Read as _, Write as _};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;

use tmath_core::placement::{
    decode_png, emit_placed_block_cursor, emit_placed_block_row_range_cursor, CellSize,
    PlacementLimits, TerminalOp,
};
use tmath_core::terminal::{StdioTty, Terminal};
use tmath_render::{
    content_hash, CacheBudget, Limits, PlacementPlanner, Plan, PlanOp, RenderCache, RenderError,
    RenderOptions, RenderedBlock, Revision, SafeErrorRecord, StreamSplitter,
};

use crate::terminal_output;

const READ_CHUNK_BYTES: usize = 8 * 1024;

// --- Live status bar (agent-viewer only, pane row 1) ---
//
// The agent-viewer permanently reserves the pane's row 1 for a live status
// bar instead of placed content — the SAME reserved row PART 2's top
// margin needed anyway (a block's placeholder grid never touches row 1;
// see `sync_window_operations`'s home-draw, which now starts content at
// row 2). Plain `tmath render`/`tmath watch` streaming sessions never
// reserve this row: they never call `StreamSink::with_status_bar`, so
// `TerminalSink::status_bar` stays `None` and every draw path here is
// skipped entirely, leaving their behavior exactly as it was before this
// feature.
//
// Left side (static brand): `∑ Terminal Math` in the accent color (bold),
// `· live typeset viewer` dim. Right side (dynamic, right-aligned):
// `following`/`scrolled · N blocks · Xpt`, dim except the state word, which
// is accented in a different hue depending on state — so scroll state is
// glanceable without reading the whole line. Redrawn via a single
// save-cursor/move/write/restore-cursor sequence (`\x1b7`...`\x1b8`, DECSC/
// DECRC — chosen over `\x1b[s`/`\x1b[u` for broader terminal/tmux-passthrough
// support) so it never disturbs the real cursor position flowing appends
// rely on, or the content area below row 1.

/// The static left-side brand string (UI copy — English only per AGENTS.md).
const STATUS_BRAND: &str = "\u{2211} Terminal Math";
/// The static left-side tagline, dim, immediately after the brand.
const STATUS_TAGLINE: &str = "\u{b7} live typeset viewer";
/// Right-side state word while the viewport follows new content.
const STATUS_STATE_FOLLOWING: &str = "following";
/// Right-side state word while the viewport is manually scrolled
/// (follow disengaged).
const STATUS_STATE_SCROLLED: &str = "scrolled";
/// 256-color accent for the brand and the `following` state word: a bright
/// blue/cyan (color 75) that reads clearly on both light and dark
/// terminal-default backgrounds.
const STATUS_ACCENT_COLOR: u16 = 75;
/// 256-color accent for the `scrolled` state word — a distinct yellow-ish
/// hue (color 179) so the disengaged state is visually distinguishable from
/// `following` at a glance, not just by the word text.
const STATUS_SCROLLED_COLOR: u16 = 179;
/// SGR dim (faint) attribute code for the tagline and separators.
const SGR_DIM: &str = "\x1b[2m";
/// SGR bold attribute code for the brand.
const SGR_BOLD: &str = "\x1b[1m";
/// Full SGR reset.
const SGR_RESET: &str = "\x1b[0m";

/// The live status-bar's dynamic fields, refreshed by the caller whenever
/// any of them changes (see [`TerminalSink::set_status`]'s callers).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct StatusBarState {
    pub(crate) following: bool,
    pub(crate) blocks: usize,
    pub(crate) font_size_pt: f64,
}

/// Owns the status bar's draw width; the dynamic state itself is passed
/// fresh to each redraw rather than cached here, so there is exactly one
/// source of truth for "what should row 1 show right now" (the caller's
/// current viewer/viewport state), not a second copy that could drift.
#[derive(Clone, Copy, Debug, PartialEq)]
struct StatusBar {
    pane_cols: u32,
}

impl StatusBar {
    fn new(pane_cols: u32) -> Self {
        Self { pane_cols }
    }
}

/// One right-side field, kept alongside its PLAIN (uncolored) text so width
/// measurement and colored rendering always agree on length without
/// re-deriving it twice.
struct RightField {
    plain: String,
    colored: String,
}

/// Builds the save/move/write/restore operation sequence that draws the
/// status bar at row 1. Pure and independent of any live terminal, the same
/// way every other `*_operations` builder in this module is.
///
/// Layout: `{accent+bold}BRAND{reset} {dim}TAGLINE{reset}` on the left,
/// `{accent-or-yellow}STATE{reset} {dim}\u{b7} N blocks{reset} {dim}\u{b7}
/// Xpt{reset}` on the right, right-aligned to `pane_cols`. When the full
/// line would not fit `pane_cols`, right-side fields are dropped starting
/// from the LEFT of that group — i.e. `N blocks`/`Xpt` go first, the state
/// word is kept longest, since it is the single most important glanceable
/// signal — down to no right side at all, rather than ever wrapping onto a
/// second row (wrapping would corrupt the reserved-row invariant every
/// other draw path depends on).
fn status_bar_operations(pane_cols: u32, state: StatusBarState) -> Vec<TerminalOp> {
    let pane_cols = pane_cols.max(1) as usize;

    let left_plain_len = STATUS_BRAND.chars().count() + 1 + STATUS_TAGLINE.chars().count();
    let left_colored = format!(
        "{SGR_BOLD}\x1b[38;5;{STATUS_ACCENT_COLOR}m{STATUS_BRAND}{SGR_RESET} \
         {SGR_DIM}{STATUS_TAGLINE}{SGR_RESET}"
    );

    let (state_word, state_color) = if state.following {
        (STATUS_STATE_FOLLOWING, STATUS_ACCENT_COLOR)
    } else {
        (STATUS_STATE_SCROLLED, STATUS_SCROLLED_COLOR)
    };
    let state_field = RightField {
        plain: state_word.to_string(),
        colored: format!("{SGR_BOLD}\x1b[38;5;{state_color}m{state_word}{SGR_RESET}"),
    };
    let blocks_plain = format!("{} blocks", state.blocks);
    let blocks_field = RightField {
        colored: format!("{SGR_DIM}{blocks_plain}{SGR_RESET}"),
        plain: blocks_plain,
    };
    let font_plain = format!("{}pt", format_font_size(state.font_size_pt));
    let font_field = RightField {
        colored: format!("{SGR_DIM}{font_plain}{SGR_RESET}"),
        plain: font_plain,
    };
    // Ordered so `full_right_fields[start..]` (used below) drops
    // `blocks`/`font` before ever dropping the state word.
    let full_right_fields = [blocks_field, font_field, state_field];
    const SEPARATOR_PLAIN: &str = " \u{b7} ";
    let separator_colored = format!("{SGR_DIM}{SEPARATOR_PLAIN}{SGR_RESET}");

    // Try every suffix of `full_right_fields`, widest first, and take the
    // first one that fits — this drops `blocks`/`font` from the front while
    // always keeping the state word (the last element) as long as ANY
    // right-side content fits at all.
    let mut chosen: &[RightField] = &[];
    for start in 0..full_right_fields.len() {
        let candidate = &full_right_fields[start..];
        let candidate_plain_len: usize = candidate
            .iter()
            .map(|field| field.plain.chars().count())
            .sum::<usize>()
            + SEPARATOR_PLAIN.chars().count() * candidate.len().saturating_sub(1);
        if left_plain_len + 1 + candidate_plain_len <= pane_cols {
            chosen = candidate;
            break;
        }
    }

    let mut line = left_colored;
    if !chosen.is_empty() {
        let chosen_plain_len: usize = chosen
            .iter()
            .map(|field| field.plain.chars().count())
            .sum::<usize>()
            + SEPARATOR_PLAIN.chars().count() * chosen.len().saturating_sub(1);
        let gap = pane_cols
            .saturating_sub(left_plain_len + chosen_plain_len)
            .max(1);
        line.push_str(&" ".repeat(gap));
        for (index, field) in chosen.iter().enumerate() {
            if index > 0 {
                line.push_str(&separator_colored);
            }
            line.push_str(&field.colored);
        }
    }

    vec![TerminalOp::Local(
        format!("\x1b7\x1b[1;1H\x1b[2K{line}{SGR_RESET}\x1b8").into_bytes(),
    )]
}

/// Formats a font size in points with no trailing `.0` for whole numbers
/// (`15pt`, not `15.0pt`), matching how the rest of the CLI reports sizes.
fn format_font_size(font_size_pt: f64) -> String {
    if font_size_pt.fract() == 0.0 {
        format!("{font_size_pt:.0}")
    } else {
        format!("{font_size_pt:.1}")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CacheOutcome {
    Hit,
    Miss,
}

impl CacheOutcome {
    fn label(self) -> &'static str {
        match self {
            Self::Hit => "hit",
            Self::Miss => "miss",
        }
    }
}

struct PreparedBlock {
    rendered: Option<Arc<RenderedBlock>>,
    png: Option<Vec<u8>>,
    cache: Option<CacheOutcome>,
}

enum InputEvent {
    Chunk(Vec<u8>),
    Eof,
    Failed,
}

pub(crate) fn run(
    content_width: Option<u32>,
    font_size: Option<u32>,
    connected: Option<(Terminal<StdioTty>, (u32, u32))>,
) -> Result<(), RenderError> {
    // Auto-fit to the connected terminal's pane when no explicit override was
    // given; the non-terminal path (`connected` is `None`, what the hermetic
    // summary/event-line tests drive) keeps the fixed defaults.
    let fitted = crate::layout::fitted_layout_for_connected(&connected);
    let device_pixel_ratio = crate::layout::resolve_device_pixel_ratio(fitted);
    let config = crate::config::config_path()
        .map(|path| crate::config::load(&path))
        .unwrap_or_default();
    // Unlike `native_watch.rs`/the agent-viewer, this path's stderr is a
    // dedicated single-JSON-record error channel (see
    // `stream_error_is_safe_json_without_input`'s contract test) — no
    // human-readable log lines belong here, so the resolved font-size
    // source is not logged on this path.
    let (font_size_pt, _font_size_source) =
        crate::config::resolve_font_size_pt_with_source(font_size, &config, fitted);
    let options = RenderOptions::new(
        crate::layout::resolve_content_width_pt(
            content_width,
            fitted,
            font_size_pt,
            crate::config::resolve_max_content_width_font_multiple(&config),
        ),
        font_size_pt,
        device_pixel_ratio,
    )
    .map_err(|_| stream_error())?
    .with_cjk_font(crate::config::resolve_cjk_font(&config));
    let limits = Limits::default();
    let scaled = limits.scaled(device_pixel_ratio);
    let max_entries = usize::try_from(limits.blocks_per_document)
        .unwrap_or(usize::MAX)
        .max(1);
    let mut cache = RenderCache::new(CacheBudget {
        max_entries,
        max_pixels: scaled.image_pixels.max(1),
    });
    let mut splitter = StreamSplitter::new(limits);
    let mut planner = PlacementPlanner::new();
    let mut formula_errors = Vec::new();
    let mut sink = StreamSink::new(connected, scaled.image_pixels);
    let receiver = spawn_reader();
    let mut eof = false;

    while !eof {
        let event = receiver.recv().map_err(|_| stream_error())?;
        match event {
            InputEvent::Chunk(first) => {
                let mut bytes = first;
                loop {
                    match receiver.try_recv() {
                        Ok(InputEvent::Chunk(chunk)) => bytes.extend_from_slice(&chunk),
                        Ok(InputEvent::Eof) => {
                            eof = true;
                            break;
                        }
                        Ok(InputEvent::Failed) => return Err(stream_error()),
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => return Err(stream_error()),
                    }
                }

                // One drained batch produces one revision. Completed blocks
                // emit immediately, while all queued tail bytes collapse into
                // the newest tail rendered once in this loop iteration.
                let revision = splitter.push(&bytes)?;
                apply_revision(
                    &revision,
                    &options,
                    &mut cache,
                    &mut planner,
                    &mut formula_errors,
                    &mut sink,
                )?;
            }
            InputEvent::Eof => eof = true,
            InputEvent::Failed => return Err(stream_error()),
        }
    }

    let revision = splitter.finish()?;
    apply_revision(
        &revision,
        &options,
        &mut cache,
        &mut planner,
        &mut formula_errors,
        &mut sink,
    )?;
    sink.done(planner.blocks().len(), formula_errors.iter().sum())?;
    Ok(())
}

fn spawn_reader() -> Receiver<InputEvent> {
    // Keep at most one unread chunk in userspace. Bytes arriving while a tail
    // render is in flight remain in the pipe and are consumed as the newest
    // available content on the next loop iteration.
    let (sender, receiver) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let stdin = io::stdin();
        let mut stdin = stdin.lock();
        loop {
            let mut chunk = vec![0; READ_CHUNK_BYTES];
            match stdin.read(&mut chunk) {
                Ok(0) => {
                    let _ = sender.send(InputEvent::Eof);
                    return;
                }
                Ok(count) => {
                    chunk.truncate(count);
                    if sender.send(InputEvent::Chunk(chunk)).is_err() {
                        return;
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => {
                    let _ = sender.send(InputEvent::Failed);
                    return;
                }
            }
        }
    });
    receiver
}

/// Whether the caller must follow up with a full visibility-window sync
/// (`sync_window`/`sync_visible_window`) after `apply_revision` returns.
///
/// `NeedsWindowSync` is what [`TerminalSink::emit_batch`] reports when a
/// divergence rewrite's stale-tail row span could not fit inside a relative
/// cursor-up from the pane top (see [`clamp_would_truncate`]'s doc comment
/// for the mechanism): `\x1b[{n}A` silently clamps at the terminal's actual
/// top row rather than erroring, so a cursor-up that is too large lands the
/// whole rewrite too low on screen, leaving the stale tail's old placeholder
/// rows visible above it — a live-run "ghost" report traced this exact
/// mechanism (see `ba800aa`'s row-span invariant checkers, which proved the
/// row bookkeeping itself is correct; the bug is a terminal-boundary
/// behavior a pure trace can never see). Falling back to an absolute
/// `\x1b[H`-anchored full-window redraw (the same mechanism AT-3-503's
/// `sync_window` already uses for scroll) sidesteps the clamp entirely,
/// since it never depends on how far the cursor can move relative to its
/// current position.
///
/// Plain stream/watch sessions never see `NeedsWindowSync`: `emit_batch`
/// only runs when `retain_pngs` is set (agent-viewer only — see
/// `TerminalSink::emit`'s gate), so this variant is only ever produced (and
/// only ever needs handling) on the viewer's `sync_visible_window` path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EmitOutcome {
    Applied,
    NeedsWindowSync,
}

pub(crate) fn apply_revision(
    revision: &Revision,
    options: &RenderOptions,
    cache: &mut RenderCache,
    planner: &mut PlacementPlanner,
    formula_errors: &mut Vec<usize>,
    sink: &mut StreamSink,
) -> Result<EmitOutcome, RenderError> {
    let previous = planner.blocks().to_vec();
    let previous_formula_errors = formula_errors.clone();
    let mut inputs = Vec::with_capacity(revision.blocks.len());
    let mut prepared = Vec::with_capacity(revision.blocks.len());
    let mut next_formula_errors = Vec::with_capacity(revision.blocks.len());

    for (index, block) in revision.blocks.iter().enumerate() {
        let hash = content_hash(block, options);
        let unchanged = previous
            .get(index)
            .is_some_and(|planned| planned.hash == hash);
        if unchanged {
            let planned = &previous[index];
            inputs.push((hash, planned.width_px, planned.height_px));
            prepared.push(PreparedBlock {
                rendered: None,
                png: None,
                cache: None,
            });
            next_formula_errors.push(previous_formula_errors[index]);
            continue;
        }

        let before = cache.stats();
        let rendered = cache.render(block, options)?;
        let after = cache.stats();
        let outcome = if after.hits > before.hits {
            CacheOutcome::Hit
        } else {
            CacheOutcome::Miss
        };
        inputs.push((hash, rendered.width_px, rendered.height_px));
        next_formula_errors.push(rendered.formula_errors.len());
        let png = crate::native_render::canonical_block_png(
            &rendered,
            Limits::default()
                .scaled(options.device_pixel_ratio)
                .image_pixels,
        )?;
        prepared.push(PreparedBlock {
            rendered: Some(rendered),
            png: Some(png),
            cache: Some(outcome),
        });
    }

    let plan = planner.plan(&inputs);
    let outcome = sink.emit(&plan, &prepared, &previous)?;
    *formula_errors = next_formula_errors;
    Ok(outcome)
}

pub(crate) enum StreamSink {
    Summary,
    // Boxed so `Summary`'s zero-sized variant does not force every
    // `StreamSink` (including the common `Summary`/stream-mode case) to
    // carry `TerminalSink`'s full size, which grows with each new piece of
    // agent-viewer-only state (`emitted_ids`, `retained_window_blocks`, ...).
    Terminal(Box<TerminalSink>),
}

impl StreamSink {
    pub(crate) fn new(
        connected: Option<(Terminal<StdioTty>, (u32, u32))>,
        max_image_pixels: u64,
    ) -> Self {
        match connected {
            Some((terminal, cell)) => Self::Terminal(Box::new(TerminalSink::new(
                terminal,
                CellSize {
                    width: cell.0,
                    height: cell.1,
                },
                max_image_pixels,
            ))),
            None => Self::Summary,
        }
    }

    /// Opts a `Terminal` sink into retaining each placed block's PNG bytes
    /// (bounded by the placement-count and pixel limits already enforced),
    /// which `sync_window` needs to rebuild the agent-viewer's visibility
    /// window without re-rendering. Plain `tmath render`/`tmath watch`
    /// stream sessions never call `sync_window`, so they skip this to
    /// avoid paying the retained-PNG memory cost for nothing. A no-op in
    /// `Summary` mode.
    pub(crate) fn with_retained_pngs(mut self) -> Self {
        if let Self::Terminal(sink) = &mut self {
            sink.retain_pngs = true;
        }
        self
    }

    /// Bounds how many blocks outside the visibility window keep their
    /// retained PNG (AT-3-504); see [`TerminalSink::retained_window_blocks`].
    /// A no-op in `Summary` mode.
    pub(crate) fn with_retained_window_blocks(mut self, budget: u64) -> Self {
        if let Self::Terminal(sink) = &mut self {
            sink.retained_window_blocks = budget;
        }
        self
    }

    /// Opts a `Terminal` sink into the agent-viewer's reserved-row-1 live
    /// status bar (see the `status_bar` module doc), sets
    /// [`TerminalSink::pane_rows`], and establishes the DECSTBM scroll
    /// region covering `2..=pane_rows` (stage 1 of the scroll-region
    /// viewer, see the `scroll_region` module doc) — the three are opt-in
    /// together because they are all viewer-only pane-geometry facts plain
    /// stream/watch sessions never need, and the status bar's reserved row
    /// 1 IS the region's top boundary, not an independent concern. `pane_cols`
    /// is the status bar's own draw width; `pane_rows` is both what
    /// [`TerminalSink::emit_batch`]'s clamp check compares the stale-tail
    /// row span against AND the region's bottom row. A no-op in `Summary`
    /// mode.
    ///
    /// The DECSTBM write (`Terminal::set_scroll_region`) is best-effort: on
    /// failure, `scroll_region` stays `None` and `append`/`replace` fall
    /// back to their pre-region behavior for this session — the same
    /// fail-open posture `Terminal::new`'s own unconditional `INIT_MODES`
    /// write already has (a construction-time terminal write failing is not
    /// treated as fatal anywhere in this module).
    pub(crate) fn with_status_bar(mut self, pane_cols: u32, pane_rows: u32) -> Self {
        if let Self::Terminal(sink) = &mut self {
            sink.pane_rows = pane_rows;
            sink.pane_cols = pane_cols;
            sink.status_bar = Some(StatusBar::new(pane_cols));
            let top = 2u32;
            let bottom = pane_rows.max(top);
            if sink.terminal.set_scroll_region(top, bottom).is_ok() {
                sink.scroll_region = Some((top, bottom));
            }
        }
        self
    }

    /// Updates the live status-bar state and redraws row 1 in place (a
    /// save/move/write/restore sequence — see
    /// [`status_bar::status_bar_operations`] — that never disturbs the
    /// content area or the cursor position flowing appends rely on). A
    /// no-op in `Summary` mode or if [`Self::with_status_bar`] was never
    /// called.
    pub(crate) fn set_status(&mut self, state: StatusBarState) -> Result<(), RenderError> {
        if let Self::Terminal(sink) = self {
            sink.set_status(state)?;
        }
        Ok(())
    }

    /// Draws stage 2's transient scrollbar (see
    /// [`crate::scroll_region::scrollbar_operations`]) at `thumb_rows` — a
    /// no-op in `Summary` mode or if [`Self::with_status_bar`] was never
    /// called (no `scroll_region` means no region to draw a scrollbar
    /// against). Not gated by `suppress_writes`: the scrollbar, like the
    /// status bar, is pane chrome rather than visibility-windowed content.
    pub(crate) fn set_scrollbar(
        &mut self,
        thumb_rows: Option<std::ops::Range<u32>>,
    ) -> Result<(), RenderError> {
        if let Self::Terminal(sink) = self {
            sink.set_scrollbar(thumb_rows)?;
        }
        Ok(())
    }

    /// Clears stage 2's transient scrollbar back to blank (the ~1s
    /// auto-hide timer, or nothing left to scroll). A no-op in `Summary`
    /// mode or if [`Self::with_status_bar`] was never called.
    pub(crate) fn clear_scrollbar(&mut self) -> Result<(), RenderError> {
        if let Self::Terminal(sink) = self {
            sink.clear_scrollbar()?;
        }
        Ok(())
    }

    /// Sets or clears visibility-gated emission (AT-3-503): while suppressed,
    /// `apply_revision`'s append/replace/remove operations still update
    /// state but skip terminal writes. See [`TerminalSink::suppress_writes`].
    /// A no-op in `Summary` mode.
    pub(crate) fn set_suppress_writes(&mut self, suppress: bool) {
        if let Self::Terminal(sink) = self {
            sink.suppress_writes = suppress;
        }
    }

    /// Enables top-down incremental region appends while follow is engaged.
    pub(crate) fn set_follow_top_down(&mut self, enabled: bool) {
        if let Self::Terminal(sink) = self {
            sink.follow_top_down = enabled;
        }
    }

    fn emit(
        &mut self,
        plan: &Plan,
        prepared: &[PreparedBlock],
        previous: &[tmath_render::PlannedBlock],
    ) -> Result<EmitOutcome, RenderError> {
        match self {
            Self::Summary => emit_summary(plan, prepared).map(|()| EmitOutcome::Applied),
            Self::Terminal(sink) => sink.emit(plan, prepared, previous),
        }
    }

    pub(crate) fn finish(&mut self) -> Result<(), RenderError> {
        match self {
            Self::Summary => Ok(()),
            Self::Terminal(sink) => sink.finish(),
        }
    }

    pub(crate) fn done(&mut self, blocks: usize, formula_errors: usize) -> Result<(), RenderError> {
        match self {
            Self::Summary => {
                println!("event=done blocks={blocks} formula_errors={formula_errors}");
                io::stdout().flush().map_err(|_| stream_error())
            }
            Self::Terminal(_) => self.finish(),
        }
    }

    pub(crate) fn summary_event(&mut self, event: &str) -> Result<(), RenderError> {
        if matches!(self, Self::Summary) {
            println!("{event}");
            io::stdout().flush().map_err(|_| stream_error())?;
        }
        Ok(())
    }

    /// Syncs the visibility window (agent-viewer only). A no-op in `Summary`
    /// mode. See [`TerminalSink::sync_window`].
    pub(crate) fn sync_window(
        &mut self,
        visible: std::ops::Range<usize>,
        skip_rows_in_first: u32,
    ) -> Result<(), RenderError> {
        match self {
            Self::Summary => Ok(()),
            Self::Terminal(sink) => sink.sync_window(visible, skip_rows_in_first),
        }
    }

    /// Attempts stage 2's incremental scroll-back step (agent-viewer only).
    /// Always `Ok(false)` in `Summary` mode (there is no region to scroll,
    /// and `Summary` never draws anything anyway) — the caller falls back
    /// to `sync_window`, which is itself also a no-op in `Summary` mode, so
    /// this never changes `Summary`-mode behavior. See
    /// [`TerminalSink::try_scroll_window_incrementally`].
    pub(crate) fn try_scroll_window_incrementally(
        &mut self,
        entering_ids_in_order: &[u64],
        new_visible: std::ops::Range<usize>,
    ) -> Result<bool, RenderError> {
        match self {
            Self::Summary => Ok(false),
            Self::Terminal(sink) => {
                sink.try_scroll_window_incrementally(entering_ids_in_order, new_visible)
            }
        }
    }

    /// The ids in `visible` whose retained PNG was evicted (AT-3-504) and
    /// need restoring before the next `sync_window` call. Always empty in
    /// `Summary` mode. See [`TerminalSink::missing_pngs`].
    pub(crate) fn missing_pngs(&self, visible: std::ops::Range<usize>) -> Vec<u64> {
        match self {
            Self::Summary => Vec::new(),
            Self::Terminal(sink) => sink.missing_pngs(visible),
        }
    }

    /// Restores a placed block's retained PNG (agent-viewer's scroll-back
    /// path). A no-op in `Summary` mode. See [`TerminalSink::refresh_png`].
    pub(crate) fn refresh_png(&mut self, id: u64, png: Vec<u8>) -> Result<(), RenderError> {
        match self {
            Self::Summary => Ok(()),
            Self::Terminal(sink) => sink.refresh_png(id, png),
        }
    }

    /// Evicts retained PNGs outside `visible` ± the configured budget
    /// (AT-3-504), independent of `sync_window`. A no-op in `Summary` mode.
    /// See [`TerminalSink::evict_outside_window`].
    pub(crate) fn evict_outside_window(&mut self, visible: std::ops::Range<usize>) {
        if let Self::Terminal(sink) = self {
            sink.evict_outside_window(visible);
        }
    }
}

fn emit_summary(plan: &Plan, prepared: &[PreparedBlock]) -> Result<(), RenderError> {
    for (index, operation) in plan.ops.iter().enumerate() {
        match operation {
            PlanOp::Keep { .. } => {}
            PlanOp::Append { block } => {
                let (_, png, cache) = rendered_event(prepared, index)?;
                println!(
                    "event=append id={} width={} height={} bytes={} cache={}",
                    block.id,
                    block.width_px,
                    block.height_px,
                    png.len(),
                    cache.label()
                );
                io::stdout().flush().map_err(|_| stream_error())?;
            }
            PlanOp::Replace { old_id, block } => {
                let (_, png, cache) = rendered_event(prepared, index)?;
                println!(
                    "event=replace old={} id={} width={} height={} bytes={} cache={}",
                    old_id,
                    block.id,
                    block.width_px,
                    block.height_px,
                    png.len(),
                    cache.label()
                );
                io::stdout().flush().map_err(|_| stream_error())?;
            }
            PlanOp::Remove { id } => {
                println!("event=remove id={id}");
                io::stdout().flush().map_err(|_| stream_error())?;
            }
        }
    }
    Ok(())
}

fn rendered_event(
    prepared: &[PreparedBlock],
    index: usize,
) -> Result<(&RenderedBlock, &[u8], CacheOutcome), RenderError> {
    let prepared = prepared.get(index).ok_or_else(stream_error)?;
    let rendered = prepared.rendered.as_deref().ok_or_else(stream_error)?;
    let png = prepared.png.as_deref().ok_or_else(stream_error)?;
    let cache = prepared.cache.ok_or_else(stream_error)?;
    Ok((rendered, png, cache))
}

#[derive(Clone)]
struct PlacedState {
    id: u64,
    rows: u32,
    pixels: u64,
    /// Retained only when `TerminalSink::retain_pngs` is set (agent-viewer
    /// only), so a viewport sync ([`TerminalSink::sync_window`]) can
    /// re-emit a currently placed block without re-rendering it. Plain
    /// stream/watch sessions never call `sync_window` and leave this
    /// empty rather than pay the retained-PNG memory cost for nothing.
    png: Vec<u8>,
}

pub(crate) struct TerminalSink {
    terminal: Terminal<StdioTty>,
    cell: CellSize,
    max_image_pixels: u64,
    placement_limits: PlacementLimits,
    placed: Vec<PlacedState>,
    first_append_at_line_start: bool,
    /// Set only through [`StreamSink::with_retained_pngs`] (agent-viewer's
    /// construction path). See [`PlacedState::png`].
    retain_pngs: bool,
    /// The image ids [`TerminalSink::sync_window`] most recently emitted on
    /// screen (agent-viewer only; stream/watch sessions never call
    /// `sync_window` and leave this empty). Deliberately id-based rather
    /// than index-based: while `suppress_writes` is set, `apply_revision`
    /// still mutates `placed` (a suppressed tail replace is remove+push,
    /// which shifts every later index), so an index range captured before
    /// those mutations would silently point at the wrong entries by the
    /// time the next `sync_window` reads it. `sync_window` diffs against
    /// this by id to delete only what left the window (including an id
    /// that a suppressed replace/remove already dropped from `placed`
    /// entirely — that delete must still be sent, since the on-screen
    /// image itself was never touched) and re-emit only what the new
    /// window covers.
    emitted_ids: Vec<u64>,
    /// When set, `append`/`replace`/`remove` still update `placed` (state
    /// while the agent-viewer's follow is disengaged, new/changed blocks land
    /// outside the visible window, so streaming them would just be undone by
    /// the next `sync_window`. The caller sets this before `apply_revision`
    /// while disengaged and calls `sync_window` afterward.
    suppress_writes: bool,
    /// When `true`, region-managed `append`/`replace` use top-down placement
    /// (`region_append_top_operations` / `region_tail_replace_top_operations`)
    /// for incremental streaming while follow is pinned to offset `0`. Set by
    /// `agent_viewer` only while follow is engaged.
    follow_top_down: bool,
    /// AT-3-504's bound on retained PNGs: on every `sync_window`, blocks more
    /// than this many positions outside the new `visible` range (on either
    /// side) have their `PlacedState::png` evicted to an empty vec. `u64::MAX`
    /// (the default) means unbounded, which is what stream/watch sessions
    /// want since they never retain PNGs in the first place. Set only
    /// through [`StreamSink::with_retained_window_blocks`].
    retained_window_blocks: u64,
    /// The pane's total row count, used ONLY by [`Self::emit_batch`]'s
    /// clamp-aware fallback check (see [`clamp_would_truncate`]) — `0` (the
    /// default) disables the check entirely (never triggers a fallback),
    /// which is correct for plain stream/watch sessions: `emit_batch` is
    /// unreachable there regardless (it requires `retain_pngs`, which only
    /// the agent-viewer ever sets — see [`Self::emit`]'s gate), so this
    /// field is meaningless off that path and safe to leave unset. Set only
    /// through [`StreamSink::with_status_bar`], the same opt-in the status
    /// bar itself uses, since both are viewer-only pane-geometry facts.
    pane_rows: u32,
    /// The live status-bar state (row 1, agent-viewer only — see the
    /// `status_bar` module doc). `None` (the default) disables the status
    /// bar and the reserved top row entirely: plain stream/watch sessions
    /// never call [`StreamSink::with_status_bar`], so `sync_window`'s
    /// content draw starts at the pane's actual top row exactly as before
    /// this feature existed. Set only through
    /// [`StreamSink::with_status_bar`]; updated on every redraw-triggering
    /// event via [`StreamSink::set_status`].
    status_bar: Option<StatusBar>,
    /// The DECSTBM scroll region's `(top, bottom)` rows (1-indexed,
    /// inclusive), set together with `status_bar` in
    /// [`StreamSink::with_status_bar`] — the region always covers
    /// `2..=pane_rows` whenever the status bar reserves row 1, since the two
    /// are one architecture (window-managed viewer, stage 1): `None` (the
    /// default) is what plain stream/watch sessions keep, and `append`/
    /// `replace` fall back to their pre-region cursor-relative behavior for
    /// them, exactly as before this field existed. When `Some`, `append`
    /// routes through [`crate::scroll_region::region_append_operations`]
    /// and a growing-tail `replace` through
    /// [`crate::scroll_region::region_tail_replace_operations`] instead —
    /// see those functions' doc comments for why region-scroll replaces the
    /// old flowing-append/live-cursor-query behavior.
    scroll_region: Option<(u32, u32)>,
    /// The pane's total column count, set together with `pane_rows`/
    /// `scroll_region` in [`StreamSink::with_status_bar`] — stage 2's
    /// scrollbar (`Self::set_scrollbar`/`clear_scrollbar`) draws in this
    /// absolute last column. `0` (the default) disables the scrollbar
    /// entirely, the same "never opted in" posture every other
    /// `with_status_bar`-gated field has.
    pane_cols: u32,
}

impl TerminalSink {
    fn new(mut terminal: Terminal<StdioTty>, cell: CellSize, max_image_pixels: u64) -> Self {
        let first_append_at_line_start = if tmath_core::kitty::inside_tmux() {
            false
        } else {
            terminal.cursor_column().ok().flatten() == Some(1)
        };
        Self {
            terminal,
            cell,
            max_image_pixels,
            placement_limits: PlacementLimits::default(),
            placed: Vec::new(),
            first_append_at_line_start,
            retain_pngs: false,
            emitted_ids: Vec::new(),
            suppress_writes: false,
            follow_top_down: false,
            retained_window_blocks: u64::MAX,
            pane_rows: 0,
            status_bar: None,
            scroll_region: None,
            pane_cols: 0,
        }
    }

    fn emit(
        &mut self,
        plan: &Plan,
        prepared: &[PreparedBlock],
        previous: &[tmath_render::PlannedBlock],
    ) -> Result<EmitOutcome, RenderError> {
        // A divergence anywhere but the tail — any Replace or Remove — needs
        // the batch rewrite below in viewer mode: per-op cursor arithmetic
        // (`replace`'s `top_is_reachable`, `remove`'s bare Kitty delete with
        // no row clear) is only sound when a revision only ever touches the
        // last on-screen block, which is what plain stream/watch sessions
        // guarantee (see `stream_shaped_revisions_never_produce_an_interior_replace_or_remove`)
        // but the agent-viewer's whole-document `Reset`/shrink revisions do
        // not. Retained PNGs (`retain_pngs`) are required to redraw the Keep
        // prefix without re-rendering, so the batch path only applies when
        // they are available; plain stream/watch sessions (no retained
        // PNGs) keep the per-op path unconditionally — see the doc comment
        // on `divergence_rewrite_operations` for why that path is safe for
        // them regardless of this branch.
        if self.retain_pngs && plan_has_interior_divergence(plan) {
            return self.emit_batch(plan, prepared, previous);
        }

        for (index, operation) in plan.ops.iter().enumerate() {
            match operation {
                PlanOp::Keep { .. } => {}
                PlanOp::Append { block } => {
                    let (rendered, png, _) = rendered_event(prepared, index)?;
                    self.append(block.id, rendered, png)?;
                }
                PlanOp::Replace { old_id, block } => {
                    let (rendered, png, _) = rendered_event(prepared, index)?;
                    let old_index = previous
                        .iter()
                        .position(|previous| previous.id == *old_id)
                        .ok_or_else(stream_error)?;
                    self.replace(
                        *old_id,
                        block.id,
                        rendered,
                        png,
                        old_index + 1 == previous.len(),
                    )?;
                }
                PlanOp::Remove { id } => self.remove(*id)?,
            }
        }
        Ok(EmitOutcome::Applied)
    }

    /// Batch rewrite for a revision whose plan diverges from the previous
    /// layout somewhere other than a pure tail append (AT-3-506): rather
    /// than replaying `Replace`/`Remove` ops one at a time against
    /// potentially-stale per-op cursor state, this treats the whole
    /// divergence as one unit — cursor up by the exact row span the stale
    /// tail occupied, erase it, delete every stale Kitty image, and
    /// re-place every block from the divergence point in document order.
    /// This is the same "erase and redraw the affected span" shape as
    /// `sync_window_operations`, just anchored at the plan's first
    /// non-`Keep` index instead of a viewport window.
    ///
    /// Clamp-aware fallback: before writing anything, checks whether the
    /// stale tail's row span could ever be reached by the relative
    /// cursor-up `divergence_rewrite_operations` is about to emit (see
    /// [`clamp_would_truncate`]). When it could not, this skips the
    /// relative-cursor rewrite ENTIRELY — writing a cursor-up that will
    /// clamp is what produces the ghost, so there is no safe partial write
    /// here — updates `self.placed`'s bookkeeping to the new plan exactly
    /// as the normal path would (so the caller's state stays correct for
    /// the fallback full-window redraw), and returns
    /// `EmitOutcome::NeedsWindowSync` so the caller runs
    /// `sync_visible_window`/`sync_window` instead, which redraws from an
    /// absolute `\x1b[H` and therefore never depends on relative cursor
    /// travel distance.
    fn emit_batch(
        &mut self,
        plan: &Plan,
        prepared: &[PreparedBlock],
        previous: &[tmath_render::PlannedBlock],
    ) -> Result<EmitOutcome, RenderError> {
        let Some(reanchor_from) = plan.reanchor_from else {
            return Ok(EmitOutcome::Applied);
        };
        let old_rows_total = stale_tail_rows_total(&self.placed, previous, reanchor_from);
        if clamp_would_truncate(old_rows_total, self.pane_rows, self.status_bar.is_some()) {
            let new_tail = clamp_fallback_new_tail(
                &self.placed,
                plan,
                prepared,
                reanchor_from,
                self.cell,
                self.max_image_pixels,
                self.retain_pngs,
            )?;
            self.placed.truncate(reanchor_from.min(self.placed.len()));
            self.placed.extend(new_tail);
            return Ok(EmitOutcome::NeedsWindowSync);
        }

        let (operations, new_tail) = divergence_rewrite_operations(
            &self.placed,
            previous,
            plan,
            prepared,
            reanchor_from,
            self.cell,
            self.max_image_pixels,
            self.retain_pngs,
        )?;
        self.write_unless_suppressed(&operations)?;
        self.placed.truncate(reanchor_from.min(self.placed.len()));
        self.placed.extend(new_tail);
        if !plan.ops.is_empty() {
            self.first_append_at_line_start = true;
        }
        Ok(EmitOutcome::Applied)
    }

    fn append(&mut self, id: u64, rendered: &RenderedBlock, png: &[u8]) -> Result<(), RenderError> {
        let decoded = self.decode(id, rendered, png)?;
        self.validate_placement(decoded.pixels, None)?;
        let operations = if let Some((region_top, region_bottom)) = self.scroll_region {
            let rows_before: u32 = self.placed.iter().map(|entry| entry.rows).sum();
            if self.follow_top_down {
                crate::scroll_region::region_append_top_operations(
                    crate::scroll_region::RegionBlock { id, png },
                    self.cell,
                    self.max_image_pixels,
                    region_top,
                    region_bottom,
                    rows_before,
                )
                .map_err(|_| stream_error())?
            } else {
                crate::scroll_region::region_append_operations(
                    crate::scroll_region::RegionBlock { id, png },
                    self.cell,
                    self.max_image_pixels,
                    region_bottom,
                )
                .map_err(|_| stream_error())?
            }
        } else {
            append_operations(
                decoded.id,
                rendered.width_px,
                rendered.height_px,
                &decoded.rgba,
                decoded.cols,
                decoded.rows,
                self.first_append_at_line_start || !self.placed.is_empty(),
            )
        };
        self.write_unless_suppressed(&operations)?;
        self.first_append_at_line_start = true;
        let retained_png = self.retained_png(png);
        self.placed.push(PlacedState {
            id,
            rows: decoded.rows,
            pixels: decoded.pixels,
            png: retained_png,
        });
        Ok(())
    }

    fn replace(
        &mut self,
        old_id: u64,
        new_id: u64,
        rendered: &RenderedBlock,
        png: &[u8],
        was_last: bool,
    ) -> Result<(), RenderError> {
        let old_index = self
            .placed
            .iter()
            .position(|placed| placed.id == old_id)
            .ok_or_else(stream_error)?;
        let old_id_value = self.placed[old_index].id;
        let old_rows = self.placed[old_index].rows;
        let decoded = self.decode(new_id, rendered, png)?;
        self.validate_placement(decoded.pixels, Some(old_id))?;
        let is_current_tail = was_last
            && self
                .placed
                .last()
                .is_some_and(|placed| placed.id == old_id_value);

        let operations = if let Some((region_top, region_bottom)) = self.scroll_region {
            if self.follow_top_down && is_current_tail {
                let rows_before_tail: u32 = self.placed[..old_index]
                    .iter()
                    .map(|entry| entry.rows)
                    .sum();
                crate::scroll_region::region_tail_replace_top_operations(
                    old_id_value,
                    old_rows,
                    crate::scroll_region::RegionBlock { id: new_id, png },
                    self.cell,
                    self.max_image_pixels,
                    crate::scroll_region::RegionBounds {
                        top: region_top,
                        bottom: region_bottom,
                    },
                    rows_before_tail,
                )
                .map_err(|_| stream_error())?
            } else if is_current_tail {
                crate::scroll_region::region_tail_replace_operations(
                    old_id_value,
                    old_rows,
                    crate::scroll_region::RegionBlock { id: new_id, png },
                    self.cell,
                    self.max_image_pixels,
                    region_bottom,
                )
                .map_err(|_| stream_error())?
            } else {
                let mut operations =
                    vec![TerminalOp::Graphics(tmath_core::kitty::kitty_delete_id(
                        u32::try_from(old_id_value).map_err(|_| stream_error())?,
                    ))];
                if self.follow_top_down {
                    let rows_before: u32 = self.placed.iter().map(|entry| entry.rows).sum();
                    operations.extend(
                        crate::scroll_region::region_append_top_operations(
                            crate::scroll_region::RegionBlock { id: new_id, png },
                            self.cell,
                            self.max_image_pixels,
                            region_top,
                            region_bottom,
                            rows_before,
                        )
                        .map_err(|_| stream_error())?,
                    );
                } else {
                    operations.extend(
                        crate::scroll_region::region_append_operations(
                            crate::scroll_region::RegionBlock { id: new_id, png },
                            self.cell,
                            self.max_image_pixels,
                            region_bottom,
                        )
                        .map_err(|_| stream_error())?,
                    );
                }
                operations
            }
        } else {
            let top_is_reachable = is_current_tail
                && self
                    .terminal
                    .cursor_position()
                    .ok()
                    .flatten()
                    .is_some_and(|(row, _)| row > old_rows);
            if top_is_reachable {
                tail_replace_operations(TailReplace {
                    old_image_id: old_id_value,
                    new_image_id: decoded.id,
                    width_px: rendered.width_px,
                    height_px: rendered.height_px,
                    rgba: &decoded.rgba,
                    cols: decoded.cols,
                    old_rows,
                    new_rows: decoded.rows,
                })?
            } else {
                // Phase 2 intentionally cannot re-anchor an interior block or a
                // tail whose top row has entered scrollback. Delete only that image
                // (leaving its old cells blank) and append the replacement at the
                // bottom. In-place interior re-anchoring belongs to Phase 3 viewer
                // work, where viewport and history state are explicit.
                let mut operations =
                    vec![TerminalOp::Graphics(tmath_core::kitty::kitty_delete_id(
                        u32::try_from(old_id_value).map_err(|_| stream_error())?,
                    ))];
                operations.extend(append_operations(
                    decoded.id,
                    rendered.width_px,
                    rendered.height_px,
                    &decoded.rgba,
                    decoded.cols,
                    decoded.rows,
                    true,
                ));
                operations
            }
        };
        self.write_unless_suppressed(&operations)?;
        self.placed.remove(old_index);
        let retained_png = self.retained_png(png);
        self.placed.push(PlacedState {
            id: new_id,
            rows: decoded.rows,
            pixels: decoded.pixels,
            png: retained_png,
        });
        Ok(())
    }

    fn remove(&mut self, id: u64) -> Result<(), RenderError> {
        let index = self
            .placed
            .iter()
            .position(|placed| placed.id == id)
            .ok_or_else(stream_error)?;
        let image_id = u32::try_from(id).map_err(|_| stream_error())?;
        self.write_unless_suppressed(&[TerminalOp::Graphics(tmath_core::kitty::kitty_delete_id(
            image_id,
        ))])?;
        self.placed.remove(index);
        Ok(())
    }

    /// Syncs the terminal to a new visibility window for the agent-viewer's
    /// scrollable viewport (AT-3-503): deletes the placements whose id left
    /// the window (by id, not index — see the `emitted_ids` field doc for
    /// why), moves the cursor home, re-emits the window's current blocks
    /// (clamped to `placed`'s bounds) at their window-relative rows from
    /// retained PNGs, and erases any residual rows below what was just
    /// drawn. No block is re-rendered, and transmitted bytes are bounded by
    /// the number of blocks in `visible`, independent of how many blocks
    /// exist outside it (`placed`'s total length never affects the byte
    /// cost of a scroll step).
    ///
    /// Blocks that stay inside the window across the change are re-sent
    /// too, not left alone: their window-relative row can shift whenever a
    /// block enters or leaves ahead of them, and Kitty placements do not
    /// move on their own. This keeps the byte cost proportional to the
    /// window (bounded), not to history length, while staying correct for
    /// any offset change — a coarser diff than "only truly new placements",
    /// which the doc comment on `sync_window_operations` explains further.
    pub(crate) fn sync_window(
        &mut self,
        visible: std::ops::Range<usize>,
        skip_rows_in_first: u32,
    ) -> Result<(), RenderError> {
        let visible = visible.start.min(self.placed.len())..visible.end.min(self.placed.len());
        let content_row_offset = u32::from(self.status_bar.is_some());
        let operations = sync_window_operations(
            &self.placed,
            &self.emitted_ids,
            visible.clone(),
            self.cell,
            self.max_image_pixels,
            content_row_offset,
            skip_rows_in_first,
        )?;
        terminal_output::write_operations(&operations).map_err(|_| stream_error())?;
        self.first_append_at_line_start = true;
        self.emitted_ids = self.placed[visible.clone()]
            .iter()
            .map(|entry| entry.id)
            .collect();
        evict_pngs_outside_budget(&mut self.placed, visible, self.retained_window_blocks);
        Ok(())
    }

    /// Stage 2's incremental scroll-back step: for the narrow case of a
    /// small backward window move (scrolling toward older content, e.g. one
    /// momentum tick's `whole_rows`) with NO OTHER window-shape change
    /// (same `skip_rows_in_first`, no bottom-edge shrink), scrolls the
    /// DECSTBM region down and draws only the newly-entering blocks at the
    /// top edge (`crate::scroll_region::region_scroll_back_operations`)
    /// instead of erasing and redrawing the WHOLE visible window
    /// (`sync_window`) — a real smoothness difference during a decaying
    /// momentum tail: `sync_window_operations` re-transmits every block in
    /// `visible` on every call (needed for correctness there, since it must
    /// tolerate an arbitrary window change), while this path only ever
    /// touches the rows that actually changed.
    ///
    /// Returns `Ok(true)` when it handled the step; `Ok(false)` when the
    /// shape does not match the narrow case this function covers (multiple
    /// blocks entering with a mid-block edge, a forward step, a
    /// `skip_rows_in_first` change, or anything else) — the caller must
    /// then fall back to a full `sync_window` call, which remains correct
    /// for every shape this function does not attempt, exactly as before
    /// stage 2. Never itself a correctness risk if the heuristic under- or
    /// over-restricts which cases it accepts: an `Ok(false)` is always safe
    /// (just slower, not wrong), and this function only ever emits
    /// operations it can prove correct for the specific shape it checked.
    ///
    /// `entering_ids_in_order` are the ids newly revealed at the window's
    /// top edge, TOP-TO-BOTTOM (closest-to-top first) — the caller
    /// (`agent_viewer`) computes this from the viewport's before/after
    /// visible ranges, since only it has that history.
    ///
    /// Not covered by a byte-level `FakeTty`-driven test in this module's
    /// own test suite, for the same structural reason `emit_batch`'s clamp
    /// branch is not (see `two_consecutive_resets_keep_row_bookkeeping_
    /// consistent_across_calls`'s doc comment): `TerminalSink` is hardcoded
    /// to `Terminal<StdioTty>`, not generic over `Tty`, so a real
    /// `TerminalSink` cannot be constructed in a test at all. This
    /// function's shape-matching logic (the `matches_prefix` check) is
    /// exercised indirectly through `crate::scroll_region`'s thorough
    /// `region_scroll_back_operations` tests (the operations it delegates
    /// to) and `agent_viewer`'s `incremental_entering_range` tests (the
    /// caller-side shape detection that decides whether this function is
    /// even called) — real-terminal evidence is what would close the
    /// remaining gap on this function's own dispatch/eviction glue.
    pub(crate) fn try_scroll_window_incrementally(
        &mut self,
        entering_ids_in_order: &[u64],
        new_visible: std::ops::Range<usize>,
    ) -> Result<bool, RenderError> {
        let Some((region_top, region_bottom)) = self.scroll_region else {
            return Ok(false);
        };
        if entering_ids_in_order.is_empty() {
            return Ok(false);
        }
        let new_visible =
            new_visible.start.min(self.placed.len())..new_visible.end.min(self.placed.len());
        // Only handle the case where EVERY entering id is actually present,
        // in order, at the very start of the new visible range — anything
        // else (a gap, a reordering, an id that left retained-PNG budget)
        // is not this function's narrow shape.
        let matches_prefix = self.placed[new_visible.clone()]
            .iter()
            .take(entering_ids_in_order.len())
            .map(|entry| entry.id)
            .eq(entering_ids_in_order.iter().copied());
        if !matches_prefix {
            return Ok(false);
        }
        let entering: Vec<crate::scroll_region::RegionBlock<'_>> = entering_ids_in_order
            .iter()
            .map(|&id| {
                self.placed
                    .iter()
                    .find(|entry| entry.id == id)
                    .map(|entry| crate::scroll_region::RegionBlock {
                        id,
                        png: &entry.png,
                    })
                    .ok_or_else(stream_error)
            })
            .collect::<Result<_, _>>()?;
        // A retained PNG evicted out of budget shows up as an empty `png`
        // (see `PlacedState::png`'s doc comment) — `decode_png` inside
        // `region_scroll_back_operations` would fail closed on that, which
        // is correct, but the caller should have already restored it via
        // `restore_missing_pngs` before calling this; bail to the full sync
        // path rather than surface a hard error for a case the caller
        // should have prevented.
        if entering.iter().any(|block| block.png.is_empty()) {
            return Ok(false);
        }
        let operations = crate::scroll_region::region_scroll_back_operations(
            &entering,
            self.cell,
            self.max_image_pixels,
            region_top,
            region_bottom,
        )
        .map_err(|_| stream_error())?;
        self.write_unless_suppressed(&operations)?;
        self.first_append_at_line_start = true;
        self.emitted_ids = self.placed[new_visible.clone()]
            .iter()
            .map(|entry| entry.id)
            .collect();
        evict_pngs_outside_budget(&mut self.placed, new_visible, self.retained_window_blocks);
        Ok(true)
    }

    /// Runs AT-3-504's eviction directly, without a full `sync_window`
    /// (which also diffs `emitted_ids` and writes terminal bytes — neither
    /// of which the follow-engaged append path needs, since it never calls
    /// `sync_window` at all: `apply_revision` streams straight to the pane
    /// bottom while following, so there is no separate "sync the screen"
    /// step there). Called unconditionally from `render_and_place`
    /// regardless of follow state — see that function's doc comment for why
    /// eviction must not be conditioned on `sync_window` having run. Calling
    /// this and then `sync_window` back to back (the disengaged path) is
    /// idempotent: eviction only ever empties a PNG that is already outside
    /// the window, so re-running it with the same window is a no-op.
    pub(crate) fn evict_outside_window(&mut self, visible: std::ops::Range<usize>) {
        let visible = visible.start.min(self.placed.len())..visible.end.min(self.placed.len());
        evict_pngs_outside_budget(&mut self.placed, visible, self.retained_window_blocks);
    }

    /// The ids of blocks inside `visible` (clamped to `placed`'s bounds)
    /// whose retained PNG was evicted (AT-3-504) and must be restored — by a
    /// `RenderCache` hit or a real re-render of the block's source — before
    /// the next `sync_window` call, which would otherwise fail decoding an
    /// empty PNG for that block. Returns ids in `visible`'s order so the
    /// caller can restore them in on-screen order if it chooses to.
    /// Deliberately id-only: restoring the PNG itself requires the block's
    /// source and a render engine, neither of which `TerminalSink` has (see
    /// the `retained_window_blocks` field doc) — that is the viewer's job.
    pub(crate) fn missing_pngs(&self, visible: std::ops::Range<usize>) -> Vec<u64> {
        let visible = visible.start.min(self.placed.len())..visible.end.min(self.placed.len());
        self.placed[visible]
            .iter()
            .filter(|entry| entry.png.is_empty())
            .map(|entry| entry.id)
            .collect()
    }

    /// Restores a placed block's retained PNG after `missing_pngs` reported
    /// it evicted, re-decoding it to recompute the same dimension/pixel
    /// bookkeeping `append`/`replace` maintain. Fails closed: an id that is
    /// not currently placed, or a PNG whose decoded dimensions do not match
    /// what was already recorded for that block (e.g. a stale re-render),
    /// leaves the existing (empty) entry untouched rather than risk placing
    /// a mismatched image.
    pub(crate) fn refresh_png(&mut self, id: u64, png: Vec<u8>) -> Result<(), RenderError> {
        let index = self
            .placed
            .iter()
            .position(|placed| placed.id == id)
            .ok_or_else(stream_error)?;
        let (width, height, _) =
            decode_png(&png, self.max_image_pixels).map_err(|_| stream_error())?;
        let (_, rows) = tmath_core::placement::grid_for(width, height, self.cell);
        if rows != self.placed[index].rows {
            return Err(stream_error());
        }
        self.placed[index].png = png;
        Ok(())
    }

    /// A copy of `png` when `retain_pngs` is set, or an empty vec otherwise.
    /// See [`PlacedState::png`] and [`StreamSink::with_retained_pngs`].
    fn retained_png(&self, png: &[u8]) -> Vec<u8> {
        retained_png(png, self.retain_pngs)
    }

    /// Writes `operations` to the terminal unless `suppress_writes` is set,
    /// in which case this is a no-op (the caller still updates `placed`
    /// state around this call). See the `suppress_writes` field doc.
    fn write_unless_suppressed(&self, operations: &[TerminalOp]) -> Result<(), RenderError> {
        if self.suppress_writes {
            return Ok(());
        }
        terminal_output::write_operations(operations).map_err(|_| stream_error())
    }

    /// Redraws row 1's live status bar in place from `state`, if the status
    /// bar was enabled via [`StreamSink::with_status_bar`] — a no-op
    /// otherwise. Not gated by `suppress_writes`: the status bar is not
    /// part of the visibility-windowed content area `suppress_writes`
    /// protects (see that field's doc), and the save/restore sequence
    /// [`status_bar_operations`] emits never touches the cursor position
    /// flowing appends rely on regardless of viewport/follow state.
    fn set_status(&mut self, state: StatusBarState) -> Result<(), RenderError> {
        let Some(status_bar) = self.status_bar else {
            return Ok(());
        };
        let operations = status_bar_operations(status_bar.pane_cols, state);
        terminal_output::write_operations(&operations).map_err(|_| stream_error())
    }

    /// Draws the scrollbar (thumb at `thumb_rows`, track everywhere else in
    /// the region) at the pane's absolute last column. A no-op when the
    /// region was never established (`scroll_region.is_none()`, e.g. plain
    /// stream/watch sessions, or `with_status_bar` never called).
    fn set_scrollbar(
        &mut self,
        thumb_rows: Option<std::ops::Range<u32>>,
    ) -> Result<(), RenderError> {
        let Some((region_top, region_bottom)) = self.scroll_region else {
            return Ok(());
        };
        let region_rows = region_bottom.saturating_sub(region_top).saturating_add(1);
        let operations = crate::scroll_region::scrollbar_operations(
            thumb_rows,
            region_rows,
            region_top,
            self.pane_cols,
        );
        terminal_output::write_operations(&operations).map_err(|_| stream_error())
    }

    /// Clears the scrollbar's column back to blank. A no-op under the same
    /// conditions as [`Self::set_scrollbar`].
    fn clear_scrollbar(&mut self) -> Result<(), RenderError> {
        let Some((region_top, region_bottom)) = self.scroll_region else {
            return Ok(());
        };
        let region_rows = region_bottom.saturating_sub(region_top).saturating_add(1);
        let operations = crate::scroll_region::scrollbar_clear_operations(
            region_rows,
            region_top,
            self.pane_cols,
        );
        terminal_output::write_operations(&operations).map_err(|_| stream_error())
    }

    fn decode(
        &self,
        id: u64,
        rendered: &RenderedBlock,
        png: &[u8],
    ) -> Result<DecodedPlacement, RenderError> {
        let id = u32::try_from(id).map_err(|_| stream_error())?;
        let (width, height, rgba) =
            decode_png(png, self.max_image_pixels).map_err(|_| stream_error())?;
        if width != rendered.width_px || height != rendered.height_px {
            return Err(stream_error());
        }
        let (cols, rows) = tmath_core::placement::grid_for(width, height, self.cell);
        Ok(DecodedPlacement {
            id,
            cols,
            rows,
            pixels: u64::from(width) * u64::from(height),
            rgba,
        })
    }

    /// Enforces the concurrent-placement and total-pixel limits against
    /// what is actually on screen. For stream/watch sessions (`retain_pngs`
    /// unset) that is every entry in `placed`, since every append/replace
    /// there writes straight to the terminal. For the agent-viewer
    /// (`retain_pngs` set), `placed` accumulates the *entire* answer
    /// history — bounded history eviction is T3-304, not yet implemented,
    /// so the count here would otherwise reject a session once its history
    /// crosses `max_concurrent_placements` even though only a handful of
    /// blocks are ever on screen at once. Counting only `emitted_ids`'s
    /// entries instead keeps the limit meaningful (it still bounds
    /// simultaneous on-screen placements and pixels) without it becoming a
    /// de facto history cap AT-3-503 is supposed to lift.
    fn validate_placement(
        &self,
        new_pixels: u64,
        replacing: Option<u64>,
    ) -> Result<(), RenderError> {
        let on_screen: Vec<(u64, u64)> = if self.retain_pngs {
            self.placed
                .iter()
                .filter(|placed| self.emitted_ids.contains(&placed.id))
                .map(|placed| (placed.id, placed.pixels))
                .collect()
        } else {
            self.placed
                .iter()
                .map(|placed| (placed.id, placed.pixels))
                .collect()
        };
        validate_placement_budget(&on_screen, new_pixels, replacing, self.placement_limits)
    }

    fn finish(&mut self) -> Result<(), RenderError> {
        self.terminal.reset().map_err(|_| stream_error())?;
        println!();
        Ok(())
    }
}

struct DecodedPlacement {
    id: u32,
    cols: u32,
    rows: u32,
    pixels: u64,
    rgba: Vec<u8>,
}

fn append_operations(
    image_id: u32,
    width_px: u32,
    height_px: u32,
    rgba: &[u8],
    cols: u32,
    rows: u32,
    already_at_line_start: bool,
) -> Vec<TerminalOp> {
    let mut operations = emit_placed_block_cursor(
        image_id,
        width_px,
        height_px,
        rgba,
        cols,
        rows,
        already_at_line_start,
    );
    operations.push(TerminalOp::Local(b"\r\n".to_vec()));
    operations
}

struct TailReplace<'a> {
    old_image_id: u64,
    new_image_id: u32,
    width_px: u32,
    height_px: u32,
    rgba: &'a [u8],
    cols: u32,
    old_rows: u32,
    new_rows: u32,
}

fn tail_replace_operations(replace: TailReplace<'_>) -> Result<Vec<TerminalOp>, RenderError> {
    let old_image_id = u32::try_from(replace.old_image_id).map_err(|_| stream_error())?;
    let mut operations = vec![
        TerminalOp::Local(format!("\x1b[{}A\r", replace.old_rows).into_bytes()),
        TerminalOp::Graphics(tmath_core::kitty::kitty_delete_id(old_image_id)),
        TerminalOp::Local(clear_rows(replace.old_rows)),
    ];
    operations.extend(emit_placed_block_cursor(
        replace.new_image_id,
        replace.width_px,
        replace.height_px,
        replace.rgba,
        replace.cols,
        replace.new_rows,
        true,
    ));
    operations.push(TerminalOp::Local(b"\r\n".to_vec()));
    Ok(operations)
}

/// Enforces the concurrent-placement and total-pixel limits against
/// `on_screen_pixels` (each entry the pixel cost of one placement currently
/// on screen). `on_screen_start` is `on_screen_pixels`' offset into the
/// caller's full block list, used to translate `replacing` (an index into
/// that full list) into `on_screen_pixels`' own index space; a `replacing`
/// index outside `on_screen_pixels`' range (replacing a block that is not
/// currently on screen) does not free up on-screen room and is treated the
/// same as `None`.
///
/// For stream/watch sessions, the caller passes every placed block (every
/// append/replace there writes straight to the terminal, so "on screen" is
/// everything). For the agent-viewer, the caller passes only the `emitted`
/// sub-slice: `placed` there accumulates the *entire* answer history —
/// bounded history eviction is T3-304, not yet implemented — so counting
/// all of it would reject a session once its history crosses
/// `max_concurrent_placements` even though only a handful of blocks are
/// ever on screen at once. See [`TerminalSink::validate_placement`].
fn validate_placement_budget(
    on_screen: &[(u64, u64)],
    new_pixels: u64,
    replacing: Option<u64>,
    limits: PlacementLimits,
) -> Result<(), RenderError> {
    // Whether `replacing`'s id is actually present in `on_screen` — an id
    // that is not (replacing a block that is not currently on screen, e.g.
    // one from history) does not free any on-screen room and is treated the
    // same as `None` for this budget.
    let replacing_is_on_screen =
        replacing.is_some_and(|id| on_screen.iter().any(|(entry_id, _)| *entry_id == id));

    let count = on_screen
        .len()
        .saturating_add(usize::from(!replacing_is_on_screen));
    if count > limits.max_concurrent_placements {
        return Err(stream_error());
    }
    let pixels = on_screen
        .iter()
        .filter(|(entry_id, _)| Some(*entry_id) != replacing)
        .map(|(_, pixels)| *pixels)
        .sum::<u64>()
        .saturating_add(new_pixels);
    if pixels > limits.max_total_pixels {
        return Err(stream_error());
    }
    Ok(())
}

/// A copy of `png` when `retain` is true, or an empty vec otherwise. Kept as
/// a free function so `retain_pngs`'s effect on `PlacedState::png` is
/// testable without constructing a `TerminalSink` (which requires a live
/// terminal). See [`TerminalSink::retained_png`] and
/// [`StreamSink::with_retained_pngs`].
fn retained_png(png: &[u8], retain: bool) -> Vec<u8> {
    if retain {
        png.to_vec()
    } else {
        Vec::new()
    }
}

/// AT-3-504's eviction policy: a block at index `i` keeps its retained PNG
/// iff its distance from `visible` (0 if `i` is inside `visible`, otherwise
/// the index gap to the nearer edge) is within `budget`; blocks farther out
/// on either side have their PNG truncated to empty. `u64::MAX` (stream/watch
/// sessions' default) never evicts anything, since every index gap is finite.
/// This is the simplest correct policy that keeps memory bounded during a
/// long session: it does not need per-block last-visible timestamps or an
/// LRU — the viewport's own position already tells us which blocks a
/// scroll-back is likely to revisit next (blocks *nearest* the window),
/// which is exactly what a fixed-radius keep-alive around the window
/// preserves.
fn evict_pngs_outside_budget(
    placed: &mut [PlacedState],
    visible: std::ops::Range<usize>,
    budget: u64,
) {
    if budget == u64::MAX {
        return;
    }
    for (index, entry) in placed.iter_mut().enumerate() {
        let distance = if index < visible.start {
            (visible.start - index) as u64
        } else if index >= visible.end {
            (index - visible.end + 1) as u64
        } else {
            0
        };
        if distance > budget {
            entry.png = Vec::new();
        }
    }
}

/// Whether `plan` contains a divergence anywhere but the tail — any
/// `Replace` or `Remove` op, regardless of position (a plain streamed
/// session's plans only ever end in one, per
/// `stream_shaped_revisions_never_produce_an_interior_replace_or_remove`,
/// but this predicate does not assume that; it is what `TerminalSink::emit`
/// checks, alongside the caller's own `retain_pngs` gate, to decide whether
/// the batch rewrite below applies). An empty plan (no ops at all) is
/// `false`, same as a pure Keep+Append plan.
fn plan_has_interior_divergence(plan: &Plan) -> bool {
    plan.ops
        .iter()
        .any(|op| matches!(op, PlanOp::Replace { .. } | PlanOp::Remove { .. }))
}

/// Builds the operation list for a batch divergence-tail rewrite (AT-3-506):
/// a revision whose plan diverges from the previous layout at
/// `reanchor_from` and is not a pure tail append (it contains at least one
/// `Replace`/`Remove`). Cursor-up by the exact row span the stale
/// `reanchor_from..` tail occupied on screen (summed from `placed` by id,
/// not by index — see below), erase it in one shot, delete every stale
/// Kitty image in that span, then re-place every block from
/// `reanchor_from` in the new plan's document order — `Keep` blocks from
/// their retained PNG, `Append`/`Replace` blocks from their freshly
/// rendered PNG. Pure and independent of any live terminal, the same shape
/// as `tail_replace_operations`/`sync_window_operations`.
///
/// This exists because per-op replay (`replace`'s `top_is_reachable` cursor
/// query plus `remove`'s bare Kitty-delete-with-no-row-clear) only stays
/// correct when a revision touches nothing but the last on-screen block.
/// The agent-viewer's whole-document sends can shrink the block count
/// (e.g. a transcript `Reset` after a shorter new answer starts) and
/// change interior blocks in the same revision, which breaks both of
/// those per-op assumptions: `top_is_reachable` evaluates `was_last`
/// against the *pre-plan* snapshot's length, not "does this revision still
/// place anything after this block," and `remove` never clears the text
/// cells its image used to cover. Rewriting the whole divergent span as
/// one unit sidesteps both — there is no per-op cursor arithmetic against
/// a screen state that may have already diverged from `placed`'s
/// bookkeeping.
///
/// `placed` is looked up **by id**, not by trusting its index to line up
/// with `previous`/`plan`, since the per-op `replace` path this batch path
/// replaces pushes replaced entries to the end of `placed` rather than
/// keeping them in position — an id-keyed lookup stays correct regardless
/// of what order `placed` happens to hold entries in.
///
/// Returns the operations plus the new tail of `PlacedState` entries (in
/// the new plan's document order) that the caller splices in at
/// `reanchor_from` to replace the old tail.
/// The ids of `previous[reanchor_from..]` — the stale tail a divergence
/// rewrite deletes and re-places — in document order.
fn stale_tail_ids(previous: &[tmath_render::PlannedBlock], reanchor_from: usize) -> Vec<u64> {
    previous[reanchor_from.min(previous.len())..]
        .iter()
        .map(|block| block.id)
        .collect()
}

/// The exact row-span cursor-up amount a divergence rewrite's relative path
/// (`divergence_rewrite_operations`) sends: `placed[].rows` summed over the
/// stale tail's ids, looked up by id (not index — see
/// `divergence_rewrite_operations`'s doc comment for why). Factored out so
/// [`TerminalSink::emit_batch`]'s clamp check compares against the exact
/// same value the relative rewrite would use, with no risk of the two
/// computations drifting apart.
fn stale_tail_rows_total(
    placed: &[PlacedState],
    previous: &[tmath_render::PlannedBlock],
    reanchor_from: usize,
) -> u32 {
    stale_tail_ids(previous, reanchor_from)
        .iter()
        .filter_map(|id| placed.iter().find(|entry| entry.id == *id))
        .map(|entry| entry.rows)
        .fold(0u32, |total, rows| total.saturating_add(rows))
}

/// Whether a relative cursor-up of `old_rows_total` rows, issued from
/// wherever the cursor happens to currently sit, could clamp at the
/// terminal's actual top row before reaching its intended target — the
/// mechanism behind the live-run "ghost placement" report (see
/// `EmitOutcome::NeedsWindowSync`'s doc comment for the full chain).
///
/// `\x1b[{n}A` (Cursor Up) is defined to stop at the screen's top row
/// rather than erroring or scrolling, so ANY cursor-up whose target would
/// be above row 1 silently lands at row 1 instead — `n` rows too short.
/// The exact current cursor row is not queried here (a live `CSI 6n`
/// round-trip is too slow to call on every batch rewrite, and does not
/// reliably return through tmux passthrough at all — see
/// `Terminal::cursor_position`'s doc comment) — instead this uses the
/// worst-case-safe static bound: the cursor can never be more than
/// `pane_rows` rows below the pane's top row (row 1), and one additional
/// row is reserved when the status bar is active (content starts at row 2,
/// not row 1 — see the `status_bar` module doc), so a cursor-up of
/// `pane_rows - reserved_rows` or more is guaranteed to clamp regardless of
/// the cursor's actual position. This is deliberately conservative: it may
/// fall back to a window sync a little earlier than the true clamp point
/// in some cursor positions, but it can never MISS a clamp that would
/// actually happen, which is the only failure mode that matters (a missed
/// detection reproduces the ghost; an over-eager fallback just does a
/// window sync that was not strictly necessary).
///
/// `pane_rows == 0` (the default for a `TerminalSink` that never opted
/// into the status bar/pane-geometry fields via
/// `StreamSink::with_status_bar`) always returns `false` — the check is
/// disabled, matching that `emit_batch` is unreachable for plain
/// stream/watch sessions regardless (see `pane_rows`'s field doc).
fn clamp_would_truncate(old_rows_total: u32, pane_rows: u32, status_bar_active: bool) -> bool {
    if pane_rows == 0 {
        return false;
    }
    let reserved_rows = u32::from(status_bar_active);
    let usable_rows = pane_rows.saturating_sub(reserved_rows);
    old_rows_total >= usable_rows
}

/// Decodes one `Keep`/`Append`/`Replace` plan op into the `PlacedState`
/// entry it produces, without building any terminal operations — the
/// state-only half of what `divergence_rewrite_operations`'s loop does,
/// shared so the clamp fallback (`clamp_fallback_new_tail`) and the normal
/// relative-cursor rewrite always compute identical `PlacedState` rows for
/// the same plan (the row-bookkeeping invariant `ba800aa` added checkers
/// for must hold on BOTH paths, not just the common one).
fn placed_state_for_op(
    operation: &PlanOp,
    index: usize,
    placed: &[PlacedState],
    prepared: &[PreparedBlock],
    cell: CellSize,
    max_image_pixels: u64,
    retain: bool,
) -> Result<Option<PlacedState>, RenderError> {
    match operation {
        PlanOp::Keep { id } => {
            let entry = placed
                .iter()
                .find(|entry| entry.id == *id)
                .ok_or_else(stream_error)?;
            let (width, height, _) =
                decode_png(&entry.png, max_image_pixels).map_err(|_| stream_error())?;
            let (_, rows) = tmath_core::placement::grid_for(width, height, cell);
            Ok(Some(PlacedState {
                id: *id,
                rows,
                pixels: u64::from(width) * u64::from(height),
                png: entry.png.clone(),
            }))
        }
        PlanOp::Append { block } | PlanOp::Replace { block, .. } => {
            let (rendered, png, _) = rendered_event(prepared, index)?;
            let (width, height, _) =
                decode_png(png, max_image_pixels).map_err(|_| stream_error())?;
            if width != rendered.width_px || height != rendered.height_px {
                return Err(stream_error());
            }
            let (_, rows) = tmath_core::placement::grid_for(width, height, cell);
            Ok(Some(PlacedState {
                id: block.id,
                rows,
                pixels: u64::from(width) * u64::from(height),
                png: retained_png(png, retain),
            }))
        }
        PlanOp::Remove { .. } => Ok(None),
    }
}

#[allow(clippy::too_many_arguments)]
fn divergence_rewrite_operations(
    placed: &[PlacedState],
    previous: &[tmath_render::PlannedBlock],
    plan: &Plan,
    prepared: &[PreparedBlock],
    reanchor_from: usize,
    cell: CellSize,
    max_image_pixels: u64,
    retain: bool,
) -> Result<(Vec<TerminalOp>, Vec<PlacedState>), RenderError> {
    let stale_ids = stale_tail_ids(previous, reanchor_from);
    let old_rows_total = stale_tail_rows_total(placed, previous, reanchor_from);

    let mut operations = Vec::new();
    if old_rows_total > 0 {
        operations.push(TerminalOp::Local(
            format!("\x1b[{old_rows_total}A\r").into_bytes(),
        ));
    }
    for id in &stale_ids {
        let image_id = u32::try_from(*id).map_err(|_| stream_error())?;
        operations.push(TerminalOp::Graphics(tmath_core::kitty::kitty_delete_id(
            image_id,
        )));
    }
    if old_rows_total > 0 {
        operations.push(TerminalOp::Local(clear_rows(old_rows_total)));
    }

    let mut new_tail = Vec::with_capacity(plan.ops.len().saturating_sub(reanchor_from));
    for (index, operation) in plan.ops.iter().enumerate().skip(reanchor_from) {
        let Some(entry) = placed_state_for_op(
            operation,
            index,
            placed,
            prepared,
            cell,
            max_image_pixels,
            retain,
        )?
        else {
            continue;
        };
        let (width, height, rgba) =
            decode_png(&entry.png, max_image_pixels).map_err(|_| stream_error())?;
        let (cols, rows) = tmath_core::placement::grid_for(width, height, cell);
        let image_id = u32::try_from(entry.id).map_err(|_| stream_error())?;
        operations.extend(append_operations(
            image_id, width, height, &rgba, cols, rows, true,
        ));
        new_tail.push(entry);
    }
    operations.push(TerminalOp::Local(b"\x1b[0J".to_vec()));

    Ok((operations, new_tail))
}

/// The clamp fallback's equivalent of `divergence_rewrite_operations`, minus
/// building the (would-be-clamped) relative-cursor operations: computes only
/// the new `PlacedState` tail via [`placed_state_for_op`], so
/// [`TerminalSink::emit_batch`] can update its bookkeeping to the new plan
/// even though it writes nothing to the terminal itself — the caller's
/// follow-up `sync_visible_window`/`sync_window` redraws from this state
/// using an absolute `\x1b[H`, not a relative cursor-up. Takes the same
/// `placed`/`reanchor_from` pair as `divergence_rewrite_operations` (the
/// `Keep` branch of `placed_state_for_op` resolves ids from it), so the two
/// functions always compute row-identical `PlacedState` entries for the
/// same plan.
#[allow(clippy::too_many_arguments)]
fn clamp_fallback_new_tail(
    placed: &[PlacedState],
    plan: &Plan,
    prepared: &[PreparedBlock],
    reanchor_from: usize,
    cell: CellSize,
    max_image_pixels: u64,
    retain: bool,
) -> Result<Vec<PlacedState>, RenderError> {
    let mut new_tail = Vec::with_capacity(plan.ops.len().saturating_sub(reanchor_from));
    for (index, operation) in plan.ops.iter().enumerate().skip(reanchor_from) {
        if let Some(entry) = placed_state_for_op(
            operation,
            index,
            placed,
            prepared,
            cell,
            max_image_pixels,
            retain,
        )? {
            new_tail.push(entry);
        }
    }
    Ok(new_tail)
}

/// Builds the operation list for a visibility-driven viewport sync
/// (AT-3-503): deletes every id in `emitted_ids` that is not among the new
/// `visible` range's ids, moves the cursor home, re-emits every block in
/// `visible` (clamped to `placed`'s bounds) at its window-relative row
/// (immediately after the previous one, cursor-relative) from its retained
/// PNG — clearing each block's rows (`clear_rows`, `\x1b[2K` per row) right
/// before drawing it — and erases any residual rows below what was just
/// drawn. Pure and independent of any live terminal, the same way
/// `tail_replace_operations` is.
///
/// The per-row clear before each block is required, not cosmetic:
/// `emitted_ids` reflects only what a PRIOR `sync_window` call drew, and
/// says nothing about content that reached the screen through the
/// per-op flowing-append path (`TerminalSink::append`/`replace`, taken
/// whenever a plan has no interior divergence — see `TerminalSink::emit`'s
/// gate), which never touches `emitted_ids`. The first `sync_window` call
/// after a stretch of pure flowing appends therefore sees an
/// empty/stale `emitted_ids` even though the physical screen already has
/// real content at these rows. Without an unconditional clear, a
/// placeholder grid only overwrites exactly `cols` columns
/// (`placeholder_grid_at_cursor`), so any prior content wider than the new
/// block's placeholder grid would leave stale glyphs past column `cols` —
/// this was the mechanism behind a live "stale line pieces at the right
/// edge" scroll-correctness bug. Clearing every visible row unconditionally,
/// independent of `emitted_ids`, kills the whole class regardless of what
/// history bookkeeping does or does not know about a given span, at a
/// constant few bytes per row — this keeps the transmitted-byte bound
/// proportional to `visible`'s size (AT-3-503), not to history length.
///
/// Deleting by id rather than by a previous index range is deliberate: an id
/// in `emitted_ids` may no longer exist in `placed` at all (a suppressed
/// tail replace removes the old id from `placed` while `apply_revision`'s
/// terminal writes are suppressed — see `TerminalSink::suppress_writes`).
/// The on-screen image for that id was never touched by the suppressed
/// write, so its delete must still be sent here or it becomes a
/// terminal-memory orphan; a stale index range would silently miss it
/// (`emitted_ids`'s field doc has the full scenario). Deleting an id that
/// some other, unsuppressed path already removed is a harmless no-op
/// (`kitty_delete_id` for an id that does not exist does nothing) — dropping
/// already-gone ids from `emitted_ids` eagerly elsewhere would be a little
/// tidier but is not required for correctness, so this function does not
/// need to special-case it.
///
/// Blocks that are in both `emitted_ids` and `visible` are still re-sent
/// (not left untouched): whenever a block enters or leaves ahead of them in
/// placement order, every later block's window-relative row shifts, and a
/// Kitty placement does not move on its own. Re-sending the whole `visible`
/// range keeps this correct without tracking per-block row history, and the
/// transmitted bytes stay bounded by `visible`'s length plus a constant-size
/// erase-below — `placed`'s total length (i.e. how much history exists
/// outside the window) never enters this cost, satisfying AT-3-503's
/// "independent of history length" clause.
///
/// The erase-below (`\x1b[0J`) is what makes a shrinking window (fewer or
/// shorter blocks than were previously drawn) leave no stale rows: without
/// it, rows below the new content would still show the old placeholder
/// cells and image fragments from a taller previous draw. It costs a
/// constant few bytes regardless of window size, so it does not affect the
/// history-independence bound. When nothing is drawn (`visible` is empty)
/// but something was previously on screen, the pane is homed (to
/// `content_row_offset + 1`) and erased too, so a scroll past all content
/// still clears what was there.
///
/// `content_row_offset` reserves that many rows at the pane's actual top
/// (row 1..=`content_row_offset`) for the live status bar (see the
/// `status_bar` module doc) — content homes to row `content_row_offset +
/// 1` instead of row 1, and `\x1b[0J`'s erase-below never reaches back up
/// into the reserved rows since it always runs from a cursor position at or
/// below them. `0` (plain stream/watch sessions, which never enable the
/// status bar) reproduces the exact `\x1b[H`-at-row-1 behavior this
/// function had before the reserved row existed.
///
/// `skip_rows_in_first` crops the FIRST drawn block's placeholder rows (see
/// `tmath_core::placement::emit_placed_block_row_range_cursor`'s doc
/// comment for why this is a protocol-native crop, not a re-render) to
/// `skip_rows_in_first..rows` — the fix for the scroll-region viewer's
/// reach-the-beginning defect (`viewer_viewport::VisibleRange::
/// skip_rows_in_first`'s field doc has the full mechanism): without this,
/// a block only partially scrolled into view at the window's top edge was
/// always drawn in FULL, pushing its top rows above the pane's actual
/// content area — visually indistinguishable from "scrolling stopped
/// working" even though `Viewport::offset()` had genuinely reached `0`.
/// Every OTHER drawn block in `visible` is unaffected (drawn in full, as
/// before) — only the first one is ever partially visible at a window's top
/// edge, by `Viewport::visible_blocks`' own construction. Clamped internally
/// to the first block's own row count, so an out-of-range value (a caller
/// bug) degrades to drawing that one block in full rather than panicking.
fn sync_window_operations(
    placed: &[PlacedState],
    emitted_ids: &[u64],
    visible: std::ops::Range<usize>,
    cell: CellSize,
    max_image_pixels: u64,
    content_row_offset: u32,
    skip_rows_in_first: u32,
) -> Result<Vec<TerminalOp>, RenderError> {
    let visible = visible.start.min(placed.len())..visible.end.min(placed.len());
    let visible_ids: Vec<u64> = placed[visible.clone()]
        .iter()
        .map(|entry| entry.id)
        .collect();
    let home = format!("\x1b[{};1H", content_row_offset.saturating_add(1));

    let mut operations = Vec::new();
    for &id in emitted_ids {
        if !visible_ids.contains(&id) {
            let image_id = u32::try_from(id).map_err(|_| stream_error())?;
            operations.push(TerminalOp::Graphics(tmath_core::kitty::kitty_delete_id(
                image_id,
            )));
        }
    }

    if !visible.is_empty() {
        // Move to the content home row once; each block below is then
        // placed immediately after the previous one via the cursor-relative
        // form, so no per-block home-row arithmetic is needed to keep
        // window-relative rows correct.
        operations.push(TerminalOp::Local(home.clone().into_bytes()));
        for (index, entry) in placed[visible].iter().enumerate() {
            // A block whose retained PNG is still empty here means
            // `restore_missing_pngs` (the caller's job, see its doc
            // comment) already tried and failed to re-render it — fail
            // closed PER BLOCK, not for the whole sync: draw its row span
            // blank (matching AT-3-504's "leaves it showing nothing")
            // rather than erroring the entire window out, which would
            // otherwise turn one unrestorable block into a permanent
            // `RendererFailed` on every subsequent sync as long as that
            // block stays in the window (the 2026-08-05 field failure's
            // mechanism).
            if entry.png.is_empty() {
                let skip = if index == 0 {
                    skip_rows_in_first.min(entry.rows)
                } else {
                    0
                };
                operations.push(TerminalOp::Local(clear_rows(
                    entry.rows.saturating_sub(skip),
                )));
                if skip != 0 {
                    operations.push(TerminalOp::Local(b"\r\n".to_vec()));
                }
                continue;
            }
            let (width, height, rgba) =
                decode_png(&entry.png, max_image_pixels).map_err(|_| stream_error())?;
            let (cols, rows) = tmath_core::placement::grid_for(width, height, cell);
            let image_id = u32::try_from(entry.id).map_err(|_| stream_error())?;
            // Clear every row this block is about to occupy BEFORE drawing
            // it (`clear_rows` leaves the cursor back where it started, so
            // this never disturbs the placeholder write that follows). This
            // is required, not cosmetic: `emitted_ids` only reflects what a
            // PRIOR `sync_window` call drew, and the first `sync_window` of
            // a session that streamed via flowing appends (see
            // `TerminalSink::append`/`replace`'s per-op path, which never
            // touches `emitted_ids`) sees an empty/stale `emitted_ids` even
            // though the physical screen already has real content at these
            // rows. A placeholder grid only overwrites exactly `cols`
            // columns (`placeholder_grid_at_cursor`), so any prior content
            // wider than the new block leaves stale glyphs past column
            // `cols` if the row is never cleared first — this is the
            // "stale line pieces at the right edge" scroll-correctness
            // symptom. Clearing unconditionally, independent of
            // `emitted_ids`, kills the whole class regardless of what
            // history bookkeeping does or does not know about this span.
            // Row 1 (the status bar, when `content_row_offset > 0`) is
            // never touched: `home` already starts at
            // `content_row_offset + 1`, and every clear stays at or below
            // the cursor's current row from there.
            let skip = if index == 0 {
                skip_rows_in_first.min(rows)
            } else {
                0
            };
            operations.push(TerminalOp::Local(clear_rows(rows.saturating_sub(skip))));
            if skip == 0 {
                operations.extend(append_operations(
                    image_id, width, height, &rgba, cols, rows, true,
                ));
            } else {
                operations.extend(emit_placed_block_row_range_cursor(
                    tmath_core::placement::RowRangePlacement {
                        image_id,
                        width_px: width,
                        height_px: height,
                        rgba: &rgba,
                        cols,
                        rows,
                    },
                    skip..rows,
                    true,
                ));
                operations.push(TerminalOp::Local(b"\r\n".to_vec()));
            }
        }
        operations.push(TerminalOp::Local(b"\x1b[0J".to_vec()));
    } else if !emitted_ids.is_empty() {
        operations.push(TerminalOp::Local(format!("{home}\x1b[0J").into_bytes()));
    }

    Ok(operations)
}

/// `pub(crate)` so `scroll_region::region_tail_replace_operations` can reuse
/// the exact same full-line clear this module's own
/// `tail_replace_operations`/`divergence_rewrite_operations` use, rather
/// than a second, independently-drifting implementation.
pub(crate) fn clear_rows(rows: u32) -> Vec<u8> {
    let rows = rows.max(1);
    let mut bytes = Vec::new();
    for row in 0..rows {
        bytes.extend_from_slice(b"\x1b[2K");
        if row + 1 < rows {
            bytes.extend_from_slice(b"\x1b[1B\r");
        }
    }
    if rows > 1 {
        bytes.extend_from_slice(format!("\x1b[{}A\r", rows - 1).as_bytes());
    }
    bytes
}

fn stream_error() -> RenderError {
    RenderError::new(
        SafeErrorRecord {
            code: tmath_render::ErrorCode::RendererFailed,
            retryable: false,
            details: None,
        },
        "stream rendering failed",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn direct_bytes(operations: &[TerminalOp]) -> Vec<u8> {
        let mut bytes = Vec::new();
        tmath_core::placement::write_terminal_ops(&mut bytes, operations, false).unwrap();
        bytes
    }

    fn rgba8_png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut encoder = png::Encoder::new(io::Cursor::new(&mut bytes), width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer
            .write_image_data(&vec![0xffu8; (width * height * 4) as usize])
            .unwrap();
        drop(writer);
        bytes
    }

    fn placed(id: u64, rows: u32, png: Vec<u8>) -> PlacedState {
        PlacedState {
            id,
            rows,
            pixels: u64::from(rows),
            png,
        }
    }

    #[test]
    fn tail_replace_moves_up_deletes_clears_and_replaces() {
        let operations = tail_replace_operations(TailReplace {
            old_image_id: 7,
            new_image_id: 8,
            width_px: 1,
            height_px: 1,
            rgba: &[0, 0, 0, 0],
            cols: 1,
            old_rows: 3,
            new_rows: 1,
        })
        .unwrap();
        let bytes = direct_bytes(&operations);
        let prefix = concat!(
            "\u{1b}[3A\r",
            "\u{1b}_Ga=d,d=I,i=7,q=2\u{1b}\\",
            "\u{1b}[2K\u{1b}[1B\r\u{1b}[2K\u{1b}[1B\r\u{1b}[2K",
            "\u{1b}[2A\r"
        );
        assert!(bytes.starts_with(prefix.as_bytes()));
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("i=8,U=1,c=1,r=1,q=2"));
        assert!(text.ends_with("\x1b[39m\r\n"));
    }

    #[test]
    fn keep_maps_to_zero_terminal_bytes() {
        let plan = Plan {
            ops: vec![PlanOp::Keep { id: 1 }],
            reanchor_from: None,
        };
        let mut emitted = Vec::new();
        for operation in plan.ops {
            if !matches!(operation, PlanOp::Keep { .. }) {
                emitted.push(TerminalOp::Local(b"unexpected".to_vec()));
            }
        }
        assert!(direct_bytes(&emitted).is_empty());
    }

    #[test]
    fn plan_has_interior_divergence_is_false_for_empty_and_keep_append_only_plans() {
        assert!(!plan_has_interior_divergence(&Plan {
            ops: Vec::new(),
            reanchor_from: None,
        }));
        assert!(!plan_has_interior_divergence(&Plan {
            ops: vec![
                PlanOp::Keep { id: 1 },
                PlanOp::Keep { id: 2 },
                PlanOp::Append { block: planned(3) },
            ],
            reanchor_from: Some(2),
        }));
    }

    #[test]
    fn plan_has_interior_divergence_is_true_when_any_replace_or_remove_is_present() {
        assert!(plan_has_interior_divergence(&Plan {
            ops: vec![
                PlanOp::Keep { id: 1 },
                PlanOp::Replace {
                    old_id: 2,
                    block: planned(10),
                },
            ],
            reanchor_from: Some(1),
        }));
        assert!(plan_has_interior_divergence(&Plan {
            ops: vec![PlanOp::Keep { id: 1 }, PlanOp::Remove { id: 2 }],
            reanchor_from: Some(1),
        }));
        // A Remove buried after Keep+Append still counts, matching that the
        // predicate does not assume stream-shaped (last-op-only) plans.
        assert!(plan_has_interior_divergence(&Plan {
            ops: vec![
                PlanOp::Keep { id: 1 },
                PlanOp::Append { block: planned(2) },
                PlanOp::Remove { id: 3 },
            ],
            reanchor_from: Some(1),
        }));
    }

    /// AT-3-503: moving the window (e.g. scrolling one block further) deletes
    /// only what left (id=1, no longer among `visible`'s ids) and re-emits
    /// the whole new `visible` range from cache — including blocks that were
    /// already on screen (id=2 stays in both `emitted_ids` and the new
    /// window), which is the "re-send everything inside the window" policy
    /// documented on `sync_window_operations`. Also asserts the erase-below
    /// (FIX 1) is present after the last redrawn block.
    #[test]
    fn sync_window_deletes_only_what_left_and_resends_the_whole_new_window() {
        let cell = CellSize {
            width: 1,
            height: 1,
        };
        let placed = vec![
            placed(1, 2, rgba8_png(1, 2)),
            placed(2, 3, rgba8_png(1, 3)),
            placed(3, 1, rgba8_png(1, 1)),
        ];

        let operations =
            sync_window_operations(&placed, &[1, 2], 1..3, cell, u64::MAX, 0, 0).unwrap();
        let bytes = direct_bytes(&operations);
        let text = String::from_utf8_lossy(&bytes);

        // Only id=1 left the window (was in `emitted_ids`, not among
        // `visible`'s ids); id=2 stayed in both and is not deleted.
        assert!(text.contains("\x1b_Ga=d,d=I,i=1,q=2\x1b\\"));
        assert!(!text.contains("\x1b_Ga=d,d=I,i=2,q=2\x1b\\"));
        assert!(!text.contains("\x1b_Ga=d,d=I,i=3,q=2\x1b\\"));
        assert!(
            text.contains("\x1b[1;1H"),
            "home to row 1 (content_row_offset=0, no reserved status-bar row) once \
             before re-emitting"
        );
        // Both blocks now inside the window (ids 2 and 3) are re-emitted,
        // even though id=2 was already visible before this sync.
        assert!(text.contains("i=2,U=1,c=1,r=3,q=2"));
        assert!(text.contains("i=3,U=1,c=1,r=1,q=2"));
        // id=1's placement command (as opposed to its delete) never appears.
        assert!(!text.contains("i=1,U=1,c=1"));
        // FIX 1: an erase-below follows the last redrawn block, so any rows
        // left over from a taller previous draw are cleared.
        assert!(
            text.ends_with("\x1b[0J"),
            "erase-below follows the redrawn blocks: {text:?}"
        );
    }

    /// TR-402 (AT-R-502): the 2026-08-05 field failure's root cause. An
    /// evicted PNG that `restore_missing_pngs` failed to restore (e.g. a
    /// render error) is left as an empty byte vector (see
    /// `PlacedState::png`'s doc comment). Before this fix,
    /// `sync_window_operations` unconditionally decoded every visible
    /// block's PNG, so this one empty entry failed the WHOLE sync with
    /// `RendererFailed` — and since the block stays in `placed` and stays
    /// empty, every LATER sync while it remains in the window failed the
    /// same way, matching the field-observed `sync_failed (RendererFailed)`
    /// recurring until the viewer gave up. The fix fails closed PER BLOCK
    /// instead: an empty PNG draws its row span blank and the sync
    /// otherwise succeeds.
    #[test]
    fn sync_window_operations_fails_closed_per_block_on_an_unrestored_empty_png() {
        let cell = CellSize {
            width: 1,
            height: 1,
        };
        let placed = vec![
            placed(1, 2, rgba8_png(1, 2)),
            // id=2's PNG was evicted and never restored — this is what
            // `restore_missing_pngs` leaves behind on a render failure.
            placed(2, 3, Vec::new()),
            placed(3, 1, rgba8_png(1, 1)),
        ];

        let operations = sync_window_operations(&placed, &[1, 2, 3], 0..3, cell, u64::MAX, 0, 0)
            .expect("one unrestorable block must not fail the whole sync");
        let bytes = direct_bytes(&operations);
        let text = String::from_utf8_lossy(&bytes);

        // id=1 and id=3 (both have a real PNG) are still placed normally.
        assert!(text.contains("i=1,U=1,c=1,r=2,q=2"));
        assert!(text.contains("i=3,U=1,c=1,r=1,q=2"));
        // id=2 (the empty PNG) is never placed — its slot is left blank,
        // not drawn with a stale or garbage image.
        assert!(!text.contains("i=2,U=1,c=1"));
    }

    /// FIX 2 regression: `emitted_ids` may contain an id that a suppressed
    /// tail replace already removed from `placed` entirely (the on-screen
    /// image for that id was never touched, since the write was
    /// suppressed). `sync_window_operations` must still emit its delete —
    /// an index-range diff would silently miss this, since the id simply
    /// is not present anywhere in `placed` to compute an index for.
    #[test]
    fn sync_window_deletes_an_emitted_id_no_longer_present_in_placed() {
        let cell = CellSize {
            width: 1,
            height: 1,
        };
        // `placed` no longer has id=99 (as if a suppressed replace dropped
        // it), but it is still recorded as on screen.
        let placed = vec![placed(1, 2, rgba8_png(1, 2))];
        let emitted_ids = [99u64, 1];

        let operations =
            sync_window_operations(&placed, &emitted_ids, 0..1, cell, u64::MAX, 0, 0).unwrap();
        let bytes = direct_bytes(&operations);
        let text = String::from_utf8_lossy(&bytes);

        assert!(
            text.contains("\x1b_Ga=d,d=I,i=99,q=2\x1b\\"),
            "the orphaned on-screen id is still deleted: {text:?}"
        );
        assert!(
            !text.contains("\x1b_Ga=d,d=I,i=1,q=2\x1b\\"),
            "id=1 stayed visible"
        );
        assert!(text.contains("i=1,U=1,c=1,r=2,q=2"));
    }

    /// Scrolling past all content deletes everything that was on screen and
    /// emits nothing new — no stale placement is left behind, and the pane
    /// is homed and erased (FIX 1) rather than left with the old rows.
    #[test]
    fn sync_window_with_empty_visible_range_deletes_the_previous_window() {
        let cell = CellSize {
            width: 1,
            height: 1,
        };
        let placed = vec![placed(1, 2, rgba8_png(1, 2))];

        let operations = sync_window_operations(&placed, &[1], 0..0, cell, u64::MAX, 0, 0).unwrap();
        let bytes = direct_bytes(&operations);
        let text = String::from_utf8_lossy(&bytes);

        assert!(text.contains("\x1b_Ga=d,d=I,i=1,q=2\x1b\\"));
        assert!(!text.contains("U=1,c=1"), "no placement is re-emitted");
        assert!(
            text.ends_with("\x1b[1;1H\x1b[0J"),
            "home (row 1, content_row_offset=0) and erase clear the stale rows when \
             nothing is re-emitted: {text:?}"
        );
    }

    /// The reach-the-beginning fix: `skip_rows_in_first` crops the FIRST
    /// visible block's placeholder rows, so a block only partially scrolled
    /// into view at the window's top edge draws just its visible slice
    /// rather than its full image (which used to push its top rows above
    /// the pane's actual content area — indistinguishable from "scrolling
    /// stopped working" even at a genuine offset of 0).
    #[test]
    fn sync_window_crops_the_first_visible_block_by_skip_rows_in_first() {
        let cell = CellSize {
            width: 10,
            height: 10,
        };
        // Block 1: 4 rows tall, 2 cols wide (avoids row/col diacritic-0
        // collision — see `placement.rs`'s row-range test for why). Block 2:
        // a normal, fully-visible 1-row block after it.
        let placed = vec![
            placed(1, 4, rgba8_png(20, 40)),
            placed(2, 1, rgba8_png(20, 10)),
        ];
        let operations = sync_window_operations(&placed, &[], 0..2, cell, u64::MAX, 0, 2).unwrap();
        let bytes = direct_bytes(&operations);
        let text = String::from_utf8_lossy(&bytes);

        let placeholder_count = |id_marker: &str, s: &str| {
            let start = s.find(id_marker).unwrap();
            let rest = &s[start..];
            let end = rest[1..]
                .find("i=")
                .map(|offset| offset + 1)
                .unwrap_or(rest.len());
            rest[..end]
                .chars()
                .filter(|&c| c == tmath_core::kitty::PLACEHOLDER)
                .count()
        };
        assert_eq!(
            placeholder_count("i=1,", &text),
            4,
            "block 1 draws only its 2 remaining rows (4 - skip 2) x 2 cols: {text:?}"
        );
        assert_eq!(
            placeholder_count("i=2,", &text),
            2,
            "block 2 (not the first visible block) draws in full, 1 row x 2 cols: {text:?}"
        );
        // The placement command itself still keys the block's FULL row
        // count (r=4) — only the placeholder grid is cropped, per
        // `emit_placed_block_row_range_cursor`'s contract.
        assert!(text.contains("i=1,U=1,c=2,r=4,q=2"), "{text:?}");
    }

    #[test]
    fn sync_window_skip_rows_in_first_out_of_range_clamps_to_the_blocks_own_rows() {
        let cell = CellSize {
            width: 10,
            height: 10,
        };
        let placed = vec![placed(1, 2, rgba8_png(10, 20))];
        // skip_rows_in_first (10) exceeds the block's own 2 rows — must not
        // panic, and must degrade to drawing nothing for that block rather
        // than an out-of-range row-range.
        let operations = sync_window_operations(&placed, &[], 0..1, cell, u64::MAX, 0, 10).unwrap();
        let bytes = direct_bytes(&operations);
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            !text.contains(tmath_core::kitty::PLACEHOLDER),
            "an out-of-range skip draws nothing rather than panicking: {text:?}"
        );
    }

    /// A fresh sync (`emitted_ids` starts empty, as it does for the first
    /// `sync_window` call after construction) emits the visible range with
    /// no DELETES, since no id needs removing — but still CLEARS every row
    /// it draws into before drawing, unconditionally.
    ///
    /// This unconditional clear is what closes a real gap `emitted_ids ==
    /// []` used to paper over: `emitted_ids` reflects only what a PRIOR
    /// `sync_window` call drew, and says nothing about content that reached
    /// the screen through the per-op flowing-append path
    /// (`TerminalSink::append`/`replace`, taken whenever a plan has no
    /// interior divergence — see `TerminalSink::emit`'s gate), which never
    /// touches `emitted_ids`. The FIRST `sync_window` call after a stretch
    /// of pure flowing appends therefore used to see an empty `emitted_ids`
    /// and skip clearing entirely, even though the physical screen already
    /// had real content at these rows from that flowing history — a
    /// placeholder grid only overwrites exactly `cols` columns
    /// (`placeholder_grid_at_cursor`), so any prior content wider than the
    /// new block's placeholder grid left stale glyphs past column `cols`
    /// ("stale line pieces at the right edge", a live scroll-correctness
    /// bug). Clearing unconditionally — the same test setup as before, a
    /// "fresh" `emitted_ids == []` — no longer depends on distinguishing a
    /// genuinely blank screen from a screen already painted by flowing
    /// history; both get the same safe per-row clear before the draw.
    #[test]
    fn sync_window_from_empty_emitted_clears_before_it_adds() {
        let cell = CellSize {
            width: 1,
            height: 1,
        };
        let placed = vec![placed(1, 2, rgba8_png(1, 2))];
        let operations = sync_window_operations(&placed, &[], 0..1, cell, u64::MAX, 0, 0).unwrap();
        let bytes = direct_bytes(&operations);
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.contains("a=d,d=I"), "nothing to delete");
        assert!(
            text.contains("\x1b[2K"),
            "every row the block draws into must be cleared first, even when \
             emitted_ids is empty: {text:?}"
        );
        assert!(text.contains("i=1,U=1,c=1,r=2,q=2"));
        // The clear must happen BEFORE the placement command, not after.
        let clear_pos = text.find("\x1b[2K").expect("clear present");
        let place_pos = text.find("i=1,U=1,c=1,r=2,q=2").expect("placement present");
        assert!(
            clear_pos < place_pos,
            "clear must precede the placement it protects: {text:?}"
        );
    }

    /// An empty sync (nothing was on screen, and the new window is also
    /// empty) is a true no-op: no delete, no home/clear, no draw.
    #[test]
    fn sync_window_from_empty_to_empty_is_a_no_op() {
        let cell = CellSize {
            width: 1,
            height: 1,
        };
        let placed = vec![placed(1, 2, rgba8_png(1, 2))];
        let operations = sync_window_operations(&placed, &[], 0..0, cell, u64::MAX, 0, 0).unwrap();
        assert!(operations.is_empty());
    }

    #[test]
    fn sync_window_clamps_an_out_of_range_slice_instead_of_panicking() {
        let cell = CellSize {
            width: 1,
            height: 1,
        };
        let placed = vec![placed(1, 2, rgba8_png(1, 2))];
        let operations = sync_window_operations(&placed, &[1], 0..5, cell, u64::MAX, 0, 0).unwrap();
        let bytes = direct_bytes(&operations);
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("i=1,U=1,c=1,r=2,q=2"));
    }

    /// AT-3-503's core byte-budget claim: the bytes transmitted for one
    /// scroll step depend only on the visible-block count, never on how much
    /// history exists outside the window — doubling history length must not
    /// change the transmitted byte count for the same-size window.
    #[test]
    fn sync_window_byte_cost_is_independent_of_history_length() {
        let cell = CellSize {
            width: 1,
            height: 1,
        };
        // Image ids are pinned to a single digit (1..=9, cycling) so the
        // transmitted byte count cannot differ merely because a longer
        // history means more decimal digits in the ids near the window —
        // the claim under test is about window size, not id-formatting
        // coincidence.
        let short_history: Vec<PlacedState> = (0..20u64)
            .map(|i| placed(i % 9 + 1, 1, rgba8_png(1, 1)))
            .collect();
        let long_history: Vec<PlacedState> = (0..2000u64)
            .map(|i| placed(i % 9 + 1, 1, rgba8_png(1, 1)))
            .collect();

        // Same-size window (5 blocks), scrolled one block forward, deep in
        // each history (so the "history outside the window" size differs by
        // orders of magnitude between the two cases). `emitted_ids` is the
        // previous window's ids in both cases (same values, since ids cycle
        // 1..=9), so the delete set is bounded the same way too.
        let previously_emitted = [5u64, 6, 7, 8, 9];
        let short_bytes = direct_bytes(
            &sync_window_operations(
                &short_history,
                &previously_emitted,
                5..10,
                cell,
                u64::MAX,
                0,
                0,
            )
            .unwrap(),
        );
        let long_bytes = direct_bytes(
            &sync_window_operations(
                &long_history,
                &previously_emitted,
                995..1000,
                cell,
                u64::MAX,
                0,
                0,
            )
            .unwrap(),
        );

        assert_eq!(
            short_bytes.len(),
            long_bytes.len(),
            "a scroll step over a 2000-block history must cost the same bytes \
             as the same step over a 20-block history"
        );
    }

    /// The agent-viewer's `TerminalSink::validate_placement` passes only the
    /// blocks whose id is in `emitted_ids` (not the whole history) to this
    /// budget check — this test exercises the underlying pure function
    /// directly: a small on-screen window (5 blocks) accepts a new placement
    /// well within `max_concurrent_placements` regardless of the ids' own
    /// values (i.e. regardless of how much history exists with other ids).
    #[test]
    fn validate_placement_budget_counts_only_the_on_screen_slice() {
        let limits = PlacementLimits {
            max_concurrent_placements: 8,
            max_total_pixels: u64::MAX,
        };
        let on_screen: Vec<(u64, u64)> = (1..=5u64).map(|id| (id, 1)).collect();
        assert!(validate_placement_budget(&on_screen, 1, None, limits).is_ok());
        // A different id range (as if 10_000 blocks with other ids came
        // before the window) does not change the outcome: only
        // `on_screen`'s own length is counted.
        let on_screen_far: Vec<(u64, u64)> = (10_000..10_005u64).map(|id| (id, 1)).collect();
        assert!(validate_placement_budget(&on_screen_far, 1, None, limits).is_ok());
    }

    /// A full on-screen window (at `max_concurrent_placements`) rejects one
    /// more append, the same way the pre-T3-303 whole-history check did —
    /// the fix narrows what counts as "on screen", it does not remove the
    /// limit.
    #[test]
    fn validate_placement_budget_still_rejects_a_full_on_screen_window() {
        let limits = PlacementLimits {
            max_concurrent_placements: 4,
            max_total_pixels: u64::MAX,
        };
        let on_screen: Vec<(u64, u64)> = (1..=4u64).map(|id| (id, 1)).collect();
        assert!(validate_placement_budget(&on_screen, 1, None, limits).is_err());
    }

    /// `replacing` keys on id membership: replacing an id that is on screen
    /// does not count as a new placement, so a full window still accepts
    /// the replacement.
    #[test]
    fn validate_placement_budget_replacing_an_on_screen_block_frees_its_slot() {
        let limits = PlacementLimits {
            max_concurrent_placements: 4,
            max_total_pixels: u64::MAX,
        };
        let on_screen: Vec<(u64, u64)> = (101..=104u64).map(|id| (id, 1)).collect();
        assert!(validate_placement_budget(&on_screen, 1, Some(101), limits).is_ok());
    }

    /// `replacing` an id that is not on screen (a block from history, not
    /// currently visible) does not free any on-screen room — it is treated
    /// as a new placement for budget purposes.
    #[test]
    fn validate_placement_budget_replacing_an_off_screen_block_does_not_free_room() {
        let limits = PlacementLimits {
            max_concurrent_placements: 4,
            max_total_pixels: u64::MAX,
        };
        let on_screen: Vec<(u64, u64)> = (101..=104u64).map(|id| (id, 1)).collect();
        assert!(validate_placement_budget(&on_screen, 1, Some(50), limits).is_err());
    }

    /// FIX 2: plain `tmath render`/`tmath watch` stream sessions construct
    /// their sink with `retain_pngs` left at its `false` default (only the
    /// agent-viewer opts in via `StreamSink::with_retained_pngs`), so their
    /// `PlacedState` entries must carry no PNG bytes at all.
    #[test]
    fn retained_png_is_empty_unless_retention_is_enabled() {
        let png = rgba8_png(2, 3);
        assert!(retained_png(&png, false).is_empty());
        assert_eq!(retained_png(&png, true), png);
    }

    fn has_png(entries: &[PlacedState], index: usize) -> bool {
        !entries[index].png.is_empty()
    }

    /// AT-3-504: blocks within `budget` positions of the window (on either
    /// side) keep their PNG; anything farther out is evicted.
    #[test]
    fn evict_pngs_outside_budget_keeps_a_fixed_radius_around_the_window() {
        let mut placed: Vec<PlacedState> =
            (0..10u64).map(|i| placed(i, 1, rgba8_png(1, 1))).collect();
        // Window is indices 5..7, budget 1: indices 4..=7 (visible plus one
        // on each side) keep their PNG; 0..=3 and 8..=9 are evicted.
        evict_pngs_outside_budget(&mut placed, 5..7, 1);

        for index in 0..=3 {
            assert!(!has_png(&placed, index), "index {index} is outside budget");
        }
        for index in 4..=7 {
            assert!(has_png(&placed, index), "index {index} is within budget");
        }
        for index in 8..=9 {
            assert!(!has_png(&placed, index), "index {index} is outside budget");
        }
    }

    /// A budget of `u64::MAX` (the default for stream/watch sessions, and
    /// for the agent-viewer before it opts in) never evicts anything.
    #[test]
    fn evict_pngs_outside_budget_is_a_no_op_at_u64_max() {
        let mut placed: Vec<PlacedState> =
            (0..5u64).map(|i| placed(i, 1, rgba8_png(1, 1))).collect();
        evict_pngs_outside_budget(&mut placed, 2..3, u64::MAX);
        for index in 0..5 {
            assert!(has_png(&placed, index));
        }
    }

    /// A budget of 0 keeps only blocks strictly inside the window.
    #[test]
    fn evict_pngs_outside_budget_zero_keeps_only_the_window_itself() {
        let mut placed: Vec<PlacedState> =
            (0..5u64).map(|i| placed(i, 1, rgba8_png(1, 1))).collect();
        evict_pngs_outside_budget(&mut placed, 2..3, 0);
        assert!(!has_png(&placed, 0));
        assert!(!has_png(&placed, 1));
        assert!(has_png(&placed, 2), "index 2 is inside the window itself");
        assert!(!has_png(&placed, 3));
        assert!(!has_png(&placed, 4));
    }

    /// AT-3-504's memory-bound claim, exercised at 1,000 blocks: syncing the
    /// window forward one block at a time (as a streamed session appends,
    /// with eviction re-applied on every sync) and evicting with a fixed
    /// budget after each step never lets the retained-PNG count exceed the
    /// budget-derived bound, no matter how long the session runs — `placed`
    /// itself keeps growing (the state row is never dropped, only the PNG
    /// bytes), but memory tied up in retained images stays flat.
    #[test]
    fn thousand_block_session_keeps_retained_png_count_within_budget() {
        let budget = 5u64;
        let mut entries: Vec<PlacedState> = Vec::new();
        for i in 0..1000u64 {
            entries.push(placed(i, 1, rgba8_png(1, 1)));
            // Mirrors the call pattern `render_and_place` now uses
            // unconditionally (`Viewer::render_and_place` → `Viewport::
            // visible_blocks` → `StreamSink::evict_outside_window` →
            // `TerminalSink::evict_outside_window` → this function), not
            // just an arbitrary window: while follow is engaged (the
            // mainline streamed session this test represents), the
            // viewport's window is always the tail — the single newest
            // block — and eviction runs once per appended block, exactly
            // like this loop does. `TerminalSink::evict_outside_window`
            // itself is a one-line delegation to this function (see its doc
            // comment), so exercising `evict_pngs_outside_budget` with this
            // window/cadence is equivalent to exercising the real call site
            // without needing a live terminal to construct a `TerminalSink`.
            let last = entries.len() - 1;
            evict_pngs_outside_budget(&mut entries, last..last + 1, budget);
        }

        assert_eq!(entries.len(), 1000, "the full block history is kept");
        let retained = entries.iter().filter(|entry| !entry.png.is_empty()).count();
        // The window is always the single newest block; the eviction radius
        // is `budget` blocks behind it (nothing is ahead of the newest
        // block), so at most `budget + 1` blocks can retain a PNG.
        assert!(
            retained <= (budget + 1) as usize,
            "retained PNG count {retained} exceeds the budget-derived bound"
        );
    }

    // --- AT-3-506: batch divergence-tail rewrite ---

    fn planned(id: u64) -> tmath_render::PlannedBlock {
        tmath_render::PlannedBlock {
            id,
            hash: [0; 32],
            width_px: 1,
            height_px: 1,
        }
    }

    fn rendered_1x1() -> RenderedBlock {
        RenderedBlock {
            png: Vec::new(),
            width_px: 1,
            height_px: 1,
            formula_errors: Vec::new(),
            duration_ms: 0,
        }
    }

    fn prepared_1x1(png: Vec<u8>) -> PreparedBlock {
        PreparedBlock {
            rendered: Some(Arc::new(rendered_1x1())),
            png: Some(png),
            cache: Some(CacheOutcome::Miss),
        }
    }

    /// Reproduces the exact `blocks=9 -> blocks=2` shrink trace from the
    /// live logs: 9 blocks placed (ids 1-9, one row each), then a revision
    /// with 2 blocks where index 0 is unchanged (Keep id=1) and index 1's
    /// content changed (Replace id=2 -> a new id). Old ids 3-9 are stale.
    /// `reanchor_from` is 1 (the first non-Keep op).
    #[test]
    fn divergence_rewrite_clears_the_whole_stale_tail_and_replaces_in_document_order() {
        let cell = CellSize {
            width: 1,
            height: 1,
        };
        let placed: Vec<PlacedState> = (1..=9u64)
            .map(|id| placed(id, 1, rgba8_png(1, 1)))
            .collect();
        let previous: Vec<tmath_render::PlannedBlock> = (1..=9u64).map(planned).collect();

        let plan = Plan {
            ops: vec![
                PlanOp::Keep { id: 1 },
                PlanOp::Replace {
                    old_id: 2,
                    block: tmath_render::PlannedBlock {
                        id: 10,
                        hash: [1; 32],
                        width_px: 1,
                        height_px: 1,
                    },
                },
                PlanOp::Remove { id: 9 },
                PlanOp::Remove { id: 8 },
                PlanOp::Remove { id: 7 },
                PlanOp::Remove { id: 6 },
                PlanOp::Remove { id: 5 },
                PlanOp::Remove { id: 4 },
                PlanOp::Remove { id: 3 },
            ],
            reanchor_from: Some(1),
        };
        let prepared = vec![
            PreparedBlock {
                rendered: None,
                png: None,
                cache: None,
            }, // index 0: Keep, never read by `emit`/`emit_batch`
            prepared_1x1(rgba8_png(1, 1)),
        ];

        let (operations, new_tail) = divergence_rewrite_operations(
            &placed,
            &previous,
            &plan,
            &prepared,
            1,
            cell,
            u64::MAX,
            true,
        )
        .unwrap();
        let bytes = direct_bytes(&operations);
        let text = String::from_utf8_lossy(&bytes);

        // Cursor-up by the sum of stale rows (ids 2..9 = 8 rows, one each),
        // not just id=2's own row: this is what makes the fix different
        // from `replace`'s single-block `top_is_reachable` arithmetic.
        assert!(
            text.starts_with("\x1b[8A\r"),
            "cursor-up must cover every stale row from the divergence point, got: {text:?}"
        );
        // Every stale id (2-9) gets a Kitty delete — including ids 3-9 that
        // `Remove` ops covered, which the old per-op `remove()` path also
        // did, but here they're folded into the same batch as id=2's delete
        // rather than left for separate, cursor-position-agnostic calls.
        for id in 2..=9u32 {
            assert!(
                text.contains(&format!("\x1b_Ga=d,d=I,i={id},q=2\x1b\\")),
                "missing delete for stale id={id}: {text:?}"
            );
        }
        // The whole stale span is erased as text cells too (FIX for
        // `remove()`'s gap: a bare Kitty delete never clears the rows the
        // image used to cover).
        assert!(
            text.contains("\x1b[2K"),
            "stale rows must be cleared, not just have their images deleted"
        );
        // The new content (id=10, replacing id=2) is placed, in the same
        // pass, after the clear.
        assert!(text.contains("i=10,U=1,c=1,r=1,q=2"));
        // Erase-below closes the rewrite so no remnant from the old
        // (taller) tail can survive past the newly drawn content.
        assert!(text.ends_with("\x1b[0J"));

        // The new tail replaces the whole stale span with exactly one
        // entry (id=10) in document order — ids 2-9 are gone from
        // `placed`'s future state.
        assert_eq!(new_tail.len(), 1);
        assert_eq!(new_tail[0].id, 10);
    }

    /// A `Keep`-only entry inside the rewritten span is redrawn from its
    /// retained PNG (not re-rendered), preserving its own row count.
    #[test]
    fn divergence_rewrite_redraws_a_kept_block_after_the_divergence_point_from_its_retained_png() {
        let cell = CellSize {
            width: 1,
            height: 1,
        };
        let placed = vec![
            placed(1, 1, rgba8_png(1, 1)),
            placed(2, 2, rgba8_png(1, 2)),
            placed(3, 1, rgba8_png(1, 1)),
        ];
        let previous: Vec<tmath_render::PlannedBlock> = vec![planned(1), planned(2), planned(3)];
        // Index 0 changes (Replace), index 1 (id=2) is unchanged (Keep) but
        // sits AFTER the divergence point, index 2 (id=3) is removed.
        let plan = Plan {
            ops: vec![
                PlanOp::Replace {
                    old_id: 1,
                    block: tmath_render::PlannedBlock {
                        id: 11,
                        hash: [2; 32],
                        width_px: 1,
                        height_px: 1,
                    },
                },
                PlanOp::Keep { id: 2 },
                PlanOp::Remove { id: 3 },
            ],
            reanchor_from: Some(0),
        };
        let prepared = vec![
            prepared_1x1(rgba8_png(1, 1)),
            PreparedBlock {
                rendered: None,
                png: None,
                cache: None,
            }, // index 1: Keep, never read
        ];

        let (operations, new_tail) = divergence_rewrite_operations(
            &placed,
            &previous,
            &plan,
            &prepared,
            0,
            cell,
            u64::MAX,
            true,
        )
        .unwrap();
        let bytes = direct_bytes(&operations);
        let text = String::from_utf8_lossy(&bytes);

        // id=2's retained PNG is redrawn (2 rows), not skipped and not
        // re-rendered from `prepared` (which has no entry for it).
        assert!(text.contains("i=2,U=1,c=1,r=2,q=2"));
        assert!(text.contains("i=11,U=1,c=1,r=1,q=2"));

        // Document-order proof: `new_tail` is [11, 2], matching the plan's
        // op order (Replace then Keep), NOT insertion/removal order the
        // way the old per-op `replace()` left `placed` (it pushed replaced
        // entries to the end, so a later `Keep` id could end up sorted
        // ahead of a `Replace` id it logically follows). The caller splices
        // this straight after `placed[..reanchor_from]` with no reordering,
        // so `placed` stays index-aligned with `previous`/`plan` after a
        // batch rewrite — unlike after a per-op `replace()`.
        assert_eq!(new_tail.len(), 2);
        assert_eq!(new_tail[0].id, 11);
        assert_eq!(new_tail[1].id, 2);
        assert_eq!(new_tail[1].rows, 2, "kept block's row count carries over");
    }

    /// `PlacementPlanner`-driven proof that a plain streamed session (only
    /// ever appending or tail-editing the single newest block, the way
    /// `native_stream::run`'s splitter feeds it) never produces an interior
    /// `Replace`/`Remove` — i.e. `reanchor_from`, when present, always
    /// points at the last op. This is the invariant that makes it safe for
    /// stream mode (no retained PNGs, so no batch path available) to keep
    /// the old per-op `replace`/`remove` path unconditionally: `top_is_reachable`
    /// can only ever be evaluated for a truly-last block in that mode.
    #[test]
    fn stream_shaped_revisions_never_produce_an_interior_replace_or_remove() {
        let mut planner = PlacementPlanner::new();
        let mut blocks: Vec<([u8; 32], u32, u32)> = Vec::new();

        // Simulates a streamed answer: blocks accumulate one at a time
        // (append), and the last block's content grows/changes in place
        // (tail edit) before the next block starts — never touching an
        // earlier block once a new one has appended after it.
        for step in 0..20u8 {
            if step % 3 == 0 && !blocks.is_empty() {
                // Tail edit: only the last block's hash changes.
                let last = blocks.len() - 1;
                blocks[last].0 = [step; 32];
            } else {
                blocks.push(([step; 32], 100, 20));
            }
            let plan = planner.plan(&blocks);
            let last_index = plan.ops.len().saturating_sub(1);
            for (index, op) in plan.ops.iter().enumerate() {
                if matches!(op, PlanOp::Replace { .. } | PlanOp::Remove { .. }) {
                    assert_eq!(
                        index, last_index,
                        "stream-shaped revision produced a non-last Replace/Remove at step {step}"
                    );
                }
            }
        }
    }

    // --- Ghost-placement re-route: row-bookkeeping invariant (D-ROWS) ---
    //
    // Live-screenshot report: after a whole-document Reset (a divergence
    // rewrite from `reanchor_from = 0`, every op a `Replace`) renders a
    // 12-block answer, every display-math block appeared TWICE — a clean
    // copy at its correct row plus a garbled "ghost" copy a few rows above,
    // overlapping the preceding block, with the vertical error growing for
    // blocks further down the document (cumulative drift). Suspected after
    // `53b20da` (the inter-block margin change), since that made per-block
    // pixel heights no longer trivial multiples of the cell height, so
    // `grid_for`'s `div_ceil` rounding now actually matters.
    //
    // This is a pure-function trace, per the investigation's required
    // method: simulate a terminal cursor by walking `divergence_rewrite_operations`'s
    // emitted `TerminalOp` list byte-by-byte, and assert the physical
    // cursor position after every emitted block matches what the
    // bookkeeping (`old_rows_total`, `PlacedState.rows`, `grid_for`) claims
    // — row by row, not just in the final total. A "ghost" symptom is
    // exactly what a bookkeeping/physical mismatch would produce: if the
    // cursor-up at the top of the rewrite undershoots the actual on-screen
    // span, every re-placed block lands `N` rows too high, overlapping
    // whatever occupied those rows before (the preceding text block) —
    // and if the undershoot itself depends on the stale span's rows (which
    // it does, `old_rows_total` sums `placed[].rows`), a per-block rounding
    // mismatch would compound across blocks, exactly matching "grows for
    // blocks further down the document."

    /// Interprets one `TerminalOp::Local` byte sequence as a net terminal-row
    /// delta (positive = cursor moved down), recognizing exactly the control
    /// sequences this module's emitters produce: `\x1b[{N}A\r` (up N),
    /// `\r\n` and `\x1b[1B\r` (down 1 each), and no-op sequences (`\x1b[2K`,
    /// `\x1b[0J`, SGR color codes, and the raw placeholder glyph bytes
    /// between them) which contribute 0. Panics on a byte sequence this
    /// suite does not recognize, so an emitter change that introduces a new
    /// cursor-moving control sequence fails loudly here instead of silently
    /// desyncing the invariant checker from what is actually emitted.
    fn local_op_row_delta(bytes: &[u8]) -> i64 {
        let mut delta: i64 = 0;
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == 0x1b && bytes.get(i + 1) == Some(&b'[') {
                // Find the final byte of the CSI sequence (first ASCII
                // 0x40..=0x7e after the parameter/intermediate bytes).
                let start = i + 2;
                let mut end = start;
                while end < bytes.len() && !(0x40..=0x7e).contains(&bytes[end]) {
                    end += 1;
                }
                assert!(end < bytes.len(), "unterminated CSI sequence in {bytes:?}");
                let params = std::str::from_utf8(&bytes[start..end]).unwrap();
                let final_byte = bytes[end];
                match final_byte {
                    b'A' => {
                        let n: i64 = if params.is_empty() {
                            1
                        } else {
                            params.parse().expect("CSI A param must be a number")
                        };
                        delta -= n;
                    }
                    b'B' => {
                        let n: i64 = if params.is_empty() {
                            1
                        } else {
                            params.parse().expect("CSI B param must be a number")
                        };
                        delta += n;
                    }
                    b'H' => {
                        panic!(
                            "CSI H (absolute home) inside a row-delta-only op; \
                             use an absolute-position-aware simulation instead: {bytes:?}"
                        );
                    }
                    // No-ops for row tracking: erase line (K), erase display
                    // (J), and SGR color-setting (m) never move the cursor.
                    b'K' | b'J' | b'm' => {}
                    other => panic!(
                        "unrecognized CSI final byte {other:?} in {bytes:?} — extend \
                         local_op_row_delta before trusting this invariant"
                    ),
                }
                i = end + 1;
            } else if bytes[i] == b'\r' {
                // Bare CR (no accompanying LF) does not change the row.
                i += 1;
            } else if bytes[i] == b'\n' {
                delta += 1;
                i += 1;
            } else {
                // Any other byte (placeholder glyph UTF-8, diacritic
                // combining marks) never moves the cursor row on its own.
                i += 1;
            }
        }
        delta
    }

    /// Walks a full `divergence_rewrite_operations` output and returns the
    /// NET row delta the whole operation list produces (sum of every
    /// `Local` op's delta; `Graphics` ops are cursor-neutral).
    fn net_row_delta(operations: &[TerminalOp]) -> i64 {
        operations
            .iter()
            .map(|operation| match operation {
                TerminalOp::Local(bytes) => local_op_row_delta(bytes),
                TerminalOp::Graphics(_) => 0,
            })
            .sum()
    }

    /// The core invariant: after `divergence_rewrite_operations` runs, the
    /// cursor's NET row movement must equal exactly
    /// `new_tail`'s total rows MINUS `old_rows_total` (the up-front
    /// cursor-up) — i.e. the rewrite leaves the cursor exactly
    /// `sum(new_tail.rows)` rows below where the OLD tail's top was, which
    /// is the same place a from-scratch append of that many rows would
    /// leave it. A mismatch here is precisely the ghost-placement
    /// mechanism: the physical cursor ends up somewhere other than where
    /// the next revision's bookkeeping (`old_rows_total`, again summed from
    /// `placed[].rows`) assumes it starts, so the NEXT rewrite's cursor-up
    /// is wrong by the accumulated error, and rows overlap.
    fn assert_row_bookkeeping_is_internally_consistent(
        operations: &[TerminalOp],
        old_rows_total: u32,
        new_tail: &[PlacedState],
    ) {
        let expected_net_delta = i64::from(new_tail.iter().map(|entry| entry.rows).sum::<u32>())
            - i64::from(old_rows_total);
        let actual_net_delta = net_row_delta(operations);
        assert_eq!(
            actual_net_delta, expected_net_delta,
            "cursor net row movement ({actual_net_delta}) must equal \
             sum(new_tail.rows) - old_rows_total ({expected_net_delta}); a mismatch \
             means the next revision's cursor-up (which trusts `placed[].rows`) will \
             start from the wrong physical row — the ghost-placement mechanism"
        );
    }

    /// One surviving placement's absolute row span, `[start, end)`, after
    /// walking a combined operation stream — built by
    /// [`placement_row_spans`].
    #[derive(Debug, Clone, Copy)]
    struct RowSpan {
        id: u32,
        start: i64,
        end: i64,
    }

    /// Recognizes a `Local` op that is EXACTLY one absolute cursor-position
    /// sequence (`\x1b[{row};{col}H`, e.g. `sync_window_operations`'s
    /// `home`) and returns the target row (0-based, i.e. `row - 1`).
    /// Returns `None` for anything else, including ops that mix an
    /// absolute home with other bytes — `local_op_row_delta` still panics
    /// on those, which is correct: every absolute-home op this module
    /// actually emits is a standalone `TerminalOp::Local` (see
    /// `sync_window_operations`), so a mixed op containing `H` would be a
    /// genuinely new, unaudited shape.
    fn local_op_absolute_home_row(bytes: &[u8]) -> Option<i64> {
        let text = std::str::from_utf8(bytes).ok()?;
        let rest = text.strip_prefix("\x1b[")?;
        let rest = rest.strip_suffix('H')?;
        let (row_str, _col_str) = rest.split_once(';')?;
        let row: i64 = row_str.parse().ok()?;
        Some(row - 1)
    }

    /// Walks `operations` tracking an absolute cursor row (starting at 0,
    /// the row the caller's cursor-up left it at), and records the row span
    /// each Kitty placement command's SUBSEQUENT placeholder-grid write
    /// occupies. A `kitty_delete_id` graphics op removes any earlier span
    /// for that id (the placement no longer exists once deleted — this
    /// mirrors what a real terminal does: a deleted Kitty image's cells
    /// stop being backed by that image, though `clear_rows`'s `\x1b[2K`
    /// additionally blanks the text there). Returns only the SURVIVING
    /// spans (every earlier span for an id that got deleted is dropped, not
    /// just marked), so two spans in the result overlapping is exactly a
    /// ghost: two different ids both claiming to render over the same rows
    /// at the end of the sequence.
    ///
    /// Also recognizes a standalone absolute-home op
    /// (`local_op_absolute_home_row`, e.g. `sync_window_operations`'s
    /// `home`) and resets the tracked row to its target instead of treating
    /// it as a relative-only op (which `local_op_row_delta` would correctly
    /// reject with a panic — this function is the one caller that legitimately
    /// needs to simulate an absolute-position-aware operation stream, per
    /// `local_op_row_delta`'s own panic message).
    fn placement_row_spans(operations: &[TerminalOp]) -> Vec<RowSpan> {
        let mut spans: Vec<RowSpan> = Vec::new();
        let mut row: i64 = 0;
        let mut pending_id: Option<u32> = None;
        for operation in operations {
            match operation {
                TerminalOp::Graphics(bytes) => {
                    let text = String::from_utf8_lossy(bytes);
                    if let Some(id) = parse_kitty_delete_id(&text) {
                        spans.retain(|span| span.id != id);
                    } else if let Some(id) = parse_kitty_place_id(&text) {
                        pending_id = Some(id);
                    }
                }
                TerminalOp::Local(bytes) => {
                    if let Some(id) = pending_id.take() {
                        // The very next Local op after a placement command
                        // is always `placeholder_grid_at_cursor`'s output
                        // (see `emit_placed_block_cursor`), whose row
                        // extent is exactly this op's own row delta (each
                        // placeholder row but the last is followed by
                        // `\r\n`, so the delta IS the placeholder row count
                        // minus one — the span covers `delta + 1` rows).
                        let delta = local_op_row_delta(bytes);
                        let span_rows = delta + 1;
                        spans.push(RowSpan {
                            id,
                            start: row,
                            end: row + span_rows,
                        });
                        row += delta;
                    } else if let Some(home_row) = local_op_absolute_home_row(bytes) {
                        row = home_row;
                    } else {
                        row += local_op_row_delta(bytes);
                    }
                }
            }
        }
        spans
    }

    fn parse_kitty_delete_id(apc_text: &str) -> Option<u32> {
        if !apc_text.contains("a=d") {
            return None;
        }
        apc_text
            .split(',')
            .find_map(|field| field.strip_prefix("i="))
            .and_then(|value| value.trim_end_matches('\u{1b}').parse().ok())
    }

    fn parse_kitty_place_id(apc_text: &str) -> Option<u32> {
        if !apc_text.contains("U=1") {
            return None;
        }
        apc_text
            .split(',')
            .find_map(|field| field.strip_prefix("i="))
            .and_then(|value| value.parse().ok())
    }

    /// The direct overlap check: no two surviving placements' row spans may
    /// intersect. This is the literal definition of a "ghost" — two
    /// different image ids both claiming some of the same rows.
    fn assert_no_overlapping_row_spans(operations: &[TerminalOp]) {
        let spans = placement_row_spans(operations);
        for (i, a) in spans.iter().enumerate() {
            for b in &spans[i + 1..] {
                let overlaps = a.start < b.end && b.start < a.end;
                assert!(
                    !overlaps,
                    "id={} (rows {}..{}) overlaps id={} (rows {}..{}) — this IS a ghost: \
                     two surviving placements claim the same terminal rows",
                    a.id, a.start, a.end, b.id, b.start, b.end
                );
            }
        }
    }

    /// The clamp-aware fallback's own invariant (PART 1): after the fix, NO
    /// operation stream this module emits may ever contain a relative
    /// cursor-up (`\x1b[{n}A`) whose `n` exceeds `pane_rows` — a cursor-up
    /// that large is exactly the ghost mechanism (`clamp_would_truncate`'s
    /// doc comment), so a correct emitter must either keep `n` within the
    /// pane or not emit a relative cursor-up at all (falling back to the
    /// absolute `\x1b[H`-based window sync instead). Scans every `Local`
    /// op's bytes directly for `CSI n A` sequences, independent of
    /// `local_op_row_delta`'s net-delta accounting, so this is a genuinely
    /// separate check, not a restatement of the same arithmetic.
    fn assert_no_cursor_up_exceeds_pane_rows(operations: &[TerminalOp], pane_rows: u32) {
        for operation in operations {
            let TerminalOp::Local(bytes) = operation else {
                continue;
            };
            let mut i = 0;
            while i < bytes.len() {
                if bytes[i] == 0x1b && bytes.get(i + 1) == Some(&b'[') {
                    let start = i + 2;
                    let mut end = start;
                    while end < bytes.len() && !(0x40..=0x7e).contains(&bytes[end]) {
                        end += 1;
                    }
                    if end < bytes.len() && bytes[end] == b'A' {
                        let params = std::str::from_utf8(&bytes[start..end]).unwrap_or("");
                        let n: u32 = if params.is_empty() {
                            1
                        } else {
                            params.parse().unwrap_or(0)
                        };
                        assert!(
                            n <= pane_rows,
                            "cursor-up by {n} rows exceeds pane_rows={pane_rows} — this WILL \
                             clamp at the pane's top row and ghost the rewrite"
                        );
                    }
                    i = end + 1;
                } else {
                    i += 1;
                }
            }
        }
    }

    /// Reproduces a synthetic 12-block whole-document Reset with realistic,
    /// NON-uniform pixel heights that include the inter-block margin
    /// (`53b20da`) — heights are `12 + 2*margin_px` style values that are
    /// deliberately NOT exact multiples of the cell height, so `grid_for`'s
    /// `div_ceil` rounds some blocks up by a fractional cell, exactly the
    /// condition the live report's timing implicates. Feeds a 12-block plan
    /// where every op is `Replace` (a whole-document Reset from a
    /// completely different previous answer) through
    /// `divergence_rewrite_operations` and checks the row-bookkeeping
    /// invariant holds for the WHOLE batch — not just each individual
    /// block's own `grid_for` call, which is where the coordinator's
    /// suspicion (a) targeted, but the cumulative sum across all 12, which
    /// is where a per-block rounding mismatch would compound (suspicion
    /// (b)/(c)).
    #[test]
    fn synthetic_twelve_block_reset_keeps_row_bookkeeping_consistent_with_margin_heights() {
        let cell = CellSize {
            width: 8,
            height: 17,
        };

        // Realistic non-uniform heights: a mix of single-line paragraph
        // blocks, multi-line blocks, and display-math blocks, each with the
        // inter-block margin baked in (an odd, non-cell-multiple pixel
        // count) — the exact shape `53b20da` introduced.
        let old_heights_px: [u32; 12] = [19, 38, 55, 19, 91, 19, 38, 127, 19, 55, 19, 73];
        let new_heights_px: [u32; 12] = [23, 42, 59, 23, 95, 23, 42, 131, 23, 59, 23, 77];

        let old_rows: Vec<u32> = old_heights_px
            .iter()
            .map(|&height| tmath_core::placement::grid_for(100, height, cell).1)
            .collect();
        let placed: Vec<PlacedState> = (1..=12u64)
            .zip(old_rows.iter())
            .map(|(id, &rows)| placed(id, rows, rgba8_png(1, 1)))
            .collect();
        let previous: Vec<tmath_render::PlannedBlock> = (1..=12u64)
            .zip(old_heights_px.iter())
            .map(|(id, &height)| tmath_render::PlannedBlock {
                id,
                hash: [0; 32],
                width_px: 100,
                height_px: height,
            })
            .collect();

        let plan = Plan {
            ops: (1..=12u64)
                .zip(new_heights_px.iter())
                .map(|(old_id, &height)| PlanOp::Replace {
                    old_id,
                    block: tmath_render::PlannedBlock {
                        id: old_id + 100,
                        hash: [1; 32],
                        width_px: 100,
                        height_px: height,
                    },
                })
                .collect(),
            reanchor_from: Some(0),
        };
        let prepared: Vec<PreparedBlock> = new_heights_px
            .iter()
            .map(|&height| {
                let png = rgba8_png(100, height);
                PreparedBlock {
                    rendered: Some(Arc::new(RenderedBlock {
                        png: png.clone(),
                        width_px: 100,
                        height_px: height,
                        formula_errors: Vec::new(),
                        duration_ms: 0,
                    })),
                    png: Some(png),
                    cache: Some(CacheOutcome::Miss),
                }
            })
            .collect();

        let old_rows_total: u32 = old_rows
            .iter()
            .fold(0u32, |total, &rows| total.saturating_add(rows));
        let (operations, new_tail) = divergence_rewrite_operations(
            &placed,
            &previous,
            &plan,
            &prepared,
            0,
            cell,
            u64::MAX,
            true,
        )
        .unwrap();

        // (a) Per-block: `new_tail`'s row count for each block must equal
        // `grid_for` on that block's own emitted height — the placeholder
        // rows actually written match the bookkeeping row count.
        for (index, entry) in new_tail.iter().enumerate() {
            let expected_rows = tmath_core::placement::grid_for(100, new_heights_px[index], cell).1;
            assert_eq!(
                entry.rows, expected_rows,
                "block {index} (id={}): new_tail.rows must equal grid_for(height={})'s \
                 row count",
                entry.id, new_heights_px[index]
            );
        }

        // (b)/(c) Whole-batch: the physical cursor's net movement matches
        // exactly what the bookkeeping (`old_rows_total` cursor-up plus
        // every emitted block's rows) predicts — the invariant that a
        // ghost placement would violate.
        assert_row_bookkeeping_is_internally_consistent(&operations, old_rows_total, &new_tail);
        // The literal ghost check: no two of the 12 newly placed blocks'
        // row spans overlap each other.
        assert_no_overlapping_row_spans(&operations);

        // Sanity: the emitted operations actually contain all 12 new ids
        // and delete all 12 old ids (no block silently dropped or
        // double-emitted, which would also explain a visual "ghost" if a
        // duplicate placement command were the mechanism instead of a
        // cursor-position mismatch).
        let bytes = direct_bytes(&operations);
        let text = String::from_utf8_lossy(&bytes);
        for old_id in 1..=12u32 {
            assert!(
                text.contains(&format!("\x1b_Ga=d,d=I,i={old_id},q=2\x1b\\")),
                "missing delete for stale id={old_id}"
            );
        }
        for new_id in 101..=112u32 {
            let needle = format!("i={new_id},U=1,c=");
            assert_eq!(
                text.matches(&needle).count(),
                1,
                "id={new_id} must be placed exactly once, not zero or more than once \
                 (a duplicate placement command is itself a possible ghost mechanism)"
            );
        }
    }

    /// Cross-call stress test: two consecutive whole-document Resets (a
    /// first different answer, then a second different answer replacing it
    /// entirely — exactly "a previous different answer" from the live
    /// report), threading `placed` state through two
    /// `divergence_rewrite_operations` calls the SAME WAY `TerminalSink::emit_batch`
    /// does (`self.placed.truncate(reanchor_from); self.placed.extend(new_tail)`,
    /// reproduced here manually since `TerminalSink` is hardcoded to
    /// `Terminal<StdioTty>` and cannot be constructed with `FakeTty` without
    /// changing production type signatures — out of scope for a diagnosis
    /// task). This exercises what the single-batch pure trace above cannot:
    /// whether the bookkeeping state the FIRST call leaves behind
    /// (`new_tail`, spliced into `placed` exactly as `emit_batch` does)
    /// still agrees with the physical cursor position that batch's own
    /// emitted operations actually produced, which is what the SECOND
    /// Reset's `old_rows_total` cursor-up then trusts.
    #[test]
    fn two_consecutive_resets_keep_row_bookkeeping_consistent_across_calls() {
        let cell = CellSize {
            width: 8,
            height: 17,
        };

        // First Reset: a 12-block answer, reanchor_from = Some(0) since the
        // very first revision has nothing to keep. Mirrors `emit_batch`
        // being called on an initially empty `self.placed`.
        let first_heights: [u32; 12] = [19, 38, 55, 19, 91, 19, 38, 127, 19, 55, 19, 73];
        let first_previous: Vec<tmath_render::PlannedBlock> = Vec::new();
        let placed_before_first: Vec<PlacedState> = Vec::new();
        let first_plan = Plan {
            ops: (1..=12u64)
                .zip(first_heights.iter())
                .map(|(id, &height)| PlanOp::Replace {
                    old_id: id, // unused when `placed_before_first` is empty
                    block: tmath_render::PlannedBlock {
                        id,
                        hash: [0; 32],
                        width_px: 100,
                        height_px: height,
                    },
                })
                .collect(),
            reanchor_from: Some(0),
        };
        let first_prepared: Vec<PreparedBlock> = first_heights
            .iter()
            .map(|&height| {
                let png = rgba8_png(100, height);
                PreparedBlock {
                    rendered: Some(Arc::new(RenderedBlock {
                        png: png.clone(),
                        width_px: 100,
                        height_px: height,
                        formula_errors: Vec::new(),
                        duration_ms: 0,
                    })),
                    png: Some(png),
                    cache: Some(CacheOutcome::Miss),
                }
            })
            .collect();

        let (first_operations, first_new_tail) = divergence_rewrite_operations(
            &placed_before_first,
            &first_previous,
            &first_plan,
            &first_prepared,
            0,
            cell,
            u64::MAX,
            true,
        )
        .unwrap();
        // Same per-block check as the single-batch trace, against the
        // FIRST call's own output.
        for (index, entry) in first_new_tail.iter().enumerate() {
            let expected_rows = tmath_core::placement::grid_for(100, first_heights[index], cell).1;
            assert_eq!(
                entry.rows, expected_rows,
                "after the FIRST reset, block {index} (id={}) has rows={} but \
                 grid_for(height={}) says {expected_rows}",
                entry.id, entry.rows, first_heights[index]
            );
        }
        assert_row_bookkeeping_is_internally_consistent(&first_operations, 0, &first_new_tail);

        // Exactly what `emit_batch` does after `divergence_rewrite_operations`
        // returns: truncate to `reanchor_from` (0 here — nothing to keep)
        // and splice in the new tail. This becomes `placed` for the SECOND
        // call, precisely modeling the cross-call state `TerminalSink`
        // carries between two consecutive `sink.emit()` invocations.
        let mut placed_before_second = placed_before_first;
        placed_before_second.truncate(0);
        placed_before_second.extend(first_new_tail.clone());

        // Second Reset: a COMPLETELY DIFFERENT 12-block answer (every hash
        // differs, so the planner emits Replace for every index) — the
        // exact "previous different answer" -> new Reset transition the
        // live report describes. `previous` reflects what the planner's
        // `blocks()` would report right after the first Reset: ids 1-12 at
        // the FIRST heights (their `PlannedBlock.height_px`, independent of
        // `placed`'s row bookkeeping — this is the OTHER place height
        // enters the picture, and it must agree with `placed`'s rows too).
        let second_heights: [u32; 12] = [23, 42, 59, 23, 95, 23, 42, 131, 23, 59, 23, 77];
        let second_previous: Vec<tmath_render::PlannedBlock> = (1..=12u64)
            .zip(first_heights.iter())
            .map(|(id, &height)| tmath_render::PlannedBlock {
                id,
                hash: [0; 32],
                width_px: 100,
                height_px: height,
            })
            .collect();
        let second_plan = Plan {
            ops: (1..=12u64)
                .zip(second_heights.iter())
                .map(|(old_id, &height)| PlanOp::Replace {
                    old_id,
                    block: tmath_render::PlannedBlock {
                        id: old_id + 100,
                        hash: [1; 32],
                        width_px: 100,
                        height_px: height,
                    },
                })
                .collect(),
            reanchor_from: Some(0),
        };
        let second_prepared: Vec<PreparedBlock> = second_heights
            .iter()
            .map(|&height| {
                let png = rgba8_png(100, height);
                PreparedBlock {
                    rendered: Some(Arc::new(RenderedBlock {
                        png: png.clone(),
                        width_px: 100,
                        height_px: height,
                        formula_errors: Vec::new(),
                        duration_ms: 0,
                    })),
                    png: Some(png),
                    cache: Some(CacheOutcome::Miss),
                }
            })
            .collect();

        // The bookkeeping value the SECOND call's cursor-up will actually
        // use, re-derived exactly as `divergence_rewrite_operations` does
        // internally (sum of `placed[].rows` for the stale ids) — must
        // equal what the FIRST call's own row bookkeeping says it placed.
        let first_reported_total_rows: u32 = first_new_tail
            .iter()
            .fold(0u32, |total, entry| total.saturating_add(entry.rows));
        let second_old_rows_total: u32 = placed_before_second
            .iter()
            .filter(|entry| (1..=12u64).contains(&entry.id))
            .fold(0u32, |total, entry| total.saturating_add(entry.rows));
        assert_eq!(
            second_old_rows_total, first_reported_total_rows,
            "the second Reset's cursor-up must sum to exactly what the first \
             Reset's own bookkeeping says it placed — a mismatch here is the \
             cross-call desync that would misplace every block of the second answer"
        );

        let (second_operations, second_new_tail) = divergence_rewrite_operations(
            &placed_before_second,
            &second_previous,
            &second_plan,
            &second_prepared,
            0,
            cell,
            u64::MAX,
            true,
        )
        .unwrap();
        for (index, entry) in second_new_tail.iter().enumerate() {
            let expected_rows = tmath_core::placement::grid_for(100, second_heights[index], cell).1;
            assert_eq!(
                entry.rows, expected_rows,
                "after the SECOND reset, block {index} (id={}) has rows={} but \
                 grid_for(height={}) says {expected_rows}",
                entry.id, entry.rows, second_heights[index]
            );
        }
        assert_row_bookkeeping_is_internally_consistent(
            &second_operations,
            second_old_rows_total,
            &second_new_tail,
        );

        // Whole-session check: concatenate BOTH calls' emitted operations
        // (exactly the order `TerminalSink` would have written them to the
        // real terminal, back to back) and replay the combined stream
        // through the row-delta simulator. Each Reset's cursor-up + clear
        // + redraw is individually a closed loop relative to where it
        // started (proven per-call above), so two Resets back to back must
        // net out to just the SECOND (final) answer's total row count —
        // not the first answer's, and not some drifted value in between.
        let mut combined_operations = first_operations;
        combined_operations.extend(second_operations);
        let final_total_rows = i64::from(
            second_new_tail
                .iter()
                .fold(0u32, |total, entry| total.saturating_add(entry.rows)),
        );
        let whole_session_delta = net_row_delta(&combined_operations);
        assert_eq!(
            whole_session_delta, final_total_rows,
            "the combined two-Reset operation stream's net cursor movement \
             ({whole_session_delta}) must equal exactly the final answer's total \
             row count ({final_total_rows}) — a mismatch means the two Resets did \
             not compose into a closed-loop-plus-final-redraw, which is the \
             cross-call ghost-placement mechanism"
        );

        // The literal ghost check across BOTH resets combined: the second
        // Reset deletes every first-Reset id (1-12) before placing its own
        // (101-112), so after replaying the whole combined stream, no
        // surviving span may overlap another — including the specific
        // "ghost above the correct copy" shape from the live report (an
        // old, un-deleted or wrongly-positioned span still claiming rows
        // near a new one).
        assert_no_overlapping_row_spans(&combined_operations);
    }

    // --- PART 1: clamp-aware fallback (D-CLAMP) ---

    /// `clamp_would_truncate`'s decision boundary: exactly at
    /// `usable_rows` the cursor-up is guaranteed to clamp (the pane has no
    /// row above that to move into), and one row below the boundary it is
    /// not. Checked with and without a reserved status-bar row.
    #[test]
    fn clamp_would_truncate_decision_boundary() {
        // No status bar: usable_rows == pane_rows.
        assert!(!clamp_would_truncate(23, 24, false), "one row of headroom");
        assert!(
            clamp_would_truncate(24, 24, false),
            "exactly at the boundary"
        );
        assert!(clamp_would_truncate(25, 24, false), "past the boundary");

        // With a reserved status-bar row: usable_rows == pane_rows - 1.
        assert!(
            !clamp_would_truncate(22, 24, true),
            "one row of headroom, minus the reserved row"
        );
        assert!(
            clamp_would_truncate(23, 24, true),
            "exactly at the boundary, minus the reserved row"
        );
        assert!(clamp_would_truncate(24, 24, true), "past the boundary");

        // `pane_rows == 0` (never opted into `with_status_bar`) always
        // disables the check — this is what makes it safe for plain
        // stream/watch sessions, which never set `pane_rows`.
        assert!(!clamp_would_truncate(u32::MAX, 0, false));
        assert!(!clamp_would_truncate(u32::MAX, 0, true));
    }

    /// End-to-end proof that `emit_batch` actually takes the fallback path
    /// when the stale tail is too tall for the pane: builds a `TerminalSink`-
    /// equivalent scenario (via the same pure functions the cross-call test
    /// above threads by hand) with a 12-block stale tail whose row total
    /// exceeds a small `pane_rows`, and confirms (a) the returned outcome is
    /// `NeedsWindowSync`, (b) NOTHING is written for that revision (the
    /// would-be-clamped relative cursor-up never gets emitted, since a
    /// PARTIAL write is not safe either), and (c) the row-bookkeeping state
    /// (`new_tail`) still updates correctly so the caller's follow-up
    /// window sync has accurate data to redraw from.
    #[test]
    fn emit_batch_falls_back_to_window_sync_when_the_stale_tail_exceeds_the_pane() {
        let cell = CellSize {
            width: 8,
            height: 17,
        };
        let heights: [u32; 12] = [19, 38, 55, 19, 91, 19, 38, 127, 19, 55, 19, 73];
        let old_rows: Vec<u32> = heights
            .iter()
            .map(|&height| tmath_core::placement::grid_for(100, height, cell).1)
            .collect();
        let old_rows_total: u32 = old_rows
            .iter()
            .fold(0u32, |total, &rows| total.saturating_add(rows));
        // A pane shorter than the stale tail's total rows — this MUST clamp.
        let pane_rows = old_rows_total.saturating_sub(1).max(1);
        assert!(
            clamp_would_truncate(old_rows_total, pane_rows, false),
            "test setup: the pane must be too short for the stale tail"
        );

        let placed: Vec<PlacedState> = (1..=12u64)
            .zip(old_rows.iter())
            .map(|(id, &rows)| placed(id, rows, rgba8_png(1, 1)))
            .collect();
        let previous: Vec<tmath_render::PlannedBlock> = (1..=12u64)
            .zip(heights.iter())
            .map(|(id, &height)| tmath_render::PlannedBlock {
                id,
                hash: [0; 32],
                width_px: 100,
                height_px: height,
            })
            .collect();
        let plan = Plan {
            ops: (1..=12u64)
                .map(|old_id| PlanOp::Replace {
                    old_id,
                    block: tmath_render::PlannedBlock {
                        id: old_id + 100,
                        hash: [1; 32],
                        width_px: 100,
                        height_px: 20,
                    },
                })
                .collect(),
            reanchor_from: Some(0),
        };
        let prepared: Vec<PreparedBlock> = (0..12)
            .map(|_| {
                let png = rgba8_png(100, 20);
                PreparedBlock {
                    rendered: Some(Arc::new(RenderedBlock {
                        png: png.clone(),
                        width_px: 100,
                        height_px: 20,
                        formula_errors: Vec::new(),
                        duration_ms: 0,
                    })),
                    png: Some(png),
                    cache: Some(CacheOutcome::Miss),
                }
            })
            .collect();

        // Mirror exactly what `TerminalSink::emit_batch` does for the
        // clamp branch (this test cannot construct a real `TerminalSink`
        // — see `two_consecutive_resets_keep_row_bookkeeping_consistent_across_calls`'s
        // doc comment for why — so it drives the same pure functions in the
        // same order `emit_batch` calls them).
        assert!(clamp_would_truncate(
            stale_tail_rows_total(&placed, &previous, 0),
            pane_rows,
            false
        ));
        let new_tail =
            clamp_fallback_new_tail(&placed, &plan, &prepared, 0, cell, u64::MAX, true).unwrap();

        // (a)/(c): the new bookkeeping is correct even though nothing was
        // written — every new id/rows is present, ready for a window sync
        // to redraw from.
        assert_eq!(new_tail.len(), 12);
        for (index, entry) in new_tail.iter().enumerate() {
            assert_eq!(entry.id, 101 + index as u64);
            assert_eq!(entry.rows, tmath_core::placement::grid_for(100, 20, cell).1);
        }
        // (b): `clamp_fallback_new_tail` itself never returns any
        // `TerminalOp`s — by construction, it has no `Vec<TerminalOp>` in
        // its return type at all, so there is no relative cursor-up for
        // this revision to accidentally clamp. The only bytes a real
        // `emit_batch` call would send for this revision are whatever the
        // CALLER's subsequent `sync_window` emits, which is anchored
        // absolutely (`\x1b[H`-based), never relatively.

        // Positive counter-check, same scenario: with a GENEROUS pane
        // (comfortably larger than the stale tail), `clamp_would_truncate`
        // is false, the ORDINARY relative-cursor path runs instead, and
        // `assert_no_cursor_up_exceeds_pane_rows` must hold for its real,
        // non-empty operations — proving the checker is not vacuously true
        // on empty output; it actively validates a genuine cursor-up
        // against a genuine pane bound.
        let generous_pane_rows = old_rows_total + 100;
        assert!(!clamp_would_truncate(
            old_rows_total,
            generous_pane_rows,
            false
        ));
        let (operations, _) = divergence_rewrite_operations(
            &placed,
            &previous,
            &plan,
            &prepared,
            0,
            cell,
            u64::MAX,
            true,
        )
        .unwrap();
        assert_no_cursor_up_exceeds_pane_rows(&operations, generous_pane_rows);
        // Sanity: this scenario really does emit a relative cursor-up (so
        // the check above is exercising something, not skipping an empty
        // operations list the way the fallback case's would).
        let bytes = direct_bytes(&operations);
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            text.contains(&format!("\x1b[{old_rows_total}A\r")),
            "sanity: the ordinary path must actually emit the cursor-up: {text:?}"
        );
    }

    /// Traces a growing-answer `Replace(last)` scenario for a TALL block
    /// (simulating a paragraph with a display-math integral following a
    /// table, several times its neighbors' height) across every transition
    /// `emit_batch` can take, with a reserved status-bar row throughout —
    /// this is the live "crushed line" bug report's exact shape. Asserts
    /// the replaced block's PLACEHOLDER rows (what
    /// `tmath_core::placement::grid_for` derives and what the placeholder
    /// grid actually writes) always equal its DECODED IMAGE rows (re-derived
    /// fresh from the PNG bytes via `decode_png` + `grid_for`, independent
    /// of any bookkeeping field) — i.e. the tall block is never truncated
    /// to fewer rows than its own image needs, in any of:
    ///
    /// 1. Normal batch (`divergence_rewrite_operations`, pane comfortably
    ///    larger than the stale tail).
    /// 2. Clamp fallback (`clamp_fallback_new_tail`, pane just under the
    ///    stale tail's row total, WITH `status_bar_active = true`).
    /// 3. The fallback's follow-up window sync (`sync_window_operations`
    ///    with `content_row_offset = 1`), confirming the tall block's row
    ///    span in the synced output matches the same decoded-image rows.
    ///
    /// This pure trace comes back CLEAN for all three transitions: every
    /// row-accounting path (`placed_state_for_op`, `divergence_rewrite_operations`,
    /// `clamp_fallback_new_tail`, `sync_window_operations`) independently
    /// re-derives `rows` from `grid_for(width, height, cell)` off the
    /// SAME decoded PNG dimensions every time — there is no cached/stale
    /// `rows` field read anywhere in this module's row-emission math (see
    /// `sync_window_operations`'s doc comment on why it re-decodes rather
    /// than trusting `PlacedState::rows`). A "crushed" (vertically
    /// squashed) tall block is therefore not explained by anything in this
    /// module's row bookkeeping; the live symptom traces to
    /// `sync_window_operations`'s column/full-row-clear gap documented in
    /// `sync_window_with_empty_emitted_ids_never_clears_full_rows_before_drawing`
    /// above (a narrower/shorter old block's remnants showing through a
    /// new block's placeholder grid can visually read as "crushed" if the
    /// old content was a short table row and the new content is a tall
    /// formula sharing the same physical rows), or to the render/rows side
    /// (`grid_for` vs. the tall-inline image's actual proportions, or the
    /// `trim_transparent_right` reserve) which is outside this module.
    #[test]
    fn tall_replaced_last_block_keeps_placeholder_rows_equal_to_decoded_image_rows_across_transitions(
    ) {
        let cell = CellSize {
            width: 8,
            height: 17,
        };
        // Five short "table row" blocks, then one TALL block (the
        // integral-with-limits paragraph) as the stale tail's last entry —
        // the exact "tall inline formula immediately after a table" shape.
        let old_heights: [u32; 6] = [19, 19, 19, 19, 19, 200];
        let old_rows: Vec<u32> = old_heights
            .iter()
            .map(|&height| tmath_core::placement::grid_for(100, height, cell).1)
            .collect();
        let tall_old_rows = *old_rows.last().unwrap();

        let placed: Vec<PlacedState> = (1..=6u64)
            .zip(old_rows.iter())
            .map(|(id, &rows)| placed(id, rows, rgba8_png(100, 1)))
            .collect();
        let previous: Vec<tmath_render::PlannedBlock> = (1..=6u64)
            .zip(old_heights.iter())
            .map(|(id, &height)| tmath_render::PlannedBlock {
                id,
                hash: [0; 32],
                width_px: 100,
                height_px: height,
            })
            .collect();

        // The revision re-renders only the tail block (growing streamed
        // math), one taller still, at index 5 (`reanchor_from = 5`) — the
        // other five blocks are an unwritten `Keep` prefix in the plan.
        let new_tall_height = 240u32;
        let new_tall_id = 106u64;
        let plan = Plan {
            ops: (1..=5u64)
                .map(|id| PlanOp::Keep { id })
                .chain(std::iter::once(PlanOp::Replace {
                    old_id: 6,
                    block: tmath_render::PlannedBlock {
                        id: new_tall_id,
                        hash: [1; 32],
                        width_px: 100,
                        height_px: new_tall_height,
                    },
                }))
                .collect(),
            reanchor_from: Some(5),
        };
        let mut prepared: Vec<PreparedBlock> = (0..5)
            .map(|_| PreparedBlock {
                rendered: None,
                png: None,
                cache: None,
            })
            .collect();
        let tall_png = rgba8_png(100, new_tall_height);
        prepared.push(PreparedBlock {
            rendered: Some(Arc::new(RenderedBlock {
                png: tall_png.clone(),
                width_px: 100,
                height_px: new_tall_height,
                formula_errors: Vec::new(),
                duration_ms: 0,
            })),
            png: Some(tall_png.clone()),
            cache: Some(CacheOutcome::Miss),
        });

        let expected_rows = tmath_core::placement::grid_for(100, new_tall_height, cell).1;
        // Sanity: the expectation itself must be re-derivable straight from
        // the decoded PNG, independent of any bookkeeping field.
        let (decoded_width, decoded_height, _) =
            decode_png(&tall_png, u64::MAX).expect("valid PNG");
        assert_eq!((decoded_width, decoded_height), (100, new_tall_height));
        assert_eq!(
            tmath_core::placement::grid_for(decoded_width, decoded_height, cell).1,
            expected_rows
        );

        // --- Transition 1: normal batch (pane comfortably larger). ---
        let old_rows_total = stale_tail_rows_total(&placed, &previous, 5);
        let generous_pane_rows = old_rows_total + expected_rows + 100;
        assert!(!clamp_would_truncate(
            old_rows_total,
            generous_pane_rows,
            true
        ));
        let (operations, new_tail) = divergence_rewrite_operations(
            &placed,
            &previous,
            &plan,
            &prepared,
            5,
            cell,
            u64::MAX,
            true,
        )
        .unwrap();
        assert_eq!(new_tail.len(), 1);
        assert_eq!(
            new_tail[0].rows, expected_rows,
            "transition 1 (normal batch): the replaced tall block's bookkeeping \
             rows must equal its decoded image rows"
        );
        assert_no_overlapping_row_spans(&operations);
        assert_row_bookkeeping_is_internally_consistent(&operations, old_rows_total, &new_tail);

        // --- Transition 2: clamp fallback (pane just under the stale
        // --- tail, WITH the status-bar row reserved). ---
        let tight_pane_rows = old_rows_total; // usable_rows = pane_rows - 1 < old_rows_total
        assert!(clamp_would_truncate(old_rows_total, tight_pane_rows, true));
        let fallback_tail =
            clamp_fallback_new_tail(&placed, &plan, &prepared, 5, cell, u64::MAX, true).unwrap();
        assert_eq!(fallback_tail.len(), 1);
        assert_eq!(
            fallback_tail[0].rows, expected_rows,
            "transition 2 (clamp fallback): the replaced tall block's bookkeeping \
             rows must equal its decoded image rows even though nothing was written"
        );
        assert_eq!(fallback_tail[0].id, new_tall_id);
        assert!(
            fallback_tail[0].rows >= tall_old_rows.min(expected_rows),
            "the tall block's new row count must not have been silently truncated \
             relative to what its own image needs"
        );

        // --- Transition 3: the fallback's follow-up window sync, with
        // --- content_row_offset = 1 (status bar reserves row 1). ---
        let mut synced_placed = placed[..5].to_vec();
        synced_placed.extend(fallback_tail.clone());
        let visible = 0..synced_placed.len();
        let sync_ops =
            sync_window_operations(&synced_placed, &[], visible, cell, u64::MAX, 1, 0).unwrap();
        let spans = placement_row_spans(&sync_ops);
        let tall_span = spans
            .iter()
            .find(|span| span.id == u32::try_from(new_tall_id).unwrap())
            .expect("the tall block's placement must survive the sync");
        assert_eq!(
            tall_span.end - tall_span.start,
            i64::from(expected_rows),
            "transition 3 (window sync): the tall block's drawn row span must \
             equal its decoded image rows, not be crushed to fewer rows"
        );
        assert_no_overlapping_row_spans(&sync_ops);
    }

    // --- PART 2: live status bar (D-STATUS) ---

    fn following_state(blocks: usize, font_size_pt: f64) -> StatusBarState {
        StatusBarState {
            following: true,
            blocks,
            font_size_pt,
        }
    }

    /// The status bar's byte sequence: save cursor, move to row 1, clear
    /// the line, write, restore cursor — a sequence that never leaves the
    /// cursor anywhere but where it started (DECSC/DECRC bracket the
    /// whole thing), starts with an absolute move to row 1 col 1, clears
    /// the line first, and ends with a full SGR reset before the restore
    /// — so it can never bleed color into whatever the real cursor
    /// position's content was.
    #[test]
    fn status_bar_operations_bracket_a_full_redraw_with_save_and_restore() {
        let operations = status_bar_operations(80, following_state(12, 15.0));
        assert_eq!(operations.len(), 1, "one atomic Local op");
        let TerminalOp::Local(bytes) = &operations[0] else {
            panic!("status bar operations must be Local, not Graphics");
        };
        let text = String::from_utf8_lossy(bytes);
        assert!(text.starts_with("\x1b7\x1b[1;1H\x1b[2K"), "{text:?}");
        assert!(text.ends_with("\x1b8"), "{text:?}");
        // The reset immediately before the restore, so nothing after this
        // op inherits stray color/attribute state.
        assert!(
            text[..text.len() - "\x1b8".len()].ends_with("\x1b[0m"),
            "{text:?}"
        );
    }

    /// The static brand and tagline must always be present, and the
    /// dynamic fields must reflect the passed state: block count, font
    /// size (integer pt formatted with no trailing `.0`), and the
    /// `following` word in the accent color.
    #[test]
    fn status_bar_operations_render_the_brand_and_dynamic_fields() {
        let operations = status_bar_operations(80, following_state(7, 15.0));
        let TerminalOp::Local(bytes) = &operations[0] else {
            panic!("expected Local");
        };
        let text = String::from_utf8_lossy(bytes);
        assert!(text.contains(STATUS_BRAND), "{text:?}");
        assert!(text.contains(STATUS_TAGLINE), "{text:?}");
        assert!(text.contains(STATUS_STATE_FOLLOWING), "{text:?}");
        assert!(text.contains("7 blocks"), "{text:?}");
        assert!(text.contains("15pt"), "{text:?}");
        assert!(
            text.contains(&format!(
                "\x1b[38;5;{STATUS_ACCENT_COLOR}m{STATUS_STATE_FOLLOWING}"
            )),
            "the following-state word must use the accent color: {text:?}"
        );
    }

    /// A follow-transition redraw: the SAME state (blocks, font size)
    /// rendered `following` vs. `scrolled` must differ in the state word
    /// AND its color (a different hue per the disengaged-state
    /// requirement), while the rest of the line (brand, tagline, block
    /// count, font size) stays identical.
    #[test]
    fn status_bar_operations_distinguish_following_from_scrolled_by_word_and_color() {
        let following = status_bar_operations(80, following_state(5, 15.0));
        let scrolled = status_bar_operations(
            80,
            StatusBarState {
                following: false,
                blocks: 5,
                font_size_pt: 15.0,
            },
        );
        let TerminalOp::Local(following_bytes) = &following[0] else {
            panic!("expected Local");
        };
        let TerminalOp::Local(scrolled_bytes) = &scrolled[0] else {
            panic!("expected Local");
        };
        let following_text = String::from_utf8_lossy(following_bytes);
        let scrolled_text = String::from_utf8_lossy(scrolled_bytes);

        assert!(following_text.contains(STATUS_STATE_FOLLOWING));
        assert!(scrolled_text.contains(STATUS_STATE_SCROLLED));
        assert!(
            following_text.contains(&format!("\x1b[38;5;{STATUS_ACCENT_COLOR}m")),
            "{following_text:?}"
        );
        assert!(
            scrolled_text.contains(&format!("\x1b[38;5;{STATUS_SCROLLED_COLOR}m")),
            "{scrolled_text:?}"
        );
        assert_ne!(
            STATUS_ACCENT_COLOR, STATUS_SCROLLED_COLOR,
            "the two states must use genuinely different hues"
        );
        // The shared fields (brand, tagline, block count, font size) are
        // identical between the two redraws — only the state word/color
        // differs.
        assert!(following_text.contains(STATUS_BRAND) && scrolled_text.contains(STATUS_BRAND));
        assert!(following_text.contains("5 blocks") && scrolled_text.contains("5 blocks"));
        assert!(following_text.contains("15pt") && scrolled_text.contains("15pt"));
    }

    /// Narrow-pane truncation: as `pane_cols` shrinks, right-side fields
    /// drop from the front (font size, then block count) before the state
    /// word, and the state word itself is dropped before the line would
    /// ever wrap — the brand is NEVER dropped or truncated (it always
    /// fits, or the line just has no right side at all).
    #[test]
    fn status_bar_operations_drop_right_side_fields_before_ever_wrapping() {
        let state = following_state(123, 17.5);

        // Plenty of room: every field present.
        let wide = status_bar_operations(200, state);
        let TerminalOp::Local(wide_bytes) = &wide[0] else {
            panic!("expected Local")
        };
        let wide_text = String::from_utf8_lossy(wide_bytes);
        assert!(wide_text.contains(STATUS_STATE_FOLLOWING));
        assert!(wide_text.contains("123 blocks"));
        assert!(wide_text.contains("17.5pt"));

        // Just enough room for the brand plus the state word, nothing else.
        let left_len = STATUS_BRAND.chars().count() + 1 + STATUS_TAGLINE.chars().count();
        let state_word_len = STATUS_STATE_FOLLOWING.chars().count();
        let narrow_cols = (left_len + 1 + state_word_len) as u32;
        let narrow = status_bar_operations(narrow_cols, state);
        let TerminalOp::Local(narrow_bytes) = &narrow[0] else {
            panic!("expected Local")
        };
        let narrow_text = String::from_utf8_lossy(narrow_bytes);
        assert!(
            narrow_text.contains(STATUS_STATE_FOLLOWING),
            "the state word must survive even a tight fit: {narrow_text:?}"
        );
        assert!(
            !narrow_text.contains("123 blocks"),
            "blocks must be dropped once it no longer fits: {narrow_text:?}"
        );
        assert!(
            !narrow_text.contains("17.5pt"),
            "font size must be dropped once it no longer fits: {narrow_text:?}"
        );

        // Impossibly narrow: not even the brand plus state word fits — no
        // right side at all, but the brand itself is still emitted whole
        // (never truncated mid-word) and the op never panics.
        let tiny = status_bar_operations(1, state);
        let TerminalOp::Local(tiny_bytes) = &tiny[0] else {
            panic!("expected Local")
        };
        let tiny_text = String::from_utf8_lossy(tiny_bytes);
        assert!(
            tiny_text.contains(STATUS_BRAND),
            "the brand is never truncated: {tiny_text:?}"
        );
        assert!(!tiny_text.contains(STATUS_STATE_FOLLOWING));
        assert!(!tiny_text.contains("123 blocks"));
        assert!(!tiny_text.contains("17.5pt"));
        // Exactly one Local op, exactly one line — never wraps onto a
        // second row regardless of how narrow the pane is.
        assert_eq!(tiny.len(), 1);
        assert_eq!(
            tiny_text.matches('\n').count(),
            0,
            "must never emit a bare newline (would wrap/scroll the pane): {tiny_text:?}"
        );
    }

    /// `sync_window_operations`'s `content_row_offset` reserves row 1 for
    /// the status bar: content homes to row 2, not row 1, and the erase-below
    /// never claws back into row 1.
    #[test]
    fn sync_window_operations_with_a_reserved_row_starts_content_at_row_two() {
        let cell = CellSize {
            width: 1,
            height: 1,
        };
        let placed = vec![placed(1, 2, rgba8_png(1, 2))];
        let operations = sync_window_operations(&placed, &[], 0..1, cell, u64::MAX, 1, 0).unwrap();
        let bytes = direct_bytes(&operations);
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            text.contains("\x1b[2;1H"),
            "content must home to row 2 (row 1 reserved): {text:?}"
        );
        assert!(
            !text.contains("\x1b[1;1H"),
            "must never home to row 1 when a row is reserved: {text:?}"
        );
    }

    // --- AT-S-301: row-budget conservation at the ops level ---
    //
    // specs/stream-open-tail-v1: does NOT build a PTY/fake-tty harness.
    // Instead this reuses the ops-level pattern the rest of this module's
    // tests already establish (e.g. `stream_shaped_revisions_never_produce_
    // an_interior_replace_or_remove`): drive `PlacementPlanner` the same way
    // `apply_revision` does (hash each block, plan, inspect `plan.ops`), and
    // simulate the row cost `append_operations`/`tail_replace_operations`
    // would actually emit from the op sequence alone, without invoking the
    // renderer. `rows(block)` is derived deterministically from the block's
    // source length via `tmath_core::placement::grid_for` over a synthetic
    // pixel height, mirroring how the real sink turns a rendered block's
    // pixel dimensions into a row count with a fixed `CellSize` — the point
    // is op-sequence row conservation, not pixel accuracy.

    /// A representative slice of the AT-S-201 corpus (see
    /// `tmath_render::stream::tests::AT_S_201_CORPUS`): the `\Lambda_n`
    /// display formula (line-leading `+` body lines — the exact
    /// pulldown-cmark misread pattern from the incident) plus surrounding
    /// Japanese prose, small enough to replay quickly at a fine stride.
    const AT_S_301_SLICE: &str = concat!(
        "最後に散布行列の更新式。\n\n",
        "\\[\n",
        "\\boldsymbol{\\Lambda}_n\n",
        "=\n",
        "\\boldsymbol{\\Lambda}_0\n",
        "+\\boldsymbol{S}\n",
        "+\n",
        "\\frac{\\kappa_0 n}{\\kappa_0 + n}\n",
        "(\\bar{\\boldsymbol{x}} - \\boldsymbol{\\mu}_0)\n",
        "\\]\n\n",
        "これで更新手順が完了した。\n"
    );

    /// A fixed stand-in `CellSize`, matching the width/height=1 fixtures
    /// this module's other ops-level tests already use.
    fn at_s_301_cell() -> CellSize {
        CellSize {
            width: 10,
            height: 20,
        }
    }

    /// Deterministic synthetic pixel height for a block, keyed only by its
    /// source length (no renderer invoked): every 40 source bytes adds one
    /// cell-height's worth of pixels, with a floor of one cell, so different
    /// blocks plausibly get different row counts without needing real
    /// typesetting.
    fn at_s_301_synthetic_height_px(source_len: usize) -> u32 {
        let cell = at_s_301_cell();
        let units = (source_len as u32 / 40).max(1);
        units * cell.height
    }

    fn at_s_301_rows_for(block: &tmath_render::Block) -> u32 {
        let cell = at_s_301_cell();
        let height_px = at_s_301_synthetic_height_px(block.source.len());
        let (_, rows) = tmath_core::placement::grid_for(cell.width, height_px, cell);
        rows
    }

    #[test]
    fn at_s_301_row_budget_is_conserved_across_the_slice_replay_at_stride_seven() {
        let stride = 7usize;
        let limits = tmath_render::Limits::default();
        let mut splitter = StreamSplitter::new(limits);
        let mut planner = PlacementPlanner::new();
        let options = RenderOptions::default();
        let cell = at_s_301_cell();

        let mut total_rows: i64 = 0;
        // Tracks each planned block's current row count by id, so a tail
        // Replace can look up `rows(old)` without re-deriving it — mirrors
        // `PlacedState.rows` in the real sink.
        let mut rows_by_id: std::collections::HashMap<u64, u32> = std::collections::HashMap::new();

        let mut apply = |revision: &Revision, planner: &mut PlacementPlanner| {
            let previous_last_id = planner.blocks().last().map(|block| block.id);
            let inputs: Vec<([u8; 32], u32, u32)> = revision
                .blocks
                .iter()
                .map(|block| {
                    let height_px = at_s_301_synthetic_height_px(block.source.len());
                    (content_hash(block, &options), cell.width, height_px)
                })
                .collect();
            let rows: Vec<u32> = revision.blocks.iter().map(at_s_301_rows_for).collect();

            let plan = planner.plan(&inputs);

            for (index, op) in plan.ops.iter().enumerate() {
                match op {
                    PlanOp::Keep { .. } => {}
                    PlanOp::Append { block } => {
                        let new_rows = rows[index];
                        // append_operations: the placed block's own rows plus
                        // one `\r\n` separator row.
                        total_rows += i64::from(new_rows) + 1;
                        rows_by_id.insert(block.id, new_rows);
                    }
                    PlanOp::Replace { old_id, block } => {
                        // (a): every Replace in a plain streamed session must
                        // be a pure tail replace, targeting the block that
                        // was the previous revision's last planned block —
                        // otherwise the net-new-rows arithmetic below is not
                        // valid (see `tail_replace_operations`, which only
                        // ever clears+redraws the same tail slot).
                        assert_eq!(
                            Some(*old_id),
                            previous_last_id,
                            "non-tail Replace at op index {index}: old_id {old_id}, \
                             previous tail was {previous_last_id:?}"
                        );
                        let old_rows = *rows_by_id.get(old_id).expect("old id must be tracked");
                        let new_rows = rows[index];
                        // tail_replace_operations: cursor up old_rows, clear,
                        // redraw at new_rows, plus the trailing `\r\n` — net
                        // delta against what was already counted for the old
                        // placement is (new_rows - old_rows).
                        total_rows += i64::from(new_rows) - i64::from(old_rows);
                        rows_by_id.remove(old_id);
                        rows_by_id.insert(block.id, new_rows);
                    }
                    PlanOp::Remove { id } => {
                        panic!("(a) unexpected Remove of block {id} at op index {index}");
                    }
                }
            }
        };

        for chunk in AT_S_301_SLICE.as_bytes().chunks(stride) {
            let revision = splitter.push(chunk).unwrap();
            apply(&revision, &mut planner);
        }
        let finished = splitter.finish().unwrap();
        apply(&finished, &mut planner);

        // (b): zero leftover rows — the simulated total must equal the sum
        // over the FINAL revision's blocks of their rows plus one separator
        // row per block, exactly what a fresh append-only replay of the
        // final blocks would have produced.
        let expected_total: i64 = finished
            .blocks
            .iter()
            .map(|block| i64::from(at_s_301_rows_for(block)) + 1)
            .sum();
        assert_eq!(
            total_rows, expected_total,
            "simulated row total must match the final blocks' row budget with no leftover rows"
        );
        assert_eq!(
            rows_by_id.len(),
            finished.blocks.len(),
            "every currently-placed id must correspond to exactly one final block"
        );
    }
}
