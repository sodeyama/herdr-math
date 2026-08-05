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
use tmath_core::ipc::{RenderOptions as IpcRenderOptions, RenderResponse, IPC_MAX_REQUEST_BYTES};
use tmath_core::placement::{
    decode_png, emit_placed_block_cursor, CellSize, PlacementError, PlacementLimits,
    PlacementTracker,
};
use tmath_core::terminal::{StdioTty, Terminal, Tty};

use crate::render::{render_document_text, renderer_worker_path};

mod agent_allowlist;
mod agent_viewer;
mod agent_watcher;
mod config;
mod layout;
mod native_render;
mod native_stream;
mod native_watch;
mod render;
mod scroll_region;
mod terminal_output;
mod transcript_adapter;
mod viewer_viewport;

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
        "watch" => watch(&args[1..]),
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
         USAGE:\n  tmath render [OPTIONS] <file | ->\n  tmath watch [OPTIONS] <file>\n  tmath agent [OPTIONS]\n  tmath agent-viewer <socket-path>\n  tmath agent-enable [<dir>]\n  tmath agent-disable [<dir>]\n  tmath agent-allowed [<dir>]\n  tmath diagnose\n  tmath --help\n  tmath --version\n\
         \n\
         OPTIONS (render):\n  --content-width <px>  Render width in pixels (overrides auto-fit; default 480 without a terminal)\n  --font-size <px>      Base font size in pixels (overrides auto-fit; default 14 without a terminal)\n\
  --engine <engine>     Renderer: native or node (default native; node is deprecated)\n\
         \n\
         OPTIONS (watch):\n  --content-width <px>  Render width in pixels (overrides auto-fit; default 480 without a terminal)\n  --font-size <px>      Base font size in pixels (overrides auto-fit; default 14 without a terminal)\n\
  --engine <engine>     Renderer: native only (default native)\n\
  --poll-ms <ms>        Fallback poll interval when native watching fails (default 250)\n\
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
         `tmath watch` monitors the file's parent directory and updates only\n\
         changed blocks. Ctrl-C exits; non-terminal mode also exits on SIGTERM.\n\
         \n\
         With the default `native` engine and a connected terminal, content width, font\n\
         size, and device pixel ratio are auto-fit to the terminal's measured\n\
         cell size and pane width so the image fits the pane and its text size\n\
         matches the surrounding terminal text. Precedence: an explicit\n\
         `--content-width`/`--font-size` value always wins; otherwise the\n\
         auto-fit value applies when a terminal is connected; otherwise the\n\
         fixed defaults above apply. `--engine node` and a non-terminal\n\
         destination always use the fixed defaults (plus any explicit\n\
         override). `tmath agent-viewer` always auto-fits its pane; it has no\n\
         CLI override.\n\
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RenderEngine {
    Node,
    Native,
}

/// Parsed render arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RenderArgs {
    input: String,
    content_width: Option<u32>,
    font_size: Option<u32>,
    engine: RenderEngine,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WatchArgs {
    input: String,
    content_width: Option<u32>,
    font_size: Option<u32>,
    engine: RenderEngine,
    poll_ms: u64,
}

fn parse_render_args(args: &[String]) -> Result<RenderArgs, String> {
    let mut input: Option<String> = None;
    let mut content_width: Option<u32> = None;
    let mut font_size: Option<u32> = None;
    let mut engine = RenderEngine::Native;
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
            "--engine" => {
                let value = args.get(index + 1).ok_or("--engine needs a value")?;
                engine = match value.as_str() {
                    "node" => RenderEngine::Node,
                    "native" => RenderEngine::Native,
                    _ => {
                        return Err(format!(
                            "invalid render engine {value:?}; expected 'node' or 'native'"
                        ))
                    }
                };
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
        engine,
    })
}

fn parse_watch_args(args: &[String]) -> Result<WatchArgs, String> {
    let mut input: Option<String> = None;
    let mut content_width: Option<u32> = None;
    let mut font_size: Option<u32> = None;
    let mut engine = RenderEngine::Native;
    let mut poll_ms = 250u64;
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
            "--engine" => {
                let value = args.get(index + 1).ok_or("--engine needs a value")?;
                engine = match value.as_str() {
                    "node" => RenderEngine::Node,
                    "native" => RenderEngine::Native,
                    _ => return Err(format!("invalid watch engine {value:?}; expected 'native'")),
                };
                index += 2;
            }
            "--poll-ms" => {
                let value = args.get(index + 1).ok_or("--poll-ms needs a value")?;
                poll_ms = value
                    .parse::<u64>()
                    .map_err(|_| format!("invalid poll interval {value:?}"))?;
                if poll_ms == 0 {
                    return Err("--poll-ms must be greater than zero".into());
                }
                index += 2;
            }
            "--help" | "-h" => return Err("use 'tmath --help'".into()),
            other if other.starts_with('-') => {
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
    let input = input.ok_or("missing input; use 'tmath watch <file>'")?;
    Ok(WatchArgs {
        input,
        content_width,
        font_size,
        engine,
        poll_ms,
    })
}

fn render(args: &[String]) -> Result<i32, String> {
    let parsed = parse_render_args(args)?;
    if parsed.engine == RenderEngine::Native && parsed.input == "-" && !io::stdin().is_terminal() {
        return render_native_stream(&parsed);
    }
    let source = read_document(&parsed.input)?;

    let connected = if io::stdout().is_terminal() {
        Some(connect_terminal()?)
    } else {
        None
    };

    match parsed.engine {
        RenderEngine::Node => render_with_node(&parsed, &source, connected),
        RenderEngine::Native => render_with_native(&parsed, &source, connected),
    }
}

fn watch(args: &[String]) -> Result<i32, String> {
    let parsed = parse_watch_args(args)?;
    if parsed.engine == RenderEngine::Node {
        return Err("watch supports only '--engine native'; the node engine is unavailable".into());
    }
    if !io::stdout().is_terminal() && env::var_os("TMATH_WATCH_WORKER").is_none() {
        return exec_watch_supervisor(args);
    }
    let connected = if io::stdout().is_terminal() {
        Some(connect_terminal()?)
    } else {
        None
    };
    native_watch::run(
        std::path::Path::new(&parsed.input),
        parsed.content_width,
        parsed.font_size,
        parsed.poll_ms,
        connected,
    )
}

#[cfg(unix)]
fn exec_watch_supervisor(args: &[String]) -> Result<i32, String> {
    use std::os::unix::process::CommandExt as _;

    const SCRIPT: &str = r#"
bin=$1
shift
"$bin" watch "$@" &
child=$!
trap 'kill -TERM "$child" 2>/dev/null; wait "$child" 2>/dev/null; exit 0' TERM
wait "$child"
status=$?
exit "$status"
"#;

    let executable =
        env::current_exe().map_err(|error| format!("resolve tmath executable: {error}"))?;
    let error = Command::new("sh")
        .args(["-c", SCRIPT, "tmath-watch-supervisor"])
        .arg(executable)
        .args(args)
        .env("TMATH_WATCH_WORKER", "1")
        .exec();
    Err(format!("start watch signal supervisor: {error}"))
}

#[cfg(not(unix))]
fn exec_watch_supervisor(_args: &[String]) -> Result<i32, String> {
    Err("watch signal handling is unavailable on this platform".into())
}

fn render_native_stream(parsed: &RenderArgs) -> Result<i32, String> {
    let connected = if io::stdout().is_terminal() {
        match StdioTty::from_control_terminal() {
            Ok(tty) => Some(connect_terminal_with(tty)?),
            Err(_) => None,
        }
    } else {
        None
    };
    match native_stream::run(parsed.content_width, parsed.font_size, connected) {
        Ok(()) => Ok(0),
        Err(error) => {
            let record = serde_json::to_string(error.safe_record())
                .map_err(|_| "serialize native stream error".to_string())?;
            eprintln!("{record}");
            Ok(1)
        }
    }
}

fn render_with_node(
    parsed: &RenderArgs,
    source: &str,
    connected: Option<(Terminal<StdioTty>, (u32, u32))>,
) -> Result<i32, String> {
    // The node engine is out of scope for terminal-fit auto layout (V3 native
    // paths only); it keeps its own fixed-default behavior and only applies
    // an explicit CLI override or the measured device pixel ratio.
    let mut node_layout = serde_json::Map::new();
    if let Some(width) = parsed.content_width {
        node_layout.insert("contentWidthPx".into(), json!(width));
    }
    if let Some(size) = parsed.font_size {
        node_layout.insert("fontSizePx".into(), json!(size));
    }
    if let Some((_, cell)) = &connected {
        let scale = layout::device_scale_factor(*cell);
        node_layout.insert("deviceScaleFactor".into(), json!(scale));
    }
    let options = (!node_layout.is_empty()).then_some(IpcRenderOptions {
        limits: None,
        layout: Some(serde_json::Value::Object(node_layout)),
    });

    match render_document_text(source, options)? {
        RenderResponse::Success(success) => {
            let png = BASE64
                .decode(success.base64.as_bytes())
                .map_err(|_| "renderer returned invalid base64 PNG".to_string())?;
            match connected {
                Some((terminal, cell)) => place_in_terminal(terminal, cell, &png),
                None => {
                    println!(
                        "ok width={} height={} bytes={} renderer={}",
                        success.width, success.height, success.bytes, success.renderer
                    );
                    Ok(0)
                }
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

fn render_with_native(
    parsed: &RenderArgs,
    source: &str,
    connected: Option<(Terminal<StdioTty>, (u32, u32))>,
) -> Result<i32, String> {
    let fitted = layout::fitted_layout_for_connected(&connected);
    let content_width_pt = layout::resolve_content_width_pt(parsed.content_width, fitted);
    let font_config = config::config_path()
        .map(|path| config::load(&path))
        .unwrap_or_default();
    let (font_size_pt, font_size_source) =
        config::resolve_font_size_pt_with_source(parsed.font_size, &font_config, fitted);
    eprintln!(
        "tmath: font_size source={} value={font_size_pt}",
        font_size_source.label()
    );
    let device_pixel_ratio = layout::resolve_device_pixel_ratio(fitted);
    let cjk_font = config::resolve_cjk_font(&font_config);
    let result = native_render::render_document_native(
        source,
        content_width_pt.round() as u32,
        font_size_pt.round() as u32,
        device_pixel_ratio,
        cjk_font,
    );
    let success = match result {
        Ok(success) => success,
        Err(error) => {
            let record = serde_json::to_string(error.safe_record())
                .map_err(|_| "serialize native renderer error".to_string())?;
            eprintln!("{record}");
            return Ok(1);
        }
    };

    match connected {
        Some((terminal, cell)) => place_in_terminal(terminal, cell, &success.png),
        None => {
            println!(
                "ok width={} height={} bytes={} renderer=native formula_errors={}",
                success.width,
                success.height,
                success.png.len(),
                success.formula_errors
            );
            Ok(0)
        }
    }
}

/// Connects to the real terminal, confirms Kitty graphics support, and
/// measures the cell size, before any rendering happens so the renderer can
/// rasterize at the terminal's actual pixel density.
fn connect_terminal() -> Result<(Terminal<StdioTty>, (u32, u32)), String> {
    connect_terminal_with(StdioTty::default())
}

fn connect_terminal_with(tty: StdioTty) -> Result<(Terminal<StdioTty>, (u32, u32)), String> {
    let mut terminal =
        Terminal::new(tty, 1).map_err(|error| format!("initialize terminal: {error}"))?;
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
            "main.rs:connect_terminal",
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
    Ok((terminal, cell))
}

/// Places a rendered PNG into a real terminal's main buffer as a
/// scrollback-anchored virtual placement, then restores the terminal.
///
/// `terminal` and `cell` come from `connect_terminal`, called before
/// rendering so the renderer could rasterize at the terminal's actual pixel
/// density.
fn place_in_terminal(
    mut terminal: Terminal<StdioTty>,
    cell: (u32, u32),
    png: &[u8],
) -> Result<i32, String> {
    const MAX_PIXELS: u64 = 64 * 1024 * 1024;
    let (width, height, rgba) = decode_png(png, MAX_PIXELS)
        .map_err(|error: PlacementError| format!("decode rendered image: {error}"))?;

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

    println!("native renderer: in-process (default)");

    match renderer_worker_path() {
        Ok(_) => println!("renderer subprocess: available (optional; --engine node)"),
        Err(message) => {
            println!("renderer subprocess: unavailable ({message}; optional for --engine node)")
        }
    }

    match Command::new("node")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
    {
        Ok(status) if status.success() => println!("node: available (optional; --engine node)"),
        _ => println!("node: unavailable (optional; --engine node)"),
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
        assert_eq!(parsed.engine, RenderEngine::Native);

        let parsed = parse_render_args(&args(&[
            "--content-width",
            "800",
            "--font-size",
            "18",
            "--engine",
            "node",
            "-",
        ]))
        .unwrap();
        assert_eq!(parsed.input, "-");
        assert_eq!(parsed.content_width, Some(800));
        assert_eq!(parsed.font_size, Some(18));
        assert_eq!(parsed.engine, RenderEngine::Node);
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
        assert!(
            parse_render_args(&args(&["--engine", "unknown", "-"])).is_err(),
            "unknown engine"
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
        assert!(help.contains("watch"));
        assert!(help.contains("diagnose"));
        assert!(help.contains("agent"));
        assert!(help.contains("agent-viewer"));
        assert!(help.contains("agent-enable"));
        assert!(help.contains("agent-disable"));
        assert!(help.contains("agent-allowed"));
        assert!(help.contains("--content-width"));
        assert!(help.contains("--font-size"));
        assert!(help.contains("--engine"));
        assert!(help.contains("--source-pane"));
        assert!(help.contains("--poll-ms"));
    }

    #[test]
    fn parses_watch_arguments() {
        let parsed = parse_watch_args(&args(&["doc.md"])).unwrap();
        assert_eq!(parsed.input, "doc.md");
        assert_eq!(parsed.engine, RenderEngine::Native);
        assert_eq!(parsed.poll_ms, 250);

        let parsed = parse_watch_args(&args(&[
            "--content-width",
            "800",
            "--font-size",
            "18",
            "--engine",
            "native",
            "--poll-ms",
            "100",
            "doc.md",
        ]))
        .unwrap();
        assert_eq!(parsed.content_width, Some(800));
        assert_eq!(parsed.font_size, Some(18));
        assert_eq!(parsed.poll_ms, 100);
    }

    #[test]
    fn rejects_invalid_watch_arguments() {
        assert!(parse_watch_args(&args(&[])).is_err());
        assert!(parse_watch_args(&args(&["--poll-ms", "0", "doc.md"])).is_err());
        assert!(parse_watch_args(&args(&["--poll-ms", "abc", "doc.md"])).is_err());
        assert!(parse_watch_args(&args(&["--bogus", "doc.md"])).is_err());
    }
}
