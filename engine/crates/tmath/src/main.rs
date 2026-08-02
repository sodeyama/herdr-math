//! `tmath` — standalone terminal math/document renderer CLI.
//!
//! `tmath render <file | ->` reads a document, forwards it to the one-shot
//! TypeScript renderer subprocess over stdin/stdout, and — when running against
//! a real Kitty-graphics terminal — places the rendered image as a
//! scrollback-anchored placement in the main buffer. When stdout is not a
//! terminal, it reports the bounded response instead.

use std::env;
use std::io::{self, IsTerminal as _, Read as _, Write as _};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use tmath_core::ipc::{
    EitherKind, IpcError, RenderRequest, RenderResponse, IPC_MAX_REQUEST_BYTES,
    IPC_MAX_RESPONSE_BYTES, IPC_PROTOCOL,
};
use tmath_core::placement::{
    decode_png, emit_placed_block, CellSize, PlacementError, PlacementLimits, PlacementTracker,
};
use tmath_core::terminal::{StdioTty, Terminal};

const RENDER_TIMEOUT: Duration = Duration::from_secs(15);

fn main() {
    let code = match run() {
        Ok(code) => code,
        Err(message) => {
            eprintln!("tmath: {message}");
            2
        }
    };
    std::process::exit(code);
}

fn run() -> Result<i32, String> {
    let args: Vec<String> = env::args().skip(1).collect();
    let Some(command) = args.first() else {
        return Err("usage: tmath render <file | ->".into());
    };
    match command.as_str() {
        "render" => render(&args[1..]),
        "--help" | "-h" => {
            println!("usage: tmath render <file | ->");
            println!("  -  read the document from stdin");
            Ok(0)
        }
        "--version" | "-V" => {
            println!("tmath {}", env!("CARGO_PKG_VERSION"));
            Ok(0)
        }
        other => Err(format!("unknown command {other:?}; use 'render'")),
    }
}

fn render(args: &[String]) -> Result<i32, String> {
    if args.len() != 1 {
        return Err("usage: tmath render <file | ->".into());
    }
    let source = read_document(&args[0])?;
    let request = RenderRequest {
        protocol: IPC_PROTOCOL.to_string(),
        kind: EitherKind::Document,
        text: Some(source),
        formulas: None,
        options: None,
    };

    let worker = renderer_worker_path()?;
    let payload = request.encode().map_err(|error| error.to_string())?;
    let response = spawn_renderer(&worker, &payload)?;
    match response {
        RenderResponse::Success(success) => {
            let png = BASE64
                .decode(success.base64.as_bytes())
                .map_err(|_| "renderer returned invalid base64 PNG".to_string())?;
            if io::stdout().is_terminal() {
                place_in_terminal(&png)
            } else {
                println!(
                    "ok width={} height={} bytes={} renderer={}",
                    success.width, success.height, success.bytes, success.renderer
                );
                Ok(0)
            }
        }
        RenderResponse::Failure(failure) => {
            eprintln!(
                "tmath: render failed: {} (retryable={})",
                failure.error.code, failure.error.retryable
            );
            Ok(1)
        }
    }
}

/// Places a rendered PNG into a real terminal's main buffer as a
/// scrollback-anchored virtual placement, then restores the terminal.
fn place_in_terminal(png: &[u8]) -> Result<i32, String> {
    const MAX_PIXELS: u64 = 64 * 1024 * 1024;
    let (width, height, rgba) = decode_png(png, MAX_PIXELS)
        .map_err(|error: PlacementError| format!("decode rendered image: {error}"))?;

    let mut terminal = Terminal::new(StdioTty::default(), 1)
        .map_err(|error| format!("initialize terminal: {error}"))?;
    if !terminal
        .probe_graphics_support()
        .map_err(|error| format!("probe graphics: {error}"))?
    {
        return Err("this terminal reports no Kitty graphics support".into());
    }
    let cell = terminal
        .cell_size()
        .map_err(|error| format!("measure cell size: {error}"))?
        .ok_or("terminal reported no usable cell size")?;

    let mut tracker = PlacementTracker::new(PlacementLimits::default());
    let block = tracker
        .reserve(
            width,
            height,
            CellSize {
                width: cell.0,
                height: cell.1,
            },
        )
        .map_err(|error: PlacementError| format!("place image: {error}"))?;
    let home_row = tracker.home_row_for_next().max(1);
    let placement = emit_placed_block(
        block.image_id,
        width,
        height,
        &rgba,
        block.cols,
        block.rows,
        home_row,
    );
    let mut stdout = io::stdout().lock();
    stdout
        .write_all(&placement)
        .map_err(|error| format!("write placement: {error}"))?;
    stdout
        .flush()
        .map_err(|error| format!("flush placement: {error}"))?;
    drop(stdout);
    terminal
        .reset()
        .map_err(|error| format!("reset terminal: {error}"))?;
    println!(
        "placed width={width} height={height} image_id={}",
        block.image_id
    );
    Ok(0)
}

fn read_document(path: &str) -> Result<String, String> {
    let mut text = String::new();
    if path == "-" {
        io::stdin()
            .take((IPC_MAX_REQUEST_BYTES + 1) as u64)
            .read_to_string(&mut text)
            .map_err(|error| format!("read stdin: {error}"))?;
    } else {
        std::fs::read_to_string(path).map_err(|error| format!("read {path}: {error}"))?;
    }
    if text.len() > IPC_MAX_REQUEST_BYTES {
        return Err(format!("document exceeds {IPC_MAX_REQUEST_BYTES} bytes"));
    }
    Ok(text)
}

fn renderer_worker_path() -> Result<PathBuf, String> {
    if let Some(path) = env::var_os("TMATH_RENDER_WORKER") {
        return Ok(PathBuf::from(path));
    }
    Err("TMATH_RENDER_WORKER must point at the built render subprocess".into())
}

fn spawn_renderer(worker: &PathBuf, request: &[u8]) -> Result<RenderResponse, String> {
    let mut child = Command::new("node")
        .arg(worker)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("spawn renderer: {error}"))?;

    let mut stdin = child.stdin.take().ok_or("renderer stdin unavailable")?;
    stdin
        .write_all(request)
        .map_err(|error| format!("write request: {error}"))?;
    drop(stdin);

    let mut stdout = child.stdout.take().ok_or("renderer stdout unavailable")?;
    let mut bytes = Vec::new();
    let timed_out = std::sync::Arc::new(std::sync::Mutex::new(false));
    let timeout_flag = std::sync::Arc::clone(&timed_out);
    let read_result = std::thread::spawn(move || {
        let limit = IPC_MAX_RESPONSE_BYTES + 1;
        let mut chunk = [0u8; 4096];
        let mut read = 0usize;
        while read < limit {
            match stdout.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    bytes.extend_from_slice(&chunk[..n]);
                    read += n;
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(format!("read response: {error}")),
            }
        }
        Ok::<Vec<u8>, String>(bytes)
    });

    let started = Instant::now();
    loop {
        if read_result.is_finished() {
            break;
        }
        if started.elapsed() >= RENDER_TIMEOUT {
            *timeout_flag.lock().unwrap() = true;
            let _ = child.kill();
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    let response_bytes = match read_result.join() {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(message)) => return Err(message),
        Err(_) => return Err("renderer reader panicked".into()),
    };
    let status = child.wait().map_err(|error| format!("wait: {error}"))?;
    if *timed_out.lock().unwrap() {
        return Err(format!(
            "renderer timed out after {} ms",
            RENDER_TIMEOUT.as_millis()
        ));
    }
    if !status.success() {
        return Err(format!("renderer exited with {status}"));
    }
    if response_bytes.len() > IPC_MAX_RESPONSE_BYTES {
        return Err(format!("response exceeds {IPC_MAX_RESPONSE_BYTES} bytes"));
    }
    let response =
        RenderResponse::parse(&response_bytes).map_err(|error: IpcError| error.to_string())?;
    Ok(response)
}
