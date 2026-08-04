//! `tmath agent-viewer` — the process that runs inside the tmux viewer split.
//!
//! It connects to the watcher's Unix socket and renders each received answer
//! document through the native V3 pipeline: [`tmath_render::parse_blocks_limited`]
//! splits the document into semantic blocks, a [`tmath_render::RenderCache`]
//! renders (or reuses) each block's PNG, and a [`tmath_render::PlacementPlanner`]
//! diffs the new block list against the previous one to produce `Keep`/
//! `Append`/`Replace`/`Remove` operations. Those operations are emitted as
//! per-block Kitty placements through [`crate::native_stream`]'s shared
//! `StreamSink`/`TerminalSink` machinery — the same emitter stream mode uses,
//! reused rather than forked.
//!
//! There is no composite RGBA buffer: unchanged blocks are never re-rendered
//! or re-transmitted, and a shorter replacement answer clears its stale
//! placement instead of leaving orphan cells (`PlanOp::Remove`).
//!
//! The viewer owns an explicit visibility window over the placed blocks
//! ([`crate::viewer_viewport::Viewport`]), per plan section D6. Follow mode
//! pins that window to the newest block as answers stream in; any manual
//! scroll input disengages follow and `End`/`F` re-engage it (AT-3-502). A
//! window change syncs the terminal from cached PNGs
//! (see [`native_stream::StreamSink::sync_window`]): placements that left
//! the window are deleted, and the window's current blocks are re-emitted —
//! no block is re-rendered, and the transmitted bytes are bounded by the
//! window size, independent of how much history exists outside it
//! (AT-3-503). While follow is disengaged, `apply_revision`'s writes for
//! new/changed blocks are suppressed (state still updates; see
//! [`native_stream::StreamSink::set_suppress_writes`]) since they would
//! land outside the window, and `sync_visible_window` reconciles the screen
//! afterward instead. `q`/`Ctrl-C` close the viewer. Render failures leave
//! earlier placements intact (fail closed).
//!
//! Retained PNGs are bounded (AT-3-504): every `render_and_place` call
//! evicts blocks more than `Limits::retained_window_blocks` positions
//! outside the current visibility window, unconditionally — not only on the
//! disengaged-follow/`sync_window` path. This matters because while
//! following, `sync_window` is never called at all (`apply_revision` streams
//! straight to the pane bottom, and the window is always the tail), so a
//! mainline streamed session with follow engaged the whole time is exactly
//! where eviction needs to run on every append, or a long session would
//! retain every block's PNG forever. The block's state (id, rows, source
//! text) is kept regardless of eviction — only the rendered bytes are
//! dropped — so memory stays flat across a long session even as
//! `planner`/`block_sources` keep growing. Scrolling back onto an evicted
//! block restores it (`restore_missing_pngs`/`restore_png_for_id`) via a
//! `RenderCache` content-hash hit or, failing that, a real re-render from
//! the retained source — never by re-fetching from the watcher. A restore
//! failure leaves that one block showing nothing rather than disturbing the
//! rest of the window (fail closed).
//!
//! The socket carries versioned delta frames in addition to whole
//! `Document` frames (AT-3-601): [`tmath_core::agent::DeltaState`]
//! reassembles the running document text from `Document`/`Append`/
//! `ReplaceTail` messages and enforces the delta protocol (a monotonic
//! sequence number, a fixed version) fail-closed — a rejected frame (unknown
//! version, duplicate/out-of-order sequence, or an invalid `ReplaceTail`
//! boundary) leaves the document and every placed block untouched and
//! invalidates delta tracking until the next whole `Document` frame
//! resyncs it. `apply_incoming_message` is the only caller of
//! `render_and_place` from the socket read loop; the block-diffing and
//! placement pipeline itself is unchanged — only how the input text is
//! assembled is new. A `Document` frame (what a V2-style source still
//! sends) is always accepted regardless of delta state, so a V3 viewer
//! stays backward compatible. Delta *emission* on the watcher side is out
//! of scope here (T3-402/403).

use std::io::{self, Read as _};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use tmath_core::agent::{Decoder, DeltaState, Message};
use tmath_core::input::InputDecoder;
use tmath_core::ipc::IPC_MAX_REQUEST_BYTES;
use tmath_core::placement::CellSize;
use tmath_core::scroll_driver::{is_exit_signal, scroll_delta};
use tmath_core::terminal::{StdioTty, Terminal};
use tmath_render::{
    Block, CacheBudget, Limits, PlacementPlanner, RenderCache, RenderOptions, StreamSplitter,
};

use crate::native_stream::{self, StreamSink};
use crate::viewer_viewport::Viewport;

const CONNECT_RETRIES: u32 = 50;
const CONNECT_RETRY_MS: u64 = 100;
const POLL_TIMEOUT: Duration = Duration::from_millis(40);

struct Viewer {
    stream: UnixStream,
    input: InputDecoder,
    messages: Decoder,
    viewport: Viewport,
    cache: RenderCache,
    limits: Limits,
    planner: PlacementPlanner,
    formula_errors: Vec<usize>,
    options: RenderOptions,
    sink: StreamSink,
    blocks_placed: usize,
    /// Measured terminal cell size, used to convert the planner's per-block
    /// pixel dimensions into the row heights the viewport tracks. Computing
    /// heights from `planner.blocks()` (rather than reading them back out of
    /// `sink`) keeps the viewport buildable and testable in `StreamSink::Summary`
    /// mode, which has no terminal and no placed-block state of its own.
    cell: CellSize,
    /// The current document's blocks, source text included, in the same
    /// order and length as `planner.blocks()`. `PlannedBlock` (what the
    /// planner keeps) has no source — only a content hash — so this is the
    /// only place a block's text survives past one `render_and_place` call.
    /// It is what lets `restore_missing_pngs` re-render a block evicted by
    /// `TerminalSink`'s AT-3-504 eviction on scroll-back, either via a
    /// `RenderCache` content-hash hit or a real re-render of the source.
    /// Each block's size is already bounded by `limits.source_bytes_per_block`
    /// (enforced by the splitter before `render_and_place` ever sees it), so
    /// this stays bounded the same way the render cache's inputs already are.
    block_sources: Vec<Block>,
    /// Reassembles the running document text from `Document`/`Append`/
    /// `ReplaceTail` frames (AT-3-601), enforcing the delta protocol's
    /// version/sequence rules fail-closed. `render_and_place` is called with
    /// `delta.document()` after every accepted message — the block-diffing
    /// path is unchanged; only how the input text is assembled is new.
    /// Constructed with `IPC_MAX_REQUEST_BYTES` as its aggregate byte bound
    /// — the same cap `encode_document` already enforces per whole-document
    /// frame, so a delta-reassembled document and a directly-sent one agree
    /// on the largest document either path can ever produce.
    delta: DeltaState,
}

pub(crate) fn run_agent_viewer(args: &[String]) -> Result<i32, String> {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("usage: tmath agent-viewer <socket-path>");
        return Ok(0);
    }
    let socket = args.first().ok_or("agent-viewer requires a socket path")?;

    let stream = connect_with_retry(socket)?;

    let mut terminal = Terminal::new(StdioTty::default(), 1)
        .map_err(|error| format!("initialize terminal: {error}"))?;
    // Inside tmux, capability and cell-size queries cannot round-trip through
    // the passthrough envelope, so graphics support is assumed (the sequences
    // are fire-and-forget transmits). Everywhere else the probe stays
    // mandatory and fail-closed.
    let tmux_passthrough = tmath_core::kitty::inside_tmux();
    if tmux_passthrough {
        let route = crate::terminal_output::selected_route()?;
        eprintln!(
            "agent-viewer: graphics route {}; require a visible Kitty-capable tmux client",
            route.label()
        );
    } else if !terminal
        .probe_graphics_support()
        .map_err(|error| format!("probe graphics: {error}"))?
    {
        let _ = terminal.reset();
        return Err("this terminal reports no Kitty graphics support".into());
    }
    // The terminal-reported cell size. Inside tmux this came from the
    // winsize fallback (see below), which is not always physical pixels —
    // `measured_cell` must not be used for anything downstream; the
    // effective (possibly `TMATH_DPR`-corrected) physical cell computed
    // below as `cell` is the only one placements, `grid_for`, and the
    // viewport may use.
    let measured_cell = terminal
        .cell_size()
        .map_err(|error| format!("measure cell size: {error}"))?
        .ok_or("terminal reported no usable cell size")?;

    // Auto-fit content width, font size, and device pixel ratio to this
    // pane's measured geometry so the rendered images match the viewer
    // pane's width and the surrounding terminal text size (there is no CLI
    // override for the viewer, which always runs against a real terminal).
    let pane_size = terminal
        .size()
        .map_err(|error| format!("measure viewer pane size: {error}"))?;
    // `tmux_passthrough` (computed above as `inside_tmux()`) is exactly the
    // condition under which `Terminal::cell_size` took the winsize fallback
    // (the `CSI 16t` pixel query is unusable inside tmux) — see the
    // `TMATH_DPR` section of `layout`'s module doc for why that fallback can
    // report logical rather than physical pixels and why an explicit
    // override is needed to correct it.
    let tmath_dpr_env = std::env::var("TMATH_DPR").ok();
    let dpr_override =
        crate::layout::resolve_dpr_override(tmath_dpr_env.as_deref(), tmux_passthrough);
    if tmux_passthrough && tmath_dpr_env.is_some() && dpr_override.is_none() {
        // Never log the raw value: it is a stable, small piece of config,
        // but keeping the log purely event-shaped (per AGENTS.md) avoids any
        // habit of echoing user-controlled strings here.
        eprintln!("agent-viewer: TMATH_DPR invalid, ignoring");
    }
    let fitted = crate::layout::terminal_fit_layout(
        measured_cell.0,
        measured_cell.1,
        pane_size.cols,
        dpr_override,
    );
    // `fitted.effective_cell_px` is the physical cell the fit actually used
    // (measured × dpr_override when one applied, unchanged otherwise) — see
    // the FIX note on `TerminalFitLayout::effective_cell_px`. Every
    // downstream consumer (the sink's placement grid, `viewer.cell`,
    // `block_heights`/`grid_for`, the viewport) uses this `cell`, never
    // `measured_cell`, so a corrected dpr and the cell it was derived from
    // never disagree.
    let cell = CellSize {
        width: fitted.effective_cell_px.0,
        height: fitted.effective_cell_px.1,
    };
    eprintln!(
        "agent-viewer: fitted cell_w_px={} cell_h_px={} dpr={} dpr_override={} content_width_pt={:.1} pane_cols={}",
        cell.width,
        cell.height,
        fitted.device_pixel_ratio,
        dpr_override.is_some(),
        fitted.content_width_pt,
        pane_size.cols
    );
    // The agent-viewer has no CLI flag of its own (it is spawned by `tmath
    // agent`, not run directly), so its font size precedence is env > config
    // > auto-fit > default — reads the config file directly at startup, per
    // the config module's doc comment.
    let font_config = crate::config::config_path()
        .map(|path| crate::config::load(&path))
        .unwrap_or_default();
    let (font_size_pt, font_size_source) =
        crate::config::resolve_font_size_pt_with_source(None, &font_config, Some(fitted));
    eprintln!(
        "agent-viewer: font_size source={} value={font_size_pt}",
        font_size_source.label()
    );
    let options = RenderOptions::new(
        fitted.content_width_pt,
        font_size_pt,
        fitted.device_pixel_ratio,
    )
    .map_err(|_| "invalid agent-viewer render options".to_string())?;
    let device_pixel_ratio = fitted.device_pixel_ratio;
    let limits = Limits::default();
    let scaled = limits.scaled(device_pixel_ratio);
    let max_entries = usize::try_from(limits.blocks_per_document)
        .unwrap_or(usize::MAX)
        .max(1);
    let cache = RenderCache::new(CacheBudget {
        max_entries,
        max_pixels: scaled.image_pixels.max(1),
    });
    let sink = StreamSink::new(
        Some((terminal, (cell.width, cell.height))),
        scaled.image_pixels,
    )
    .with_retained_pngs()
    .with_retained_window_blocks(limits.retained_window_blocks);

    let mut viewer = Viewer {
        stream,
        input: InputDecoder::new(),
        messages: Decoder::new(),
        viewport: Viewport::new(pane_size.rows),
        cache,
        limits,
        planner: PlacementPlanner::new(),
        formula_errors: Vec::new(),
        options,
        sink,
        blocks_placed: 0,
        cell,
        block_sources: Vec::new(),
        delta: DeltaState::new(IPC_MAX_REQUEST_BYTES),
    };
    let _ = viewer.stream.set_nonblocking(true);
    eprintln!("agent-viewer: connected; q/Ctrl-C closes");

    let loop_result = run_viewer_loop(&mut viewer);
    let _ = viewer.sink.finish();
    loop_result
}

fn connect_with_retry(socket: &str) -> Result<UnixStream, String> {
    let mut last = None;
    for _ in 0..CONNECT_RETRIES {
        match UnixStream::connect(socket) {
            Ok(stream) => return Ok(stream),
            Err(error) => {
                last = Some(error);
                std::thread::sleep(Duration::from_millis(CONNECT_RETRY_MS));
            }
        }
    }
    Err(format!(
        "could not connect to {}: {}",
        socket,
        last.map(|e| e.to_string()).unwrap_or_default()
    ))
}

fn run_viewer_loop(viewer: &mut Viewer) -> Result<i32, String> {
    loop {
        let start = Instant::now();

        // Read viewer-control messages from the watcher.
        if stream_readable(&viewer.stream) {
            let mut chunk = [0u8; 4096];
            match viewer.stream.read(&mut chunk) {
                Ok(0) => {
                    eprintln!("agent-viewer: watcher closed; finishing");
                    return Ok(0);
                }
                Ok(n) => {
                    viewer.messages.push(&chunk[..n]);
                    while let Some(message) = viewer.messages.next_message() {
                        match message {
                            Ok(Message::Quit) => return Ok(0),
                            Ok(message) => apply_incoming_message(viewer, &message)?,
                            Err(error) => {
                                eprintln!("agent-viewer: malformed_message dropped ({error:?})")
                            }
                        }
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(format!("read socket: {error}")),
            }
        }

        // Terminal input: `q`/Ctrl-C close the viewer. `End`/`F` re-engage
        // follow and jump the viewport to the bottom; any other scroll-shaped
        // input disengages follow and moves the viewport. A window change
        // triggers a redraw of the newly visible blocks.
        if stdin_readable() {
            let mut chunk = [0u8; 256];
            let mut stdin = io::stdin();
            match stdin.read(&mut chunk) {
                Ok(n) if n > 0 => {
                    viewer.input.push(&chunk[..n]);
                    while let Some(event) = viewer.input.next_event() {
                        if is_exit_signal(&event) {
                            return Ok(0);
                        }
                        handle_scroll_input(viewer, &event);
                    }
                }
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(format!("read stdin: {error}")),
            }
        }

        let elapsed = start.elapsed();
        if elapsed < POLL_TIMEOUT {
            std::thread::sleep(POLL_TIMEOUT - elapsed);
        }
    }
}

/// Routes one decoded input event through the viewport (AT-3-502): `End`/`F`
/// re-engage follow and jump to the bottom; any other scroll-shaped event
/// (wheel, arrows, `j`/`k`, `PgUp`/`PgDn`, `Home`) disengages follow and moves
/// the window by the same row mapping stream mode's scroll driver uses. A
/// window that actually moved triggers a redraw of the newly visible blocks.
fn handle_scroll_input(viewer: &mut Viewer, event: &tmath_core::input::Event) {
    use tmath_core::input::{Event, KeyEvent};
    use tmath_core::mouse::Key;

    let following_before = viewer.viewport.following();
    let moved = match event {
        Event::Key(KeyEvent {
            key: Key::End,
            ctrl: false,
            ..
        })
        | Event::Key(KeyEvent {
            key: Key::Char('F'),
            ctrl: false,
            ..
        }) => {
            let before = viewer.viewport.offset();
            viewer.viewport.jump_to_bottom_and_follow();
            viewer.viewport.offset() != before
        }
        other => match scroll_delta(other, None) {
            Some(delta) => viewer.viewport.scroll_by(delta),
            None => false,
        },
    };

    if viewer.viewport.following() != following_before {
        eprintln!("agent-viewer: follow={}", viewer.viewport.following());
    }
    // Render/limit failures elsewhere in this module are fail-closed (log and
    // keep the previous placements intact); a sync failure follows the same
    // contract rather than tearing down the viewer process over a scroll step.
    if moved {
        if let Err(error) = sync_visible_window(viewer) {
            eprintln!("agent-viewer: {error}");
        }
    }
}

fn stream_readable(stream: &UnixStream) -> bool {
    use rustix::event::{PollFlags, Timespec};
    let mut fds = [rustix::event::PollFd::new(stream, PollFlags::IN)];
    let timeout = Timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    rustix::event::poll(&mut fds, Some(&timeout))
        .map(|n| n > 0)
        .unwrap_or(false)
}

fn stdin_readable() -> bool {
    use rustix::event::{PollFlags, Timespec};
    let stdin = io::stdin();
    let mut fds = [rustix::event::PollFd::new(&stdin, PollFlags::IN)];
    let timeout = Timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    rustix::event::poll(&mut fds, Some(&timeout))
        .map(|n| n > 0)
        .unwrap_or(false)
}

/// Applies one incoming `Document`/`Append`/`ReplaceTail` message to the
/// running document (AT-3-601) and, if it changed the text, feeds the
/// reassembled document through the existing `render_and_place` path — the
/// same block-diffing/placement pipeline `Document`-only messages always
/// used. A rejected delta (unknown version, bad sequence, or an invalid
/// `ReplaceTail` boundary) is fail-closed: it is logged with a stable error
/// code only (never message content), the previous document and all placed
/// blocks stay exactly as they were, and — per `DeltaState`'s resync
/// policy — every later delta is rejected too until the next `Document`
/// frame. `Quit` never reaches here (handled directly in the read loop).
fn apply_incoming_message(viewer: &mut Viewer, message: &Message) -> Result<(), String> {
    match viewer.delta.apply(message) {
        Ok(Some(text)) => {
            let text = text.to_string();
            render_and_place(viewer, &text)
        }
        Ok(None) => Ok(()),
        Err(error) => {
            eprintln!("agent-viewer: delta_rejected ({error:?})");
            Ok(())
        }
    }
}

/// Converts the planner's current per-block pixel dimensions into the row
/// heights the viewport tracks, using the measured terminal cell size.
fn block_heights(viewer: &Viewer) -> Vec<u32> {
    viewer
        .planner
        .blocks()
        .iter()
        .map(|block| {
            tmath_core::placement::grid_for(block.width_px, block.height_px, viewer.cell).1
        })
        .collect()
}

/// Syncs the terminal to the viewport's current visible-block range
/// (AT-3-503): restores any evicted PNGs the new window now covers
/// (AT-3-504's scroll-back path), then deletes placements that left the
/// window and re-emits the window's current blocks from cache, without
/// touching anything outside it. An empty visible range (no blocks, or the
/// window scrolled past all content) is passed through so a previously
/// non-empty window gets its placements deleted too, rather than left stale
/// on screen.
fn sync_visible_window(viewer: &mut Viewer) -> Result<(), String> {
    let visible = viewer.viewport.visible_blocks();
    let range = visible.first..visible.last_exclusive;
    restore_missing_pngs(viewer, range.clone());
    viewer
        .sink
        .sync_window(range)
        .map_err(|error| format!("sync_failed ({:?})", error.safe_record().code))
}

/// AT-3-504's scroll-back restore: for every block in `range` whose retained
/// PNG `TerminalSink` evicted (out of budget, then scrolled back into view),
/// re-renders it via [`restore_png_for_id`] and pushes the result back into
/// the sink. Fails closed per block: a render or refresh failure for one
/// block leaves it showing nothing (its retained PNG stays empty, so
/// `sync_window` simply skips re-emitting it — it does not fail the whole
/// sync or disturb any other placement) and is logged; the viewer keeps
/// running.
fn restore_missing_pngs(viewer: &mut Viewer, range: std::ops::Range<usize>) {
    let missing_ids = viewer.sink.missing_pngs(range);
    for id in missing_ids {
        let Some(png) = restore_png_for_id(viewer, id) else {
            continue;
        };
        if let Err(error) = viewer.sink.refresh_png(id, png) {
            eprintln!(
                "agent-viewer: restore_failed ({:?}) id={id}",
                error.safe_record().code
            );
        }
    }
}

/// Re-renders block `id`'s PNG from `viewer.block_sources` — first via
/// `RenderCache` (a content-hash hit means the block's pixels are still
/// cached from when it was first placed or from an identical block
/// elsewhere in the document, so no render work happens), falling back to a
/// real render of the block's source. Returns `None` (and logs) on any
/// failure: an id no longer in the planner, a missing source, a render
/// error, or an encode error. Kept independent of `viewer.sink` so the
/// restore logic itself — the part that actually decides what bytes come
/// back for a given id — is testable without a live terminal.
fn restore_png_for_id(viewer: &mut Viewer, id: u64) -> Option<Vec<u8>> {
    let index = viewer
        .planner
        .blocks()
        .iter()
        .position(|block| block.id == id)?;
    let Some(source) = viewer.block_sources.get(index) else {
        eprintln!("agent-viewer: restore_failed (missing_source) id={id}");
        return None;
    };
    let rendered = match viewer.cache.render(source, &viewer.options) {
        Ok(rendered) => rendered,
        Err(error) => {
            eprintln!(
                "agent-viewer: restore_failed ({:?}) id={id}",
                error.safe_record().code
            );
            return None;
        }
    };
    let scaled_image_pixels = viewer
        .limits
        .scaled(viewer.options.device_pixel_ratio)
        .image_pixels;
    match crate::native_render::canonical_block_png(&rendered, scaled_image_pixels) {
        Ok(png) => Some(png),
        Err(error) => {
            eprintln!(
                "agent-viewer: restore_failed ({:?}) id={id}",
                error.safe_record().code
            );
            None
        }
    }
}

/// Splits the received document into blocks, plans per-block placement
/// operations against the previous document's blocks, and emits only the
/// operations the plan calls for (append/replace/remove); unchanged blocks
/// are never re-rendered or re-transmitted. Render or limit failures leave
/// the previously placed blocks intact (fail closed).
///
/// The watcher sends whole answers, not deltas, so a fresh [`StreamSplitter`]
/// is used per document: its job is only to turn this one text into the
/// current block list (with any unterminated fence/`$$` at the end handled
/// the same way the stream and watch paths handle it). Placement identity
/// across documents lives in `viewer.planner`, which persists across calls;
/// that is what lets `apply_revision` recognize an unchanged prefix, a
/// changed tail, or a shorter answer between two whole-document sends.
fn render_and_place(viewer: &mut Viewer, text: &str) -> Result<(), String> {
    let mut splitter = StreamSplitter::new(viewer.limits);
    let revision = splitter
        .push(text.as_bytes())
        .and_then(|_| splitter.finish());
    let revision = match revision {
        Ok(revision) => revision,
        Err(error) => {
            eprintln!(
                "agent-viewer: renderer_rejected ({:?})",
                error.safe_record().code
            );
            return Ok(());
        }
    };

    // While follow is disengaged, new/changed blocks land outside the
    // reader's visible window (new content only ever appends at the bottom;
    // an edited block that happens to be inside the window is still covered
    // because `sync_visible_window` below re-emits the whole window, not
    // just what changed). Suppressing terminal writes here means
    // `apply_revision` still updates all placement state (ids, hashes, the
    // render cache, retained PNGs) exactly as it would streaming — only the
    // terminal bytes are skipped, so the pane is not disturbed by output the
    // reader cannot currently see. `sync_visible_window` afterward is what
    // actually reconciles the screen, and its cost is bounded by the window
    // size (AT-3-503), not by how much history changed.
    viewer
        .sink
        .set_suppress_writes(!viewer.viewport.following());
    if let Err(error) = native_stream::apply_revision(
        &revision,
        &viewer.options,
        &mut viewer.cache,
        &mut viewer.planner,
        &mut viewer.formula_errors,
        &mut viewer.sink,
    ) {
        viewer.sink.set_suppress_writes(false);
        eprintln!(
            "agent-viewer: render_failed ({:?})",
            error.safe_record().code
        );
        return Ok(());
    }
    viewer.sink.set_suppress_writes(false);
    // Keep the source text in step with `planner.blocks()` (same order and
    // length) so a later scroll-back restore can re-render any block whose
    // retained PNG was evicted. See the `block_sources` field doc.
    viewer.block_sources = revision.blocks;

    // Feed the new block heights into the viewport: while follow is engaged
    // the window re-pins to the bottom, matching what `apply_revision` just
    // streamed directly, so no extra sync is needed. While disengaged, the
    // offset is deliberately left as-is per AT-3-502, and the writes above
    // were suppressed, so the screen is still showing the reader's prior
    // window — `sync_visible_window` reconciles it against the (possibly
    // now-different) window contents using only cached PNGs.
    viewer.viewport.set_block_heights(block_heights(viewer));
    // AT-3-504: evict retained PNGs outside the window ± budget on every
    // append, regardless of follow state. While following, `sync_window` is
    // never called (`apply_revision` streams straight to the pane bottom,
    // and the viewport window is always the tail), so this is the only
    // place a mainline follow session ever trims history — without it, a
    // long streamed session would retain every block's PNG forever. The
    // disengaged path below also evicts inside `sync_window`; running both
    // is idempotent (eviction only ever empties an already-out-of-window
    // PNG), so no branching is needed here.
    let visible = viewer.viewport.visible_blocks();
    viewer
        .sink
        .evict_outside_window(visible.first..visible.last_exclusive);
    if !viewer.viewport.following() {
        if let Err(error) = sync_visible_window(viewer) {
            eprintln!("agent-viewer: {error}");
        }
    }
    viewer.blocks_placed = viewer.planner.blocks().len();
    let stats = viewer.cache.stats();
    eprintln!(
        "agent-viewer: placed blocks={} formula_errors={} cache_hits={} cache_misses={}",
        viewer.blocks_placed,
        viewer.formula_errors.iter().sum::<usize>(),
        stats.hits,
        stats.misses
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follow_disengages_on_scroll_and_reengages_on_end_or_shift_f() {
        let mut decoder = InputDecoder::new();
        decoder.push(b"\x1b[<65;10;20M");
        let wheel = decoder.next_event().expect("wheel event");

        // A tall viewport in rows relative to the content built below leaves
        // scroll_delta's clamp with room to move, so the assertions exercise
        // the follow flag transition rather than getting clamped to 0 either
        // way.
        let mut viewer = test_viewer(5);
        viewer.viewport.set_block_heights(vec![50]);
        assert!(viewer.viewport.following());
        handle_scroll_input(&mut viewer, &wheel);
        assert!(
            !viewer.viewport.following(),
            "manual scroll disengages follow"
        );

        let mut decoder = InputDecoder::new();
        decoder.push(b"\x1b[4~");
        let end = decoder.next_event().expect("end key");
        handle_scroll_input(&mut viewer, &end);
        assert!(viewer.viewport.following(), "End re-engages follow");

        handle_scroll_input(&mut viewer, &wheel);
        assert!(!viewer.viewport.following());
        let mut decoder = InputDecoder::new();
        decoder.push(b"F");
        let shift_f = decoder.next_event().expect("F key");
        handle_scroll_input(&mut viewer, &shift_f);
        assert!(viewer.viewport.following(), "F re-engages follow");
    }

    /// AT-3-502: while follow is engaged, an appended block keeps the
    /// viewport pinned to the bottom (the newest block stays visible).
    #[test]
    fn append_while_following_keeps_the_viewport_pinned_to_the_tail() {
        let mut viewer = test_viewer(2);
        render_and_place(&mut viewer, "One.\n\n").unwrap();
        assert!(viewer.viewport.following());
        assert_eq!(
            viewer.viewport.offset(),
            viewer.viewport.max_offset(),
            "follow keeps the window pinned to the bottom after the first block"
        );

        render_and_place(&mut viewer, "One.\n\nTwo.\n\nThree.\n\n").unwrap();
        assert!(
            viewer.viewport.following(),
            "appending does not disengage follow"
        );
        assert_eq!(
            viewer.viewport.offset(),
            viewer.viewport.max_offset(),
            "follow re-pins to the bottom as blocks are appended"
        );
    }

    /// AT-3-504's wiring, exercised through the real `render_and_place` call
    /// pattern rather than `evict_pngs_outside_budget` directly: with follow
    /// engaged the whole time (the mainline streamed session — `sync_window`
    /// is never reached on this path), appending many blocks one at a time
    /// must not fail or panic, `planner`/`block_sources` must keep the full
    /// history (eviction only drops PNG bytes, never block state), and
    /// follow must stay pinned to the tail throughout. `Summary` mode (no
    /// terminal) makes the actual PNG eviction unobservable here — that is
    /// covered by `native_stream`'s pure-function test — but this confirms
    /// `render_and_place`'s unconditional `evict_outside_window` call does
    /// not disturb anything else over a long append sequence.
    #[test]
    fn many_appends_with_follow_engaged_evict_without_disturbing_state() {
        let mut viewer = test_viewer(2);
        let mut text = String::new();
        for line in 0..200 {
            text.push_str(&format!("Block {line}.\n\n"));
            render_and_place(&mut viewer, &text).unwrap();
        }

        assert!(viewer.viewport.following(), "follow was never disengaged");
        assert_eq!(
            viewer.planner.blocks().len(),
            200,
            "the full block history is kept even though eviction ran on every append"
        );
        assert_eq!(
            viewer.block_sources.len(),
            200,
            "block sources stay in step with the planner's block list"
        );
        assert_eq!(
            viewer.viewport.offset(),
            viewer.viewport.max_offset(),
            "follow keeps the window pinned to the tail across the whole sequence"
        );
    }

    /// FIX (TMATH_DPR hotfix): `block_heights` (and therefore the viewport's
    /// row math) must use `viewer.cell` as the *physical* cell the image was
    /// actually rasterized at — never the raw measured logical cell a
    /// `TMATH_DPR` override was meant to correct. This is exactly the bug
    /// the fix closes: leaving `viewer.cell` at the stale logical cell after
    /// applying a dpr override makes `grid_for` divide by a cell half the
    /// real size, doubling every computed row (and column) count. Simulates
    /// the effect directly (constructing `run_agent_viewer`'s real terminal
    /// path is not hermetically testable) by building two otherwise
    /// identical viewers that differ only in `cell`, matching the
    /// pre-fix (logical) and post-fix (physical, `layout::terminal_fit_
    /// layout`'s `effective_cell_px`) values for the 7x15-logical /
    /// dpr-2-override case from `layout`'s own test.
    #[test]
    fn block_heights_uses_the_effective_physical_cell_not_the_logical_one() {
        // Small enough, relative to a rendered block's real pixel height, to
        // guarantee `div_ceil` actually produces different row counts for
        // the physical vs. logical cell rather than both rounding up to 1.
        let physical_cell = CellSize {
            width: 2,
            height: 4,
        };
        let logical_cell = CellSize {
            width: 1,
            height: 2,
        };

        let mut correct = test_viewer(24);
        correct.cell = physical_cell;
        render_and_place(&mut correct, "One block of prose here.\n\n").unwrap();
        let correct_heights = block_heights(&correct);

        let mut buggy = test_viewer(24);
        buggy.cell = logical_cell;
        render_and_place(&mut buggy, "One block of prose here.\n\n").unwrap();
        let buggy_heights = block_heights(&buggy);

        assert_eq!(correct_heights.len(), 1);
        assert_eq!(buggy_heights.len(), 1);
        // Both viewers rendered the exact same block, so its raw pixel
        // height is identical in both — `block_heights` must divide that
        // shared pixel height by each viewer's own `cell.height`, so a cell
        // half the size on the height axis must yield roughly double the
        // row count. Cross-check against `grid_for` directly (the function
        // `block_heights` is documented to delegate to) rather than
        // asserting a hardcoded number, since the exact pixel height
        // depends on the renderer.
        let rendered_height_px = correct.planner.blocks()[0].height_px;
        assert_eq!(rendered_height_px, buggy.planner.blocks()[0].height_px);
        let expected_correct = tmath_core::placement::grid_for(
            correct.planner.blocks()[0].width_px,
            rendered_height_px,
            physical_cell,
        )
        .1;
        let expected_buggy = tmath_core::placement::grid_for(
            buggy.planner.blocks()[0].width_px,
            rendered_height_px,
            logical_cell,
        )
        .1;
        assert_eq!(correct_heights[0], expected_correct);
        assert_eq!(buggy_heights[0], expected_buggy);
        assert!(
            buggy_heights[0] > correct_heights[0],
            "a stale logical cell inflates the computed row count: \
             correct={correct_heights:?} buggy={buggy_heights:?}"
        );
    }

    /// AT-3-601 happy path: a `Document` followed by an `Append` and a
    /// `ReplaceTail` each drive `render_and_place` through the normal
    /// block-diffing path — the delta protocol only changes how the input
    /// text is assembled, not what happens to it afterward.
    #[test]
    fn document_append_and_replace_tail_all_reach_render_and_place() {
        use tmath_core::agent::Message;

        let mut viewer = test_viewer(24);
        apply_incoming_message(
            &mut viewer,
            &Message::Document {
                text: "One.\n\n".to_string(),
            },
        )
        .unwrap();
        assert_eq!(viewer.planner.blocks().len(), 1);
        assert_eq!(viewer.delta.document(), "One.\n\n");

        apply_incoming_message(
            &mut viewer,
            &Message::Append {
                version: tmath_core::agent::DELTA_PROTOCOL_VERSION,
                seq: 1,
                text: "Two.\n\n".to_string(),
            },
        )
        .unwrap();
        assert_eq!(viewer.planner.blocks().len(), 2);
        assert_eq!(viewer.delta.document(), "One.\n\nTwo.\n\n");

        let keep_bytes = "One.\n\n".len();
        apply_incoming_message(
            &mut viewer,
            &Message::ReplaceTail {
                version: tmath_core::agent::DELTA_PROTOCOL_VERSION,
                seq: 2,
                keep_bytes,
                text: "Three.\n\n".to_string(),
            },
        )
        .unwrap();
        assert_eq!(viewer.delta.document(), "One.\n\nThree.\n\n");
        assert_eq!(viewer.planner.blocks().len(), 2);
    }

    /// AT-3-601 fail-closed: a rejected delta (here, an out-of-order
    /// sequence number) must not call `render_and_place` at all — the
    /// previously placed blocks and document text stay exactly as they were.
    #[test]
    fn a_rejected_delta_does_not_reach_render_and_place() {
        use tmath_core::agent::Message;

        let mut viewer = test_viewer(24);
        apply_incoming_message(
            &mut viewer,
            &Message::Document {
                text: "One.\n\n".to_string(),
            },
        )
        .unwrap();
        let blocks_before = viewer.planner.blocks().len();

        // seq=5 with nothing at seq=1..=4 first is out of order.
        apply_incoming_message(
            &mut viewer,
            &Message::Append {
                version: tmath_core::agent::DELTA_PROTOCOL_VERSION,
                seq: 5,
                text: "orphan".to_string(),
            },
        )
        .unwrap();

        assert_eq!(
            viewer.planner.blocks().len(),
            blocks_before,
            "the rejected delta never reached render_and_place"
        );
        assert_eq!(viewer.delta.document(), "One.\n\n");
    }

    /// AT-3-502: this test replaces the pre-T3-302 behavior where
    /// `render_and_place` unconditionally re-engaged follow after every
    /// document. Once a manual scroll has disengaged follow, an appended
    /// block must not silently re-engage it, and the scrolled-up offset must
    /// not jump to the new bottom.
    #[test]
    fn append_while_disengaged_does_not_reengage_follow_or_move_the_offset() {
        let mut viewer = test_viewer(2);
        render_and_place(&mut viewer, "One.\n\nTwo.\n\nThree.\n\n").unwrap();
        assert!(viewer.viewport.following());

        let mut decoder = InputDecoder::new();
        decoder.push(b"\x1b[<64;10;20M"); // wheel up
        let wheel_up = decoder.next_event().expect("wheel event");
        handle_scroll_input(&mut viewer, &wheel_up);
        assert!(
            !viewer.viewport.following(),
            "manual scroll disengages follow"
        );
        let offset_after_scroll = viewer.viewport.offset();

        render_and_place(&mut viewer, "One.\n\nTwo.\n\nThree.\n\nFour.\n\n").unwrap();
        assert!(
            !viewer.viewport.following(),
            "appending while disengaged must not re-engage follow"
        );
        assert_eq!(
            viewer.viewport.offset(),
            offset_after_scroll,
            "the offset measured from the top stays stable across an append while disengaged"
        );
    }

    /// AT-3-503: `render_and_place` suppresses terminal writes while follow
    /// is disengaged (visibility-gated emission), but placement *state* must
    /// still update exactly as if writes were not suppressed — the planner's
    /// block list, ids, and viewport heights all reflect the new document,
    /// so re-engaging follow (`End`) immediately shows the correct tail
    /// without needing another `render_and_place` call.
    #[test]
    fn append_while_disengaged_still_updates_state_for_the_next_sync() {
        let mut viewer = test_viewer(2);
        render_and_place(&mut viewer, "One.\n\nTwo.\n\n").unwrap();

        let mut decoder = InputDecoder::new();
        decoder.push(b"\x1b[<64;10;20M"); // wheel up
        let wheel_up = decoder.next_event().expect("wheel event");
        handle_scroll_input(&mut viewer, &wheel_up);
        assert!(!viewer.viewport.following());

        let rows_before = viewer.viewport.total_rows();
        render_and_place(&mut viewer, "One.\n\nTwo.\n\nThree.\n\nFour.\n\n").unwrap();
        assert_eq!(
            viewer.planner.blocks().len(),
            4,
            "the planner's block state reflects the new document even though \
             the writes for it were suppressed"
        );
        assert!(
            viewer.viewport.total_rows() > rows_before,
            "viewport heights grow from the updated planner state (two new \
             blocks), not skipped alongside the writes"
        );

        let mut decoder = InputDecoder::new();
        decoder.push(b"\x1b[4~");
        let end = decoder.next_event().expect("end key");
        handle_scroll_input(&mut viewer, &end);
        assert!(viewer.viewport.following(), "End re-engages follow");
        assert_eq!(
            viewer.viewport.offset(),
            viewer.viewport.max_offset(),
            "re-engaging follow jumps straight to the up-to-date tail"
        );
    }

    /// AT-3-504: `restore_png_for_id` re-renders a placed block's PNG from
    /// `block_sources` via the `RenderCache` (a cache hit here, since the
    /// block was already rendered once by the `render_and_place` call
    /// above) and returns bytes that decode to a valid, non-empty PNG.
    #[test]
    fn restore_png_for_id_re_renders_a_known_block_from_cache() {
        let mut viewer = test_viewer(24);
        render_and_place(&mut viewer, "One.\n\nTwo.\n\n").unwrap();
        let id = viewer.planner.blocks()[0].id;
        let hits_before = viewer.cache.stats().hits;

        let png = restore_png_for_id(&mut viewer, id).expect("restore succeeds for a known id");
        assert!(!png.is_empty(), "restored bytes are a real PNG");
        assert!(
            viewer.cache.stats().hits > hits_before,
            "the block was already rendered once, so this restore is a cache hit"
        );
    }

    /// An id that is not (or no longer) in the planner's block list fails
    /// closed: `restore_png_for_id` returns `None` rather than panicking or
    /// fabricating bytes.
    #[test]
    fn restore_png_for_id_returns_none_for_an_unknown_id() {
        let mut viewer = test_viewer(24);
        render_and_place(&mut viewer, "One.\n\n").unwrap();
        assert!(restore_png_for_id(&mut viewer, 999_999).is_none());
    }

    /// AT-3-504's render-fallback path: with a cache too small to hold both
    /// blocks, the second block's render evicts the first from
    /// `RenderCache`, so restoring the first block's id is a genuine
    /// re-render (a cache miss), not a hit — and it still produces valid
    /// bytes identical to the original render, from `block_sources` alone.
    #[test]
    fn restore_png_for_id_falls_back_to_a_real_rerender_on_a_cache_miss() {
        let mut viewer = test_viewer_with_cache_capacity(24, 1);
        render_and_place(&mut viewer, "One.\n\nTwo different enough to evict.\n\n").unwrap();
        let stats_after_render = viewer.cache.stats();
        assert_eq!(
            stats_after_render.entries, 1,
            "the 1-entry cache evicted the first block's render already"
        );

        let first_id = viewer.planner.blocks()[0].id;
        let misses_before = viewer.cache.stats().misses;
        let png = restore_png_for_id(&mut viewer, first_id)
            .expect("restore succeeds via a real re-render");
        assert!(!png.is_empty());
        assert!(
            viewer.cache.stats().misses > misses_before,
            "the first block's render was evicted, so restoring it is a cache miss \
             (a real re-render from block_sources), not a hit"
        );
    }

    fn test_viewer_with_cache_capacity(pane_rows: u32, max_entries: usize) -> Viewer {
        let mut viewer = test_viewer(pane_rows);
        viewer.cache = RenderCache::new(CacheBudget {
            max_entries,
            max_pixels: u64::MAX,
        });
        viewer
    }

    fn test_viewer(pane_rows: u32) -> Viewer {
        let limits = Limits::default();
        let options = RenderOptions::default();
        Viewer {
            stream: pair_socket(),
            input: InputDecoder::new(),
            messages: Decoder::new(),
            viewport: Viewport::new(pane_rows),
            cache: RenderCache::new(CacheBudget {
                max_entries: 16,
                max_pixels: u64::MAX,
            }),
            limits,
            planner: PlacementPlanner::new(),
            formula_errors: Vec::new(),
            options,
            sink: StreamSink::new(None, limits.image_pixels),
            blocks_placed: 0,
            cell: CellSize {
                width: 1,
                height: 1,
            },
            block_sources: Vec::new(),
            delta: DeltaState::new(IPC_MAX_REQUEST_BYTES),
        }
    }

    fn pair_socket() -> UnixStream {
        let (a, _b) = UnixStream::pair().expect("socket pair");
        a
    }

    /// AT-3-501-shaped: appending a block to a placed answer reuses the
    /// existing blocks' placement ids and allocates exactly one new id for
    /// the appended block.
    #[test]
    fn append_answer_reuses_prior_block_ids_and_allocates_one_new_id() {
        let mut viewer = test_viewer(24);
        render_and_place(&mut viewer, "First block.\n\n").unwrap();
        let first_ids: Vec<_> = viewer.planner.blocks().iter().map(|b| b.id).collect();
        assert_eq!(first_ids.len(), 1);

        render_and_place(&mut viewer, "First block.\n\nSecond block.\n\n").unwrap();
        let second_ids: Vec<_> = viewer.planner.blocks().iter().map(|b| b.id).collect();
        assert_eq!(second_ids.len(), 2);
        assert_eq!(
            second_ids[0], first_ids[0],
            "the unchanged first block keeps its placement id"
        );
        assert_ne!(
            second_ids[1], first_ids[0],
            "the appended block gets a fresh id"
        );
    }

    /// AT-3-505: a shorter replacement answer drops the trailing blocks that
    /// no longer exist rather than leaving their placements orphaned.
    #[test]
    fn shorter_replacement_answer_drops_stale_trailing_blocks() {
        let mut viewer = test_viewer(24);
        render_and_place(&mut viewer, "One.\n\nTwo.\n\nThree.\n\n").unwrap();
        assert_eq!(viewer.planner.blocks().len(), 3);

        render_and_place(&mut viewer, "One.\n\n").unwrap();
        assert_eq!(
            viewer.planner.blocks().len(),
            1,
            "stale trailing blocks are gone from the planner's placed state, \
             which is what drives the Remove ops for their placements"
        );
    }

    /// Re-sending the exact same document produces no new placement ids and
    /// no cache misses: the planner reports every block as `Keep` and the
    /// cache is not touched again for unchanged content.
    #[test]
    fn unchanged_resend_allocates_no_new_ids_and_touches_no_new_cache_entries() {
        let mut viewer = test_viewer(24);
        render_and_place(&mut viewer, "Stable answer.\n\n").unwrap();
        let ids_before: Vec<_> = viewer.planner.blocks().iter().map(|b| b.id).collect();
        let stats_before = viewer.cache.stats();

        render_and_place(&mut viewer, "Stable answer.\n\n").unwrap();
        let ids_after: Vec<_> = viewer.planner.blocks().iter().map(|b| b.id).collect();
        let stats_after = viewer.cache.stats();

        assert_eq!(ids_before, ids_after, "unchanged blocks keep their ids");
        assert_eq!(
            stats_before, stats_after,
            "an unchanged document does not touch the render cache again"
        );
    }

    /// Cache hit path: identical block content across two different answers
    /// (a growing prefix plus a second, distinct answer that repeats it)
    /// renders the shared content once and reuses it on the second answer.
    #[test]
    fn repeated_block_content_across_answers_is_a_cache_hit() {
        let mut viewer = test_viewer(24);
        render_and_place(&mut viewer, "Shared line.\n\n").unwrap();
        let after_first = viewer.cache.stats();
        assert_eq!(after_first.misses, 1);
        assert_eq!(after_first.hits, 0);

        // A distinct second answer whose first block repeats the exact same
        // source: the planner allocates a new id (position-scoped identity),
        // but the cache serves the cached render instead of re-rendering.
        render_and_place(&mut viewer, "Different opener.\n\nShared line.\n\n").unwrap();
        let after_second = viewer.cache.stats();
        assert!(
            after_second.hits > after_first.hits,
            "repeated block content is served from the cache: {after_second:?}"
        );
    }
}
