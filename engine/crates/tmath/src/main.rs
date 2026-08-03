//! `tmath` — standalone terminal math/document renderer CLI.
//!
//! `tmath render <file | ->` reads a document, forwards it to the one-shot
//! TypeScript renderer subprocess over stdin/stdout, and — when running against
//! a real Kitty-graphics terminal — places the rendered image as a
//! scrollback-anchored placement in the main buffer. When stdout is not a
//! terminal, it reports the bounded response instead. `tmath diagnose` reports
//! local capability status. `tmath agent` watches a tmux pane running a coding
//! agent and shows each finished answer (with its math) in a split viewer pane.

use std::env;
use std::fs::File;
use std::io::{self, IsTerminal as _, Read as _};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use serde_json::json;
use tmath_core::ipc::{RenderOptions, RenderResponse, IPC_MAX_REQUEST_BYTES};
use tmath_core::placement::{
    decode_png, emit_placed_block_cursor, CellSize, PlacementError, PlacementLimits,
    PlacementTracker,
};
use tmath_core::terminal::{StdioTty, Terminal, Tty};

use crate::render::{render_document_text, renderer_worker_path};

mod agent_allowlist;
mod agent_viewer;
mod agent_watcher;
mod render;
mod terminal_output;

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
        "agent" => agent_watcher::run_agent(&args[1..]),
        "agent-viewer" => agent_viewer::run_agent_viewer(&args[1..]),
        "agent-enable" => agent_allowlist::run_enable(&args[1..]),
        "agent-disable" => agent_allowlist::run_disable(&args[1..]),
        "agent-allowed" => agent_allowlist::run_allowed(&args[1..]),
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
         USAGE:\n  tmath render [OPTIONS] <file | ->\n  tmath agent [OPTIONS]\n  tmath agent-viewer <socket-path>\n  tmath agent-enable [<dir>]\n  tmath agent-disable [<dir>]\n  tmath agent-allowed [<dir>]\n  tmath diagnose\n  tmath --help\n  tmath --version\n\
         \n\
         OPTIONS (render):\n  --content-width <px>  Render width in pixels (default 480)\n  --font-size <px>      Base font size in pixels (default 14)\n\
         \n\
         OPTIONS (agent):\n  --source-pane <id>  tmux pane to watch (default: current pane)\n  --percent <p>       Viewer split width in percent (default 35)\n  --wait-ms <ms>      Answer settle debounce (default 600)\n  --poll-ms <ms>      Pane poll interval (default 250)\n  --history <lines>   Scrollback lines to capture (default 500)\n\
         \n\
         OPTIONS (agent-enable / agent-disable / agent-allowed):\n  <dir>  Target directory (default: current directory)\n\
         \n\
         ENVIRONMENT:\n  TMATH_TMUX_TRANSPORT=client-tty|passthrough\n\
                              Select the tmux graphics route (default client-tty)\n\
         \n\
         With `-`, the document is read from stdin. When stdout is a terminal\n\
         with Kitty graphics support, the image is placed in the main buffer so\n\
         it scrolls with the shell scrollback; `q` or Ctrl-C exits.\n\
         \n\
         `tmath agent` runs inside tmux, watches a pane running a coding agent\n\
         (Claude Code, Codex, opencode, and similar), and shows each finished\n\
         answer as Markdown + rendered math in a right-hand viewer pane.\n\
         `tmath agent-viewer` is the helper that renders into that pane.\n\
         \n\
         `tmath agent-enable`/`agent-disable` register or remove a directory\n\
         (and its subdirectories) from the shell auto-watch allowlist;\n\
         `tmath agent-allowed` checks it by exit code (0/1, silent) for the\n\
         installed shell integration.\n",
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

    let response = render_document_text(&source, options)?;
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
    // Inside tmux, capability queries cannot round-trip through passthrough,
    // so graphics support is assumed. Enable the window's passthrough option
    // so the forwarded image reaches the outer terminal; everywhere else the
    // probe stays mandatory and fail-closed.
    if tmath_core::kitty::inside_tmux() {
        let route = terminal_output::selected_route()?;
        if route == terminal_output::Route::TmuxPassthrough && !enable_tmux_passthrough() {
            eprintln!(
                "tmath: tmux passthrough unavailable; run 'tmux set-option -w allow-passthrough on'"
            );
        }
    } else {
        let graphics_supported = terminal
            .probe_graphics_support()
            .map_err(|error| format!("probe graphics: {error}"))?;
        // #region agent log
        terminal_output::debug_log(
            "F,H,I",
            "main.rs:place_in_terminal",
            "direct Kitty graphics probe completed",
            serde_json::json!({"graphicsSupported": graphics_supported}),
        );
        // #endregion
        if !graphics_supported {
            return Err("this terminal reports no Kitty graphics support".into());
        }
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
    // #region agent log
    terminal_output::debug_log(
        "F,I",
        "main.rs:place_in_terminal",
        "computed image and placeholder geometry",
        serde_json::json!({
            "imageWidth": width,
            "imageHeight": height,
            "cellWidth": cell.0,
            "cellHeight": cell.1,
            "placeholderCols": block.cols,
            "placeholderRows": block.rows,
            "imageId": block.image_id
        }),
    );
    // #endregion
    // #region agent log
    terminal_output::debug_log_current(
        "H15,H17,H18,H19",
        "main.rs:place_in_terminal",
        "placing cursor-relative render",
        serde_json::json!({
            "imageWidth": width,
            "imageHeight": height,
            "placementCols": block.cols,
            "placementRows": block.rows,
            "stdinIsTerminal": io::stdin().is_terminal(),
            "tmuxPane": std::env::var("TMUX_PANE").unwrap_or_else(|_| "<unset>".into())
        }),
    );
    // #endregion
    // Inside tmux, `CSI 6n` is answered with the pane-relative cursor, not the
    // outer terminal's, so it cannot tell us whether the *outer* line already
    // starts at column 1; keep the conservative always-advance behavior there.
    // Directly connected, query the real cursor column so a render invoked
    // right after the shell's own newline (e.g. a piped `tmath render -`)
    // does not add a second blank line before the image.
    let already_at_line_start = if tmath_core::kitty::inside_tmux() {
        false
    } else {
        terminal
            .cursor_column()
            .map_err(|error| format!("query cursor position: {error}"))?
            == Some(1)
    };
    let placement = emit_placed_block_cursor(
        block.image_id,
        width,
        height,
        &rgba,
        block.cols,
        block.rows,
        already_at_line_start,
    );
    terminal_output::write_operations(&placement)?;

    // When the document came from the terminal (file argument), enter the
    // interactive scroll loop so the user can scroll with the wheel/keys and
    // exit with `q`. When it came from a pipe (`tmath render -`), the user
    // cannot type into it, so holding the terminal would hang the shell;
    // instead place the scrollback-anchored image and return immediately.
    if io::stdin().is_terminal() {
        run_scroll_loop(terminal.tty_mut()).map_err(|error| format!("input loop: {error}"))?;
    }
    terminal
        .reset()
        .map_err(|error| format!("reset terminal: {error}"))?;
    // #region agent log
    terminal_output::debug_log_current(
        "H18",
        "main.rs:place_in_terminal",
        "placement command completed",
        serde_json::json!({
            "pipedInput": !io::stdin().is_terminal(),
            "imageId": block.image_id
        }),
    );
    // #endregion
    println!();
    Ok(0)
}

/// Reads terminal input through the bounded decoder until the user presses
/// `q` or `Ctrl-C`, feeding scroll events into the driver. Input comes from
/// the control device (the real terminal even when stdin carried the piped
/// document). `Ctrl-C` is consumed so it never reaches the shell; `q` exits
/// normally. Either way the caller resets the terminal.
fn run_scroll_loop(tty: &mut StdioTty) -> std::io::Result<()> {
    use tmath_core::input::InputDecoder;
    use tmath_core::scroll_driver::{is_exit_signal, ScrollDriver};

    let mut decoder = InputDecoder::new();
    let mut driver = ScrollDriver::new(1024.0);
    let start = Instant::now();
    let mut chunk = [0u8; 256];
    loop {
        let n = match tty.read(&mut chunk) {
            Ok(0) => return Ok(()),
            Ok(n) => n,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::NotConnected => return Ok(()),
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

/// Best-effort enable of tmux passthrough for the current window, so an image
/// carrying tmux DCS sequences is forwarded to the outer terminal. Returns
/// whether tmux reported success.
pub(crate) fn enable_tmux_passthrough() -> bool {
    Command::new("tmux")
        .args(["set-option", "-w", "allow-passthrough", "on"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
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

    for line in terminal_output::tmux_diagnostics() {
        println!("{line}");
    }
    if tmath_core::kitty::inside_tmux() && terminal_output::selected_route().is_err() {
        problems += 1;
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
    if tmath_core::kitty::inside_tmux() || !io::stdin().is_terminal() || !io::stdout().is_terminal()
    {
        return None;
    }
    let mut terminal = Terminal::new(StdioTty::default(), 1).ok()?;
    let result = terminal.probe_graphics_support().ok();
    let _ = terminal.reset();
    result
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
        assert!(help.contains("agent"));
        assert!(help.contains("agent-viewer"));
        assert!(help.contains("agent-enable"));
        assert!(help.contains("agent-disable"));
        assert!(help.contains("agent-allowed"));
        assert!(help.contains("--content-width"));
        assert!(help.contains("--font-size"));
        assert!(help.contains("--source-pane"));
    }
}
