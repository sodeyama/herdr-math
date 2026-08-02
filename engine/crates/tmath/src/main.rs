//! `tmath` — standalone terminal math/document renderer CLI.
//!
//! `tmath render <file | ->` reads a document, forwards it to the one-shot
//! TypeScript renderer subprocess over stdin/stdout, and — when running against
//! a real Kitty-graphics terminal — places the rendered image as a
//! scrollback-anchored placement in the main buffer. When stdout is not a
//! terminal, it reports the bounded response instead. `tmath diagnose` reports
//! local capability status.

use std::env;
use std::fs::File;
use std::io::{self, IsTerminal as _, Read as _, Write as _};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use serde_json::json;
use tmath_core::ipc::{
    EitherKind, IpcError, RenderOptions, RenderRequest, RenderResponse, IPC_MAX_REQUEST_BYTES,
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
        eprint!("{}", help_text());
        return Err("missing command; use 'render' or 'diagnose'".into());
    };
    match command.as_str() {
        "render" => render(&args[1..]),
        "diagnose" => diagnose(&args[1..]),
        "--help" | "-h" | "help" => {
            print!("{}", help_text());
            Ok(0)
        }
        "--version" | "-V" | "version" => {
            println!("tmath {}", env!("CARGO_PKG_VERSION"));
            Ok(0)
        }
        other => Err(format!("unknown command {other:?}; use 'render'")),
    }
}

fn help_text() -> String {
    format!(
        "tmath {version} — render Markdown + LaTeX as scrollback-anchored images.\n\
         \n\
         USAGE:\n  tmath render [OPTIONS] <file | ->\n  tmath diagnose\n  tmath --help\n  tmath --version\n\
         \n\
         OPTIONS:\n  --content-width <px>  Render width in pixels (default 480)\n\
         \x20 --font-size <px>      Base font size in pixels (default 14)\n\
         \n\
         With `-`, the document is read from stdin. When stdout is a terminal\n\
         with Kitty graphics support, the image is placed in the main buffer so\n\
         it scrolls with the shell scrollback; `q` or Ctrl-C exits.\n",
        version = env!("CARGO_PKG_VERSION")
    )
}

/// Parsed render arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RenderArgs {
    input: String,
    content_width: Option<u32>,
    font_size: Option<u32>,
}

fn parse_render_args(args: &[String]) -> Result<RenderArgs, String> {
    let mut input: Option<String> = None;
    let mut content_width: Option<u32> = None;
    let mut font_size: Option<u32> = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--content-width" => {
                let value = args.get(index + 1).ok_or("--content-width needs a value")?;
                content_width = Some(
                    value
                        .parse()
                        .map_err(|_| format!("invalid content width {value:?}"))?,
                );
                index += 2;
            }
            "--font-size" => {
                let value = args.get(index + 1).ok_or("--font-size needs a value")?;
                font_size = Some(
                    value
                        .parse()
                        .map_err(|_| format!("invalid font size {value:?}"))?,
                );
                index += 2;
            }
            "--help" | "-h" => return Err("use 'tmath --help'".into()),
            other if other.starts_with('-') && other != "-" => {
                return Err(format!("unknown option {other:?}"));
            }
            other => {
                if input.is_some() {
                    return Err("expected one input path".into());
                }
                input = Some(other.to_string());
                index += 1;
            }
        }
    }
    let input = input.ok_or("missing input; use 'tmath render <file | ->'")?;
    Ok(RenderArgs {
        input,
        content_width,
        font_size,
    })
}

fn render(args: &[String]) -> Result<i32, String> {
    let parsed = parse_render_args(args)?;
    let source = read_document(&parsed.input)?;
    let mut options: Option<RenderOptions> = None;
    if parsed.content_width.is_some() || parsed.font_size.is_some() {
        let mut layout = serde_json::Map::new();
        if let Some(width) = parsed.content_width {
            layout.insert("contentWidthPx".into(), json!(width));
        }
        if let Some(size) = parsed.font_size {
            layout.insert("fontSizePx".into(), json!(size));
        }
        options = Some(RenderOptions {
            limits: None,
            layout: Some(serde_json::Value::Object(layout)),
        });
    }
    let request = RenderRequest {
        protocol: IPC_PROTOCOL.to_string(),
        kind: EitherKind::Document,
        text: Some(source),
        formulas: None,
        options,
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

    run_scroll_loop().map_err(|error| format!("input loop: {error}"))?;
    terminal
        .reset()
        .map_err(|error| format!("reset terminal: {error}"))?;
    println!(
        "placed width={width} height={height} image_id={}",
        block.image_id
    );
    Ok(0)
}

/// Reads raw stdin through the bounded decoder until the user presses `q` or
/// `Ctrl-C`, feeding scroll events into the driver. `Ctrl-C` is consumed so it
/// never reaches the shell; `q` exits normally. Either way the caller resets
/// the terminal.
fn run_scroll_loop() -> std::io::Result<()> {
    use tmath_core::input::InputDecoder;
    use tmath_core::scroll_driver::{is_exit_signal, ScrollDriver};

    let mut decoder = InputDecoder::new();
    let mut driver = ScrollDriver::new(1024.0);
    let start = Instant::now();
    let mut chunk = [0u8; 256];
    loop {
        let mut stdin = io::stdin();
        let n = match stdin.read(&mut chunk) {
            Ok(0) => return Ok(()),
            Ok(n) => n,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        decoder.push(&chunk[..n]);
        while let Some(event) = decoder.next_event() {
            if is_exit_signal(&event) {
                return Ok(());
            }
            let _ = driver.handle(&event, Some(24.0));
            let _ = driver.step(1.0 / 60.0);
        }
        // Bound the total interactive wait so a non-interactive run cannot hang.
        if start.elapsed() > Duration::from_secs(5) {
            return Ok(());
        }
    }
}

fn read_document(path: &str) -> Result<String, String> {
    let mut text = String::new();
    if path == "-" {
        io::stdin()
            .take((IPC_MAX_REQUEST_BYTES + 1) as u64)
            .read_to_string(&mut text)
            .map_err(|error| format!("read stdin: {error}"))?;
    } else {
        File::open(path)
            .map_err(|error| format!("open {path}: {error}"))?
            .take((IPC_MAX_REQUEST_BYTES + 1) as u64)
            .read_to_string(&mut text)
            .map_err(|error| format!("read {path}: {error}"))?;
    }
    if text.len() > IPC_MAX_REQUEST_BYTES {
        return Err(format!("document exceeds {IPC_MAX_REQUEST_BYTES} bytes"));
    }
    Ok(text)
}

/// Reports local capabilities with a stable status per check and a non-zero
/// exit when a required capability is missing.
fn diagnose(args: &[String]) -> Result<i32, String> {
    if !args.is_empty() && args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("usage: tmath diagnose");
        return Ok(0);
    }
    let mut problems = 0u32;

    match renderer_worker_path() {
        Ok(_) => println!("renderer subprocess: available"),
        Err(message) => {
            println!("renderer subprocess: missing ({message})");
            problems += 1;
        }
    }

    match Command::new("node")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
    {
        Ok(status) if status.success() => println!("node: available"),
        _ => {
            println!("node: missing");
            problems += 1;
        }
    }

    if io::stdout().is_terminal() {
        println!("stdout: terminal");
    } else {
        println!("stdout: not a terminal (image transport unavailable here)");
    }

    // Probes that need a real terminal only run when stdout is a tty.
    if io::stdout().is_terminal() && !io::stdin().is_terminal() {
        println!("stdin: not a terminal (input events unavailable)");
    }

    let graphics = probe_graphics_from_tty();
    match graphics {
        Some(true) => println!("kitty graphics: supported"),
        Some(false) => {
            println!("kitty graphics: unsupported");
            problems += 1;
        }
        None => println!("kitty graphics: not probed (no stdin terminal)"),
    }

    if problems == 0 {
        Ok(0)
    } else {
        Err(format!(
            "{problems} required capability/capabilities missing; see above"
        ))
    }
}

/// Runs a Kitty graphics probe against the real tty; `None` when no tty exists.
fn probe_graphics_from_tty() -> Option<bool> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return None;
    }
    let mut terminal = Terminal::new(StdioTty::default(), 1).ok()?;
    let result = terminal.probe_graphics_support().ok();
    let _ = terminal.reset();
    result
}

fn renderer_worker_path() -> Result<PathBuf, String> {
    if let Some(path) = env::var_os("TMATH_RENDER_WORKER") {
        return Ok(PathBuf::from(path));
    }
    // Local checkout convenience: `npm run build` produces
    // `dist/renderer/subprocess.js` in the repository root (relative to CWD),
    // or `../dist/renderer/subprocess.js` relative to the binary when the
    // binary lives in `target/debug/`.
    let candidates = ["dist/renderer/subprocess.js".to_string(), {
        // `target/debug/tmath` → repo root → `dist/renderer/subprocess.js`
        let exe = env::current_exe().unwrap_or_default();
        exe.parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
            .map(|p| {
                p.join("dist/renderer/subprocess.js")
                    .to_string_lossy()
                    .into_owned()
            })
            .unwrap_or_default()
    }];
    for candidate in candidates {
        if !candidate.is_empty() && PathBuf::from(&candidate).is_file() {
            return Ok(PathBuf::from(candidate));
        }
    }
    Err("renderer subprocess not found; build it with `npm ci && npm run build`, or set TMATH_RENDER_WORKER".into())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_render_arguments() {
        let parsed = parse_render_args(&args(&["doc.md"])).unwrap();
        assert_eq!(parsed.input, "doc.md");
        assert_eq!(parsed.content_width, None);

        let parsed =
            parse_render_args(&args(&["--content-width", "800", "--font-size", "18", "-"]))
                .unwrap();
        assert_eq!(parsed.input, "-");
        assert_eq!(parsed.content_width, Some(800));
        assert_eq!(parsed.font_size, Some(18));
    }

    #[test]
    fn rejects_invalid_render_arguments() {
        assert!(parse_render_args(&args(&[])).is_err(), "missing input");
        assert!(
            parse_render_args(&args(&["a.md", "b.md"])).is_err(),
            "two inputs"
        );
        assert!(
            parse_render_args(&args(&["--content-width", "abc", "-"])).is_err(),
            "bad width"
        );
        assert!(
            parse_render_args(&args(&["--bogus", "-"])).is_err(),
            "unknown option"
        );
    }

    #[test]
    fn progress_preserves_options() {
        let parsed = parse_render_args(&args(&["--font-size", "20", "doc.md"])).unwrap();
        assert_eq!(parsed.font_size, Some(20));
        assert_eq!(parsed.input, "doc.md");
    }

    #[test]
    fn help_mentions_commands_and_options() {
        let help = help_text();
        assert!(help.contains("render"));
        assert!(help.contains("diagnose"));
        assert!(help.contains("--content-width"));
        assert!(help.contains("--font-size"));
    }

    #[test]
    fn renderer_worker_path_honors_the_environment_variable() {
        // The workspace denies unsafe code, so this uses only the safe std env
        // API. No other test reads TMATH_RENDER_WORKER, so the temporary value
        // cannot race with parallel test threads here.
        std::env::set_var("TMATH_RENDER_WORKER", "/tmp/example/subprocess.js");
        let path = renderer_worker_path().unwrap();
        assert_eq!(path, PathBuf::from("/tmp/example/subprocess.js"));
        std::env::remove_var("TMATH_RENDER_WORKER");
    }
}
