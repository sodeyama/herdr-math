//! Incremental native rendering for `tmath render --engine native -`.

use std::io::{self, Read as _, Write as _};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;

use tmath_core::placement::{
    decode_png, emit_placed_block_cursor, CellSize, PlacementLimits, TerminalOp,
};
use tmath_core::terminal::{StdioTty, Terminal};
use tmath_render::{
    content_hash, CacheBudget, Limits, PlacementPlanner, Plan, PlanOp, RenderCache, RenderError,
    RenderOptions, RenderedBlock, Revision, SafeErrorRecord, StreamSplitter,
};

use crate::terminal_output;

const READ_CHUNK_BYTES: usize = 8 * 1024;

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
        crate::layout::resolve_content_width_pt(content_width, fitted),
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

pub(crate) fn apply_revision(
    revision: &Revision,
    options: &RenderOptions,
    cache: &mut RenderCache,
    planner: &mut PlacementPlanner,
    formula_errors: &mut Vec<usize>,
    sink: &mut StreamSink,
) -> Result<(), RenderError> {
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
    sink.emit(&plan, &prepared, &previous)?;
    *formula_errors = next_formula_errors;
    Ok(())
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

    /// Sets or clears visibility-gated emission (AT-3-503): while suppressed,
    /// `apply_revision`'s append/replace/remove operations still update
    /// state but skip terminal writes. See [`TerminalSink::suppress_writes`].
    /// A no-op in `Summary` mode.
    pub(crate) fn set_suppress_writes(&mut self, suppress: bool) {
        if let Self::Terminal(sink) = self {
            sink.suppress_writes = suppress;
        }
    }

    fn emit(
        &mut self,
        plan: &Plan,
        prepared: &[PreparedBlock],
        previous: &[tmath_render::PlannedBlock],
    ) -> Result<(), RenderError> {
        match self {
            Self::Summary => emit_summary(plan, prepared),
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
    ) -> Result<(), RenderError> {
        match self {
            Self::Summary => Ok(()),
            Self::Terminal(sink) => sink.sync_window(visible),
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
    /// only) but skip writing to the terminal (AT-3-503's visibility-gated
    /// emission): while the agent-viewer's follow is disengaged, new/changed
    /// blocks land outside the visible window, so streaming them to the pane
    /// bottom would just be undone by the next `sync_window`. The caller
    /// (`agent_viewer`) sets this before `apply_revision` while disengaged
    /// and calls `sync_window` afterward to reconcile the screen with the
    /// (possibly changed) window contents. Always `false` for stream/watch
    /// sessions, which never disengage follow.
    suppress_writes: bool,
    /// AT-3-504's bound on retained PNGs: on every `sync_window`, blocks more
    /// than this many positions outside the new `visible` range (on either
    /// side) have their `PlacedState::png` evicted to an empty vec. `u64::MAX`
    /// (the default) means unbounded, which is what stream/watch sessions
    /// want since they never retain PNGs in the first place. Set only
    /// through [`StreamSink::with_retained_window_blocks`].
    retained_window_blocks: u64,
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
            retained_window_blocks: u64::MAX,
        }
    }

    fn emit(
        &mut self,
        plan: &Plan,
        prepared: &[PreparedBlock],
        previous: &[tmath_render::PlannedBlock],
    ) -> Result<(), RenderError> {
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
        Ok(())
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
    fn emit_batch(
        &mut self,
        plan: &Plan,
        prepared: &[PreparedBlock],
        previous: &[tmath_render::PlannedBlock],
    ) -> Result<(), RenderError> {
        let Some(reanchor_from) = plan.reanchor_from else {
            return Ok(());
        };
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
        Ok(())
    }

    fn append(&mut self, id: u64, rendered: &RenderedBlock, png: &[u8]) -> Result<(), RenderError> {
        let decoded = self.decode(id, rendered, png)?;
        self.validate_placement(decoded.pixels, None)?;
        let operations = append_operations(
            decoded.id,
            rendered.width_px,
            rendered.height_px,
            &decoded.rgba,
            decoded.cols,
            decoded.rows,
            self.first_append_at_line_start || !self.placed.is_empty(),
        );
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
        let top_is_reachable = was_last
            && self
                .placed
                .last()
                .is_some_and(|placed| placed.id == old_id_value)
            && self
                .terminal
                .cursor_position()
                .ok()
                .flatten()
                .is_some_and(|(row, _)| row > old_rows);

        let operations = if top_is_reachable {
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
            let mut operations = vec![TerminalOp::Graphics(tmath_core::kitty::kitty_delete_id(
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
    ) -> Result<(), RenderError> {
        let visible = visible.start.min(self.placed.len())..visible.end.min(self.placed.len());
        let operations = sync_window_operations(
            &self.placed,
            &self.emitted_ids,
            visible.clone(),
            self.cell,
            self.max_image_pixels,
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
    let stale_ids: Vec<u64> = previous[reanchor_from.min(previous.len())..]
        .iter()
        .map(|block| block.id)
        .collect();
    let old_rows_total: u32 = stale_ids
        .iter()
        .filter_map(|id| placed.iter().find(|entry| entry.id == *id))
        .map(|entry| entry.rows)
        .fold(0u32, |total, rows| total.saturating_add(rows));

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
        match operation {
            PlanOp::Keep { id } => {
                let entry = placed
                    .iter()
                    .find(|entry| entry.id == *id)
                    .ok_or_else(stream_error)?;
                let (width, height, rgba) =
                    decode_png(&entry.png, max_image_pixels).map_err(|_| stream_error())?;
                let (cols, rows) = tmath_core::placement::grid_for(width, height, cell);
                let image_id = u32::try_from(*id).map_err(|_| stream_error())?;
                operations.extend(append_operations(
                    image_id, width, height, &rgba, cols, rows, true,
                ));
                new_tail.push(PlacedState {
                    id: *id,
                    rows,
                    pixels: u64::from(width) * u64::from(height),
                    png: entry.png.clone(),
                });
            }
            PlanOp::Append { block } | PlanOp::Replace { block, .. } => {
                let (rendered, png, _) = rendered_event(prepared, index)?;
                let (width, height, rgba) =
                    decode_png(png, max_image_pixels).map_err(|_| stream_error())?;
                if width != rendered.width_px || height != rendered.height_px {
                    return Err(stream_error());
                }
                let (cols, rows) = tmath_core::placement::grid_for(width, height, cell);
                let image_id = u32::try_from(block.id).map_err(|_| stream_error())?;
                operations.extend(append_operations(
                    image_id, width, height, &rgba, cols, rows, true,
                ));
                new_tail.push(PlacedState {
                    id: block.id,
                    rows,
                    pixels: u64::from(width) * u64::from(height),
                    png: retained_png(png, retain),
                });
            }
            PlanOp::Remove { .. } => {}
        }
    }
    operations.push(TerminalOp::Local(b"\x1b[0J".to_vec()));

    Ok((operations, new_tail))
}

/// Builds the operation list for a visibility-driven viewport sync
/// (AT-3-503): deletes every id in `emitted_ids` that is not among the new
/// `visible` range's ids, moves the cursor home, re-emits every block in
/// `visible` (clamped to `placed`'s bounds) at its window-relative row
/// (immediately after the previous one, cursor-relative) from its retained
/// PNG, and erases any residual rows below what was just drawn. Pure and
/// independent of any live terminal, the same way `tail_replace_operations`
/// is.
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
/// but something was previously on screen, the pane is homed and erased
/// too, so a scroll past all content still clears what was there.
fn sync_window_operations(
    placed: &[PlacedState],
    emitted_ids: &[u64],
    visible: std::ops::Range<usize>,
    cell: CellSize,
    max_image_pixels: u64,
) -> Result<Vec<TerminalOp>, RenderError> {
    let visible = visible.start.min(placed.len())..visible.end.min(placed.len());
    let visible_ids: Vec<u64> = placed[visible.clone()]
        .iter()
        .map(|entry| entry.id)
        .collect();

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
        // Move home once; each block below is then placed immediately after
        // the previous one via the cursor-relative form, so no per-block
        // home-row arithmetic is needed to keep window-relative rows correct.
        operations.push(TerminalOp::Local(b"\x1b[H".to_vec()));
        for entry in &placed[visible] {
            let (width, height, rgba) =
                decode_png(&entry.png, max_image_pixels).map_err(|_| stream_error())?;
            let (cols, rows) = tmath_core::placement::grid_for(width, height, cell);
            let image_id = u32::try_from(entry.id).map_err(|_| stream_error())?;
            operations.extend(append_operations(
                image_id, width, height, &rgba, cols, rows, true,
            ));
        }
        operations.push(TerminalOp::Local(b"\x1b[0J".to_vec()));
    } else if !emitted_ids.is_empty() {
        operations.push(TerminalOp::Local(b"\x1b[H\x1b[0J".to_vec()));
    }

    Ok(operations)
}

fn clear_rows(rows: u32) -> Vec<u8> {
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

        let operations = sync_window_operations(&placed, &[1, 2], 1..3, cell, u64::MAX).unwrap();
        let bytes = direct_bytes(&operations);
        let text = String::from_utf8_lossy(&bytes);

        // Only id=1 left the window (was in `emitted_ids`, not among
        // `visible`'s ids); id=2 stayed in both and is not deleted.
        assert!(text.contains("\x1b_Ga=d,d=I,i=1,q=2\x1b\\"));
        assert!(!text.contains("\x1b_Ga=d,d=I,i=2,q=2\x1b\\"));
        assert!(!text.contains("\x1b_Ga=d,d=I,i=3,q=2\x1b\\"));
        assert!(text.contains("\x1b[H"), "home once before re-emitting");
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
            sync_window_operations(&placed, &emitted_ids, 0..1, cell, u64::MAX).unwrap();
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

        let operations = sync_window_operations(&placed, &[1], 0..0, cell, u64::MAX).unwrap();
        let bytes = direct_bytes(&operations);
        let text = String::from_utf8_lossy(&bytes);

        assert!(text.contains("\x1b_Ga=d,d=I,i=1,q=2\x1b\\"));
        assert!(!text.contains("U=1,c=1"), "no placement is re-emitted");
        assert!(
            text.ends_with("\x1b[H\x1b[0J"),
            "home and erase clear the stale rows when nothing is re-emitted: {text:?}"
        );
    }

    /// A fresh sync (`emitted_ids` starts empty, as it does for the first
    /// `sync_window` call after construction) emits the visible range with
    /// no deletes, since nothing was on screen to remove.
    #[test]
    fn sync_window_from_empty_emitted_only_adds() {
        let cell = CellSize {
            width: 1,
            height: 1,
        };
        let placed = vec![placed(1, 2, rgba8_png(1, 2))];
        let operations = sync_window_operations(&placed, &[], 0..1, cell, u64::MAX).unwrap();
        let bytes = direct_bytes(&operations);
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.contains("a=d,d=I"), "nothing to delete");
        assert!(text.contains("i=1,U=1,c=1,r=2,q=2"));
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
        let operations = sync_window_operations(&placed, &[], 0..0, cell, u64::MAX).unwrap();
        assert!(operations.is_empty());
    }

    #[test]
    fn sync_window_clamps_an_out_of_range_slice_instead_of_panicking() {
        let cell = CellSize {
            width: 1,
            height: 1,
        };
        let placed = vec![placed(1, 2, rgba8_png(1, 2))];
        let operations = sync_window_operations(&placed, &[1], 0..5, cell, u64::MAX).unwrap();
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
            &sync_window_operations(&short_history, &previously_emitted, 5..10, cell, u64::MAX)
                .unwrap(),
        );
        let long_bytes = direct_bytes(
            &sync_window_operations(
                &long_history,
                &previously_emitted,
                995..1000,
                cell,
                u64::MAX,
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
}
