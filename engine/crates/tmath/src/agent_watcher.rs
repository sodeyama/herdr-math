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
    capture, display_pane, encode_document, encode_quit, encode_replace_tail, find_answer,
    kill_pane, pane_current_path, shell_quote, split_viewer, PaneId,
};
use tmath_core::input::InputDecoder;
use tmath_core::scroll_driver::is_exit_signal;

use crate::transcript_adapter::{
    project_transcript_dir, resolve_transcript_file, TranscriptAdapter, TranscriptDelta,
    TranscriptOpenMode,
};

/// Ceiling (ms) on holding a pending answer, so a repainting agent never
/// wedges the viewer.
const MAX_HOLD_MS: u64 = 3000;
/// Re-check which Claude Code transcript file is live every N poll ticks.
const TRANSCRIPT_RERESOLVE_POLLS: u64 = 4;
/// Fall back to tmux capture when the transcript adapter stays idle this
/// many poll ticks without ever sending a document to the viewer.
const TRANSCRIPT_IDLE_FALLBACK_POLLS: u64 = 120;
/// Safety cap on a single captured snapshot.
const SNAPSHOT_MAX_BYTES: usize = 4 * 1024 * 1024;
/// Separator inserted between two accumulated answers (and between a
/// message's own text blocks) so the reassembled document keeps Markdown
/// block structure instead of fusing turn boundaries into one paragraph.
const ANSWER_SEPARATOR: &str = "\n\n";
/// Soft budget (bytes) on the frozen (already-completed) portion of the
/// accumulated chat-log document, well below `DeltaState`'s hard
/// `max_document_bytes` cap (`IPC_MAX_REQUEST_BYTES`, 192 MiB) and the
/// renderer's `blocks_per_document` cap (4096 Markdown blocks). AT-3-604:
/// a long session's frozen history WILL eventually cross this in real use;
/// crossing it trims the oldest frozen answers and forces a full
/// `Document` resync with the trimmed tail — trimming is the only thing
/// ever allowed to drop old content, a new answer never does.
const FROZEN_HISTORY_SOFT_BUDGET_BYTES: usize = 8 * 1024 * 1024;

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
    let file_config = crate::config::load_active();
    let parsed = parse_agent_args(args, &file_config.agent)?;
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
    let dpr = crate::config::resolve_device_pixel_ratio_config(&file_config.agent)
        .map(|value| value.to_string());
    let viewer_log = match crate::config::resolve_viewer_log_config(&file_config.agent) {
        Some(true) => Some("1".to_string()),
        _ => None,
    };
    let font_size_pt = crate::config::resolve_font_size_pt_env_or_config(&file_config)
        .map(format_font_size_pt);
    let viewer_cmd = viewer_command(
        &exe,
        &socket_path,
        crate::config::resolve_tmux_transport(&file_config.agent).as_deref(),
        dpr.as_deref(),
        viewer_log.as_deref(),
        font_size_pt.as_deref(),
    );
    let viewer_pane = spawn_viewer_pane(&parsed, &source, &viewer_cmd)?;
    let route = crate::terminal_output::selected_route()?;
    if route == crate::terminal_output::Route::TmuxPassthrough {
        let _ = crate::enable_tmux_passthrough();
    }

    // D5's source-adapter priority: prefer a Claude Code transcript when one
    // can be located and opened, since it yields the original Markdown
    // source with no capture-side heuristics. `transcript` being `None`
    // (not found, or a later read/parse failure) always means "use the
    // existing tmux capture-pane path below" — the two never run at once,
    // and there is no separate error state to recover from: `None` itself
    // is the fallback.
    let watcher_started = std::time::SystemTime::now();
    let transcript_project_dir = transcript_project_dir_for(&source);
    let transcript = transcript_project_dir
        .as_deref()
        .and_then(|dir| open_transcript_in(dir, watcher_started, TranscriptAttach::Initial));
    eprintln!(
        "tmath agent: source={}",
        if transcript.is_some() {
            "transcript"
        } else {
            "capture"
        }
    );
    let mut transcript = transcript;
    let mut transcript_idle_polls = 0u64;
    let mut transcript_sent_any = false;
    let mut poll_tick = 0u64;
    // AT-3-604: one history model, shared across whichever source is
    // active this session — the transcript path grows it incrementally
    // (`Append`/`AnswerBoundary`), the capture path replaces `current`
    // wholesale each time its own boundary/settle heuristics decide an
    // answer changed. Either way `emit_current_answer_update` is what
    // decides `ReplaceTail` vs. a trimmed `Document` resync.
    let mut history = AnswerHistory::new();

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

        if let Some(adapter) = transcript.as_mut() {
            if !pane_alive(&source) {
                eprintln!("tmath agent: source pane closed; stopping");
                return finish(&mut peer, &viewer_pane);
            }
            match adapter.poll() {
                Ok(deltas) => {
                    if !deltas.is_empty() {
                        transcript_idle_polls = 0;
                    } else {
                        transcript_idle_polls += 1;
                    }
                    for delta in deltas {
                        emit_transcript_delta(&mut peer, &mut history, delta);
                        transcript_sent_any = true;
                    }
                }
                Err(_) => {
                    // Fail closed: degrade to the capture adapter for the
                    // rest of this session rather than retry a transcript
                    // that stopped making sense (AT-3-602). No content, only
                    // the event name, ever reaches this log line.
                    eprintln!("tmath agent: source=capture (transcript_degraded)");
                    transcript = None;
                }
            }
            if transcript.is_some() {
                poll_tick += 1;
                if poll_tick.is_multiple_of(TRANSCRIPT_RERESOLVE_POLLS) {
                    if let Some(dir) = transcript_project_dir.as_deref() {
                        try_reresolve_transcript(
                            dir,
                            watcher_started,
                            transcript.as_mut().unwrap(),
                        );
                    }
                }
                if !transcript_sent_any && transcript_idle_polls >= TRANSCRIPT_IDLE_FALLBACK_POLLS {
                    eprintln!("tmath agent: source=capture (transcript_idle)");
                    transcript = None;
                } else {
                    std::thread::sleep(Duration::from_millis(parsed.poll_ms));
                    continue;
                }
            }
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
        } else if snapshot != baseline {
            match find_answer(&baseline, &snapshot) {
                Some(answer) => {
                    if seen_answers.contains(&answer.text) {
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
                // The capture path has no explicit turn-boundary signal
                // (unlike the transcript adapter's `AnswerBoundary`), so
                // "is this the same answer continuing, or a new one
                // starting" is inferred the same way `seen_answers`
                // already does elsewhere: `text` extending the most
                // recently emitted current answer is growth (the settle
                // heuristics re-fired because the agent kept writing);
                // anything else — including a shrink (AT-3-505) — is a
                // new answer, and the previous one is frozen first.
                let is_growth = !history.current.is_empty() && text.starts_with(&history.current);
                emit_document(&mut peer, &mut history, text.clone(), is_growth);
                remember_answer(&mut seen_answers, text.clone());
                baseline = snapshot_for_emit;
                eprintln!("tmath agent: document_sent bytes={}", text.len());
            } else {
                pending = Some((text, snapshot_for_emit, since));
            }
        }

        std::thread::sleep(Duration::from_millis(parsed.poll_ms));
        poll_tick += 1;
        if poll_tick.is_multiple_of(TRANSCRIPT_RERESOLVE_POLLS) && transcript.is_none() {
            if let Some(dir) = transcript_project_dir.as_deref() {
                transcript = open_transcript_in(dir, watcher_started, TranscriptAttach::JoinLive);
                if transcript.is_some() {
                    eprintln!("tmath agent: source=transcript");
                    transcript_idle_polls = 0;
                }
            }
        }
    }
}

fn remember_answer(seen: &mut VecDeque<String>, answer: String) {
    const MAX_SEEN_ANSWERS: usize = 32;
    if seen.len() == MAX_SEEN_ANSWERS {
        seen.pop_front();
    }
    seen.push_back(answer);
}

fn format_font_size_pt(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        format!("{value:.1}")
    }
}

fn parse_agent_args(args: &[String], file: &crate::config::AgentConfig) -> Result<WatcherArgs, String> {
    let mut percent = file
        .viewer_percent
        .unwrap_or(crate::config::DEFAULT_AGENT_VIEWER_PERCENT);
    let mut wait_ms = file
        .wait_ms
        .unwrap_or(crate::config::DEFAULT_AGENT_WAIT_MS);
    let mut poll_ms = file
        .poll_ms
        .unwrap_or(crate::config::DEFAULT_AGENT_POLL_MS);
    let mut history = file
        .history_lines
        .unwrap_or(crate::config::DEFAULT_AGENT_HISTORY_LINES);
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
/// forwarding tmux/DPR/log overrides because tmux panes inherit the server's
/// environment rather than the watcher's.
fn viewer_command(
    exe: &std::path::Path,
    socket: &std::path::Path,
    transport: Option<&str>,
    dpr: Option<&str>,
    viewer_log: Option<&str>,
    font_size_pt: Option<&str>,
) -> String {
    let mut env_pairs = Vec::new();
    if let Some(value) = transport {
        env_pairs.push(format!("TMATH_TMUX_TRANSPORT={}", shell_quote(value)));
    }
    if let Some(value) = dpr {
        env_pairs.push(format!("TMATH_DPR={}", shell_quote(value)));
    }
    if let Some(value) = viewer_log {
        env_pairs.push(format!("TMATH_VIEWER_LOG={}", shell_quote(value)));
    }
    if let Some(value) = font_size_pt {
        env_pairs.push(format!("TMATH_FONT_SIZE_PT={}", shell_quote(value)));
    }
    let exe = shell_quote(&exe.display().to_string());
    let socket = shell_quote(&socket.display().to_string());
    if env_pairs.is_empty() {
        format!("{exe} agent-viewer {socket}")
    } else {
        format!("env {} {exe} agent-viewer {socket}", env_pairs.join(" "))
    }
}

fn capture_source(pane: &PaneId, history: u32) -> Result<String, String> {
    let snapshot = tmux_output(&capture(pane, history))?;
    Ok(strip_own_log_lines(&snapshot))
}

/// Drops this watcher's own stderr banner/status lines from a captured
/// snapshot. When the source pane is the watcher's own controlling terminal
/// (self-watch), those lines land directly in the captured pane and would
/// otherwise be mistaken for fresh answer content, causing the watcher to
/// treat its own logging as an endless stream of new "answers".
fn strip_own_log_lines(snapshot: &str) -> String {
    const OWN_LOG_PREFIX: &str = "tmath agent: ";
    snapshot
        .lines()
        .map(|line| match line.find(OWN_LOG_PREFIX) {
            Some(index) => &line[..index],
            None => line,
        })
        .collect::<Vec<_>>()
        .join("\n")
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

/// Resolves the Claude Code transcript directory for the watched pane's
/// current working directory, or `None` when unavailable.
fn transcript_project_dir_for(source: &PaneId) -> Option<std::path::PathBuf> {
    let home = env::var_os("HOME")?;
    let cwd = tmux_output(&pane_current_path(source)).ok()?;
    let cwd = cwd.trim();
    if cwd.is_empty() {
        return None;
    }
    project_transcript_dir(std::path::Path::new(&home), std::path::Path::new(cwd))
}

/// Whether a transcript open is the watcher's first attach attempt or a
/// later join while the capture adapter was running.
enum TranscriptAttach {
    /// Watcher startup: honour [`resolve_transcript_file`]'s FromStart/Tail choice.
    Initial,
    /// Capture-to-transcript upgrade: tail at EOF so a long-running watcher
    /// never replays an entire prior session in one burst when the JSONL's
    /// mtime refreshes on the next user turn.
    JoinLive,
}

/// Locates and opens a Claude Code transcript for the watched pane, or
/// `None` when there is no live session file yet — the caller's fallback is
/// simply "keep using the capture adapter", not a retry loop here.
fn open_transcript_in(
    dir: &std::path::Path,
    watcher_started: std::time::SystemTime,
    attach: TranscriptAttach,
) -> Option<TranscriptAdapter> {
    let (file, mode) = resolve_transcript_file(dir, watcher_started)?;
    let mode = match attach {
        TranscriptAttach::Initial => mode,
        TranscriptAttach::JoinLive => TranscriptOpenMode::Tail,
    };
    TranscriptAdapter::open(&file, mode).ok()
}

/// Switches to a newer live transcript file when one appears after watcher
/// startup — for example when auto-watch opened a stale JSONL at EOF while
/// Claude Code created a fresh session file for the current run.
fn try_reresolve_transcript(
    dir: &std::path::Path,
    watcher_started: std::time::SystemTime,
    adapter: &mut TranscriptAdapter,
) {
    let Some((file, mode)) = resolve_transcript_file(dir, watcher_started) else {
        return;
    };
    if file == adapter.path() {
        return;
    }
    if let Ok(reopened) = TranscriptAdapter::open(&file, mode) {
        *adapter = reopened;
        eprintln!("tmath agent: transcript_reresolved");
    }
}

/// AT-3-604's accumulation model: answers pile up like a chat log rather
/// than each new answer replacing the last. `frozen` holds every answer the
/// watcher has decided is finished, already joined with
/// [`ANSWER_SEPARATOR`] in order; `current` is the answer still streaming
/// (or, for the capture path, the one most recently observed). The wire
/// document is always conceptually `frozen + separator-if-both-nonempty +
/// current`, but only `emit_*` methods below ever construct and send that
/// full text — callers mutate state through `freeze_current`/`grow_current`
/// and let this type decide Document vs. ReplaceTail vs. trim.
///
/// Per-answer lengths are tracked (not just the joined `frozen` string) so
/// trimming can drop whole answers from the head without re-deriving
/// boundaries from separator text, which would be ambiguous if an answer's
/// own Markdown ever contained a blank line matching the separator.
struct AnswerHistory {
    /// Byte length of each frozen answer, oldest first, as it appears
    /// (with its leading separator, except the very first) in `frozen`.
    frozen_answer_bytes: VecDeque<usize>,
    frozen: String,
    current: String,
    seq: u64,
    /// Soft trim budget in bytes. Always
    /// `FROZEN_HISTORY_SOFT_BUDGET_BYTES` outside tests; overridable via
    /// `with_budget` so trim-boundary tests can exercise it without
    /// allocating/transmitting multi-megabyte fixtures.
    budget: usize,
    /// Whether a `Document` frame has ever been sent for this peer
    /// connection. `DeltaState::apply` rejects the very first delta frame
    /// it ever sees (`check_delta`'s `(delta_valid, last_seq)` starts
    /// `(false, None)`, so no `seq` satisfies it) — a fresh viewer
    /// connection MUST see a `Document` before any `ReplaceTail`/`Append`,
    /// regardless of which source adapter is driving `AnswerHistory` or
    /// whether this is the transcript path's guaranteed-first `Reset` or
    /// the capture path's first settled answer (which has no `Reset`
    /// signal at all).
    ever_sent_document: bool,
}

impl AnswerHistory {
    fn new() -> Self {
        Self::with_budget(FROZEN_HISTORY_SOFT_BUDGET_BYTES)
    }

    fn with_budget(budget: usize) -> Self {
        Self {
            frozen_answer_bytes: VecDeque::new(),
            frozen: String::new(),
            current: String::new(),
            seq: 0,
            budget,
            ever_sent_document: false,
        }
    }

    /// The full accumulated document as it should appear in the viewer.
    fn document(&self) -> String {
        if self.frozen.is_empty() {
            self.current.clone()
        } else if self.current.is_empty() {
            self.frozen.clone()
        } else {
            format!("{}{}{}", self.frozen, ANSWER_SEPARATOR, self.current)
        }
    }

    /// A full resync: drops all accumulated state and starts a fresh
    /// current answer. Used for `TranscriptDelta::Reset` (adapter open or
    /// rotation reopen) and for trim resyncs.
    fn reset_current(&mut self, text: String) {
        self.frozen_answer_bytes.clear();
        self.frozen.clear();
        self.current = text;
        self.seq = 0;
    }

    /// Freezes the current answer into history if it is non-empty. A
    /// boundary with nothing accumulated since the last freeze (e.g. a
    /// lone tool-result line's boundary, or two boundaries with no
    /// intervening text) is a no-op, matching `TranscriptDelta::
    /// AnswerBoundary`'s doc comment.
    fn freeze_current(&mut self) {
        if self.current.is_empty() {
            return;
        }
        let separator_len = if self.frozen.is_empty() {
            0
        } else {
            self.frozen.push_str(ANSWER_SEPARATOR);
            ANSWER_SEPARATOR.len()
        };
        self.frozen_answer_bytes
            .push_back(separator_len + self.current.len());
        self.frozen.push_str(&self.current);
        self.current.clear();
    }

    /// Appends `text` to the in-progress current answer (the transcript
    /// path already includes the leading blank-line separator between
    /// same-answer message fragments; the capture path replaces `current`
    /// wholesale via `replace_current` instead of calling this).
    fn grow_current(&mut self, text: &str) {
        self.current.push_str(text);
    }

    /// Capture-path equivalent of `grow_current`: the capture adapter
    /// yields whole current-answer snapshots rather than incremental
    /// fragments, so this replaces `current` outright instead of
    /// appending.
    fn replace_current(&mut self, text: String) {
        self.current = text;
    }

    /// Drops the oldest frozen answers until the frozen portion is back
    /// under budget, or exactly one answer remains (a single answer over
    /// budget is kept whole rather than corrupted by a partial trim).
    /// Returns `true` if anything was trimmed, telling the caller a full
    /// `Document` resync (not a `ReplaceTail`) is required, since trimming
    /// changes bytes at offset 0, which no `keep_bytes` tail-replace can
    /// express.
    fn trim_if_over_budget(&mut self) -> bool {
        if self.frozen.len() <= self.budget {
            return false;
        }
        let mut trimmed = false;
        while self.frozen.len() > self.budget && self.frozen_answer_bytes.len() > 1 {
            let Some(oldest_bytes) = self.frozen_answer_bytes.pop_front() else {
                break;
            };
            self.frozen.drain(..oldest_bytes);
            trimmed = true;
        }
        trimmed
    }
}

/// Sends one transcript-derived delta to the viewer, folding it into
/// `history`'s accumulation model (AT-3-604) before choosing the T3-401
/// wire message: `Reset` and a trim both need a whole `Document` frame
/// (and reset the sequence counter, matching `DeltaState`'s resync
/// contract on the receiving end); `AnswerBoundary` freezes the current
/// answer and sends nothing on its own (there is no text change to show
/// yet); `Append` grows the current answer and sends `ReplaceTail` so the
/// still-streaming answer updates in place without re-sending frozen
/// history bytes.
fn emit_transcript_delta(
    peer: &mut Option<UnixStream>,
    history: &mut AnswerHistory,
    delta: TranscriptDelta,
) {
    match delta {
        TranscriptDelta::Reset(text) => {
            history.reset_current(text);
            send_document(peer, history);
        }
        TranscriptDelta::AnswerBoundary => {
            history.freeze_current();
            // No text changed as of this instant; nothing to send. The
            // frozen bytes this boundary just committed become the
            // `keep_bytes` base for the next `Append`'s `ReplaceTail`.
        }
        TranscriptDelta::Append(text) => {
            history.grow_current(&text);
            emit_current_answer_update(peer, history);
        }
    }
}

/// Sends the current answer's latest state after it grew, choosing between
/// a bounded `ReplaceTail` (the common case) and a full `Document` resync.
/// A resync is required both when growth pushed the frozen portion over
/// its soft budget (trimming changes byte 0, which no tail-replace can
/// express) and, unconditionally, the very first time anything is ever
/// sent to this peer (`DeltaState::apply` rejects any delta frame before
/// the first `Document`; see `AnswerHistory::ever_sent_document`'s doc
/// comment) — the capture path's first settled answer has no `Reset`
/// signal of its own to guarantee this, unlike the transcript path.
fn emit_current_answer_update(peer: &mut Option<UnixStream>, history: &mut AnswerHistory) {
    let must_resync = history.trim_if_over_budget() || !history.ever_sent_document;
    if must_resync {
        history.seq = 0;
        send_document(peer, history);
        return;
    }
    let keep_bytes = history.frozen.len();
    let separator = if history.frozen.is_empty() {
        ""
    } else {
        ANSWER_SEPARATOR
    };
    let tail = format!("{separator}{}", history.current);
    history.seq += 1;
    let frame = encode_replace_tail(history.seq, keep_bytes, &tail);
    send_frame(peer, frame, "replace-tail");
}

/// Capture-path emission (AT-3-604's capture rewire): folds one settled
/// answer snapshot into `history`. The capture adapter's existing
/// boundary/settle heuristics (`find_answer`, the `pending` debounce in
/// `run_agent`) stay exactly as they are — this only changes what happens
/// to a settled answer once they decide to emit it. `is_growth` tells this
/// whether `text` is the same answer continuing (the settle heuristics
/// re-fired because the answer kept changing) or a genuinely new answer
/// (the previous one is done; freeze it before starting this one), which
/// the caller determines from `seen_answers`/prefix comparison since the
/// capture path has no explicit turn-boundary signal like the transcript
/// adapter's `AnswerBoundary`.
fn emit_document(
    peer: &mut Option<UnixStream>,
    history: &mut AnswerHistory,
    text: String,
    is_growth: bool,
) {
    if !is_growth {
        history.freeze_current();
    }
    history.replace_current(text);
    emit_current_answer_update(peer, history);
}

fn send_document(peer: &mut Option<UnixStream>, history: &mut AnswerHistory) {
    let frame = encode_document(&history.document());
    history.ever_sent_document = true;
    send_frame(peer, frame, "document");
}

fn send_frame(
    peer: &mut Option<UnixStream>,
    frame: Result<Vec<u8>, tmath_core::agent::CodecError>,
    kind: &str,
) {
    let Ok(frame) = frame else {
        eprintln!("tmath agent: {kind} exceeds renderer bound");
        return;
    };
    match peer.as_mut() {
        Some(stream) => {
            if let Err(error) = stream.write_all(&frame) {
                eprintln!("tmath agent: viewer disconnected ({error}); dropping");
                *peer = None;
            }
        }
        None => eprintln!("tmath agent: no viewer connected; {kind} dropped"),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AgentConfig;
    use crate::transcript_adapter::{TranscriptAdapter, TranscriptOpenMode};

    fn file_defaults() -> AgentConfig {
        AgentConfig::default()
    }

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn strip_own_log_lines_removes_a_full_log_line() {
        let snapshot = "❯ prompt\n\ntmath agent: document_sent bytes=119\n";
        assert_eq!(strip_own_log_lines(snapshot), "❯ prompt\n\n");
    }

    #[test]
    fn strip_own_log_lines_truncates_a_line_that_ends_with_a_log_message() {
        // Self-watch: the watcher's own stderr banner lands mid-line, right
        // after the last answer line captured from the same pane/tty.
        let snapshot = "  └ Successfully loaded skilltmath agent: document_sent bytes=119\n";
        assert_eq!(
            strip_own_log_lines(snapshot),
            "  └ Successfully loaded skill"
        );
    }

    #[test]
    fn strip_own_log_lines_leaves_unrelated_content_untouched() {
        let snapshot = "❯ Derive the result.\nThe answer is $x=2$.\n❯ ";
        assert_eq!(strip_own_log_lines(snapshot), snapshot);
    }

    #[test]
    fn defaults_apply_when_no_options_are_given() {
        let parsed = parse_agent_args(&args(&[]), &file_defaults()).unwrap();
        assert_eq!(parsed.percent, crate::config::DEFAULT_AGENT_VIEWER_PERCENT);
        assert_eq!(parsed.wait_ms, crate::config::DEFAULT_AGENT_WAIT_MS);
        assert_eq!(parsed.poll_ms, crate::config::DEFAULT_AGENT_POLL_MS);
        assert_eq!(parsed.history, crate::config::DEFAULT_AGENT_HISTORY_LINES);
        assert!(parsed.source.is_none(), "source resolved from env later");
    }

    #[test]
    fn parses_agent_options() {
        let parsed = parse_agent_args(
            &args(&[
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
        ]),
            &file_defaults(),
        )
        .unwrap();
        assert_eq!(parsed.source.as_ref().unwrap().as_str(), "%7");
        assert_eq!(parsed.percent, 50);
        assert_eq!(parsed.wait_ms, 300);
        assert_eq!(parsed.poll_ms, 100);
        assert_eq!(parsed.history, 200);
    }

    #[test]
    fn rejects_bad_options() {
        let defaults = file_defaults();
        assert!(parse_agent_args(&args(&["--percent", "0"]), &defaults).is_err());
        assert!(parse_agent_args(&args(&["--percent", "100"]), &defaults).is_err());
        assert!(parse_agent_args(&args(&["--history", "abc"]), &defaults).is_err());
        assert!(parse_agent_args(&args(&["--bogus"]), &defaults).is_err());
        assert!(parse_agent_args(&args(&["ignored"]), &defaults).is_err());
    }

    #[test]
    fn rejects_invalid_pane_ids() {
        let defaults = file_defaults();
        assert!(parse_agent_args(&args(&["--source-pane", "7"]), &defaults).is_err());
        assert!(parse_agent_args(&args(&["--source-pane", "%zz"]), &defaults).is_err());
        let parsed = parse_agent_args(&args(&["--source-pane", "%2"]), &defaults).unwrap();
        assert_eq!(parsed.source.unwrap().as_str(), "%2");
    }

    #[test]
    fn tmux_capture_arguments_are_bounded() {
        let pane = PaneId::new("%4").unwrap();
        let cmd = capture(&pane, crate::config::DEFAULT_AGENT_HISTORY_LINES);
        assert_eq!(cmd.last().map(String::as_str), Some("-500"));
        assert_eq!(cmd[3], "%4");
    }

    #[test]
    fn viewer_command_runs_the_viewer_without_optional_env_overrides() {
        let cmd = viewer_command(
            std::path::Path::new("/opt/tools/tmath"),
            std::path::Path::new("/tmp/tmath-agent-1.sock"),
            None,
            None,
            None,
            None,
        );
        assert_eq!(
            cmd,
            "'/opt/tools/tmath' agent-viewer '/tmp/tmath-agent-1.sock'"
        );
    }

    #[test]
    fn viewer_command_quotes_paths_with_spaces() {
        let cmd = viewer_command(
            std::path::Path::new("/opt/my tools/tmath"),
            std::path::Path::new("/tmp/tmath agent-1.sock"),
            None,
            None,
            None,
            None,
        );
        assert_eq!(
            cmd,
            "'/opt/my tools/tmath' agent-viewer '/tmp/tmath agent-1.sock'"
        );
    }

    #[test]
    fn viewer_command_forwards_an_explicit_tmux_transport() {
        let cmd = viewer_command(
            std::path::Path::new("/opt/tools/tmath"),
            std::path::Path::new("/tmp/tmath-agent-1.sock"),
            Some("passthrough"),
            None,
            None,
            None,
        );
        assert!(cmd.contains("TMATH_TMUX_TRANSPORT='passthrough'"));
    }

    /// tmux `split-window` panes start with the server's environment, not
    /// the watcher process's, so `TMATH_DPR` (like `TMATH_TMUX_TRANSPORT`)
    /// must be forwarded explicitly on the spawn command line or the viewer
    /// pane never sees it.
    #[test]
    fn viewer_command_forwards_an_explicit_dpr_override() {
        let cmd = viewer_command(
            std::path::Path::new("/opt/tools/tmath"),
            std::path::Path::new("/tmp/tmath-agent-1.sock"),
            None,
            Some("2"),
            None,
            None,
        );
        assert!(cmd.contains("TMATH_DPR='2'"));
    }

    #[test]
    fn viewer_command_forwards_both_transport_and_dpr_together() {
        let cmd = viewer_command(
            std::path::Path::new("/opt/tools/tmath"),
            std::path::Path::new("/tmp/tmath-agent-1.sock"),
            Some("passthrough"),
            Some("3"),
            None,
            None,
        );
        assert!(cmd.contains("TMATH_TMUX_TRANSPORT='passthrough'"));
        assert!(cmd.contains("TMATH_DPR='3'"));
    }

    #[test]
    fn viewer_command_with_neither_override_omits_both_variables() {
        let cmd = viewer_command(
            std::path::Path::new("/opt/tools/tmath"),
            std::path::Path::new("/tmp/tmath-agent-1.sock"),
            None,
            None,
            None,
            None,
        );
        assert!(!cmd.contains("TMATH_TMUX_TRANSPORT"));
        assert!(!cmd.contains("TMATH_DPR"));
        assert!(!cmd.contains("TMATH_VIEWER_LOG"));
        assert!(!cmd.contains("TMATH_FONT_SIZE_PT"));
        assert!(!cmd.contains("TMATH_RENDER_WORKER"));
    }

    /// Same forwarding requirement as `TMATH_DPR`/`TMATH_TMUX_TRANSPORT`:
    /// the viewer's diagnostics are off by default (see
    /// `agent_viewer::viewer_log_enabled`), so an evidence run that sets
    /// `TMATH_VIEWER_LOG` before running `tmath agent` needs it on the
    /// viewer pane's spawn command line to reach the viewer process at all.
    #[test]
    fn viewer_command_forwards_an_explicit_viewer_log_flag() {
        let cmd = viewer_command(
            std::path::Path::new("/opt/tools/tmath"),
            std::path::Path::new("/tmp/tmath-agent-1.sock"),
            None,
            None,
            Some("1"),
            None,
        );
        assert!(cmd.contains("TMATH_VIEWER_LOG='1'"));
    }

    #[test]
    fn viewer_command_forwards_an_explicit_font_size_override() {
        let cmd = viewer_command(
            std::path::Path::new("/opt/tools/tmath"),
            std::path::Path::new("/tmp/tmath-agent-1.sock"),
            None,
            None,
            None,
            Some("16"),
        );
        assert!(cmd.contains("TMATH_FONT_SIZE_PT='16'"));
    }

    #[test]
    fn viewer_command_forwards_all_four_overrides_together() {
        let cmd = viewer_command(
            std::path::Path::new("/opt/tools/tmath"),
            std::path::Path::new("/tmp/tmath-agent-1.sock"),
            Some("passthrough"),
            Some("3"),
            Some("1"),
            Some("16"),
        );
        assert!(cmd.contains("TMATH_TMUX_TRANSPORT='passthrough'"));
        assert!(cmd.contains("TMATH_DPR='3'"));
        assert!(cmd.contains("TMATH_VIEWER_LOG='1'"));
        assert!(cmd.contains("TMATH_FONT_SIZE_PT='16'"));
    }

    // AT-3-602 supervisor fix 1: reassembling a two-message answer must not
    // fuse the messages into one paragraph. This drives the actual wire path
    // (`emit_transcript_delta` -> a real `UnixStream` -> `Decoder` ->
    // `DeltaState::apply`, exactly as the viewer would see it) rather than
    // asserting on `TranscriptDelta` text directly, so it proves the
    // document-level invariant the supervisor asked for.
    #[test]
    fn a_two_message_answer_reassembles_with_a_blank_line_between_messages() {
        let (mut tx, mut rx) = UnixStream::pair().unwrap();
        rx.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut peer = Some(tx.try_clone().unwrap());
        let mut history = AnswerHistory::new();

        emit_transcript_delta(
            &mut peer,
            &mut history,
            TranscriptDelta::Reset("Part one.".to_string()),
        );
        emit_transcript_delta(
            &mut peer,
            &mut history,
            TranscriptDelta::Append("\n\nPart two.".to_string()),
        );
        tx.flush().unwrap();
        drop(tx);
        drop(peer);

        let mut bytes = Vec::new();
        rx.read_to_end(&mut bytes).unwrap();

        let mut decoder = tmath_core::agent::Decoder::new();
        decoder.push(&bytes);
        let mut state = tmath_core::agent::DeltaState::new(usize::MAX);
        let mut applied = 0;
        while let Some(message) = decoder.next_message() {
            state.apply(&message.unwrap()).unwrap();
            applied += 1;
        }

        assert_eq!(applied, 2, "both frames decoded");
        assert_eq!(state.document(), "Part one.\n\nPart two.");
    }

    // AT-3-602 supervisor fix 1, Reset side: the first block of a fresh
    // answer must not gain a leading separator.
    #[test]
    fn a_reset_does_not_get_a_leading_separator() {
        let (mut tx, mut rx) = UnixStream::pair().unwrap();
        rx.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut peer = Some(tx.try_clone().unwrap());
        let mut history = AnswerHistory::new();

        emit_transcript_delta(
            &mut peer,
            &mut history,
            TranscriptDelta::Reset("Only message.".to_string()),
        );
        tx.flush().unwrap();
        drop(tx);
        drop(peer);

        let mut bytes = Vec::new();
        rx.read_to_end(&mut bytes).unwrap();

        let mut decoder = tmath_core::agent::Decoder::new();
        decoder.push(&bytes);
        let mut state = tmath_core::agent::DeltaState::new(usize::MAX);
        while let Some(message) = decoder.next_message() {
            state.apply(&message.unwrap()).unwrap();
        }

        assert_eq!(state.document(), "Only message.");
    }

    // --- AT-3-604: AnswerHistory accumulation, ReplaceTail, and trim ---

    /// The core chat-log requirement: a boundary between two answers must
    /// not drop the first one. `document()` after the boundary and a new
    /// answer's growth shows both answers present, separated.
    #[test]
    fn a_boundary_freezes_the_first_answer_instead_of_dropping_it() {
        let mut history = AnswerHistory::new();
        history.reset_current("First answer.".to_string());
        history.freeze_current();
        history.grow_current("Second answer.");
        assert_eq!(history.document(), "First answer.\n\nSecond answer.");
    }

    /// A boundary with nothing accumulated since the last freeze (a lone
    /// tool-result line, or two boundaries in a row) must not insert an
    /// empty "answer" or a stray separator.
    #[test]
    fn a_boundary_with_no_current_text_is_a_no_op() {
        let mut history = AnswerHistory::new();
        history.reset_current("Only answer.".to_string());
        history.freeze_current();
        history.freeze_current(); // second boundary, nothing changed since
        assert_eq!(history.document(), "Only answer.");
    }

    /// Three answers accumulate in order across two boundaries — proves
    /// accumulation is not limited to a single pair.
    #[test]
    fn three_answers_accumulate_across_two_boundaries() {
        let mut history = AnswerHistory::new();
        history.reset_current("One.".to_string());
        history.freeze_current();
        history.grow_current("Two.");
        history.freeze_current();
        history.grow_current("Three.");
        assert_eq!(history.document(), "One.\n\nTwo.\n\nThree.");
    }

    /// `emit_current_answer_update` sends `ReplaceTail` (not a full
    /// `Document`) while the current answer keeps growing under budget —
    /// this is T3-401's ReplaceTail seam finally exercised for real. Drives
    /// the actual wire path so the assertion covers what the viewer would
    /// decode, not just `AnswerHistory`'s in-memory state.
    #[test]
    fn growth_under_budget_sends_replace_tail_not_a_full_document() {
        let (mut tx, mut rx) = UnixStream::pair().unwrap();
        rx.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut peer = Some(tx.try_clone().unwrap());
        let mut history = AnswerHistory::new();

        emit_transcript_delta(
            &mut peer,
            &mut history,
            TranscriptDelta::Reset("First answer.".to_string()),
        );
        emit_transcript_delta(&mut peer, &mut history, TranscriptDelta::AnswerBoundary);
        emit_transcript_delta(
            &mut peer,
            &mut history,
            TranscriptDelta::Append("Second answer.".to_string()),
        );
        tx.flush().unwrap();
        drop(tx);
        drop(peer);

        let mut bytes = Vec::new();
        rx.read_to_end(&mut bytes).unwrap();
        let mut decoder = tmath_core::agent::Decoder::new();
        decoder.push(&bytes);
        let mut messages = Vec::new();
        while let Some(message) = decoder.next_message() {
            messages.push(message.unwrap());
        }

        assert_eq!(messages.len(), 2, "boundary alone sends nothing");
        assert!(matches!(
            messages[0],
            tmath_core::agent::Message::Document { .. }
        ));
        assert!(
            matches!(messages[1], tmath_core::agent::Message::ReplaceTail { .. }),
            "growth after a boundary must use ReplaceTail, not Document: {:?}",
            messages[1]
        );

        let mut state = tmath_core::agent::DeltaState::new(usize::MAX);
        for message in &messages {
            state.apply(message).unwrap();
        }
        assert_eq!(state.document(), "First answer.\n\nSecond answer.");
    }

    /// Growth past the boundary keeps replacing only the tail across
    /// multiple `Append`s within the same answer — `keep_bytes` stays
    /// pinned at the frozen length while the current answer keeps
    /// extending in place, never re-sending frozen bytes.
    #[test]
    fn multiple_appends_within_one_answer_all_replace_tail_from_the_same_frozen_offset() {
        let (mut tx, mut rx) = UnixStream::pair().unwrap();
        rx.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut peer = Some(tx.try_clone().unwrap());
        let mut history = AnswerHistory::new();

        emit_transcript_delta(
            &mut peer,
            &mut history,
            TranscriptDelta::Reset("History.".to_string()),
        );
        emit_transcript_delta(&mut peer, &mut history, TranscriptDelta::AnswerBoundary);
        emit_transcript_delta(
            &mut peer,
            &mut history,
            TranscriptDelta::Append("Grow".to_string()),
        );
        emit_transcript_delta(
            &mut peer,
            &mut history,
            TranscriptDelta::Append("ing more".to_string()),
        );
        tx.flush().unwrap();
        drop(tx);
        drop(peer);

        let mut bytes = Vec::new();
        rx.read_to_end(&mut bytes).unwrap();
        let mut decoder = tmath_core::agent::Decoder::new();
        decoder.push(&bytes);
        let mut state = tmath_core::agent::DeltaState::new(usize::MAX);
        let mut replace_tail_count = 0;
        while let Some(message) = decoder.next_message() {
            let message = message.unwrap();
            if let tmath_core::agent::Message::ReplaceTail { keep_bytes, .. } = &message {
                assert_eq!(
                    *keep_bytes,
                    "History.".len(),
                    "keep_bytes must stay pinned at the frozen length across both appends"
                );
                replace_tail_count += 1;
            }
            state.apply(&message).unwrap();
        }
        assert_eq!(replace_tail_count, 2);
        assert_eq!(state.document(), "History.\n\nGrowing more");
    }

    /// Test-only trim budget, small enough that fixtures stay in the
    /// hundreds of bytes rather than the megabytes
    /// `FROZEN_HISTORY_SOFT_BUDGET_BYTES` uses in production — a real-sized
    /// fixture would exceed `UnixStream::pair`'s socket buffer and
    /// deadlock a synchronous `write_all`/`read_to_end` test.
    const TEST_TRIM_BUDGET_BYTES: usize = 32;

    /// Trim boundary: pushing the frozen portion over budget drops the
    /// oldest frozen answers (never the newest, never the in-progress
    /// current answer) and stays under budget afterward.
    #[test]
    fn trim_drops_the_oldest_frozen_answers_and_keeps_the_newest() {
        let mut history = AnswerHistory::with_budget(TEST_TRIM_BUDGET_BYTES);
        // Two answers, each comfortably under budget alone but together
        // over it.
        history.reset_current("oldest:12345678901234".to_string());
        history.freeze_current();
        history.replace_current("newest:12345678901234".to_string());
        history.freeze_current();

        assert!(
            history.frozen.len() > TEST_TRIM_BUDGET_BYTES,
            "test setup must actually exceed the budget"
        );
        let trimmed = history.trim_if_over_budget();
        assert!(trimmed, "over-budget frozen history must report a trim");
        assert!(history.frozen.len() <= TEST_TRIM_BUDGET_BYTES);
        assert!(
            !history.frozen.contains("oldest:"),
            "the oldest frozen answer must be dropped"
        );
        assert!(
            history.frozen.contains("newest:"),
            "the newest frozen answer must survive the trim"
        );
    }

    /// A single frozen answer larger than the whole soft budget is kept
    /// whole rather than partially corrupted — trimming only ever drops
    /// whole answers from the head, and refuses to drop the last one.
    #[test]
    fn trim_never_drops_the_last_remaining_answer_even_if_it_alone_exceeds_budget() {
        let mut history = AnswerHistory::with_budget(TEST_TRIM_BUDGET_BYTES);
        let oversized = "b".repeat(TEST_TRIM_BUDGET_BYTES + 16);
        history.reset_current(oversized.clone());
        history.freeze_current();

        let trimmed = history.trim_if_over_budget();
        assert!(
            !trimmed,
            "a single over-budget answer must be kept whole, not trimmed away"
        );
        assert!(history.frozen.contains(&oversized));
    }

    /// A trim must never happen silently while the current answer is
    /// mid-stream: `emit_current_answer_update` detects the trim and
    /// switches to a full `Document` resync (never a `ReplaceTail`, which
    /// cannot express a change at byte 0) so the viewer's document stays
    /// correct. This is the trim boundary exercised through the actual
    /// wire path.
    #[test]
    fn crossing_the_trim_budget_during_growth_sends_a_document_resync_not_replace_tail() {
        let (mut tx, mut rx) = UnixStream::pair().unwrap();
        rx.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut peer = Some(tx.try_clone().unwrap());
        let mut history = AnswerHistory::with_budget(TEST_TRIM_BUDGET_BYTES);

        // Freeze two answers whose combined bytes exceed the tiny test
        // budget, observed through the `ReplaceTail` vs. `Document` choice
        // on the growth that follows and calls
        // `emit_current_answer_update`.
        emit_transcript_delta(
            &mut peer,
            &mut history,
            TranscriptDelta::Reset("first answer text".to_string()),
        );
        emit_transcript_delta(&mut peer, &mut history, TranscriptDelta::AnswerBoundary);
        emit_transcript_delta(
            &mut peer,
            &mut history,
            TranscriptDelta::Append("second answer text".to_string()),
        );
        emit_transcript_delta(&mut peer, &mut history, TranscriptDelta::AnswerBoundary);
        // Frozen is now over budget. The next Append must trigger a trim
        // and therefore a Document resync.
        emit_transcript_delta(
            &mut peer,
            &mut history,
            TranscriptDelta::Append("final growth".to_string()),
        );
        tx.flush().unwrap();
        drop(tx);
        drop(peer);

        let mut bytes = Vec::new();
        rx.read_to_end(&mut bytes).unwrap();
        let mut decoder = tmath_core::agent::Decoder::new();
        decoder.push(&bytes);
        let mut messages = Vec::new();
        while let Some(message) = decoder.next_message() {
            messages.push(message.unwrap());
        }
        let last = messages.last().unwrap();
        assert!(
            matches!(last, tmath_core::agent::Message::Document { .. }),
            "the append that crosses the trim budget must resync with a \
             full Document, not a ReplaceTail: {last:?}"
        );
        if let tmath_core::agent::Message::Document { text } = last {
            assert!(
                text.ends_with("final growth"),
                "the trimmed resync must still carry the latest growth"
            );
            assert!(
                !text.contains("first answer text"),
                "the resync document must reflect the trim (oldest answer \
                 dropped), not the untrimmed full history"
            );
            assert!(
                text.contains("second answer text"),
                "the newest frozen answer must survive the trim"
            );
        }
    }

    // --- AT-3-604 capture-path rewire ---

    /// The capture path's settle heuristics re-firing on a growing answer
    /// (prefix-extends the previously emitted current answer) must use
    /// `ReplaceTail`, matching the transcript path's growth behavior.
    #[test]
    fn capture_growth_of_the_same_answer_uses_replace_tail() {
        let (mut tx, mut rx) = UnixStream::pair().unwrap();
        rx.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut peer = Some(tx.try_clone().unwrap());
        let mut history = AnswerHistory::new();

        emit_document(&mut peer, &mut history, "The answer".to_string(), false);
        emit_document(
            &mut peer,
            &mut history,
            "The answer is complete.".to_string(),
            true,
        );
        tx.flush().unwrap();
        drop(tx);
        drop(peer);

        let mut bytes = Vec::new();
        rx.read_to_end(&mut bytes).unwrap();
        let mut decoder = tmath_core::agent::Decoder::new();
        decoder.push(&bytes);
        let mut messages = Vec::new();
        while let Some(message) = decoder.next_message() {
            messages.push(message.unwrap());
        }
        assert!(matches!(
            messages[0],
            tmath_core::agent::Message::Document { .. }
        ));
        assert!(
            matches!(messages[1], tmath_core::agent::Message::ReplaceTail { .. }),
            "settle re-fire on a growing answer must ReplaceTail: {:?}",
            messages[1]
        );

        let mut state = tmath_core::agent::DeltaState::new(usize::MAX);
        for message in &messages {
            state.apply(message).unwrap();
        }
        assert_eq!(state.document(), "The answer is complete.");
    }

    /// A genuinely new capture-path answer (not a growth of the previous
    /// one) freezes the previous answer into history instead of replacing
    /// it — proving the capture rewire actually accumulates like the
    /// transcript path does, not just resets per answer as before.
    #[test]
    fn capture_new_answer_freezes_the_previous_one_into_history() {
        let mut history = AnswerHistory::new();
        let mut peer: Option<UnixStream> = None;

        emit_document(&mut peer, &mut history, "First answer.".to_string(), false);
        emit_document(
            &mut peer,
            &mut history,
            "Unrelated second answer.".to_string(),
            false,
        );

        assert_eq!(
            history.document(),
            "First answer.\n\nUnrelated second answer."
        );
    }

    /// AT-3-603 wire-level: a synthesized streaming JSONL fixture replayed
    /// through the transcript adapter and watcher emission path grows the
    /// decoded document incrementally with ReplaceTail frames after the
    /// initial Document resync.
    #[test]
    fn streaming_transcript_fixture_replay_emits_document_then_replace_tails() {
        use std::fs::{self, OpenOptions};
        use std::io::Write as _;

        fn append_line(path: &std::path::Path, line: &str) {
            let mut file = OpenOptions::new().append(true).open(path).unwrap();
            writeln!(file, "{line}").unwrap();
        }

        let lines: Vec<String> =
            include_str!("../../../../tests/fixtures/agents/streaming-transcript.jsonl")
                .lines()
                .filter(|line| !line.trim().is_empty())
                .map(str::to_string)
                .collect();

        let path = std::env::temp_dir().join(format!(
            "tmath-watcher-replay-{}-{}.jsonl",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::write(&path, "").unwrap();

        let (mut tx, mut rx) = UnixStream::pair().unwrap();
        rx.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut peer = Some(tx.try_clone().unwrap());
        let mut history = AnswerHistory::new();
        let mut adapter = TranscriptAdapter::open(&path, TranscriptOpenMode::FromStart).unwrap();

        for line in &lines {
            append_line(&path, line);
            for delta in adapter.poll().unwrap() {
                emit_transcript_delta(&mut peer, &mut history, delta);
            }
        }
        tx.flush().unwrap();
        drop(tx);
        drop(peer);

        let mut bytes = Vec::new();
        rx.read_to_end(&mut bytes).unwrap();
        let mut decoder = tmath_core::agent::Decoder::new();
        decoder.push(&bytes);
        let mut messages = Vec::new();
        while let Some(message) = decoder.next_message() {
            messages.push(message.unwrap());
        }
        let mut state = tmath_core::agent::DeltaState::new(usize::MAX);
        for message in &messages {
            state.apply(message).unwrap();
        }

        assert!(
            matches!(
                messages.first(),
                Some(tmath_core::agent::Message::Document { .. })
            ),
            "first frame must be a Document resync: {messages:?}"
        );
        assert!(
            messages
                .iter()
                .skip(1)
                .any(|message| matches!(message, tmath_core::agent::Message::ReplaceTail { .. })),
            "streaming growth must use ReplaceTail after the first Document: {messages:?}"
        );
        assert!(
            state.document().contains("Linear regression"),
            "reassembled document must contain fixture prose"
        );
        assert!(
            state.document().contains("Gauss–Markov"),
            "reassembled document must contain later streamed paragraphs"
        );
        let _ = fs::remove_file(path);
    }
}
