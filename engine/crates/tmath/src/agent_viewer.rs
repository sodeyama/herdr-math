//! `tmath agent-viewer` — the process that runs inside the tmux viewer split.
//!
//! It connects to the watcher's Unix socket, renders each new answer document
//! through the one-shot renderer, and places the result as a scrollback-anchored
//! Kitty image in its own pane, replacing the previous image. `q`/`Ctrl-C`
//! close the viewer; the scroll driver maps wheel/arrow input to a re-placed,
//! vertically shifted image. Render failures leave the previous image intact.

use std::io::{self, Read as _, Write as _};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use tmath_core::agent::{Decoder, Message};
use tmath_core::input::InputDecoder;
use tmath_core::ipc::{RenderResponse, IPC_MAX_REQUEST_BYTES};
use tmath_core::placement::{
    decode_png, emit_placed_block, emit_replaced_block, CellSize, PlacementLimits, PlacementTracker,
};
use tmath_core::scroll_driver::{is_exit_signal, ScrollDriver};
use tmath_core::terminal::{StdioTty, Terminal};

use crate::render::{render_document_text, renderer_worker_path};

const MAX_PIXELS: u64 = 64 * 1024 * 1024;
const CONNECT_RETRIES: u32 = 50;
const CONNECT_RETRY_MS: u64 = 100;
const POLL_TIMEOUT: Duration = Duration::from_millis(40);

/// The currently placed image, kept so a new document can replace it and the
/// scroll driver can re-place it at a shifted home row.
struct ImageState {
    image_id: u32,
    width: u32,
    height: u32,
    rgba: Vec<u8>,
    cols: u32,
    rows: u32,
    base_home: u32,
}

struct Viewer {
    tracker: PlacementTracker,
    cell: CellSize,
    stream: UnixStream,
    current: Option<ImageState>,
    scroll: ScrollDriver,
    emitted_offset: i64,
    input: InputDecoder,
    messages: Decoder,
}

pub(crate) fn run_agent_viewer(args: &[String]) -> Result<i32, String> {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("usage: tmath agent-viewer <socket-path>");
        return Ok(0);
    }
    let socket = args.first().ok_or("agent-viewer requires a socket path")?;
    let _ = renderer_worker_path()?;

    let stream = connect_with_retry(socket)?;

    let mut terminal = Terminal::new(StdioTty::default(), 1)
        .map_err(|error| format!("initialize terminal: {error}"))?;
    if !terminal
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

    let mut viewer = Viewer {
        tracker: PlacementTracker::new(PlacementLimits::default()),
        cell,
        stream,
        current: None,
        scroll: ScrollDriver::new(0.0),
        emitted_offset: 0,
        input: InputDecoder::new(),
        messages: Decoder::new(),
    };
    let _ = viewer.stream.set_nonblocking(true);
    eprintln!("agent-viewer: connected; q/Ctrl-C closes");

    let loop_result = run_viewer_loop(&mut viewer);
    let _ = terminal.reset();
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

        // Terminal input: `q`/Ctrl-C close the viewer; everything else scrolls.
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
                        let _ = viewer.scroll.handle(&event, None);
                    }
                }
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(format!("read stdin: {error}")),
            }
        }

        // Advance the scroll easing and re-place the image when the offset
        // moved from the last emitted home row.
        let _ = viewer.scroll.step(0.02);
        reemit_if_moved(viewer)?;

        let elapsed = start.elapsed();
        if elapsed < POLL_TIMEOUT {
            std::thread::sleep(POLL_TIMEOUT - elapsed);
        }
    }
}

fn scroll_offset(scroll: &ScrollDriver, rows: u32) -> i64 {
    (scroll.position().round() as i64).clamp(0, rows as i64)
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

/// Renders a document and replaces the previous image, keeping the previous
/// image intact (fail closed) on any render or limit error.
fn render_and_place(viewer: &mut Viewer, text: &str) -> Result<(), String> {
    if text.len() > IPC_MAX_REQUEST_BYTES {
        eprintln!("agent-viewer: renderer_input_limit");
        return Ok(());
    }
    let response = match render_document_text(text, None) {
        Ok(response) => response,
        Err(message) => {
            eprintln!("agent-viewer: render_failed ({message})");
            return Ok(());
        }
    };
    let RenderResponse::Success(success) = &response else {
        eprintln!("agent-viewer: renderer_rejected");
        return Ok(());
    };
    let png = match BASE64.decode(success.base64.as_bytes()) {
        Ok(png) => png,
        Err(_) => {
            eprintln!("agent-viewer: render_invalid_base64");
            return Ok(());
        }
    };
    let (width, height, rgba) = match decode_png(&png, MAX_PIXELS) {
        Ok(decoded) => decoded,
        Err(error) => {
            eprintln!("agent-viewer: invalid_image ({error})");
            return Ok(());
        }
    };

    let (block, base_home) = match viewer.current.as_ref() {
        Some(previous) => {
            let block = match viewer
                .tracker
                .replace(previous.image_id, width, height, viewer.cell)
            {
                Ok(block) => block,
                Err(error) => {
                    eprintln!("agent-viewer: placement_limit ({error})");
                    return Ok(());
                }
            };
            (block, previous.base_home)
        }
        None => {
            let block = match viewer.tracker.reserve(width, height, viewer.cell) {
                Ok(block) => block,
                Err(error) => {
                    eprintln!("agent-viewer: placement_limit ({error})");
                    return Ok(());
                }
            };
            (block, 1)
        }
    };

    let home = base_home;
    let bytes = if viewer.current.is_some() {
        emit_replaced_block(
            block.image_id,
            width,
            height,
            &rgba,
            block.cols,
            block.rows,
            home,
        )
    } else {
        emit_placed_block(
            block.image_id,
            width,
            height,
            &rgba,
            block.cols,
            block.rows,
            home,
        )
    };
    let mut stdout = io::stdout().lock();
    stdout
        .write_all(&tmath_core::kitty::wrapped_for_tty(&bytes))
        .map_err(|error| format!("write placement: {error}"))?;
    stdout
        .flush()
        .map_err(|error| format!("flush placement: {error}"))?;

    viewer.current = Some(ImageState {
        image_id: block.image_id,
        width,
        height,
        rgba,
        cols: block.cols,
        rows: block.rows,
        base_home: home,
    });
    viewer.scroll = ScrollDriver::new(block.rows as f32);
    viewer.emitted_offset = 0;
    eprintln!(
        "agent-viewer: placed image={} rows={} bytes={}",
        block.image_id, block.rows, success.bytes
    );
    Ok(())
}

/// Re-places the current image shifted by the current eased scroll offset,
/// when the offset moved from the last emitted home row.
fn reemit_if_moved(viewer: &mut Viewer) -> Result<bool, String> {
    let rows = viewer.current.as_ref().map_or(0, |image| image.rows);
    let offset = scroll_offset(&viewer.scroll, rows);
    if viewer.current.is_none() || offset == viewer.emitted_offset {
        return Ok(false);
    }
    let home;
    let bytes;
    {
        let image = viewer.current.as_ref().expect("checked above");
        home = (image.base_home as i64 - offset).clamp(1, image.base_home as i64) as u32;
        bytes = emit_replaced_block(
            image.image_id,
            image.width,
            image.height,
            &image.rgba,
            image.cols,
            image.rows,
            home,
        );
    }
    let mut stdout = io::stdout().lock();
    stdout
        .write_all(&tmath_core::kitty::wrapped_for_tty(&bytes))
        .map_err(|error| format!("write scroll placement: {error}"))?;
    stdout
        .flush()
        .map_err(|error| format!("flush scroll placement: {error}"))?;
    viewer.emitted_offset = offset;
    Ok(true)
}
