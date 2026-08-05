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
    let known_outer = known_outer_terminal();
    // #region agent log
    debug_log(
        "A,B",
        "terminal_output.rs:selected_route",
        "selecting tmux graphics route",
        serde_json::json!({
            "transportEnv": transport_env.as_deref().unwrap_or("<unset>"),
            "knownOuter": known_outer,
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
    if transport_env.is_none() && !known_outer {
        return Err(
            "tmux outer terminal is not a verified Kitty target; refusing placeholder output \
             (set TMATH_TMUX_TRANSPORT=client-tty or passthrough to override)"
                .into(),
        );
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
    route
}

/// Returns a refusal reason when Kitty graphics would be written to stdout in
/// a context where the host UI is likely to show the raw APC payload as text
/// (coding-agent tool shells, piped `tmath render -`, and similar).
pub(crate) fn stdout_graphics_refusal(route: Route) -> Option<&'static str> {
    if !matches!(route, Route::Direct | Route::TmuxPassthrough) {
        return None;
    }
    if !io::stdin().is_terminal() {
        return Some(REFUSAL_PIPED_STDIN);
    }
    if embedded_coding_terminal() && !tmath_core::kitty::inside_tmux() {
        return Some(REFUSAL_EMBEDDED_TERMINAL);
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

pub(crate) fn tmux_diagnostics() -> Vec<String> {
    if !tmath_core::kitty::inside_tmux() {
        return Vec::new();
    }
    let version =
        tmux_value(&["display-message", "-p", "#{version}"]).unwrap_or_else(|| "unknown".into());
    let termname = tmux_value(&["display-message", "-p", "#{client_termname}"])
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "unknown".into());
    let passthrough = tmux_value(&["show-options", "-w", "-v", "allow-passthrough"])
        .unwrap_or_else(|| "unknown".into());
    let route = selected_route().map(Route::label).unwrap_or("unavailable");
    vec![
        format!("tmux: {version}"),
        format!("tmux client terminal: {termname}"),
        format!("tmux allow-passthrough: {passthrough}"),
        format!("tmux graphics route: {route}"),
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

fn known_outer_terminal() -> bool {
    let advertised = tmux_value(&["display-message", "-p", "#{client_termname}"])
        .map(|name| {
            let name = name.to_ascii_lowercase();
            name.contains("ghostty") || name.contains("kitty") || name.contains("wezterm")
        })
        .unwrap_or(false);
    advertised
        || query_client_tty_path()
            .as_deref()
            .map(client_owner_is_known)
            .unwrap_or(false)
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
