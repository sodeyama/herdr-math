//! Selection of the terminal graphics route.
//!
//! Pane-local bytes always go to stdout. Kitty APC commands normally use
//! direct stdout or stable tmux DCS passthrough. cmux can opt into a
//! graphics-only write to the visible tmux client's tty, bypassing its
//! currently unreliable DCS relay while leaving the placeholder grid in tmux.

use std::env;
use std::fs::{File, OpenOptions};
use std::io::{self, IsTerminal, Write as _};
use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _};
use std::process::{Command, Stdio};

use tmath_core::placement::TerminalOp;

const TRANSPORT_ENV: &str = "TMATH_TMUX_TRANSPORT";

const REFUSAL_NO_CLIENT: &str =
    "tmux has no attached client; graphics need an attached Kitty-capable terminal";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OuterTerminal {
    Verified,
    Unverified(String),
    NoClient,
}

const REFUSAL_PIPED_STDIN: &str =
    "skipped Kitty graphics on stdout for piped input; use `tmath agent` and the viewer pane";
const REFUSAL_EMBEDDED_TERMINAL: &str = "skipped Kitty graphics in the embedded coding-agent terminal; use `tmath agent` and the viewer pane";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Route {
    Direct,
    TmuxPassthrough,
    TmuxClientTty,
}

impl Route {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::TmuxPassthrough => "tmux-passthrough",
            Self::TmuxClientTty => "tmux-client-tty",
        }
    }
}

pub(crate) fn selected_route() -> Result<Route, String> {
    selected_route_detailed().map_err(|(_, message)| message)
}

pub(crate) fn selected_route_detailed() -> Result<Route, (OuterTerminal, String)> {
    if !tmath_core::kitty::inside_tmux() {
        // #region agent log
        debug_log(
            "E,H",
            "terminal_output.rs:selected_route",
            "selected direct graphics route",
            serde_json::json!({
                "tmuxPresent": false,
                "term": env::var("TERM").unwrap_or_else(|_| "<unset>".into()),
                "termProgram": env::var("TERM_PROGRAM").unwrap_or_else(|_| "<unset>".into()),
                "termProgramVersion": env::var("TERM_PROGRAM_VERSION").unwrap_or_else(|_| "<unset>".into())
            }),
        );
        // #endregion
        return Ok(Route::Direct);
    }
    let transport_env = env::var(TRANSPORT_ENV).ok();
    let outer = outer_terminal();
    // #region agent log
    debug_log(
        "A,B",
        "terminal_output.rs:selected_route",
        "selecting tmux graphics route",
        serde_json::json!({
            "transportEnv": transport_env.as_deref().unwrap_or("<unset>"),
            "knownOuter": matches!(outer, OuterTerminal::Verified),
            "outerTerminal": format!("{outer:?}"),
            "clientTermname": tmux_value(&["display-message", "-p", "#{client_termname}"]).unwrap_or_else(|| "unknown".into()),
            "allowPassthrough": tmux_value(&["show-options", "-w", "-v", "allow-passthrough"]).unwrap_or_else(|| "unknown".into())
        }),
    );
    // #endregion
    // An explicit TMATH_TMUX_TRANSPORT value is a user assertion that the
    // outer terminal renders Kitty graphics (needed when the client tty is
    // owned by a relay such as a terminal-session daemon, where neither the
    // advertised termname nor the process ancestry can reach the real
    // terminal). Without it, stay fail-closed on unverified outers.
    if transport_env.is_none() {
        if let Some(message) = outer_terminal_refusal(&outer) {
            return Err((outer, message));
        }
    }
    let route = route_from_transport_env(transport_env.as_deref());
    // #region agent log
    debug_log(
        "A",
        "terminal_output.rs:selected_route",
        "tmux graphics route selected",
        serde_json::json!({"route": route.as_ref().map(|value| value.label()).unwrap_or("error")}),
    );
    // #endregion
    route.map_err(|message| (OuterTerminal::Verified, message))
}

/// Returns a refusal reason when Kitty graphics would be written to stdout in
/// a context where the host UI is likely to show the raw APC payload as text
/// (embedded coding-agent terminals and tmux passthrough from a piped render).
pub(crate) fn stdout_graphics_refusal(route: Route) -> Option<&'static str> {
    if !matches!(route, Route::Direct | Route::TmuxPassthrough) {
        return None;
    }
    if embedded_coding_terminal() && !tmath_core::kitty::inside_tmux() {
        return Some(REFUSAL_EMBEDDED_TERMINAL);
    }
    if !io::stdin().is_terminal() && route == Route::TmuxPassthrough {
        return Some(REFUSAL_PIPED_STDIN);
    }
    None
}

fn embedded_coding_terminal() -> bool {
    matches!(
        env::var("TERM_PROGRAM")
            .map(|value| value.to_ascii_lowercase())
            .as_deref(),
        Ok("vscode") | Ok("cursor") | Ok("code")
    )
}

fn route_from_transport_env(value: Option<&str>) -> Result<Route, String> {
    match value {
        Some("passthrough") => Ok(Route::TmuxPassthrough),
        Some("client-tty") => Ok(Route::TmuxClientTty),
        Some(value) => Err(format!(
            "{TRANSPORT_ENV} must be 'passthrough' or 'client-tty', got {value:?}"
        )),
        // A direct write of graphics-only APCs to the attached client's tty
        // avoids terminal-specific DCS relay bugs in both Ghostty and cmux.
        // Pane-local bytes still go through tmux on stdout.
        None => Ok(Route::TmuxClientTty),
    }
}

fn format_graphics_route_line(route: Result<Route, String>) -> String {
    match route {
        Ok(route) => format!("tmux graphics route: {}", route.label()),
        Err(message) => format!("tmux graphics route: unavailable ({message})"),
    }
}

fn format_transport_env_line(value: Option<&str>) -> String {
    let display = value.unwrap_or("<unset>");
    format!("tmux transport env: {display}")
}

fn tmux_attached_client_count() -> String {
    tmux_lines(&["list-clients", "-F", "x"])
        .map(|lines| lines.len().to_string())
        .unwrap_or_else(|| "unknown".into())
}

pub(crate) fn tmux_diagnostics() -> Vec<String> {
    if !tmath_core::kitty::inside_tmux() {
        return Vec::new();
    }
    let version =
        tmux_value(&["display-message", "-p", "#{version}"]).unwrap_or_else(|| "unknown".into());
    let attached_clients = tmux_attached_client_count();
    let termname = tmux_value(&["display-message", "-p", "#{client_termname}"])
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "unknown".into());
    let passthrough = tmux_value(&["show-options", "-w", "-v", "allow-passthrough"])
        .unwrap_or_else(|| "unknown".into());
    let transport = env::var(TRANSPORT_ENV).ok();
    vec![
        format!("tmux: {version}"),
        format!("tmux attached clients: {attached_clients}"),
        format!("tmux client terminal: {termname}"),
        format!("tmux allow-passthrough: {passthrough}"),
        format_transport_env_line(transport.as_deref()),
        format_graphics_route_line(selected_route()),
    ]
}

pub(crate) fn write_operations(operations: &[TerminalOp]) -> Result<(), String> {
    let route = selected_route()?;
    if operations
        .iter()
        .any(|operation| matches!(operation, TerminalOp::Graphics(_)))
    {
        if let Some(reason) = stdout_graphics_refusal(route) {
            return Err(reason.into());
        }
    }
    let local_count = operations
        .iter()
        .filter(|operation| matches!(operation, TerminalOp::Local(_)))
        .count();
    let graphics: Vec<&[u8]> = operations
        .iter()
        .filter_map(|operation| match operation {
            TerminalOp::Graphics(bytes) => Some(bytes.as_slice()),
            TerminalOp::Local(_) => None,
        })
        .collect();
    // #region agent log
    debug_log(
        "C,D",
        "terminal_output.rs:write_operations",
        "writing structured terminal operations",
        serde_json::json!({
            "route": route.label(),
            "operationCount": operations.len(),
            "localCount": local_count,
            "graphicsCount": graphics.len(),
            "graphicsBytes": graphics.iter().map(|bytes| bytes.len()).sum::<usize>(),
            "allGraphicsAreApc": graphics.iter().all(|bytes| bytes.starts_with(b"\x1b_G") && bytes.ends_with(b"\x1b\\")),
            "embeddedEscapes": graphics.iter().map(|bytes| bytes.iter().filter(|byte| **byte == 0x1b).count()).sum::<usize>()
        }),
    );
    // #endregion
    // #region agent log
    debug_log_current(
        "H15,H16,H17,H18,H19",
        "terminal_output.rs:write_operations",
        "preparing terminal operation streams",
        serde_json::json!({
            "route": route.label(),
            "tmuxPane": env::var("TMUX_PANE").unwrap_or_else(|_| "<unset>".into()),
            "operationKinds": operations.iter().map(|operation| match operation {
                TerminalOp::Local(_) => "local",
                TerminalOp::Graphics(_) => "graphics"
            }).collect::<Vec<_>>(),
            "localBytes": operations.iter().filter_map(|operation| match operation {
                TerminalOp::Local(bytes) => Some(bytes.len()),
                TerminalOp::Graphics(_) => None
            }).sum::<usize>(),
            "graphicsBytes": graphics.iter().map(|bytes| bytes.len()).sum::<usize>()
        }),
    );
    // #endregion
    let placeholder_cells = operations
        .iter()
        .filter_map(|operation| match operation {
            TerminalOp::Local(bytes) => Some(bytes.as_slice()),
            TerminalOp::Graphics(_) => None,
        })
        .map(|bytes| {
            bytes
                .windows(4)
                .filter(|window| *window == [0xf4, 0x8e, 0xbb, 0xae])
                .count()
        })
        .sum::<usize>();
    // #region agent log
    debug_log(
        "F,I",
        "terminal_output.rs:write_operations",
        "validated virtual placement pairing",
        serde_json::json!({
            "placeholderCells": placeholder_cells,
            "graphicsHaveVirtualPlacement": graphics.iter().all(|bytes| bytes.windows(4).any(|window| window == b"U=1,")),
            "graphicsHaveImageId": graphics.iter().all(|bytes| bytes.windows(2).any(|window| window == b"i="))
        }),
    );
    // #endregion
    // #region agent log
    debug_log(
        "K",
        "terminal_output.rs:write_operations",
        "classified Kitty graphics actions",
        serde_json::json!({
            "transmitActions": graphics.iter().filter(|bytes| bytes.windows(4).any(|window| window == b"Ga=T")).count(),
            "placementActions": graphics.iter().filter(|bytes| bytes.windows(4).any(|window| window == b"Ga=p")).count(),
            "unicodePlacementCommands": graphics.iter().filter(|bytes| bytes.windows(4).any(|window| window == b"U=1,")).count()
        }),
    );
    // #endregion
    let mut stdout = io::stdout().lock();
    match route {
        Route::Direct => write_same_stream(&mut stdout, operations, false),
        Route::TmuxPassthrough => write_same_stream(&mut stdout, operations, true),
        Route::TmuxClientTty => {
            let mut graphics = open_client_tty()?;
            let pane = env::var("TMUX_PANE").unwrap_or_default();
            let cursor_x = tmux_int(&pane, "#{cursor_x}");
            let cursor_y = tmux_int(&pane, "#{cursor_y}");
            let pane_left = tmux_int(&pane, "#{pane_left}");
            let pane_top = tmux_int(&pane, "#{pane_top}");
            let outer_cursor_col = if pane_left >= 0 && cursor_x >= 0 {
                pane_left + cursor_x + 1
            } else {
                -1
            };
            let outer_cursor_row = if pane_top >= 0 && cursor_y >= 0 {
                pane_top + cursor_y + 1
            } else {
                -1
            };
            // #region agent log
            debug_log_current(
                "H15,H17,H19",
                "terminal_output.rs:client_tty_write",
                "writing split local and graphics streams",
                serde_json::json!({
                    "clientTtyAvailable": true,
                    "paneActive": pane_value(&pane, "#{pane_active}"),
                    "paneLeft": pane_left,
                    "paneTop": pane_top,
                    "cursorX": cursor_x,
                    "cursorY": cursor_y,
                    "outerCursorCol": outer_cursor_col,
                    "outerCursorRow": outer_cursor_row
                }),
            );
            // #endregion
            let mut cursor_row = cursor_y;
            let mut cursor_col = cursor_x;
            let mut cursor_synced = false;
            for operation in operations {
                match operation {
                    TerminalOp::Local(bytes) => {
                        // Pane-local bytes move tmux's pane cursor; the outer
                        // terminal only follows once tmux redraws. Track the
                        // expected pane-relative cursor so the graphics APC can
                        // be anchored at the matching outer terminal cell.
                        cursor_after_local(bytes, &mut cursor_row, &mut cursor_col);
                        stdout
                            .write_all(bytes)
                            .map_err(|error| format!("write pane output: {error}"))?;
                    }
                    TerminalOp::Graphics(apc) => {
                        if !cursor_synced {
                            // Position the outer terminal cursor at the cell
                            // where tmux will draw the placeholder grid, so a
                            // `U=1` placement lands on the right cells even if
                            // tmux has not redrawn its pane yet.
                            let target_row = if pane_top >= 0 && cursor_row >= 0 {
                                pane_top + cursor_row + 1
                            } else {
                                -1
                            };
                            let target_col = if pane_left >= 0 && cursor_col >= 0 {
                                pane_left + cursor_col + 1
                            } else {
                                -1
                            };
                            // #region agent log
                            debug_log_current(
                                "H17",
                                "terminal_output.rs:client_tty_write",
                                "syncing outer cursor before graphics APC",
                                serde_json::json!({
                                    "paneCursorCol": cursor_col,
                                    "paneCursorRow": cursor_row,
                                    "outerCursorCol": target_col,
                                    "outerCursorRow": target_row
                                }),
                            );
                            // #endregion
                            if target_row >= 1 && target_col >= 1 {
                                graphics
                                    .write_all(
                                        format!("\x1b[{target_row};{target_col}H").as_bytes(),
                                    )
                                    .map_err(|error| {
                                        format!("write client tty cursor sync: {error}")
                                    })?;
                            }
                            cursor_synced = true;
                        }
                        graphics
                            .write_all(apc)
                            .map_err(|error| format!("write client tty graphics: {error}"))?;
                        graphics
                            .flush()
                            .map_err(|error| format!("flush client tty graphics: {error}"))?;
                    }
                }
            }
            stdout
                .flush()
                .map_err(|error| format!("flush pane output: {error}"))
        }
    }
}

fn write_same_stream(
    writer: &mut impl io::Write,
    operations: &[TerminalOp],
    tmux_passthrough: bool,
) -> Result<(), String> {
    // #region agent log
    debug_log(
        "C",
        "terminal_output.rs:write_same_stream",
        "writing graphics through stdout stream",
        serde_json::json!({"tmuxPassthrough": tmux_passthrough}),
    );
    // #endregion
    tmath_core::placement::write_terminal_ops(writer, operations, tmux_passthrough)
        .map_err(|error| format!("write terminal output: {error}"))?;
    writer
        .flush()
        .map_err(|error| format!("flush terminal output: {error}"))
}

fn is_absent_or_empty(value: Option<&str>) -> bool {
    value.map(|value| value.is_empty()).unwrap_or(true)
}

fn classify_outer_terminal(
    client_termname: Option<&str>,
    client_tty_path: Option<&str>,
    tty_owner_is_known: bool,
) -> OuterTerminal {
    if is_absent_or_empty(client_termname) && is_absent_or_empty(client_tty_path) {
        return OuterTerminal::NoClient;
    }
    if tty_owner_is_known {
        return OuterTerminal::Verified;
    }
    if let Some(name) = client_termname.filter(|name| !name.is_empty()) {
        let lower = name.to_ascii_lowercase();
        if lower.contains("ghostty") || lower.contains("kitty") || lower.contains("wezterm") {
            return OuterTerminal::Verified;
        }
        return OuterTerminal::Unverified(name.to_string());
    }
    OuterTerminal::Unverified("unknown".into())
}

fn outer_terminal_refusal(outer: &OuterTerminal) -> Option<String> {
    match outer {
        OuterTerminal::NoClient => Some(REFUSAL_NO_CLIENT.into()),
        OuterTerminal::Unverified(name) => Some(format!(
            "tmux outer terminal '{name}' is not a verified Kitty target; set TMATH_TMUX_TRANSPORT=client-tty or passthrough to override"
        )),
        OuterTerminal::Verified => None,
    }
}

fn outer_terminal() -> OuterTerminal {
    let termname = tmux_value(&["display-message", "-p", "#{client_termname}"])
        .filter(|name| !name.is_empty());
    let tty_path = query_client_tty_path();
    let owner_known = tty_path
        .as_deref()
        .map(client_owner_is_known)
        .unwrap_or(false);
    classify_outer_terminal(termname.as_deref(), tty_path.as_deref(), owner_known)
}

fn tmux_value(args: &[&str]) -> Option<String> {
    let output = Command::new("tmux")
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn tmux_lines(args: &[&str]) -> Option<Vec<String>> {
    let output = Command::new("tmux")
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output.status.success().then(|| {
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect()
    })
}

fn tmux_int(pane: &str, format: &str) -> i64 {
    tmux_value(&["display-message", "-p", "-t", pane, format])
        .and_then(|value| value.parse().ok())
        .unwrap_or(-1)
}

fn pane_value(pane: &str, format: &str) -> String {
    tmux_value(&["display-message", "-p", "-t", pane, format]).unwrap_or_else(|| "unknown".into())
}

/// Advances a pane-relative (0-based) cursor position by the effect of a
/// pane-local byte sequence, for the narrow set of cursor movements the
/// placement emitter produces (`\r`, `\n`, `\r\n`).
fn cursor_after_local(bytes: &[u8], row: &mut i64, col: &mut i64) {
    for &byte in bytes {
        match byte {
            b'\r' => *col = 0,
            b'\n' => *row += 1,
            _ => {}
        }
    }
}

fn open_client_tty() -> Result<File, String> {
    let path = query_client_tty_path().ok_or("tmux did not report a client tty")?;
    // #region agent log
    debug_log(
        "B",
        "terminal_output.rs:open_client_tty",
        "validating tmux client tty",
        serde_json::json!({
            "hasDevTtyPrefix": path.starts_with("/dev/tty"),
            "ownerKnown": client_owner_is_known(&path)
        }),
    );
    // #endregion
    if !path.starts_with("/dev/tty")
        || path
            .bytes()
            .any(|byte| matches!(byte, b'\0' | b'\n' | b'\r'))
    {
        return Err("tmux reported an invalid client tty".into());
    }
    let before =
        std::fs::symlink_metadata(&path).map_err(|error| format!("inspect client tty: {error}"))?;
    if before.file_type().is_symlink()
        || !before.file_type().is_char_device()
        || before.uid() != rustix::process::geteuid().as_raw()
    {
        return Err("tmux client tty failed ownership or device validation".into());
    }
    let file = OpenOptions::new()
        .write(true)
        .open(&path)
        .map_err(|error| format!("open client tty: {error}"))?;
    let after = file
        .metadata()
        .map_err(|error| format!("inspect opened client tty: {error}"))?;
    if !after.file_type().is_char_device()
        || after.uid() != before.uid()
        || after.dev() != before.dev()
        || after.ino() != before.ino()
    {
        return Err("opened client tty changed during validation".into());
    }
    // #region agent log
    debug_log(
        "B,D",
        "terminal_output.rs:open_client_tty",
        "tmux client tty opened",
        serde_json::json!({
            "characterDevice": after.file_type().is_char_device(),
            "sameDevice": after.dev() == before.dev(),
            "sameInode": after.ino() == before.ino()
        }),
    );
    // #endregion
    Ok(file)
}

fn query_client_tty_path() -> Option<String> {
    let pane = env::var("TMUX_PANE").ok()?;
    tmux_value(&["display-message", "-p", "-t", &pane, "#{client_tty}"])
        .filter(|path| !path.is_empty())
}

/// macOS tmux can advertise `xterm-256color` for cmux. In that case inspect
/// only the fixed process ancestry of the validated client tty; no environment
/// dump or user-controlled command is executed.
fn client_owner_is_known(path: &str) -> bool {
    let Some(name) = path.strip_prefix("/dev/") else {
        return false;
    };
    if !name.starts_with("tty")
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return false;
    }
    let output = Command::new("ps")
        .args(["-o", "pid=,ppid=", "-t", name])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    let Ok(output) = output else {
        return false;
    };
    let pairs: Vec<(u32, u32)> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            Some((fields.next()?.parse().ok()?, fields.next()?.parse().ok()?))
        })
        .collect();
    let pids: std::collections::HashSet<u32> = pairs.iter().map(|(pid, _)| *pid).collect();
    pairs
        .iter()
        .filter(|(_, parent)| !pids.contains(parent))
        .any(|(_, parent)| known_process(*parent))
}

fn known_process(pid: u32) -> bool {
    let output = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "command="])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    let Ok(output) = output else {
        return false;
    };
    let command = String::from_utf8_lossy(&output.stdout).to_ascii_lowercase();
    command.contains("/ghostty")
        || command.contains("/cmux")
        || command.contains("/wezterm")
        || command.contains("/kitty")
}

pub(crate) fn debug_log(
    hypothesis_id: &str,
    location: &str,
    message: &str,
    data: serde_json::Value,
) {
    use std::time::{SystemTime, UNIX_EPOCH};

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    let payload = serde_json::json!({
        "sessionId": "c276a1",
        "runId": "pre-fix",
        "hypothesisId": hypothesis_id,
        "location": location,
        "message": message,
        "data": data,
        "timestamp": timestamp
    });
    write_debug_line(&payload);
}

pub(crate) fn debug_log_current(
    hypothesis_id: &str,
    location: &str,
    message: &str,
    data: serde_json::Value,
) {
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
    write_debug_line(&payload);
}

/// Appends one diagnostic line to the file named by `TMATH_DEBUG_LOG` when that
/// environment variable is set. The write is disabled by default and never uses
/// an absolute path from the source, keeping logs out of the repository.
pub(crate) fn write_debug_line(payload: &serde_json::Value) {
    let Ok(path) = env::var("TMATH_DEBUG_LOG") else {
        return;
    };
    if path.is_empty() {
        return;
    }
    if let Some(parent) = std::path::Path::new(&path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(file, "{payload}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_labels_are_stable_diagnostics() {
        assert_eq!(Route::Direct.label(), "direct");
        assert_eq!(Route::TmuxPassthrough.label(), "tmux-passthrough");
        assert_eq!(Route::TmuxClientTty.label(), "tmux-client-tty");
    }

    #[test]
    fn transport_env_selects_the_tmux_route() {
        assert_eq!(
            route_from_transport_env(None).unwrap(),
            Route::TmuxClientTty
        );
        assert_eq!(
            route_from_transport_env(Some("client-tty")).unwrap(),
            Route::TmuxClientTty
        );
        assert_eq!(
            route_from_transport_env(Some("passthrough")).unwrap(),
            Route::TmuxPassthrough
        );
        assert!(route_from_transport_env(Some("dcs")).is_err());
    }

    #[test]
    fn cursor_after_local_advances_carriage_return_and_newline() {
        let mut row = 4i64;
        let mut col = 6i64;
        cursor_after_local(b"\r\n", &mut row, &mut col);
        assert_eq!((row, col), (5, 0));
    }

    #[test]
    fn stdout_graphics_refusal_allows_client_tty_even_with_piped_stdin() {
        assert!(stdout_graphics_refusal(Route::TmuxClientTty).is_none());
    }

    #[test]
    fn classify_outer_terminal_detects_no_client() {
        assert_eq!(
            classify_outer_terminal(None, None, false),
            OuterTerminal::NoClient
        );
        assert_eq!(
            classify_outer_terminal(Some(""), None, false),
            OuterTerminal::NoClient
        );
        assert_eq!(
            classify_outer_terminal(None, Some(""), false),
            OuterTerminal::NoClient
        );
    }

    #[test]
    fn classify_outer_terminal_detects_empty_termname_with_tty() {
        assert_eq!(
            classify_outer_terminal(None, Some("/dev/ttys001"), false),
            OuterTerminal::Unverified("unknown".into())
        );
        assert_eq!(
            classify_outer_terminal(Some(""), Some("/dev/ttys001"), false),
            OuterTerminal::Unverified("unknown".into())
        );
        assert_eq!(
            classify_outer_terminal(None, Some("/dev/ttys001"), true),
            OuterTerminal::Verified
        );
    }

    #[test]
    fn classify_outer_terminal_verifies_known_terminals_case_insensitively() {
        for name in [
            "ghostty",
            "Ghostty",
            "xterm-ghostty",
            "kitty",
            "KITTY",
            "wezterm",
        ] {
            assert_eq!(
                classify_outer_terminal(Some(name), None, false),
                OuterTerminal::Verified,
                "expected {name} to verify"
            );
        }
    }

    #[test]
    fn classify_outer_terminal_marks_unrelated_names_unverified() {
        assert_eq!(
            classify_outer_terminal(Some("xterm-256color"), Some("/dev/ttys001"), false),
            OuterTerminal::Unverified("xterm-256color".into())
        );
    }

    #[test]
    fn format_graphics_route_line_shows_label_or_full_refusal() {
        assert_eq!(
            format_graphics_route_line(Ok(Route::TmuxClientTty)),
            "tmux graphics route: tmux-client-tty"
        );
        assert_eq!(
            format_graphics_route_line(Err(REFUSAL_NO_CLIENT.into())),
            format!("tmux graphics route: unavailable ({REFUSAL_NO_CLIENT})")
        );
        let unverified = "tmux outer terminal 'xterm-256color' is not a verified Kitty target; set TMATH_TMUX_TRANSPORT=client-tty or passthrough to override";
        assert_eq!(
            format_graphics_route_line(Err(unverified.into())),
            format!("tmux graphics route: unavailable ({unverified})")
        );
    }

    #[test]
    fn format_transport_env_line_shows_value_or_unset() {
        assert_eq!(
            format_transport_env_line(None),
            "tmux transport env: <unset>"
        );
        assert_eq!(
            format_transport_env_line(Some("client-tty")),
            "tmux transport env: client-tty"
        );
        assert_eq!(
            format_transport_env_line(Some("passthrough")),
            "tmux transport env: passthrough"
        );
    }

    #[test]
    fn outer_terminal_refusal_messages_match_spec() {
        assert_eq!(
            outer_terminal_refusal(&OuterTerminal::NoClient).unwrap(),
            REFUSAL_NO_CLIENT
        );
        assert_eq!(
            outer_terminal_refusal(&OuterTerminal::Unverified("xterm-256color".into())).unwrap(),
            "tmux outer terminal 'xterm-256color' is not a verified Kitty target; set TMATH_TMUX_TRANSPORT=client-tty or passthrough to override"
        );
        assert_eq!(
            outer_terminal_refusal(&OuterTerminal::Unverified("unknown".into())).unwrap(),
            "tmux outer terminal 'unknown' is not a verified Kitty target; set TMATH_TMUX_TRANSPORT=client-tty or passthrough to override"
        );
        assert!(outer_terminal_refusal(&OuterTerminal::Verified).is_none());
    }

    #[test]
    fn stdout_graphics_refusal_blocks_unsafe_direct_routes() {
        let term_program = env::var("TERM_PROGRAM").ok();
        let tmux = env::var_os("TMUX");
        env::set_var("TERM_PROGRAM", "vscode");
        env::remove_var("TMUX");
        assert!(
            stdout_graphics_refusal(Route::Direct).is_some(),
            "direct stdout graphics must be refused in coding-agent-like environments"
        );
        match term_program {
            Some(value) => env::set_var("TERM_PROGRAM", value),
            None => env::remove_var("TERM_PROGRAM"),
        }
        if let Some(value) = tmux {
            env::set_var("TMUX", value);
        }
    }
}
