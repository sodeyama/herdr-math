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

use crate::native_stream::{self, EmitOutcome, StatusBarState, StreamSink};
use crate::viewer_viewport::{self, Viewport};

const CONNECT_RETRIES: u32 = 50;
const CONNECT_RETRY_MS: u64 = 100;
const POLL_TIMEOUT: Duration = Duration::from_millis(40);
const POLL_TIMEOUT_SECS: f32 = 0.04;

/// One wheel notch's momentum impulse, rows/second — calibrated so a
/// SINGLE isolated notch (no further input) travels exactly
/// `tmath_core::scroll_driver`'s existing `WHEEL_ROWS` (3.0) rows in total
/// before settling, matching today's discrete-jump distance while
/// EASING there via momentum's decay instead of teleporting. Derived from
/// the exact total-distance integral of an exponentially decaying velocity
/// (`total = v0 / -ln(DECAY_PER_SECOND)`, the same closed form
/// `tmath_core::momentum::Momentum::tick`'s per-tick displacement uses,
/// integrated to infinity rather than over one `dt`): solving
/// `3.0 = v0 / -ln(DECAY_PER_SECOND)` for `v0` gives this constant. A fast,
/// sustained wheel spin still accumulates well past 3 rows, since
/// `Momentum::add_impulse` sums same-tick notches and later ticks add more
/// before the earlier impulse has fully decayed — this constant only
/// calibrates the SINGLE-NOTCH case.
const WHEEL_MOMENTUM_ROWS_PER_SEC: f32 = 15.154_372;

/// How long stage 2's transient scrollbar stays visible after the most
/// recent scroll step, before `run_viewer_loop`'s tick clears it — the
/// coordinator's spec: "~1s auto-hide on the same tick."
const SCROLLBAR_AUTO_HIDE: Duration = Duration::from_secs(1);

/// Environment variable that re-enables the viewer's ongoing status/
/// diagnostic `eprintln!` output (off by default per the user's live
/// verdict: the viewer pane should show rendered content only). `tmath
/// agent` (the watcher) forwards this into the viewer pane's spawn command
/// the same way it already forwards `TMATH_DPR`/`TMATH_TMUX_TRANSPORT`, so
/// an evidence run can turn viewer logs back on without editing the
/// watcher's own environment.
const VIEWER_LOG_ENV_VAR: &str = "TMATH_VIEWER_LOG";

/// Resolves [`VIEWER_LOG_ENV_VAR`] once at startup: any non-empty value
/// enables logging (matching the simple truthy convention `TMATH_VIEWER_LOG=1`
/// documents), an unset or empty value keeps the default (silent). Never
/// errors — an unusual value just falls through to the default, the same
/// "never block startup on a malformed toggle" spirit as `TMATH_DPR`.
fn viewer_log_enabled() -> bool {
    parse_viewer_log_enabled(std::env::var(VIEWER_LOG_ENV_VAR).ok().as_deref())
}

/// The parsing rule behind [`viewer_log_enabled`], factored out so it is
/// testable without mutating the process environment (which would race
/// other tests running in the same binary).
fn parse_viewer_log_enabled(raw: Option<&str>) -> bool {
    raw.is_some_and(|value| !value.is_empty())
}

/// Environment variable naming a file to route [`VIEWER_LOG_ENV_VAR`]'s
/// diagnostic output to, INSTEAD of stderr (the live scroll-lab's
/// observation 3: `eprintln!` writes reach the same physical pty as the
/// viewer's own Kitty-graphics content stream, and since they go through
/// neither `terminal_output::write_operations` nor the DECSTBM region at
/// all — they are a completely separate, uncontrolled write path straight
/// to the process's stderr fd — a log line prints at the cursor's CURRENT
/// physical position and can scroll the pane exactly the way DECSTBM was
/// built to prevent for controlled content writes, silently corrupting the
/// window-managed pane's row invariants). Unset (the default): stderr,
/// unchanged from before this variable existed.
const VIEWER_LOG_FILE_ENV_VAR: &str = "TMATH_VIEWER_LOG_FILE";

/// Redirects the process's OWN stderr file descriptor to the file named by
/// [`VIEWER_LOG_FILE_ENV_VAR`], if set and openable — every existing
/// `eprintln!` call site in this module keeps writing to "stderr" from
/// Rust's point of view, unchanged, but the underlying fd now points at the
/// file instead of the pty, so log lines never reach the managed pane at
/// all. Best-effort: an unset variable, or a file that fails to open
/// (unwritable directory, permission error), leaves stderr exactly as it
/// was — logging failures must never block viewer startup, the same
/// "never block on a malformed toggle" posture `TMATH_DPR` and
/// [`viewer_log_enabled`] already have. Must be called BEFORE any
/// `eprintln!` in this module runs (i.e. as early as possible in
/// [`run_agent_viewer`]), or earlier log lines would still reach the pty.
///
/// Not covered by a unit test: it mutates the PROCESS'S OWN stderr file
/// descriptor (`rustix::stdio::dup2_stderr`), which would silently redirect
/// every other test's output (including panic messages) running in the same
/// test binary — an unacceptable side effect for a unit test to have,
/// unlike `viewer_log_enabled`'s env-var read (also process-global, but
/// read-only and already isolated into a pure, directly-testable parsing
/// rule in `parse_viewer_log_enabled`). There is no equivalent pure
/// sub-rule to extract here beyond "does the env var name a file that opens
/// successfully," which is exactly the two `let...else` lines below —
/// nothing more complex to isolate. Manual verification: set
/// `TMATH_VIEWER_LOG=1 TMATH_VIEWER_LOG_FILE=/path/to/file` and confirm
/// diagnostic lines land in the file, not the pane.
fn redirect_viewer_log_to_file_if_configured() {
    let Some(path) = std::env::var_os(VIEWER_LOG_FILE_ENV_VAR) else {
        return;
    };
    let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    else {
        return;
    };
    let _ = rustix::stdio::dup2_stderr(&file);
}

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
    /// Resolved once at startup from `TMATH_VIEWER_LOG` (see
    /// [`viewer_log_enabled`]): gates every ongoing status/diagnostic
    /// `eprintln!` in this module so the viewer pane shows rendered content
    /// only by default. A startup abort (no graphics support, a bad socket)
    /// is unaffected — those surface as an `Err` from `run_agent_viewer`
    /// that `main()` always prints, regardless of this flag.
    log_enabled: bool,
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
    /// Stage 2's velocity-based momentum engine (`tmath_core::momentum`),
    /// distinct from `viewport.offset()`'s own clamped position: `momentum`
    /// tracks ONLY the decaying velocity a wheel flick leaves behind, so
    /// per-tick displacement is computed once here and applied to
    /// `viewport` through `Viewport::scroll_by`, the same entry point
    /// discrete keyboard/wheel-notch scrolling already uses — momentum
    /// never bypasses the viewport's own clamping/follow-disengage rules,
    /// it only supplies a different SOURCE of scroll deltas. Rests at zero
    /// velocity while following (see `run_viewer_loop`'s tick handling).
    momentum: tmath_core::momentum::Momentum,
    /// The row delta pending from same-tick wheel events, coalesced into one
    /// impulse before being fed to `momentum` (see `run_viewer_loop`'s doc
    /// comment on why coalescing matters for a wheel that reports discrete
    /// notches, not continuous pixel deltas). Reset to `0.0` every tick
    /// after being applied; never carries over between ticks — a leftover
    /// nonzero value here would double-count an already-applied impulse on
    /// the next tick.
    pending_wheel_rows: f32,
    /// Sub-row fractional displacement left over from momentum ticks,
    /// carried forward so it is not silently lost to `Viewport::offset`'s
    /// `u32` rounding (`Viewport::scroll_by` re-rounds from the CURRENT
    /// stored integer offset every call, not a running float accumulator —
    /// correct for a single discrete jump, but a decaying momentum tail's
    /// per-tick deltas are frequently well under 1.0 row once decay has run
    /// for a while, so re-rounding from zero each tick would silently
    /// discard fractional motion for several ticks in a row before it
    /// finally crosses an integer boundary, reading as a stall rather than
    /// smooth deceleration). `apply_momentum_tick` adds each tick's exact
    /// float delta here, calls `scroll_by` only with `trunc()`'s
    /// integer-crossing portion, and keeps the leftover fraction for the
    /// next tick — cleared (not carried) on any cancel/jump, since a fresh
    /// jump has no meaningful "leftover fraction" to resume from.
    momentum_remainder: f32,
    /// Stage 2's transient scrollbar auto-hide deadline: `Some(deadline)`
    /// while the scrollbar should be visible (any scroll step sets this to
    /// `now + SCROLLBAR_AUTO_HIDE` — see that constant), `None` once it has
    /// been cleared. `run_viewer_loop`'s tick checks this every tick and
    /// clears the scrollbar exactly once when `Instant::now() >= deadline`,
    /// then sets this back to `None` so the clear is not repeated every
    /// tick thereafter. A wall-clock `Instant` here (not tick-counted) is
    /// deliberate — the auto-hide timer is real-world UX timing, not part
    /// of the pure, tick-injectable momentum physics
    /// (`tmath_core::momentum::Momentum`), which stays wall-clock-free for
    /// determinism; only this ancillary visibility timer touches the clock.
    scrollbar_visible_until: Option<Instant>,
}

pub(crate) fn run_agent_viewer(args: &[String]) -> Result<i32, String> {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("usage: tmath agent-viewer <socket-path>");
        return Ok(0);
    }
    // Must run before any `eprintln!` below (including the very first one),
    // or those earlier lines would still reach the pty. A no-op unless
    // `TMATH_VIEWER_LOG_FILE` is set — see its doc comment for why this
    // exists (observation 3 from the live scroll-lab run).
    redirect_viewer_log_to_file_if_configured();
    let log_enabled = viewer_log_enabled();
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
        if log_enabled {
            eprintln!(
                "agent-viewer: graphics route {}; require a visible Kitty-capable tmux client",
                route.label()
            );
        }
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
    if log_enabled && tmux_passthrough && tmath_dpr_env.is_some() && dpr_override.is_none() {
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
    if log_enabled {
        eprintln!(
            "agent-viewer: fitted cell_w_px={} cell_h_px={} dpr={} dpr_override={} content_width_pt={:.1} pane_cols={}",
            cell.width,
            cell.height,
            fitted.device_pixel_ratio,
            dpr_override.is_some(),
            fitted.content_width_pt,
            pane_size.cols
        );
    }
    // The agent-viewer has no CLI flag of its own (it is spawned by `tmath
    // agent`, not run directly), so its font size precedence is env > config
    // > auto-fit > default — reads the config file directly at startup, per
    // the config module's doc comment.
    let font_config = crate::config::config_path()
        .map(|path| crate::config::load(&path))
        .unwrap_or_default();
    let (font_size_pt, font_size_source) =
        crate::config::resolve_font_size_pt_with_source(None, &font_config, Some(fitted));
    if log_enabled {
        eprintln!(
            "agent-viewer: font_size source={} value={font_size_pt}",
            font_size_source.label()
        );
    }
    let options = RenderOptions::new(
        fitted.content_width_pt,
        font_size_pt,
        fitted.device_pixel_ratio,
    )
    .map_err(|_| "invalid agent-viewer render options".to_string())?
    .with_cjk_font(crate::config::resolve_cjk_font(&font_config));
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
    let mut sink = StreamSink::new(
        Some((terminal, (cell.width, cell.height))),
        scaled.image_pixels,
    )
    .with_retained_pngs()
    .with_retained_window_blocks(limits.retained_window_blocks)
    .with_status_bar(pane_size.cols, pane_size.rows);

    // Row 1 is the reserved live status bar (see the `status_bar` module
    // doc in `native_stream.rs`) — it is the SAME reserved row PART 2's
    // pane-edge top margin needed anyway, not an additional one, so the
    // viewport's visible-row budget shrinks by exactly one, never two.
    let content_pane_rows = viewport_pane_rows(pane_size.rows);
    let initial_status = StatusBarState {
        following: true,
        blocks: 0,
        font_size_pt,
    };
    if let Err(error) = sink.set_status(initial_status) {
        if log_enabled {
            eprintln!(
                "agent-viewer: status_bar_failed ({:?})",
                error.safe_record().code
            );
        }
    }

    let mut viewer = Viewer {
        stream,
        input: InputDecoder::new(),
        messages: Decoder::new(),
        viewport: Viewport::new(content_pane_rows),
        cache,
        limits,
        planner: PlacementPlanner::new(),
        formula_errors: Vec::new(),
        options,
        sink,
        blocks_placed: 0,
        log_enabled,
        cell,
        block_sources: Vec::new(),
        delta: DeltaState::new(IPC_MAX_REQUEST_BYTES),
        momentum: tmath_core::momentum::Momentum::new(),
        pending_wheel_rows: 0.0,
        momentum_remainder: 0.0,
        scrollbar_visible_until: None,
    };
    let _ = viewer.stream.set_nonblocking(true);
    if log_enabled {
        eprintln!("agent-viewer: connected; q/Ctrl-C closes");
    }

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
                    if viewer.log_enabled {
                        eprintln!("agent-viewer: watcher closed; finishing");
                    }
                    return Ok(0);
                }
                Ok(n) => {
                    viewer.messages.push(&chunk[..n]);
                    while let Some(message) = viewer.messages.next_message() {
                        match message {
                            Ok(Message::Quit) => return Ok(0),
                            Ok(message) => apply_incoming_message(viewer, &message)?,
                            Err(error) => {
                                if viewer.log_enabled {
                                    eprintln!(
                                        "agent-viewer: malformed_message dropped ({error:?})"
                                    );
                                }
                            }
                        }
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(format!("read socket: {error}")),
            }
        }

        // Terminal input: `q`/Ctrl-C close the viewer. `End`/`F` re-engage
        // follow and jump the viewport to the bottom; a wheel notch is
        // COALESCED into `pending_wheel_rows` rather than moving the
        // viewport immediately (stage 2: momentum takes over the actual
        // motion, applied once per tick below, so several notches arriving
        // in the same 40ms read are summed into one impulse instead of each
        // re-triggering its own decay curve — see `WHEEL_MOMENTUM_ROWS_PER_SEC`'s
        // doc comment). Every other scroll-shaped input (arrows, `j`/`k`,
        // `PgUp`/`PgDn`, `Home`) still jumps immediately and cancels any
        // in-flight momentum, matching a keyboard user's expectation of an
        // exact, non-eased jump.
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

        // Stage 2: one momentum tick per loop iteration, regardless of
        // whether new input arrived this tick — this is what lets a flick
        // keep moving the viewport across ticks with no further wheel
        // events, and what applies this tick's coalesced wheel impulse (if
        // any). A no-op (zero displacement, no sync) when momentum is
        // already settled and no wheel event arrived, so this costs nothing
        // on an idle/following viewer.
        apply_momentum_tick(viewer);

        // Stage 2's scrollbar auto-hide: check every tick, not just after a
        // scroll step, so the ~1s timer fires even once motion has fully
        // stopped and no further ticks would otherwise touch the scrollbar
        // state at all.
        if viewer
            .scrollbar_visible_until
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            hide_scrollbar(viewer);
        }

        let elapsed = start.elapsed();
        if elapsed < POLL_TIMEOUT {
            std::thread::sleep(POLL_TIMEOUT - elapsed);
        }
    }
}

/// Routes one decoded input event (AT-3-502): `End`/`F` re-engage follow,
/// jump to the bottom, and cancel any in-flight momentum. A wheel notch
/// (`ScrollUp`/`ScrollDown`) is coalesced into `pending_wheel_rows` for
/// `apply_momentum_tick` to pick up this tick — it does NOT move the
/// viewport or disengage follow itself; that happens once, from the
/// coalesced impulse, when the tick runs. Every other scroll-shaped key
/// (arrows, `j`/`k`, `PgUp`/`PgDn`, `Home`) still jumps the viewport
/// immediately and cancels momentum, matching a keyboard user's expectation
/// of an exact, non-eased jump rather than an eased flick.
///
/// Logs WHICH input kind disengaged follow (the live scroll-lab's
/// "spontaneous follow=false" observation asked for this): a
/// same-tick-coalesced wheel notch is not itself distinguishable from a
/// keyboard jump in `run_viewer_loop`'s aggregate `follow_changed` check, so
/// this function logs the specific event immediately, before coalescing,
/// rather than after the tick applies it.
fn handle_scroll_input(viewer: &mut Viewer, event: &tmath_core::input::Event) {
    use tmath_core::input::{Event, KeyEvent};
    use tmath_core::mouse::{Key, MouseKind};

    if let Event::Key(KeyEvent {
        key: Key::End,
        ctrl: false,
        ..
    })
    | Event::Key(KeyEvent {
        key: Key::Char('F'),
        ctrl: false,
        ..
    }) = event
    {
        let following_before = viewer.viewport.following();
        let before = viewer.viewport.offset();
        viewer.viewport.jump_to_bottom_and_follow();
        viewer.momentum.cancel();
        viewer.pending_wheel_rows = 0.0;
        viewer.momentum_remainder = 0.0;
        let moved = viewer.viewport.offset() != before;
        let follow_changed = viewer.viewport.following() != following_before;
        log_follow_change(viewer, follow_changed, event);
        finish_scroll_step(viewer, moved, follow_changed);
        return;
    }

    if let Event::Mouse(mouse) = event {
        match mouse.kind {
            MouseKind::ScrollUp | MouseKind::ScrollDown => {
                let following_before = viewer.viewport.following();
                viewer.pending_wheel_rows += if mouse.kind == MouseKind::ScrollUp {
                    -1.0
                } else {
                    1.0
                };
                // `scroll_by(0.0)` clamps the CURRENT offset against the
                // CURRENT max and re-derives follow from where that lands
                // (see `Viewport::scroll_by`'s doc comment): while already
                // at the bottom this is a genuine no-op (follow stays
                // engaged, matching the coordinator's fix — a wheel-down
                // notch at the tail must never disengage follow), and while
                // scrolled up it correctly keeps follow disengaged. The
                // actual offset MOVE (if any) still happens on the next
                // tick via the coalesced impulse; this call exists only to
                // react to a follow-state change immediately, so the status
                // bar and diagnostic log update on the same input rather
                // than one tick later.
                let moved = viewer.viewport.scroll_by(0.0);
                let follow_changed = viewer.viewport.following() != following_before;
                log_follow_change(viewer, follow_changed, event);
                finish_scroll_step(viewer, moved, follow_changed);
            }
            // Move/Down/Up/ScrollLeft/ScrollRight never drive scrolling —
            // `scroll_delta` already excludes them, but matching explicitly
            // here (rather than falling through to the generic branch
            // below) keeps the coordinator's specific ask visible in the
            // code: these kinds must never disengage follow, and this
            // early return proves it structurally rather than by relying on
            // `scroll_delta`'s behavior alone.
            _ => {}
        }
        return;
    }

    let following_before = viewer.viewport.following();
    let moved = match scroll_delta(event, None) {
        Some(delta) => {
            viewer.momentum.cancel();
            viewer.pending_wheel_rows = 0.0;
            viewer.momentum_remainder = 0.0;
            viewer.viewport.scroll_by(delta)
        }
        None => false,
    };
    let follow_changed = viewer.viewport.following() != following_before;
    log_follow_change(viewer, follow_changed, event);
    finish_scroll_step(viewer, moved, follow_changed);
}

/// Applies one momentum tick (stage 2): folds this tick's coalesced wheel
/// impulse (if any) into `momentum`, advances momentum by one tick
/// (`POLL_TIMEOUT_SECS`, matching the real loop interval — see
/// `tmath_core::momentum::Momentum::tick`'s doc comment for why this stays
/// correct even if the loop's actual elapsed time drifts slightly from
/// exactly 40ms; a small fixed-step assumption here, not a live wall-clock
/// read, keeps the physics itself pure and testable, while the coalescing
/// input above still reacts to real events every real tick), and applies
/// the resulting row delta to the viewport through the same
/// `Viewport::scroll_by` entry point discrete input already uses.
///
/// Accumulates each tick's exact float delta into `momentum_remainder`
/// before calling `scroll_by` with only the integer-crossing portion
/// (`trunc()`), keeping the leftover fraction for the next tick — see
/// `Viewer::momentum_remainder`'s field doc for why this matters: a decaying
/// momentum tail frequently produces sub-1.0-row deltas per tick, and
/// `Viewport::scroll_by`'s own `u32` rounding would otherwise silently
/// discard several ticks' worth of fractional motion before it finally
/// crosses an integer boundary, reading as a stall rather than a smooth
/// decelerating scroll.
///
/// A no-op while following: momentum only ever accumulates from wheel
/// input, which itself disengages follow the instant a notch actually moves
/// the window off the bottom (see `handle_scroll_input` and
/// `Viewport::scroll_by`'s re-pin-at-bottom rule), so momentum is never fed
/// while following in practice — this only defends against a future caller
/// feeding it while following.
///
/// Momentum CAN, however, carry the window back onto the bottom mid-decay
/// (scrolling down toward the tail while a flick is still running) — this
/// is a real follow transition, unlike a jump: `Viewport::scroll_by` now
/// re-engages follow the instant `scroll_by`'s result lands on
/// `max_offset()` (the coordinator's fix), so a momentum tick must react to
/// it exactly like any other follow-changing step: cancel the now-stale
/// momentum and pending impulse (continuing to "coast" past the bottom
/// under decaying velocity would fight the re-pin), hide the scrollbar
/// immediately (a thumb position is meaningless while following, same as
/// the End/F path), and log the transition with its own distinct cause so
/// it reads differently from an explicit End/F/keyboard jump in the log.
fn apply_momentum_tick(viewer: &mut Viewer) {
    if viewer.pending_wheel_rows != 0.0 {
        viewer
            .momentum
            .add_impulse(viewer.pending_wheel_rows * WHEEL_MOMENTUM_ROWS_PER_SEC);
        viewer.pending_wheel_rows = 0.0;
    }
    if viewer.momentum.settled() {
        return;
    }
    let delta = viewer.momentum.tick(POLL_TIMEOUT_SECS);
    viewer.momentum_remainder += delta;
    let whole_rows = viewer.momentum_remainder.trunc();
    viewer.momentum_remainder -= whole_rows;
    if whole_rows == 0.0 {
        return;
    }
    // Capture the visible range BEFORE the offset moves, so a successful
    // incremental step (below) can compute exactly which block ids are
    // newly entering the top edge — `finish_scroll_step`'s other call
    // sites (jumps, follow toggles) don't need this, since a jump's window
    // change is never the narrow "N blocks entered at the top, nothing
    // else changed" shape `try_scroll_window_incrementally` handles.
    let visible_before = viewer.viewport.visible_blocks();
    let following_before = viewer.viewport.following();
    let moved = viewer.viewport.scroll_by(whole_rows);
    if viewer.viewport.following() && !following_before {
        // Momentum carried the window back onto the bottom: this IS a real
        // follow transition (re-pin), not the "always false" case the rest
        // of this function's per-tick machinery assumes. Stop the decay
        // outright rather than let it keep nudging a now-re-pinned window,
        // and reconcile everything a discrete re-engage (End/F) already
        // resets.
        viewer.momentum.cancel();
        viewer.pending_wheel_rows = 0.0;
        viewer.momentum_remainder = 0.0;
        log_follow_change_with_cause(viewer, true, "momentum-bottom");
        finish_scroll_step(viewer, moved, true);
        return;
    }
    // Momentum did not just re-engage follow (the common per-tick case:
    // still decaying, still disengaged), so `scroll_by` here re-sets an
    // already-false `follow` to itself — never a real transition — hence no
    // `follow_changed` tracking or logging on this path.
    if moved {
        finish_momentum_step(viewer, visible_before);
    }
}

/// The momentum-tick-specific tail of a scroll step (see
/// `finish_scroll_step`'s doc comment for the shared fail-closed/status-bar
/// contract every scroll path follows — this covers the same ground but
/// tries stage 2's incremental region-scroll first): compares the viewport's
/// visible range before and after this tick's offset change. When the
/// change is EXACTLY "some number of blocks newly entered at the top edge,
/// `skip_rows_in_first` unchanged, nothing left the bottom edge" (a
/// backward/scroll-back step, the common shape while decelerating toward
/// older content), it computes the entering ids and tries
/// `TerminalSink::try_scroll_window_incrementally`. Any other shape (a
/// forward step, a `skip_rows_in_first` change mid-block, an empty visible
/// range) falls back to the full `sync_visible_window`, which remains
/// correct for it exactly as before stage 2. Momentum never toggles follow
/// (see `apply_momentum_tick`'s doc comment), so there is no
/// `follow_changed` parameter here.
fn finish_momentum_step(viewer: &mut Viewer, visible_before: viewer_viewport::VisibleRange) {
    let visible_after = viewer.viewport.visible_blocks();
    if let Some(entering_range) = incremental_entering_range(visible_before, visible_after) {
        let entering_ids: Vec<u64> = viewer.planner.blocks()[entering_range]
            .iter()
            .map(|block| block.id)
            .collect();
        restore_missing_pngs(
            viewer,
            visible_after.first..visible_before.first.max(visible_after.first),
        );
        let range = visible_after.first..visible_after.last_exclusive;
        match viewer
            .sink
            .try_scroll_window_incrementally(&entering_ids, range)
        {
            Ok(true) => {
                // The incremental region-scroll path never touches
                // `clear_rows` (see `scroll_region`'s module doc), so the
                // scrollbar column is untouched here — but still redraw it
                // unconditionally: the thumb POSITION changed even though
                // its column bytes did not, and this also refreshes the
                // auto-hide deadline for this step.
                show_scrollbar(viewer);
                return;
            }
            Ok(false) => {}
            Err(error) => {
                if viewer.log_enabled {
                    eprintln!(
                        "agent-viewer: incremental_scroll_failed ({:?})",
                        error.safe_record().code
                    );
                }
            }
        }
    }
    if let Err(error) = sync_visible_window(viewer) {
        if viewer.log_enabled {
            eprintln!("agent-viewer: {error}");
        }
    }
    // The fallback path's `sync_window` uses `clear_rows` internally, which
    // can touch the scrollbar's column (see `scroll_region`'s module doc's
    // "Full-row-clear interaction" note) — redraw unconditionally rather
    // than trying to prove which specific batches were "safe."
    show_scrollbar(viewer);
}

/// Pure shape-detection for stage 2's incremental scroll-back path
/// (`finish_momentum_step`): returns the block-index range that newly
/// entered the visible window's TOP edge (`visible_after.first
/// ..visible_before.first`) when — and only when — the window change is
/// EXACTLY "some blocks entered at the top, `skip_rows_in_first` unchanged,
/// nothing left the bottom edge" (a pure backward/scroll-back step, the
/// common shape while momentum decelerates toward older content). Returns
/// `None` for every other shape (a forward step, any `skip_rows_in_first`
/// change, an empty new window, or a same-window no-op with nothing newly
/// entering) — the caller must fall back to a full `sync_window` for those,
/// which stays correct for every shape this function rejects.
///
/// Kept separate from `finish_momentum_step` (which also needs a live
/// `Viewer` to fetch retained PNGs and call `TerminalSink`) specifically so
/// this shape-matching logic is directly unit-testable without a terminal.
fn incremental_entering_range(
    visible_before: viewer_viewport::VisibleRange,
    visible_after: viewer_viewport::VisibleRange,
) -> Option<std::ops::Range<usize>> {
    let is_pure_backward_step = visible_after.skip_rows_in_first
        == visible_before.skip_rows_in_first
        && visible_after.first <= visible_before.first
        && visible_after.last_exclusive <= visible_before.last_exclusive
        && !visible_after.is_empty();
    if !is_pure_backward_step {
        return None;
    }
    let entering_range = visible_after.first..visible_before.first;
    if entering_range.is_empty() {
        None
    } else {
        Some(entering_range)
    }
}

/// Logs a follow-state transition with the SPECIFIC input kind that caused
/// it (the live scroll-lab's ask: a stray SGR event or an unexpected key
/// must be diagnosable from the log line itself, not just "follow=false"
/// with no context on why). `follow_changed` is computed by the caller
/// (which already has `following_before` in scope for its own control
/// flow), so this function only formats and gates on `log_enabled`.
fn log_follow_change(viewer: &Viewer, follow_changed: bool, event: &tmath_core::input::Event) {
    log_follow_change_with_cause(viewer, follow_changed, describe_input_kind(event));
}

/// The cause-string form of [`log_follow_change`], for a follow transition
/// that was not driven by one specific decoded input event — currently only
/// `apply_momentum_tick`'s "momentum carried the window back onto the
/// bottom" re-engage (`cause=momentum-bottom`), which has no single `Event`
/// to describe: it is the cumulative effect of several ticks' decay, not
/// one keystroke or wheel notch.
fn log_follow_change_with_cause(viewer: &Viewer, follow_changed: bool, cause: &str) {
    if !viewer.log_enabled || !follow_changed {
        return;
    }
    eprintln!(
        "agent-viewer: follow={} cause={cause}",
        viewer.viewport.following()
    );
}

/// A short, stable, content-free label for a decoded input event, safe to
/// log per AGENTS.md's privacy invariants (event/status names and bounded
/// enums only — never raw bytes or mouse coordinates).
fn describe_input_kind(event: &tmath_core::input::Event) -> &'static str {
    use tmath_core::input::{Event, KeyEvent};
    use tmath_core::mouse::{Key, MouseKind};

    match event {
        Event::Mouse(mouse) => match mouse.kind {
            MouseKind::ScrollUp => "wheel-up",
            MouseKind::ScrollDown => "wheel-down",
            MouseKind::ScrollLeft => "wheel-left",
            MouseKind::ScrollRight => "wheel-right",
            MouseKind::Move => "mouse-move",
            MouseKind::Down => "mouse-down",
            MouseKind::Up => "mouse-up",
        },
        Event::Key(KeyEvent { key, .. }) => match key {
            Key::Up => "key-up",
            Key::Down => "key-down",
            Key::PageUp => "key-page-up",
            Key::PageDown => "key-page-down",
            Key::Home => "key-home",
            Key::End => "key-end",
            Key::Char('j') => "key-j",
            Key::Char('k') => "key-k",
            Key::Char('g') => "key-g",
            Key::Char('G') => "key-G",
            Key::Char('F') => "key-F",
            Key::Char(_) => "key-other",
            _ => "key-other",
        },
        Event::Paste(_) => "paste",
        Event::Focus(_) => "focus",
    }
}

/// The tail shared by every scroll-input path (AT-3-502): render/limit
/// failures elsewhere are fail-closed (log and keep previous placements
/// intact), so a sync failure here follows the same contract rather than
/// tearing down the viewer process over a scroll step. Runs
/// `sync_visible_window` when `moved`, and redraws the status bar's
/// following/scrolled word when `follow_changed` — `sync_visible_window`
/// only redraws content, never row 1, so a follow transition that did not
/// otherwise move the window's content (e.g. already clamped at an edge)
/// still needs its own explicit redraw here. Also updates stage 2's
/// scrollbar: shown (and its auto-hide deadline reset) on any `moved` step
/// while disengaged, hidden immediately on re-engaging follow (End/F) —
/// a thumb position is not a meaningful thing to show while pinned to the
/// tail, so this does not wait for the auto-hide timer in that case.
fn finish_scroll_step(viewer: &mut Viewer, moved: bool, follow_changed: bool) {
    if moved {
        if let Err(error) = sync_visible_window(viewer) {
            if viewer.log_enabled {
                eprintln!("agent-viewer: {error}");
            }
        }
    }
    if follow_changed {
        redraw_status_bar(viewer);
    }
    if follow_changed && viewer.viewport.following() {
        hide_scrollbar(viewer);
    } else if moved {
        show_scrollbar(viewer);
    }
}

/// Draws the scrollbar at the viewport's current thumb position and resets
/// the auto-hide deadline to `now + SCROLLBAR_AUTO_HIDE` — called whenever
/// a scroll step actually moved the offset while follow is disengaged.
/// Failures are logged, never fatal, matching every other UI-chrome redraw
/// in this module (the scrollbar is not placed content).
fn show_scrollbar(viewer: &mut Viewer) {
    let thumb = viewer.viewport.scrollbar_thumb_rows();
    if let Err(error) = viewer.sink.set_scrollbar(thumb) {
        if viewer.log_enabled {
            eprintln!(
                "agent-viewer: scrollbar_failed ({:?})",
                error.safe_record().code
            );
        }
    }
    viewer.scrollbar_visible_until = Some(Instant::now() + SCROLLBAR_AUTO_HIDE);
}

/// Clears the scrollbar immediately and cancels the auto-hide deadline (so
/// `run_viewer_loop`'s tick does not also try to clear an already-cleared
/// scrollbar). Called on re-engaging follow and by the tick loop once the
/// auto-hide deadline passes.
fn hide_scrollbar(viewer: &mut Viewer) {
    if viewer.scrollbar_visible_until.is_none() {
        return;
    }
    if let Err(error) = viewer.sink.clear_scrollbar() {
        if viewer.log_enabled {
            eprintln!(
                "agent-viewer: scrollbar_failed ({:?})",
                error.safe_record().code
            );
        }
    }
    viewer.scrollbar_visible_until = None;
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
            if viewer.log_enabled {
                eprintln!("agent-viewer: delta_rejected ({error:?})");
            }
            Ok(())
        }
    }
}

/// The viewport's visible-row budget for a pane of `pane_rows` total rows:
/// `pane_rows - 1`, reserving exactly row 1 for the live status bar (see
/// the `status_bar` module doc in `native_stream.rs`) — this is the SAME
/// reserved row PART 2's pane-edge top margin needed anyway, not a second
/// one, so the budget shrinks by exactly one regardless of pane size.
/// `saturating_sub` keeps a degenerate 0-row pane at 0 rather than
/// underflowing.
fn viewport_pane_rows(pane_rows: u32) -> u32 {
    pane_rows.saturating_sub(1)
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
        .sync_window(range, visible.skip_rows_in_first)
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
            if viewer.log_enabled {
                eprintln!(
                    "agent-viewer: restore_failed ({:?}) id={id}",
                    error.safe_record().code
                );
            }
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
        if viewer.log_enabled {
            eprintln!("agent-viewer: restore_failed (missing_source) id={id}");
        }
        return None;
    };
    let rendered = match viewer.cache.render(source, &viewer.options) {
        Ok(rendered) => rendered,
        Err(error) => {
            if viewer.log_enabled {
                eprintln!(
                    "agent-viewer: restore_failed ({:?}) id={id}",
                    error.safe_record().code
                );
            }
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
            if viewer.log_enabled {
                eprintln!(
                    "agent-viewer: restore_failed ({:?}) id={id}",
                    error.safe_record().code
                );
            }
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
            if viewer.log_enabled {
                eprintln!(
                    "agent-viewer: renderer_rejected ({:?})",
                    error.safe_record().code
                );
            }
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
    let outcome = match native_stream::apply_revision(
        &revision,
        &viewer.options,
        &mut viewer.cache,
        &mut viewer.planner,
        &mut viewer.formula_errors,
        &mut viewer.sink,
    ) {
        Ok(outcome) => outcome,
        Err(error) => {
            viewer.sink.set_suppress_writes(false);
            if viewer.log_enabled {
                eprintln!(
                    "agent-viewer: render_failed ({:?})",
                    error.safe_record().code
                );
            }
            return Ok(());
        }
    };
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
    // `NeedsWindowSync` (the clamp-aware fallback, PART 1) always needs a
    // full window resync regardless of follow state: `apply_revision`
    // wrote NOTHING to the terminal for that revision (the relative
    // cursor-up would have clamped and ghosted), so even a following
    // session's screen is now stale and must be redrawn from an absolute
    // `\x1b[H`, exactly like the disengaged-follow path already does.
    if !viewer.viewport.following() || outcome == EmitOutcome::NeedsWindowSync {
        if let Err(error) = sync_visible_window(viewer) {
            if viewer.log_enabled {
                eprintln!("agent-viewer: {error}");
            }
        }
    }
    viewer.blocks_placed = viewer.planner.blocks().len();
    // Redraw the status bar's block count every time it can have changed —
    // `sync_visible_window` above already redraws content, but never row 1
    // itself, so the status bar needs its own explicit refresh here
    // regardless of which content path just ran.
    redraw_status_bar(viewer);
    if viewer.log_enabled {
        let stats = viewer.cache.stats();
        eprintln!(
            "agent-viewer: placed blocks={} formula_errors={} cache_hits={} cache_misses={}",
            viewer.blocks_placed,
            viewer.formula_errors.iter().sum::<usize>(),
            stats.hits,
            stats.misses
        );
    }
    Ok(())
}

/// Redraws the live status bar (row 1) from the viewer's current state —
/// the single call site every status-changing event (a new revision's block
/// count, a follow-state transition, a window sync) routes through, so
/// there is exactly one place that assembles a [`StatusBarState`] from
/// live viewer fields. Failures are logged, never fatal — the status bar is
/// UI chrome, not placed content, so a failed redraw must not disturb
/// anything else `render_and_place`/`handle_scroll_input` already did.
fn redraw_status_bar(viewer: &mut Viewer) {
    let state = StatusBarState {
        following: viewer.viewport.following(),
        blocks: viewer.planner.blocks().len(),
        font_size_pt: viewer.options.font_size_pt,
    };
    if let Err(error) = viewer.sink.set_status(state) {
        if viewer.log_enabled {
            eprintln!(
                "agent-viewer: status_bar_failed ({:?})",
                error.safe_record().code
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewer_log_disabled_by_default() {
        assert!(!parse_viewer_log_enabled(None), "unset stays silent");
        assert!(!parse_viewer_log_enabled(Some("")), "empty stays silent");
    }

    #[test]
    fn viewer_log_enabled_by_any_non_empty_value() {
        assert!(parse_viewer_log_enabled(Some("1")));
        assert!(parse_viewer_log_enabled(Some("true")));
        assert!(
            parse_viewer_log_enabled(Some("0")),
            "any non-empty value enables it, including a literal '0' \
             (this is a simple on/off toggle, not a boolean parse)"
        );
    }

    /// PART 2: the viewport's visible-row budget must be exactly
    /// `pane_rows - 1` — the status bar's reserved row 1 IS the top margin,
    /// not an additional reservation, so this must never subtract 2.
    #[test]
    fn viewport_pane_rows_reserves_exactly_the_status_bar_row() {
        assert_eq!(viewport_pane_rows(24), 23);
        assert_eq!(
            viewport_pane_rows(1),
            0,
            "a 1-row pane leaves no content rows"
        );
        assert_eq!(
            viewport_pane_rows(0),
            0,
            "a degenerate 0-row pane must not underflow"
        );
    }

    #[test]
    fn follow_disengages_on_scroll_and_reengages_on_end_or_shift_f() {
        // Button 64 = scroll UP (backward) — the notch that actually moves
        // the window OFF the bottom, so it is the one that must disengage
        // follow. Button 65 (scroll DOWN) at the tail is exactly the
        // coordinator's fix case: it must NOT disengage follow, since the
        // offset never leaves the bottom — covered by its own dedicated
        // test (`wheel_down_at_the_tail_never_disengages_follow`) rather
        // than here, to keep this test's scroll-then-reengage narrative
        // using the notch that genuinely moves.
        let mut decoder = InputDecoder::new();
        decoder.push(b"\x1b[<64;10;20M");
        let wheel_up = decoder.next_event().expect("wheel event");

        // A tall viewport in rows relative to the content built below leaves
        // scroll_delta's clamp with room to move, so the assertions exercise
        // the follow flag transition rather than getting clamped to 0 either
        // way.
        let mut viewer = test_viewer(5);
        viewer.viewport.set_block_heights(vec![50]);
        assert!(viewer.viewport.following());
        handle_scroll_input(&mut viewer, &wheel_up);
        for _ in 0..5 {
            apply_momentum_tick(&mut viewer); // let the coalesced impulse actually move the offset
        }
        assert!(
            !viewer.viewport.following(),
            "manual scroll off the bottom disengages follow"
        );

        let mut decoder = InputDecoder::new();
        decoder.push(b"\x1b[4~");
        let end = decoder.next_event().expect("end key");
        handle_scroll_input(&mut viewer, &end);
        assert!(viewer.viewport.following(), "End re-engages follow");

        handle_scroll_input(&mut viewer, &wheel_up);
        for _ in 0..5 {
            apply_momentum_tick(&mut viewer);
        }
        assert!(!viewer.viewport.following());
        let mut decoder = InputDecoder::new();
        decoder.push(b"F");
        let shift_f = decoder.next_event().expect("F key");
        handle_scroll_input(&mut viewer, &shift_f);
        assert!(viewer.viewport.following(), "F re-engages follow");
    }

    /// The coordinator's fix, case (a), through the exact SGR byte path a
    /// live pointer hover would produce: a scroll-DOWN notch while already
    /// following at the tail must never disengage follow or flip the
    /// status word — this was the "spontaneous follow=false" mechanism.
    #[test]
    fn wheel_down_at_the_tail_never_disengages_follow() {
        let mut decoder = InputDecoder::new();
        decoder.push(b"\x1b[<65;10;20M");
        let wheel_down = decoder.next_event().expect("wheel event");

        let mut viewer = test_viewer(5);
        viewer.viewport.set_block_heights(vec![50]);
        assert!(viewer.viewport.following());
        handle_scroll_input(&mut viewer, &wheel_down);
        assert!(
            viewer.viewport.following(),
            "a down-notch at the tail must not disengage follow"
        );
        for _ in 0..5 {
            apply_momentum_tick(&mut viewer);
        }
        assert!(
            viewer.viewport.following(),
            "still following after the tick — there was never anywhere to move"
        );
    }

    fn wheel_event(down: bool) -> tmath_core::input::Event {
        // SGR mouse report button codes: 65 = scroll down, 64 = scroll up.
        let bytes: &[u8] = if down {
            b"\x1b[<65;10;20M"
        } else {
            b"\x1b[<64;10;20M"
        };
        let mut decoder = InputDecoder::new();
        decoder.push(bytes);
        decoder.next_event().expect("wheel event")
    }

    /// Stage 2: a wheel notch disengages follow immediately but does NOT
    /// move the viewport offset itself — the actual displacement is
    /// deferred to `apply_momentum_tick`, coalesced from
    /// `pending_wheel_rows`. This is the behavior change from the
    /// pre-momentum immediate-jump `scroll_by(delta)` call.
    #[test]
    fn a_wheel_notch_disengages_follow_but_defers_movement_to_the_tick() {
        let mut viewer = test_viewer(5);
        viewer.viewport.set_block_heights(vec![50]);
        let before_offset = viewer.viewport.offset();
        // An UP notch (backward, away from the tail) — the notch that
        // ACTUALLY moves the window once its tick applies, unlike a
        // DOWN notch at the tail (covered by its own dedicated test,
        // `wheel_down_at_the_tail_never_disengages_follow`, which must
        // never disengage at all).
        handle_scroll_input(&mut viewer, &wheel_event(false));
        // `Viewport::scroll_by`'s re-pin-at-bottom rule (the coordinator's
        // fix) means `handle_scroll_input`'s immediate `scroll_by(0.0)`
        // reaction does NOT yet disengage follow here: the offset has not
        // actually moved off the bottom yet (that happens on the next
        // momentum tick), so follow correctly stays engaged until it does.
        assert!(
            viewer.viewport.following(),
            "follow must not disengage before the offset actually moves"
        );
        assert_eq!(
            viewer.viewport.offset(),
            before_offset,
            "the offset must not move until the momentum tick runs"
        );
        assert_ne!(
            viewer.pending_wheel_rows, 0.0,
            "the notch must be coalesced into the pending impulse"
        );

        // Once the tick actually applies the impulse and the offset moves
        // off the bottom, follow disengages.
        for _ in 0..5 {
            apply_momentum_tick(&mut viewer);
        }
        assert!(
            !viewer.viewport.following(),
            "follow disengages once the offset has genuinely left the bottom"
        );
    }

    /// Several wheel notches arriving before the tick runs must coalesce
    /// into ONE accumulated impulse, not each independently move the
    /// viewport (AT the coalescing claim itself, not just "eventually
    /// moves").
    #[test]
    fn multiple_same_tick_wheel_notches_coalesce_into_one_pending_impulse() {
        let mut viewer = test_viewer(5);
        viewer.viewport.set_block_heights(vec![50]); // follow pins offset to max_offset (45)
        let start_offset = viewer.viewport.offset();
        handle_scroll_input(&mut viewer, &wheel_event(true));
        handle_scroll_input(&mut viewer, &wheel_event(true));
        handle_scroll_input(&mut viewer, &wheel_event(true));
        assert_eq!(
            viewer.pending_wheel_rows, 3.0,
            "three same-direction notches sum before the tick applies them"
        );
        assert_eq!(
            viewer.viewport.offset(),
            start_offset,
            "still nothing applied to the viewport before the tick"
        );
    }

    /// Opposite-direction notches in the same tick must cancel rather than
    /// each independently move the viewport in its own direction.
    #[test]
    fn opposite_direction_notches_in_the_same_tick_cancel() {
        let mut viewer = test_viewer(5);
        viewer.viewport.set_block_heights(vec![50]);
        handle_scroll_input(&mut viewer, &wheel_event(true));
        handle_scroll_input(&mut viewer, &wheel_event(false));
        assert_eq!(viewer.pending_wheel_rows, 0.0);
    }

    /// The tick actually applies the coalesced impulse to the viewport
    /// offset, through momentum's decay, not a teleport — this is the
    /// end-to-end proof that `apply_momentum_tick` moves real content.
    /// Runs a handful of ticks (not just one) because sub-1.0-row deltas
    /// accumulate in `momentum_remainder` before crossing an integer
    /// boundary (see that field's doc comment) — a single 40ms tick's
    /// delta from one notch is well under 1.0 row, so no visible offset
    /// movement on tick 1 alone is CORRECT, not a bug; this test asserts
    /// the cumulative effect across enough ticks to actually move.
    #[test]
    fn the_momentum_tick_applies_the_coalesced_impulse_to_the_viewport() {
        let mut viewer = test_viewer(5);
        viewer.viewport.set_block_heights(vec![50]); // max_offset 45
        viewer.viewport.jump_to_bottom_and_follow();
        let start_offset = viewer.viewport.offset();
        handle_scroll_input(&mut viewer, &wheel_event(false)); // scroll up (backward)
        assert_eq!(viewer.viewport.offset(), start_offset, "not yet applied");

        for _ in 0..5 {
            apply_momentum_tick(&mut viewer);
        }
        assert!(
            viewer.viewport.offset() < start_offset,
            "scroll-up must move the offset backward within a few ticks: \
             start={start_offset} after={}",
            viewer.viewport.offset()
        );
        assert_eq!(
            viewer.pending_wheel_rows, 0.0,
            "the impulse must be drained into momentum, not left pending"
        );
    }

    /// Momentum keeps moving the viewport across MULTIPLE ticks with no
    /// further wheel input — the actual "flick and it keeps going" claim,
    /// not just a single-tick displacement. Compares two well-separated
    /// checkpoints (5 ticks vs. 10 ticks) rather than consecutive single
    /// ticks: with fractional-remainder carrying (see
    /// `Viewer::momentum_remainder`'s doc comment), any ONE tick's own
    /// delta can legitimately fail to cross an integer row boundary by
    /// itself without breaking the "keeps moving" claim over a slightly
    /// longer window — the position must still be strictly decreasing
    /// across that longer window with zero further input.
    #[test]
    fn momentum_continues_moving_across_ticks_with_no_further_input() {
        let mut viewer = test_viewer(5);
        viewer.viewport.set_block_heights(vec![200]); // plenty of room to move
        viewer.viewport.jump_to_bottom_and_follow();
        let start_offset = viewer.viewport.offset();
        handle_scroll_input(&mut viewer, &wheel_event(false));

        for _ in 0..5 {
            apply_momentum_tick(&mut viewer);
        }
        let after_five_ticks = viewer.viewport.offset();
        assert!(
            after_five_ticks < start_offset,
            "momentum must have moved the offset within 5 ticks: \
             start={start_offset} after_five={after_five_ticks}"
        );

        for _ in 0..5 {
            apply_momentum_tick(&mut viewer); // five MORE ticks, no new input
        }
        assert!(
            viewer.viewport.offset() < after_five_ticks,
            "momentum must keep moving the offset across ticks with no new \
             wheel input: after_five={after_five_ticks} after_ten={}",
            viewer.viewport.offset()
        );
    }

    /// Momentum eventually settles (stops moving the viewport) rather than
    /// running forever — proves the stop-threshold contract end to end
    /// through the viewer, not just inside `Momentum` itself.
    #[test]
    fn momentum_eventually_settles_and_stops_moving_the_viewport() {
        let mut viewer = test_viewer(5);
        viewer.viewport.set_block_heights(vec![10_000]); // never clamps mid-flight
        viewer.viewport.jump_to_bottom_and_follow();
        handle_scroll_input(&mut viewer, &wheel_event(false));
        // `momentum.settled()` reflects `Momentum`'s OWN velocity, which
        // only receives this tick's coalesced `pending_wheel_rows` inside
        // `apply_momentum_tick` itself — right after `handle_scroll_input`
        // the impulse is still sitting in `pending_wheel_rows`, unapplied,
        // so `momentum` is still (correctly) reporting settled. A
        // `while !settled` loop would therefore never run at all here; this
        // must be a do-while shape (apply first, then check) to actually
        // drive momentum through its decay.
        let mut ticks = 0;
        loop {
            apply_momentum_tick(&mut viewer);
            ticks += 1;
            assert!(ticks < 10_000, "momentum never settled through the viewer");
            if viewer.momentum.settled() {
                break;
            }
        }
        let settled_offset = viewer.viewport.offset();
        apply_momentum_tick(&mut viewer);
        assert_eq!(
            viewer.viewport.offset(),
            settled_offset,
            "once settled, further ticks must not move the offset"
        );
    }

    /// End cancels in-flight momentum cleanly: a flick in progress must not
    /// keep nudging the offset after End jumped to the tail and re-engaged
    /// follow — that would visibly fight the jump.
    #[test]
    fn end_cancels_in_flight_momentum() {
        let mut viewer = test_viewer(5);
        viewer.viewport.set_block_heights(vec![200]);
        viewer.viewport.jump_to_bottom_and_follow();
        handle_scroll_input(&mut viewer, &wheel_event(false));
        apply_momentum_tick(&mut viewer);
        assert!(
            !viewer.momentum.settled(),
            "momentum should still be decaying"
        );

        let mut decoder = InputDecoder::new();
        decoder.push(b"\x1b[4~");
        let end = decoder.next_event().expect("end key");
        handle_scroll_input(&mut viewer, &end);

        assert!(
            viewer.momentum.settled(),
            "End must cancel in-flight momentum"
        );
        assert_eq!(viewer.pending_wheel_rows, 0.0);
        assert!(viewer.viewport.following());
        let offset_at_end = viewer.viewport.offset();
        apply_momentum_tick(&mut viewer);
        assert_eq!(
            viewer.viewport.offset(),
            offset_at_end,
            "a tick after End must not move the offset — no residual momentum"
        );
    }

    /// Home (a discrete keyboard jump) also cancels in-flight momentum, the
    /// same as End — any absolute jump must not fight a decaying flick.
    #[test]
    fn home_cancels_in_flight_momentum() {
        let mut viewer = test_viewer(5);
        viewer.viewport.set_block_heights(vec![200]);
        viewer.viewport.jump_to_bottom_and_follow();
        handle_scroll_input(&mut viewer, &wheel_event(false));
        apply_momentum_tick(&mut viewer);
        assert!(!viewer.momentum.settled());

        let mut decoder = InputDecoder::new();
        decoder.push(b"\x1b[1~");
        let home = decoder.next_event().expect("home key");
        handle_scroll_input(&mut viewer, &home);

        assert!(
            viewer.momentum.settled(),
            "Home must cancel in-flight momentum"
        );
        assert_eq!(viewer.viewport.offset(), 0, "Home jumps to the very top");
    }

    /// Case (c) of the coordinator's fix: momentum decay that carries the
    /// window back onto the bottom must re-engage follow (not just leave it
    /// disengaged at `offset == max_offset` by coincidence), cancel the
    /// now-stale momentum/pending state cleanly (so a later tick cannot
    /// "coast" past the bottom), and hide the scrollbar immediately — the
    /// same reconciliation End/F already does, but triggered by decay
    /// alone, with no keyboard input at all.
    #[test]
    fn momentum_decay_reaching_the_bottom_reengages_follow_and_cancels_cleanly() {
        let mut viewer = test_viewer(5);
        viewer.viewport.set_block_heights(vec![50]); // max_offset 45
        viewer.viewport.jump_to_bottom_and_follow();

        // Scroll up just 2 rows (small, deliberately close to the bottom)
        // so momentum's own decay — with no further wheel input — is
        // enough to carry it back down to max_offset within a handful of
        // ticks.
        assert!(viewer.viewport.scroll_by(-2.0));
        assert!(!viewer.viewport.following());
        assert_eq!(viewer.viewport.offset(), 43);

        // A down notch's momentum, ticked repeatedly with no further
        // input, must eventually clamp back to 45 (the bottom) and,
        // because `Viewport::scroll_by` now re-derives follow from the
        // RESULT, that exact tick must re-engage follow.
        handle_scroll_input(&mut viewer, &wheel_event(true)); // scroll down
        let mut reengaged = false;
        for _ in 0..200 {
            apply_momentum_tick(&mut viewer);
            if viewer.viewport.following() {
                reengaged = true;
                break;
            }
        }
        assert!(
            reengaged,
            "momentum decay must eventually re-engage follow once it reaches the bottom"
        );
        assert_eq!(
            viewer.viewport.offset(),
            45,
            "settled exactly at the bottom"
        );

        // Reconciliation: momentum/pending state must be cancelled, not
        // left to keep decaying past the bottom.
        assert!(
            viewer.momentum.settled(),
            "momentum must be cancelled once it carries the window back to the bottom"
        );
        assert_eq!(viewer.pending_wheel_rows, 0.0);
        assert_eq!(viewer.momentum_remainder, 0.0);
        assert!(
            viewer.scrollbar_visible_until.is_none(),
            "the scrollbar must be hidden immediately on the momentum re-engage, \
             the same as an explicit End/F"
        );

        // A further tick must not move the offset at all — no residual
        // "coast past the bottom".
        let offset_at_reengage = viewer.viewport.offset();
        apply_momentum_tick(&mut viewer);
        assert_eq!(viewer.viewport.offset(), offset_at_reengage);
    }

    /// A scroll step that actually moves the offset while disengaged must
    /// set the scrollbar's auto-hide deadline (i.e. show it) — the
    /// `Summary`-mode `TerminalSink` makes the actual terminal write a
    /// no-op, so this test asserts the STATE MACHINE (`scrollbar_visible_
    /// until`) directly, which is exactly what `run_viewer_loop`'s tick
    /// reads to decide when to hide it.
    #[test]
    fn a_moving_scroll_step_shows_the_scrollbar() {
        let mut viewer = test_viewer(5);
        viewer.viewport.set_block_heights(vec![50]);
        assert!(viewer.scrollbar_visible_until.is_none());
        handle_scroll_input(&mut viewer, &wheel_event(false));
        for _ in 0..5 {
            apply_momentum_tick(&mut viewer);
        }
        assert!(
            viewer.scrollbar_visible_until.is_some(),
            "a moving scroll step must show the scrollbar"
        );
    }

    /// Re-engaging follow (End/F) hides the scrollbar immediately, without
    /// waiting for the auto-hide timer — a thumb position is meaningless
    /// while pinned to the tail.
    #[test]
    fn end_hides_the_scrollbar_immediately() {
        let mut viewer = test_viewer(5);
        viewer.viewport.set_block_heights(vec![200]);
        viewer.viewport.jump_to_bottom_and_follow();
        handle_scroll_input(&mut viewer, &wheel_event(false));
        for _ in 0..5 {
            apply_momentum_tick(&mut viewer);
        }
        assert!(viewer.scrollbar_visible_until.is_some());

        let mut decoder = InputDecoder::new();
        decoder.push(b"\x1b[4~");
        let end = decoder.next_event().expect("end key");
        handle_scroll_input(&mut viewer, &end);
        assert!(
            viewer.scrollbar_visible_until.is_none(),
            "End must hide the scrollbar immediately, not defer to the timer"
        );
    }

    /// The tick loop's auto-hide check: once `Instant::now()` passes the
    /// deadline, `hide_scrollbar` clears `scrollbar_visible_until` back to
    /// `None`. Drives the same condition `run_viewer_loop` checks every
    /// tick, without needing the full loop or a real sleep.
    #[test]
    fn hide_scrollbar_clears_the_deadline() {
        let mut viewer = test_viewer(5);
        viewer.viewport.set_block_heights(vec![50]);
        viewer.scrollbar_visible_until = Some(Instant::now());
        hide_scrollbar(&mut viewer);
        assert!(viewer.scrollbar_visible_until.is_none());
    }

    /// Mouse move/down/up/scroll-left/scroll-right must never disengage
    /// follow — only ScrollUp/ScrollDown do (the live scroll-lab's
    /// "spontaneous follow=false" investigation: this proves the OTHER
    /// mouse event kinds are structurally inert here, not just relying on
    /// `scroll_delta` upstream).
    #[test]
    fn non_scroll_mouse_events_never_disengage_follow() {
        use tmath_core::mouse::{Mods, MouseButton, MouseEvent, MouseKind};

        let mut viewer = test_viewer(5);
        viewer.viewport.set_block_heights(vec![50]);
        assert!(viewer.viewport.following());

        for kind in [
            MouseKind::Move,
            MouseKind::Down,
            MouseKind::Up,
            MouseKind::ScrollLeft,
            MouseKind::ScrollRight,
        ] {
            let event = tmath_core::input::Event::Mouse(MouseEvent {
                kind,
                button: MouseButton::None,
                mods: Mods::default(),
                x: 10,
                y: 10,
            });
            handle_scroll_input(&mut viewer, &event);
            assert!(
                viewer.viewport.following(),
                "{kind:?} must never disengage follow"
            );
            assert_eq!(
                viewer.pending_wheel_rows, 0.0,
                "{kind:?} must never accumulate a pending wheel impulse"
            );
        }
    }

    fn visible_range(
        first: usize,
        last_exclusive: usize,
        skip_rows_in_first: u32,
    ) -> viewer_viewport::VisibleRange {
        viewer_viewport::VisibleRange {
            first,
            last_exclusive,
            skip_rows_in_first,
        }
    }

    /// The core positive case: N blocks entered at the top edge,
    /// `skip_rows_in_first` unchanged, the bottom edge only shrank (never
    /// grew) — a pure backward step.
    #[test]
    fn incremental_entering_range_detects_a_pure_backward_step() {
        // Before: blocks [3, 8). After: blocks [1, 6) — 2 blocks (1, 2)
        // newly entered at the top; blocks 6, 7 left the bottom.
        let before = visible_range(3, 8, 0);
        let after = visible_range(1, 6, 0);
        assert_eq!(incremental_entering_range(before, after), Some(1..3));
    }

    #[test]
    fn incremental_entering_range_is_none_for_a_forward_step() {
        // A forward step (scrolling toward newer content) is not this
        // function's shape — `first` increased.
        let before = visible_range(1, 6, 0);
        let after = visible_range(3, 8, 0);
        assert_eq!(incremental_entering_range(before, after), None);
    }

    #[test]
    fn incremental_entering_range_is_none_when_skip_rows_in_first_changes() {
        // Same block-index range, but a different crop within the first
        // block — this is a partial-edge shape the incremental path does
        // not attempt (it only redraws newly ENTERING blocks, never
        // recrops an already-drawn one).
        let before = visible_range(1, 6, 2);
        let after = visible_range(1, 6, 5);
        assert_eq!(incremental_entering_range(before, after), None);
    }

    #[test]
    fn incremental_entering_range_is_none_when_nothing_actually_entered() {
        // `first` unchanged (no new block at the top) even though the
        // bottom shrank — no entering range to compute.
        let before = visible_range(2, 8, 0);
        let after = visible_range(2, 6, 0);
        assert_eq!(incremental_entering_range(before, after), None);
    }

    #[test]
    fn incremental_entering_range_is_none_for_an_empty_new_window() {
        let before = visible_range(3, 8, 0);
        let after = visible_range(0, 0, 0);
        assert_eq!(incremental_entering_range(before, after), None);
    }

    #[test]
    fn incremental_entering_range_is_none_when_the_bottom_edge_grows() {
        // `last_exclusive` increasing means content GREW into view from
        // the bottom too, not a pure "entered at the top" shape.
        let before = visible_range(3, 8, 0);
        let after = visible_range(1, 9, 0);
        assert_eq!(incremental_entering_range(before, after), None);
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
        // The immediate reaction only re-derives follow from the (still
        // unmoved) offset — see `Viewport::scroll_by`'s re-pin-at-bottom
        // rule — so disengage only becomes visible once a tick actually
        // applies the coalesced impulse and moves the offset off the
        // bottom.
        for _ in 0..5 {
            apply_momentum_tick(&mut viewer);
        }
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
        // See the sibling test's comment: disengage only becomes visible
        // once a tick actually moves the offset off the bottom.
        for _ in 0..5 {
            apply_momentum_tick(&mut viewer);
        }
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
            // Tests never want the ongoing eprintln! diagnostics on stderr;
            // `viewer_log_enabled_tests` below covers the env-parsing
            // function itself.
            log_enabled: false,
            cell: CellSize {
                width: 1,
                height: 1,
            },
            block_sources: Vec::new(),
            delta: DeltaState::new(IPC_MAX_REQUEST_BYTES),
            momentum: tmath_core::momentum::Momentum::new(),
            pending_wheel_rows: 0.0,
            momentum_remainder: 0.0,
            scrollbar_visible_until: None,
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

    // --- AT-3-603: streaming transcript replay ---

    use std::fs::{self, OpenOptions};
    use std::io::Write as _;
    use std::path::PathBuf;
    use std::time::Instant;

    use tmath_core::agent::{Message, DELTA_PROTOCOL_VERSION};

    use crate::transcript_adapter::{
        TranscriptAdapter, TranscriptDelta, TranscriptOpenMode,
    };

    fn fixture_streaming_transcript_lines() -> Vec<String> {
        include_str!("../../../../tests/fixtures/agents/streaming-transcript.jsonl")
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(str::to_string)
            .collect()
    }

    fn temp_replay_transcript() -> PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let sequence = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "tmath-stream-replay-{}-{sequence}.jsonl",
            std::process::id()
        ));
        fs::write(&path, "").unwrap();
        path
    }

    fn append_transcript_line(path: &PathBuf, line: &str) {
        let mut file = OpenOptions::new().append(true).open(path).unwrap();
        writeln!(file, "{line}").unwrap();
    }

    fn apply_transcript_delta(viewer: &mut Viewer, delta: TranscriptDelta, seq: &mut u64) {
        match delta {
            TranscriptDelta::Reset(text) => {
                *seq = 0;
                apply_incoming_message(viewer, &Message::Document { text }).unwrap();
            }
            TranscriptDelta::AnswerBoundary => {}
            TranscriptDelta::Append(text) => {
                *seq += 1;
                let keep_bytes = viewer.delta.document().len();
                apply_incoming_message(
                    viewer,
                    &Message::ReplaceTail {
                        version: DELTA_PROTOCOL_VERSION,
                        seq: *seq,
                        keep_bytes,
                        text,
                    },
                )
                .unwrap();
            }
        }
    }

    /// at a time places each new block before the next assistant line arrives.
    #[test]
    fn streaming_transcript_replay_places_blocks_incrementally() {
        let path = temp_replay_transcript();
        let lines = fixture_streaming_transcript_lines();
        let mut adapter =
            TranscriptAdapter::open(&path, TranscriptOpenMode::FromStart).unwrap();
        let mut viewer = test_viewer(24);
        let mut seq = 0u64;
        let mut max_blocks_seen = 0usize;
        let mut growth_steps = 0usize;

        for line in lines {
            append_transcript_line(&path, &line);
            let deltas = adapter.poll().unwrap();
            for delta in deltas {
                let blocks_before = viewer.planner.blocks().len();
                apply_transcript_delta(&mut viewer, delta, &mut seq);
                let blocks_after = viewer.planner.blocks().len();
                if blocks_after > blocks_before {
                    growth_steps += 1;
                    max_blocks_seen = blocks_after;
                }
            }
        }

        assert!(
            max_blocks_seen >= 3,
            "the streaming fixture must produce multiple placed blocks, got {max_blocks_seen}"
        );
        assert!(
            growth_steps >= 2,
            "blocks must grow across multiple append steps, not one monolithic placement"
        );
        let _ = fs::remove_file(path);
    }

    /// Optional release-only spot check for the G2 append ceiling; debug builds
    /// include a cold Typst/RaTeX start that would false-fail the 150 ms gate.
    #[test]
    #[ignore = "run with `cargo test -p tmath --release streaming_transcript_replay_meets_g2 -- --ignored` for AT-3-603 latency evidence"]
    fn streaming_transcript_replay_meets_g2_on_release_builds() {
        use std::time::Duration;

        let path = temp_replay_transcript();
        let lines = fixture_streaming_transcript_lines();
        let mut adapter =
            TranscriptAdapter::open(&path, TranscriptOpenMode::FromStart).unwrap();
        let mut viewer = test_viewer(24);
        let mut seq = 0u64;
        // Warm the native engine so subsequent append steps measure steady-state G2.
        render_and_place(&mut viewer, "Warmup block.\n\n").unwrap();
        let mut append_latencies = Vec::new();

        for line in lines {
            append_transcript_line(&path, &line);
            let started = Instant::now();
            let deltas = adapter.poll().unwrap();
            for delta in deltas {
                let blocks_before = viewer.planner.blocks().len();
                apply_transcript_delta(&mut viewer, delta, &mut seq);
                if viewer.planner.blocks().len() > blocks_before {
                    append_latencies.push(started.elapsed());
                }
            }
        }

        let max = append_latencies
            .iter()
            .copied()
            .max()
            .unwrap_or(Duration::ZERO);
        assert!(
            max <= Duration::from_millis(150),
            "G2 p95-style ceiling for a warmed append step: max={max:?}"
        );
        let _ = fs::remove_file(path);
    }
}
