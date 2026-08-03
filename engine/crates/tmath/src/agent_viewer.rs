//! `tmath agent-viewer` — the process that runs inside the tmux viewer split.
//!
//! It connects to the watcher's Unix socket, renders each new answer document
//! through the one-shot renderer, and places the result as a scrollback-anchored
//! Kitty image in its own pane, replacing the previous image. `q`/`Ctrl-C`
//! close the viewer; the scroll driver maps wheel/arrow input to a re-placed,
//! vertically shifted image. Render failures leave the previous image intact.

use std::io::{self, Read as _};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use tmath_core::agent::{Decoder, Message};
use tmath_core::input::InputDecoder;
use tmath_core::ipc::{RenderResponse, IPC_MAX_REQUEST_BYTES};
use tmath_core::placement::{
    decode_png, emit_placed_block, CellSize, PlacementLimits, PlacementTracker, TerminalOp,
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
    rows: u32,
    base_home: u32,
}

struct Viewer {
    tracker: PlacementTracker,
    cell: CellSize,
    viewport_cols: u32,
    viewport_rows: u32,
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
    let viewport = terminal
        .size()
        .map_err(|error| format!("measure viewer size: {error}"))?;

    let mut viewer = Viewer {
        tracker: PlacementTracker::new(PlacementLimits::default()),
        cell,
        viewport_cols: viewport.cols.max(1),
        viewport_rows: viewport.rows.max(1),
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

fn scroll_offset(scroll: &ScrollDriver, rows: u32, viewport_rows: u32) -> i64 {
    let max = rows.saturating_sub(viewport_rows.max(1)) as i64;
    (scroll.position().round() as i64).clamp(0, max)
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
    // #region agent log
    debug_log(
        "H1,H2,H3,H4,H5",
        "agent_viewer.rs:render_and_place",
        "viewer received document",
        serde_json::json!({
            "documentBytes": text.len(),
            "hasCurrentImage": viewer.current.is_some(),
            "viewportCols": viewer.viewport_cols,
            "viewportRows": viewer.viewport_rows,
            "cellWidth": viewer.cell.width,
            "cellHeight": viewer.cell.height
        }),
    );
    // #endregion
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
    let success = match &response {
        RenderResponse::Success(success) => success,
        RenderResponse::Failure(failure) => {
            // #region agent log
            debug_log(
                "H2",
                "agent_viewer.rs:renderer_rejected",
                "renderer rejected document",
                serde_json::json!({
                    "documentBytes": text.len(),
                    "errorCode": failure.error.code,
                    "retryable": failure.error.retryable
                }),
            );
            // #endregion
            eprintln!("agent-viewer: renderer_rejected");
            return Ok(());
        }
    };
    let png = match BASE64.decode(success.base64.as_bytes()) {
        Ok(png) => png,
        Err(_) => {
            eprintln!("agent-viewer: render_invalid_base64");
            return Ok(());
        }
    };
    let (new_width, new_height, new_rgba) = match decode_png(&png, MAX_PIXELS) {
        Ok(decoded) => decoded,
        Err(error) => {
            eprintln!("agent-viewer: invalid_image ({error})");
            return Ok(());
        }
    };
    let (width, height, rgba) = match viewer.current.as_ref() {
        Some(previous) => match append_rgba(
            previous,
            new_width,
            new_height,
            &new_rgba,
            viewer.cell.height,
        ) {
            Ok(composite) => {
                // #region agent log
                debug_log(
                    "H14",
                    "agent_viewer.rs:append_history",
                    "appended answer to viewer history",
                    serde_json::json!({
                        "previousHeight": previous.height,
                        "newAnswerHeight": new_height,
                        "compositeWidth": composite.0,
                        "compositeHeight": composite.1
                    }),
                );
                // #endregion
                composite
            }
            Err(error) => {
                eprintln!("agent-viewer: history_limit ({error})");
                return Ok(());
            }
        },
        None => (new_width, new_height, new_rgba),
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
    // #region agent log
    debug_log(
        "H3,H4,H5",
        "agent_viewer.rs:render_geometry",
        "render decoded and placement reserved",
        serde_json::json!({
            "imageWidth": width,
            "imageHeight": height,
            "placementCols": block.cols,
            "placementRows": block.rows,
            "viewportCols": viewer.viewport_cols,
            "viewportRows": viewer.viewport_rows,
            "replacing": viewer.current.is_some(),
            "maxScrollRows": block.rows.saturating_sub(viewer.viewport_rows)
        }),
    );
    // #endregion
    let bytes = if viewer.current.is_some() {
        viewer_replacement(
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
    crate::terminal_output::write_operations(&bytes)
        .map_err(|error| format!("write placement: {error}"))?;

    viewer.current = Some(ImageState {
        image_id: block.image_id,
        width,
        height,
        rgba,
        rows: block.rows,
        base_home: home,
    });
    let max_scroll = block.rows.saturating_sub(viewer.viewport_rows) as f32;
    viewer.scroll = ScrollDriver::new(max_scroll);
    viewer.scroll.jump_to(max_scroll);
    viewer.emitted_offset = 0;
    eprintln!(
        "agent-viewer: placed image={} rows={} bytes={}",
        block.image_id, block.rows, success.bytes
    );
    Ok(())
}

fn append_rgba(
    previous: &ImageState,
    width: u32,
    height: u32,
    rgba: &[u8],
    gap: u32,
) -> Result<(u32, u32, Vec<u8>), &'static str> {
    let composite_width = previous.width.max(width);
    let composite_height = previous
        .height
        .checked_add(gap)
        .and_then(|value| value.checked_add(height))
        .ok_or("composite dimensions overflow")?;
    let pixels = u64::from(composite_width) * u64::from(composite_height);
    if pixels > MAX_PIXELS {
        return Err("composite pixel limit exceeded");
    }
    let byte_len = usize::try_from(pixels.checked_mul(4).ok_or("composite size overflow")?)
        .map_err(|_| "composite size overflow")?;
    let mut composite = vec![0; byte_len];
    copy_rgba_rows(
        &mut composite,
        composite_width,
        0,
        previous.width,
        previous.height,
        &previous.rgba,
    );
    copy_rgba_rows(
        &mut composite,
        composite_width,
        previous.height + gap,
        width,
        height,
        rgba,
    );
    Ok((composite_width, composite_height, composite))
}

fn copy_rgba_rows(
    destination: &mut [u8],
    destination_width: u32,
    destination_y: u32,
    source_width: u32,
    source_height: u32,
    source: &[u8],
) {
    let source_stride = source_width as usize * 4;
    let destination_stride = destination_width as usize * 4;
    for row in 0..source_height as usize {
        let source_start = row * source_stride;
        let destination_start = (destination_y as usize + row) * destination_stride;
        destination[destination_start..destination_start + source_stride]
            .copy_from_slice(&source[source_start..source_start + source_stride]);
    }
}

/// Re-places the current image shifted by the current eased scroll offset,
/// when the offset moved from the last emitted home row.
fn reemit_if_moved(viewer: &mut Viewer) -> Result<bool, String> {
    let rows = viewer.current.as_ref().map_or(0, |image| image.rows);
    let offset = scroll_offset(&viewer.scroll, rows, viewer.viewport_rows);
    if viewer.current.is_none() || offset == viewer.emitted_offset {
        return Ok(false);
    }
    let home;
    let bytes;
    {
        let image = viewer.current.as_ref().expect("checked above");
        home = (image.base_home as i64 - offset).clamp(1, image.base_home as i64) as u32;
        let (cropped_height, cropped_rgba) = crop_rgba_top(
            image.width,
            image.height,
            &image.rgba,
            offset as u32,
            viewer.cell.height,
        );
        let (cropped_cols, cropped_rows) =
            tmath_core::placement::grid_for(image.width, cropped_height, viewer.cell);
        bytes = viewer_replacement(
            image.image_id,
            image.width,
            cropped_height,
            &cropped_rgba,
            cropped_cols,
            cropped_rows,
            home,
        );
    }
    crate::terminal_output::write_operations(&bytes)
        .map_err(|error| format!("write scroll placement: {error}"))?;
    viewer.emitted_offset = offset;
    Ok(true)
}

/// Clears the dedicated viewer grid before replacing an image so placeholder
/// cells from a taller previous image cannot survive below the replacement.
fn viewer_replacement(
    image_id: u32,
    width: u32,
    height: u32,
    rgba: &[u8],
    cols: u32,
    rows: u32,
    home: u32,
) -> Vec<TerminalOp> {
    // #region agent log
    debug_log(
        "H4",
        "agent_viewer.rs:viewer_replacement",
        "replacement clears viewer screen",
        serde_json::json!({
            "imageId": image_id,
            "cols": cols,
            "rows": rows,
            "home": home,
            "clearScreen": true
        }),
    );
    // #endregion
    let mut operations = vec![
        TerminalOp::Graphics(tmath_core::kitty::kitty_delete_id(image_id)),
        TerminalOp::Local(b"\x1b[H\x1b[2J".to_vec()),
    ];
    operations.extend(emit_placed_block(
        image_id, width, height, rgba, cols, rows, home,
    ));
    operations
}

fn debug_log(hypothesis_id: &str, location: &str, message: &str, data: serde_json::Value) {
    use std::time::{SystemTime, UNIX_EPOCH};

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    let payload = serde_json::json!({
        "sessionId": "f945c2",
        "runId": "pre-fix",
        "hypothesisId": hypothesis_id,
        "location": location,
        "message": message,
        "data": data,
        "timestamp": timestamp
    });
    crate::terminal_output::write_debug_line(&payload);
}

/// Crops whole terminal-cell rows from the top of a full RGBA answer.
fn crop_rgba_top(
    width: u32,
    height: u32,
    rgba: &[u8],
    offset_rows: u32,
    cell_height: u32,
) -> (u32, Vec<u8>) {
    let offset_px = offset_rows
        .saturating_mul(cell_height.max(1))
        .min(height.saturating_sub(1));
    let row_bytes = width as usize * 4;
    let start = offset_px as usize * row_bytes;
    (height - offset_px, rgba[start..].to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scroll_offset_clamps_to_the_viewport() {
        let mut decoder = tmath_core::input::InputDecoder::new();
        decoder.push(b"\x1b[<65;10;20M");
        let wheel = decoder.next_event().expect("wheel");
        let mut scroll = ScrollDriver::new(30.0);
        scroll.handle(&wheel, None);
        for _ in 0..600 {
            scroll.step(1.0 / 60.0);
            if scroll.settled() {
                break;
            }
        }
        assert_eq!(scroll_offset(&scroll, 50, 20), 3);
        assert_eq!(scroll_offset(&scroll, 50, 50), 0);
    }

    #[test]
    fn crop_removes_complete_cell_rows() {
        let rgba: Vec<u8> = (0..4 * 6 * 4).map(|value| value as u8).collect();
        let (height, cropped) = crop_rgba_top(4, 6, &rgba, 1, 2);
        assert_eq!(height, 4);
        assert_eq!(cropped, rgba[4 * 2 * 4..]);
    }

    #[test]
    fn viewer_replacement_clears_stale_placeholder_cells() {
        let operations = viewer_replacement(1, 1, 1, &[0, 0, 0, 0], 1, 1, 1);
        assert!(matches!(
            operations.get(1),
            Some(TerminalOp::Local(bytes)) if bytes == b"\x1b[H\x1b[2J"
        ));
    }

    #[test]
    fn append_rgba_keeps_previous_pixels_above_new_pixels() {
        let previous = ImageState {
            image_id: 1,
            width: 1,
            height: 1,
            rgba: vec![1, 2, 3, 4],
            rows: 1,
            base_home: 1,
        };
        let (width, height, rgba) = append_rgba(&previous, 1, 1, &[5, 6, 7, 8], 1).unwrap();
        assert_eq!((width, height), (1, 3));
        assert_eq!(rgba, vec![1, 2, 3, 4, 0, 0, 0, 0, 5, 6, 7, 8]);
    }
}
