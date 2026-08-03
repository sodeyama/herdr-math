//! `tmath agent` — watch a tmux pane for finished coding-agent answers and
//! feed them to a viewer pane as rendered documents.
//!
//! The watcher is intentionally quiet: it writes a one-line banner and bounded
//! status/failure events to stderr only, never answer content. It does not
//! touch the agent's terminal beyond `tmux capture-pane`.

use std::collections::VecDeque;
use std::env;
use std::fs;
use std::io::{self, Read as _, Write as _};
use std::os::unix::net::{UnixListener, UnixStream};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use tmath_core::agent::{
    capture, display_pane, encode_document, encode_quit, find_answer, kill_pane, shell_quote,
    split_viewer, PaneId,
};
use tmath_core::input::InputDecoder;
use tmath_core::scroll_driver::is_exit_signal;

use crate::render::renderer_worker_path;

/// Inactivity (ms) an answer must hold before it is emitted.
const DEFAULT_WAIT_MS: u64 = 600;
/// Ceiling (ms) on holding a pending answer, so a repainting agent never
/// wedges the viewer.
const MAX_HOLD_MS: u64 = 3000;
/// Pane poll interval (ms).
const DEFAULT_POLL_MS: u64 = 250;
/// Scrollback lines included in each capture.
const DEFAULT_HISTORY: u32 = 500;
/// Default viewer split width percent.
const DEFAULT_PERCENT: u32 = 35;
/// Safety cap on a single captured snapshot.
const SNAPSHOT_MAX_BYTES: usize = 4 * 1024 * 1024;

struct WatcherArgs {
    percent: u32,
    wait_ms: u64,
    poll_ms: u64,
    history: u32,
    source: Option<PaneId>,
}

pub(crate) fn run_agent(args: &[String]) -> Result<i32, String> {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!(
            "usage: tmath agent [--source-pane <id>] [--percent <p>] [--wait-ms <ms>]\n\
             \x20                  [--poll-ms <ms>] [--history <lines>]"
        );
        return Ok(0);
    }
    let parsed = parse_agent_args(args)?;
    let _ = renderer_worker_path()?;
    let source = parsed
        .source
        .clone()
        .or_else(|| {
            env::var_os("TMUX_PANE")
                .and_then(|value| value.into_string().ok())
                .and_then(|value| PaneId::new(&value))
        })
        .ok_or("tmath agent requires a tmux session ($TMUX_PANE) or --source-pane")?;

    let socket_path = env::temp_dir().join(format!("tmath-agent-{}.sock", std::process::id()));
    let _ = fs::remove_file(&socket_path);
    let listener = UnixListener::bind(&socket_path)
        .map_err(|error| format!("bind {}: {error}", socket_path.display()))?;
    listener.set_nonblocking(true).ok();

    let exe = env::current_exe().map_err(|error| format!("current exe: {error}"))?;
    let worker = renderer_worker_path()?;
    // tmux starts the viewer pane with the server's environment, so the
    // renderer worker path must be passed explicitly on the command line.
    let viewer_cmd = viewer_command(
        &exe,
        &worker,
        &socket_path,
        env::var("TMATH_TMUX_TRANSPORT").ok().as_deref(),
    );
    let viewer_pane = spawn_viewer_pane(&parsed, &source, &viewer_cmd)?;
    let route = crate::terminal_output::selected_route()?;
    if route == crate::terminal_output::Route::TmuxPassthrough {
        let _ = crate::enable_tmux_passthrough();
    }

    eprintln!(
        "tmath agent: watching {} → {}; q/Ctrl-C to stop",
        source.as_str(),
        viewer_pane.as_str()
    );

    let mut peer: Option<UnixStream> = None;
    let mut baseline = String::new();
    let mut baseline_initialized = false;
    let mut seen_answers = VecDeque::new();
    let mut pending: Option<(String, String, Instant)> = None; // (text, snapshot, since)
    let mut stdin_decoder = InputDecoder::new();

    loop {
        if peer.is_none() {
            peer = accept_peer(&listener);
        }

        if stdin_has_input() {
            let mut chunk = [0u8; 256];
            match io::stdin().read(&mut chunk) {
                Ok(n) if n > 0 => {
                    stdin_decoder.push(&chunk[..n]);
                    if let Some(event) = stdin_decoder.next_event() {
                        if is_exit_signal(&event) {
                            return finish(&mut peer, &viewer_pane);
                        }
                    }
                }
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(format!("read stdin: {error}")),
            }
        }

        // Capture the source pane and detect the newest answer. Stopping is
        // driven by the source pane closing or `q`/Ctrl-C; a missing viewer
        // pane just means documents are dropped until one reconnects.
        if !pane_alive(&source) {
            eprintln!("tmath agent: source pane closed; stopping");
            return finish(&mut peer, &viewer_pane);
        }
        let snapshot = capture_source(&source, parsed.history)?;

        if snapshot.len() > SNAPSHOT_MAX_BYTES {
            eprintln!("tmath agent: captured pane exceeds bound; skipping update");
        } else if !baseline_initialized {
            baseline = snapshot;
            baseline_initialized = true;
            if let Some(answer) = find_answer("", &baseline) {
                remember_answer(&mut seen_answers, answer.text);
            }
            // #region agent log
            debug_log(
                "H2",
                "agent_watcher.rs:initialize_baseline",
                "initialized watcher baseline without emitting",
                serde_json::json!({"baselineBytes": baseline.len()}),
            );
            // #endregion
        } else if snapshot != baseline {
            // #region agent log
            debug_log(
                "H1,H2",
                "agent_watcher.rs:changed_snapshot",
                "source snapshot changed",
                serde_json::json!({
                    "baselineBytes": baseline.len(),
                    "snapshotBytes": snapshot.len(),
                    "pending": pending.is_some()
                }),
            );
            // #endregion
            match find_answer(&baseline, &snapshot) {
                Some(answer) => {
                    // #region agent log
                    debug_log(
                        "H1,H2",
                        "agent_watcher.rs:answer_candidate",
                        "boundary produced answer candidate",
                        serde_json::json!({
                            "answerBytes": answer.text.len(),
                            "baselineBytes": baseline.len(),
                            "snapshotBytes": snapshot.len(),
                            "pendingBytes": pending.as_ref().map(|value| value.0.len())
                        }),
                    );
                    // #endregion
                    if seen_answers.contains(&answer.text) {
                        // #region agent log
                        debug_log(
                            "H12",
                            "agent_watcher.rs:duplicate_answer",
                            "ignored previously observed answer repaint",
                            serde_json::json!({
                                "answerBytes": answer.text.len(),
                                "seenCount": seen_answers.len()
                            }),
                        );
                        // #endregion
                        pending = None;
                        baseline = snapshot;
                    } else {
                        match pending.as_mut() {
                            Some((text, consumed_snapshot, since)) => {
                                // A growing answer restarts the debounce.
                                if *text != answer.text {
                                    *text = answer.text.clone();
                                    *since = Instant::now();
                                }
                                *consumed_snapshot = snapshot.clone();
                            }
                            None => {
                                pending =
                                    Some((answer.text.clone(), snapshot.clone(), Instant::now()));
                            }
                        }
                    }
                }
                None => {
                    // No proven boundary: consume the snapshot, render nothing.
                    pending = None;
                    baseline = snapshot;
                    eprintln!("tmath agent: boundary_failed");
                }
            }
        }

        if let Some((text, snapshot_for_emit, since)) = pending.take() {
            let held = since.elapsed();
            if held >= Duration::from_millis(parsed.wait_ms)
                || held >= Duration::from_millis(MAX_HOLD_MS)
            {
                // #region agent log
                debug_log(
                    "H1,H2",
                    "agent_watcher.rs:emit_document",
                    "emitting settled document",
                    serde_json::json!({
                        "documentBytes": text.len(),
                        "snapshotBytes": snapshot_for_emit.len(),
                        "heldMs": held.as_millis(),
                        "baselineBytesBefore": baseline.len()
                    }),
                );
                // #endregion
                emit_document(&mut peer, &text, &viewer_pane);
                remember_answer(&mut seen_answers, text.clone());
                baseline = snapshot_for_emit;
                eprintln!("tmath agent: document_sent bytes={}", text.len());
            } else {
                pending = Some((text, snapshot_for_emit, since));
            }
        }

        std::thread::sleep(Duration::from_millis(parsed.poll_ms));
    }
}

fn remember_answer(seen: &mut VecDeque<String>, answer: String) {
    const MAX_SEEN_ANSWERS: usize = 32;
    if seen.len() == MAX_SEEN_ANSWERS {
        seen.pop_front();
    }
    seen.push_back(answer);
}

fn parse_agent_args(args: &[String]) -> Result<WatcherArgs, String> {
    let mut percent = DEFAULT_PERCENT;
    let mut wait_ms = DEFAULT_WAIT_MS;
    let mut poll_ms = DEFAULT_POLL_MS;
    let mut history = DEFAULT_HISTORY;
    let mut source: Option<PaneId> = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--source-pane" => {
                let value = args.get(index + 1).ok_or("--source-pane needs a pane id")?;
                source = Some(PaneId::new(value).ok_or_else(|| {
                    format!("invalid tmux pane id {value:?}; expected %<digits>")
                })?);
                index += 2;
            }
            "--percent" => {
                let value = args.get(index + 1).ok_or("--percent needs a value")?;
                percent = value
                    .parse()
                    .map_err(|_| format!("invalid percent {value:?}"))?;
                index += 2;
            }
            "--wait-ms" => {
                let value = args.get(index + 1).ok_or("--wait-ms needs a value")?;
                wait_ms = value
                    .parse()
                    .map_err(|_| format!("invalid wait-ms {value:?}"))?;
                index += 2;
            }
            "--poll-ms" => {
                let value = args.get(index + 1).ok_or("--poll-ms needs a value")?;
                poll_ms = value
                    .parse()
                    .map_err(|_| format!("invalid poll-ms {value:?}"))?;
                index += 2;
            }
            "--history" => {
                let value = args.get(index + 1).ok_or("--history needs a value")?;
                history = value
                    .parse()
                    .map_err(|_| format!("invalid history {value:?}"))?;
                index += 2;
            }
            other if other.starts_with('-') => return Err(format!("unknown option {other:?}")),
            _ => return Err("unexpected positional argument".into()),
        }
    }
    if percent == 0 || percent >= 100 {
        return Err("--percent must be between 1 and 99".into());
    }
    Ok(WatcherArgs {
        percent,
        wait_ms,
        poll_ms,
        history,
        source,
    })
}

fn spawn_viewer_pane(
    args: &WatcherArgs,
    source: &PaneId,
    viewer_cmd: &str,
) -> Result<PaneId, String> {
    let split = split_viewer(args.percent, source, viewer_cmd);
    let output = tmux_output(&split)?;
    let pane = output
        .split_whitespace()
        .next()
        .and_then(PaneId::new)
        .ok_or_else(|| format!("tmux split-window printed no pane id: {output:?}"))?;
    Ok(pane)
}

/// Builds the shell command that runs the viewer in a fresh tmux pane,
/// injecting the renderer worker path because tmux panes inherit the server
/// environment rather than the watcher's.
fn viewer_command(
    exe: &std::path::Path,
    worker: &std::path::Path,
    socket: &std::path::Path,
    transport: Option<&str>,
) -> String {
    let transport = transport
        .map(|value| format!(" TMATH_TMUX_TRANSPORT={}", shell_quote(value)))
        .unwrap_or_default();
    format!(
        "env TMATH_RENDER_WORKER={}{} {} {} {}",
        shell_quote(&worker.display().to_string()),
        transport,
        shell_quote(&exe.display().to_string()),
        "agent-viewer",
        shell_quote(&socket.display().to_string())
    )
}

fn capture_source(pane: &PaneId, history: u32) -> Result<String, String> {
    tmux_output(&capture(pane, history))
}

fn pane_alive(pane: &PaneId) -> bool {
    Command::new("tmux")
        .args(display_pane(pane))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn tmux_output(args: &[String]) -> Result<String, String> {
    let output = Command::new("tmux")
        .args(args)
        .stderr(Stdio::null())
        .output()
        .map_err(|error| format!("run tmux: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "tmux {} failed with {}",
            args.first().map(String::as_str).unwrap_or(""),
            output.status
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Accepts a connecting viewer; `None` when none is ready yet.
fn accept_peer(listener: &UnixListener) -> Option<UnixStream> {
    match listener.accept() {
        Ok((stream, _)) => {
            let _ = stream.set_nonblocking(false);
            Some(stream)
        }
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => None,
        Err(_) => None,
    }
}

fn stdin_has_input() -> bool {
    use rustix::event::{PollFlags, Timespec};
    let stdin = io::stdin();
    let mut fds = [rustix::event::PollFd::new(&stdin, PollFlags::IN)];
    // Zero timeout: this is a non-blocking readiness check.
    let timeout = Timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    rustix::event::poll(&mut fds, Some(&timeout))
        .map(|n| n > 0)
        .unwrap_or(false)
}

/// Writes a document message to the viewer, tolerating a not-yet-connected or
/// disconnected viewer.
fn emit_document(peer: &mut Option<UnixStream>, text: &str, viewer_pane: &PaneId) {
    let Ok(frame) = encode_document(text) else {
        eprintln!("tmath agent: document exceeds renderer bound");
        return;
    };
    match peer.as_mut() {
        Some(stream) => {
            if let Err(error) = stream.write_all(&frame) {
                eprintln!("tmath agent: viewer disconnected ({error}); dropping");
                *peer = None;
            }
        }
        None => eprintln!(
            "tmath agent: no viewer connected for {}; document dropped",
            viewer_pane.as_str()
        ),
    }
}

fn finish(peer: &mut Option<UnixStream>, viewer_pane: &PaneId) -> Result<i32, String> {
    if let Some(stream) = peer.as_mut() {
        let _ = stream.write_all(&encode_quit());
    }
    let _ = Command::new("tmux").args(kill_pane(viewer_pane)).status();
    let _ =
        fs::remove_file(env::temp_dir().join(format!("tmath-agent-{}.sock", std::process::id())));
    Ok(0)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn defaults_apply_when_no_options_are_given() {
        let parsed = parse_agent_args(&args(&[])).unwrap();
        assert_eq!(parsed.percent, DEFAULT_PERCENT);
        assert_eq!(parsed.wait_ms, DEFAULT_WAIT_MS);
        assert_eq!(parsed.poll_ms, DEFAULT_POLL_MS);
        assert_eq!(parsed.history, DEFAULT_HISTORY);
        assert!(parsed.source.is_none(), "source resolved from env later");
    }

    #[test]
    fn parses_agent_options() {
        let parsed = parse_agent_args(&args(&[
            "--source-pane",
            "%7",
            "--percent",
            "50",
            "--wait-ms",
            "300",
            "--poll-ms",
            "100",
            "--history",
            "200",
        ]))
        .unwrap();
        assert_eq!(parsed.source.as_ref().unwrap().as_str(), "%7");
        assert_eq!(parsed.percent, 50);
        assert_eq!(parsed.wait_ms, 300);
        assert_eq!(parsed.poll_ms, 100);
        assert_eq!(parsed.history, 200);
    }

    #[test]
    fn rejects_bad_options() {
        assert!(parse_agent_args(&args(&["--percent", "0"])).is_err());
        assert!(parse_agent_args(&args(&["--percent", "100"])).is_err());
        assert!(parse_agent_args(&args(&["--history", "abc"])).is_err());
        assert!(parse_agent_args(&args(&["--bogus"])).is_err());
        assert!(parse_agent_args(&args(&["ignored"])).is_err());
    }

    #[test]
    fn rejects_invalid_pane_ids() {
        assert!(parse_agent_args(&args(&["--source-pane", "7"])).is_err());
        assert!(parse_agent_args(&args(&["--source-pane", "%zz"])).is_err());
        let parsed = parse_agent_args(&args(&["--source-pane", "%2"])).unwrap();
        assert_eq!(parsed.source.unwrap().as_str(), "%2");
    }

    #[test]
    fn tmux_capture_arguments_are_bounded() {
        let pane = PaneId::new("%4").unwrap();
        let cmd = capture(&pane, DEFAULT_HISTORY);
        assert_eq!(cmd.last().map(String::as_str), Some("-500"));
        assert_eq!(cmd[3], "%4");
    }

    #[test]
    fn viewer_command_injects_the_worker_path() {
        let cmd = viewer_command(
            std::path::Path::new("/opt/tools/tmath"),
            std::path::Path::new("/opt/site/dist/renderer/subprocess.js"),
            std::path::Path::new("/tmp/tmath-agent-1.sock"),
            None,
        );
        assert_eq!(
            cmd,
            "env TMATH_RENDER_WORKER='/opt/site/dist/renderer/subprocess.js' '/opt/tools/tmath' agent-viewer '/tmp/tmath-agent-1.sock'"
        );
    }

    #[test]
    fn viewer_command_quotes_paths_with_spaces() {
        let cmd = viewer_command(
            std::path::Path::new("/opt/my tools/tmath"),
            std::path::Path::new("/opt/my site/dist/renderer/subprocess.js"),
            std::path::Path::new("/tmp/tmath agent-1.sock"),
            None,
        );
        assert!(
            cmd.starts_with("env TMATH_RENDER_WORKER='/opt/my site/dist/renderer/subprocess.js'")
        );
        assert!(cmd.contains("'/opt/my tools/tmath' agent-viewer"));
    }

    #[test]
    fn viewer_command_forwards_an_explicit_tmux_transport() {
        let cmd = viewer_command(
            std::path::Path::new("/opt/tools/tmath"),
            std::path::Path::new("/opt/site/dist/renderer/subprocess.js"),
            std::path::Path::new("/tmp/tmath-agent-1.sock"),
            Some("passthrough"),
        );
        assert!(cmd.contains("TMATH_TMUX_TRANSPORT='passthrough'"));
    }
}
