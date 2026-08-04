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
//! window change triggers a full redraw of the visible blocks from their
//! retained PNGs (see [`native_stream::StreamSink::redraw_window`]) — no
//! block is re-rendered on scroll. `q`/`Ctrl-C` close the viewer. Render
//! failures leave earlier placements intact (fail closed).
//!
//! Re-emitting only the placements whose visibility changed (bounded bytes
//! per scroll step, AT-3-503) and bounded history eviction with re-render on
//! scroll-back (AT-3-504) are out of scope here; both build on this
//! viewport.

use std::io::{self, Read as _};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use tmath_core::agent::{Decoder, Message};
use tmath_core::input::InputDecoder;
use tmath_core::placement::CellSize;
use tmath_core::scroll_driver::{is_exit_signal, scroll_delta};
use tmath_core::terminal::{StdioTty, Terminal};
use tmath_render::{
    CacheBudget, Limits, PlacementPlanner, RenderCache, RenderOptions, StreamSplitter,
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
    let cell = terminal
        .cell_size()
        .map_err(|error| format!("measure cell size: {error}"))?
        .ok_or("terminal reported no usable cell size")?;
    let cell = CellSize {
        width: cell.0,
        height: cell.1,
    };

    // Auto-fit content width, font size, and device pixel ratio to this
    // pane's measured geometry so the rendered images match the viewer
    // pane's width and the surrounding terminal text size (there is no CLI
    // override for the viewer, which always runs against a real terminal).
    let pane_size = terminal
        .size()
        .map_err(|error| format!("measure viewer pane size: {error}"))?;
    let fitted = crate::layout::terminal_fit_layout(cell.width, cell.height, pane_size.cols);
    let options = RenderOptions::new(
        fitted.content_width_pt,
        fitted.font_size_pt,
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
    .with_retained_pngs();

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
                            Ok(Message::Document { text }) => render_and_place(viewer, &text)?,
                            Err(_) => eprintln!("agent-viewer: malformed_message dropped"),
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
    // keep the previous placements intact); a redraw failure follows the same
    // contract rather than tearing down the viewer process over a scroll step.
    if moved {
        if let Err(error) = redraw_visible_window(viewer) {
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

/// Redraws the terminal from the viewport's current visible-block range.
fn redraw_visible_window(viewer: &mut Viewer) -> Result<(), String> {
    let visible = viewer.viewport.visible_blocks();
    if visible.is_empty() {
        return Ok(());
    }
    viewer
        .sink
        .redraw_window(visible.first..visible.last_exclusive)
        .map_err(|error| format!("redraw_failed ({:?})", error.safe_record().code))
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

    if let Err(error) = native_stream::apply_revision(
        &revision,
        &viewer.options,
        &mut viewer.cache,
        &mut viewer.planner,
        &mut viewer.formula_errors,
        &mut viewer.sink,
    ) {
        eprintln!(
            "agent-viewer: render_failed ({:?})",
            error.safe_record().code
        );
        return Ok(());
    }

    // `apply_revision` already streamed the append/replace/remove operations
    // it planned straight to the pane bottom (the same as stream mode),
    // which is correct while follow is engaged but would otherwise push new
    // content into the reader's scrolled-back window. Feeding the new block
    // heights into the viewport keeps the model in sync: while follow is
    // engaged the window re-pins to the bottom, matching what was just
    // streamed, so no extra redraw is needed. While disengaged, the offset
    // is deliberately left as-is per AT-3-502, so the pane now shows the
    // freshly appended block(s) at the bottom instead of the reader's
    // scrolled-back view — an immediate redraw restores the correct window
    // from the model. This emit-then-redraw double write is the T3-302
    // placeholder full-window cost; T3-303's visibility-gated emission
    // replaces it with re-emitting only what actually changed.
    viewer.viewport.set_block_heights(block_heights(viewer));
    if !viewer.viewport.following() {
        if let Err(error) = redraw_visible_window(viewer) {
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
