//! Claude Code transcript adapter (D5, AT-3-602): tails a session's JSONL
//! transcript file and extracts assistant Markdown text deltas, read-only
//! and bounded. This is the *preferred* watcher source — it yields the
//! original Markdown source with no box-table reverse-conversion, no
//! repaint stripping, and no settle heuristics — with the tmux
//! `capture-pane` adapter (`agent_watcher.rs`'s existing `find_answer` path)
//! staying as the automatic fallback whenever a transcript file is
//! unavailable or its content stops making sense.
//!
//! Format note: Claude Code's JSONL transcript is not a public, versioned
//! contract (see `AGENTS.md`'s sources-of-truth precedence and the plan's
//! "Transcript format drift" risk entry). This adapter reads it
//! defensively: every line is independently parsed, an unrecognized shape
//! is skipped rather than treated as an error, and any read/parse failure
//! degrades to the capture adapter rather than panicking.
//!
//! # Observed structure (this adapter's contract with the file)
//!
//! One JSON object per line. Relevant fields, from live inspection of real
//! transcripts (never persisted as fixtures — synthesized JSONL is used for
//! tests instead, per `AGENTS.md`'s privacy rules):
//!
//! - `{"type": "user", ...}` — a new human turn. Marks the end of whatever
//!   assistant answer was accumulating; the next assistant text starts a
//!   fresh answer.
//! - `{"type": "assistant", "message": {"content": [...]}}` — one assistant
//!   turn fragment. `content` is a list of blocks; this adapter only reads
//!   blocks shaped `{"type": "text", "text": "..."}` and ignores every other
//!   block type (`thinking`, `tool_use`, `tool_result`, or anything else,
//!   present or future) as opaque and irrelevant.
//! - Every other top-level `type` (`attachment`, `last-prompt`, `ai-title`,
//!   `mode`, `permission-mode`, and anything not yet observed) is ignored.
//!
//! Each observed transcript records one `text` block per completed
//! paragraph-level chunk rather than growing one block's `text` field
//! in place across multiple lines, so "streamed append" here means new
//! *lines* (new blocks) arriving across polls, not a single block's text
//! value changing between reads.

use std::fs::File;
use std::io::{self, Read as _, Seek as _, SeekFrom};
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

/// Bytes read from the transcript file in one `poll()` call (D7's
/// "transcript read bytes per poll" limit). Large enough for many typical
/// answer paragraphs per poll, small enough that one poll cannot block on
/// an unbounded read.
const TRANSCRIPT_READ_BYTES_PER_POLL: usize = 1024 * 1024;
/// Maximum Markdown text extracted from a single `text` content block (or
/// several joined with "\n\n" from one message). Mirrors the order of
/// magnitude of `IPC_MAX_REQUEST_BYTES`'s per-message cap; a block larger
/// than this is truncated rather than forwarded whole, since the delta
/// protocol and renderer both have their own finite caps downstream.
const TRANSCRIPT_MAX_BLOCK_BYTES: usize = 8 * 1024 * 1024;
/// Maximum bytes carried across polls waiting for one JSONL line to
/// complete. A line larger than this is a malformed/hostile transcript and
/// is dropped fail-closed rather than grown without bound. Kept strictly
/// above `TRANSCRIPT_MAX_BLOCK_BYTES` (plus slack for JSON structure/escaping
/// overhead around the `text` field) so the block-level truncation path is
/// actually reachable instead of being shadowed by this line cap.
const TRANSCRIPT_MAX_LINE_BYTES: usize = 12 * 1024 * 1024;

/// One assistant Markdown delta extracted from newly read transcript lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TranscriptDelta {
    /// A new answer began: replace the current document with this text
    /// (mirrors a `Document` frame).
    Reset(String),
    /// The current answer grew: append this text (mirrors an `Append`
    /// frame).
    Append(String),
}

/// Why a transcript adapter could not be used, causing the watcher to fall
/// back to the capture adapter. Carries no content, only the underlying
/// I/O error (open failure, a rotated file whose reopen also failed, or any
/// other read error).
#[derive(Debug)]
pub(crate) struct TranscriptError(#[allow(dead_code)] io::Error);

impl From<io::Error> for TranscriptError {
    fn from(error: io::Error) -> Self {
        Self(error)
    }
}

/// Tails one transcript file, read-only, and extracts assistant Markdown
/// deltas from newly appended lines. Bounded per poll and resilient to
/// rotation (inode or size regression) and truncation (mid-line EOF).
pub(crate) struct TranscriptAdapter {
    path: PathBuf,
    file: File,
    inode: u64,
    offset: u64,
    /// Bytes of an incomplete trailing line, carried to the next poll.
    carry: Vec<u8>,
    /// Whether the next `text` block should start a fresh answer (`Reset`)
    /// rather than extend the current one (`Append`). Starts `true` — the
    /// first `text` block this adapter instance ever sees (right after
    /// `open`, or right after a rotation reopen) is always a `Reset`, since
    /// there is no in-progress answer yet from this instance's point of
    /// view. Set `true` again after a `user` turn and consumed by the next
    /// `text` block seen afterward.
    awaiting_new_answer: bool,
}

impl TranscriptAdapter {
    /// Opens `path` at its current end (only newly appended bytes are ever
    /// read — the adapter never loads or re-scans history already on
    /// disk).
    pub(crate) fn open(path: &Path) -> Result<Self, TranscriptError> {
        let file = File::open(path)?;
        let metadata = file.metadata()?;
        Ok(Self {
            path: path.to_path_buf(),
            inode: metadata.ino(),
            offset: metadata.len(),
            file,
            carry: Vec::new(),
            awaiting_new_answer: true,
        })
    }

    /// Reads up to `TRANSCRIPT_READ_BYTES_PER_POLL` new bytes, parses every
    /// complete line, and returns the deltas they produced (possibly
    /// empty). Detects rotation (a new inode at `path`, or the file
    /// shrinking under the tracked offset) and reopens from the start of
    /// the new file, discarding any carried partial line — a rotated file
    /// is a new stream. A read/reopen failure is reported as
    /// [`TranscriptError`]; the caller is expected to fall back to the
    /// capture adapter rather than treat this as fatal.
    pub(crate) fn poll(&mut self) -> Result<Vec<TranscriptDelta>, TranscriptError> {
        self.reopen_if_rotated()?;

        let mut buffer = vec![0u8; TRANSCRIPT_READ_BYTES_PER_POLL];
        self.file.seek(SeekFrom::Start(self.offset))?;
        let read = self.file.read(&mut buffer)?;
        if read == 0 {
            return Ok(Vec::new());
        }
        buffer.truncate(read);
        self.offset += read as u64;

        self.carry.extend_from_slice(&buffer);
        if self.carry.len() > TRANSCRIPT_MAX_LINE_BYTES {
            // A pathological single "line" (or a stream with no newlines
            // at all) must not grow the carry buffer without bound.
            self.carry.clear();
        }

        let mut deltas = Vec::new();
        while let Some(newline_at) = self.carry.iter().position(|&byte| byte == b'\n') {
            let line = self.carry.drain(..=newline_at).collect::<Vec<u8>>();
            // Drop the trailing newline itself before parsing.
            let line = &line[..line.len() - 1];
            if let Some(delta) = parse_transcript_line(line, &mut self.awaiting_new_answer) {
                deltas.push(delta);
            }
        }
        Ok(deltas)
    }

    fn reopen_if_rotated(&mut self) -> Result<(), TranscriptError> {
        let current = std::fs::metadata(&self.path)?;
        let rotated = current.ino() != self.inode || current.len() < self.offset;
        if !rotated {
            return Ok(());
        }
        let reopened = File::open(&self.path)?;
        let metadata = reopened.metadata()?;
        self.file = reopened;
        self.inode = metadata.ino();
        self.offset = 0;
        self.carry.clear();
        // A rotated file is a new stream from this adapter's point of view:
        // the first `text` block read from it is a `Reset`, exactly like
        // the first block after `open` (see the field doc on
        // `awaiting_new_answer`).
        self.awaiting_new_answer = true;
        Ok(())
    }
}

/// Parses one complete JSONL line (without its trailing newline) into at
/// most one delta. Fails closed: malformed JSON, an unrecognized shape, or
/// a block over the size cap all yield `None` rather than an error — a bad
/// line is skipped, never a reason to stop reading the transcript. Never
/// logs or otherwise surfaces the line's content.
fn parse_transcript_line(line: &[u8], awaiting_new_answer: &mut bool) -> Option<TranscriptDelta> {
    if line.len() > TRANSCRIPT_MAX_LINE_BYTES {
        return None;
    }
    let value: serde_json::Value = serde_json::from_slice(line).ok()?;
    let kind = value.get("type")?.as_str()?;

    if kind == "user" {
        *awaiting_new_answer = true;
        return None;
    }
    if kind != "assistant" {
        return None;
    }

    let content = value.get("message")?.get("content")?.as_array()?;
    // Multiple `text` blocks in one message are distinct Markdown blocks
    // (e.g. text -> tool_use -> text), not one continuous paragraph, so
    // join them with a blank line rather than concatenating them raw.
    let mut text = String::new();
    for block in content {
        if block.get("type").and_then(|value| value.as_str()) != Some("text") {
            continue;
        }
        if let Some(block_text) = block.get("text").and_then(|value| value.as_str()) {
            if !text.is_empty() {
                text.push_str("\n\n");
            }
            text.push_str(block_text);
        }
    }
    if text.is_empty() {
        return None;
    }
    if text.len() > TRANSCRIPT_MAX_BLOCK_BYTES {
        // `String::truncate` panics if the cut point is not on a char
        // boundary, which an 8 MiB cut through multibyte text (e.g.
        // Japanese) will eventually hit. Walk the cut point back to the
        // nearest earlier char boundary first so this never crashes the
        // watcher on adversarial or merely large multibyte input.
        let mut cut = TRANSCRIPT_MAX_BLOCK_BYTES;
        while cut > 0 && !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
    }

    let is_reset = std::mem::take(awaiting_new_answer);
    if is_reset {
        Some(TranscriptDelta::Reset(text))
    } else {
        // Each Append is a new message in the same in-progress answer;
        // separate it from whatever came before with a blank line so the
        // reassembled document keeps its Markdown block structure instead
        // of fusing message boundaries into one paragraph.
        Some(TranscriptDelta::Append(format!("\n\n{text}")))
    }
}

/// Resolves the Claude Code transcript directory for a working directory,
/// mirroring Claude Code's own project-slug rule: the absolute path with
/// every `/` replaced by `-`. Returns `None` when `cwd` is not absolute
/// (the rule is only meaningful for absolute paths) or `home` is empty.
pub(crate) fn project_transcript_dir(home: &Path, cwd: &Path) -> Option<PathBuf> {
    if !cwd.is_absolute() {
        return None;
    }
    let slug = cwd.to_str()?.replace('/', "-");
    Some(home.join(".claude").join("projects").join(slug))
}

/// Picks the most recently modified `*.jsonl` file directly inside `dir`,
/// or `None` when the directory does not exist or has no transcript files.
/// This is a best-effort heuristic for "the session currently running in
/// the watched pane" — multiple sessions can share a project directory, and
/// the newest-modified file is the one most likely still being appended
/// to. A wrong pick degrades gracefully: `TranscriptAdapter` only ever
/// tails forward from the moment it opens the file, and the watcher falls
/// back to the capture adapter if the chosen file goes quiet or errors.
pub(crate) fn newest_transcript_file(dir: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("jsonl") {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        if newest.as_ref().is_none_or(|(when, _)| modified > *when) {
            newest = Some((modified, path));
        }
    }
    newest.map(|(_, path)| path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    fn write_jsonl(path: &Path, lines: &[&str]) {
        let mut file = File::create(path).unwrap();
        for line in lines {
            writeln!(file, "{line}").unwrap();
        }
    }

    fn append_jsonl(path: &Path, lines: &[&str]) {
        let mut file = std::fs::OpenOptions::new().append(true).open(path).unwrap();
        for line in lines {
            writeln!(file, "{line}").unwrap();
        }
    }

    fn user_line() -> String {
        r#"{"type":"user","message":{"content":[{"type":"text","text":"hi"}]}}"#.to_string()
    }

    fn assistant_text_line(text: &str) -> String {
        serde_json::json!({
            "type": "assistant",
            "message": {"content": [{"type": "text", "text": text}]}
        })
        .to_string()
    }

    fn assistant_thinking_line() -> String {
        r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"..."}]}}"#
            .to_string()
    }

    fn temp_path(name: &str) -> PathBuf {
        // `SystemTime::now()` alone is not fine-grained enough to stay
        // unique across threads running tests in parallel (the OS clock's
        // effective resolution can coarsen under load), so pair it with a
        // process-wide atomic counter that guarantees every call gets a
        // distinct directory even when two threads call this within the
        // same tick.
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let sequence = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "tmath-transcript-test-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            sequence
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn a_single_assistant_text_block_is_a_reset_when_no_prior_answer() {
        let path = temp_path("t.jsonl");
        write_jsonl(&path, &[&assistant_text_line("Hello.")]);
        // `TranscriptAdapter::open` starts at EOF (never re-scans history);
        // `reopen_from_start` is the test helper that reads fixture content
        // written before the adapter existed.
        let mut adapter = reopen_from_start(&path);
        let deltas = adapter.poll().unwrap();
        assert_eq!(deltas, vec![TranscriptDelta::Reset("Hello.".to_string())]);
    }

    #[test]
    fn open_starts_at_eof_and_never_re_scans_history_already_on_disk() {
        let path = temp_path("t.jsonl");
        write_jsonl(&path, &[&assistant_text_line("Already on disk.")]);
        let mut adapter = TranscriptAdapter::open(&path).unwrap();
        assert_eq!(
            adapter.poll().unwrap(),
            Vec::new(),
            "open() starts at the current end of file, so pre-existing \
             content is never re-read"
        );
    }

    #[test]
    fn streamed_append_across_multiple_polls_yields_append_deltas() {
        let path = temp_path("t.jsonl");
        write_jsonl(&path, &[&assistant_text_line("First paragraph.")]);
        let mut adapter = reopen_from_start(&path);
        // The first block this adapter instance ever reads is a Reset.
        assert_eq!(
            adapter.poll().unwrap(),
            vec![TranscriptDelta::Reset("First paragraph.".to_string())]
        );

        append_jsonl(&path, &[&assistant_text_line("Second paragraph.")]);
        assert_eq!(
            adapter.poll().unwrap(),
            vec![TranscriptDelta::Append("\n\nSecond paragraph.".to_string())]
        );

        append_jsonl(&path, &[&assistant_text_line("Third paragraph.")]);
        assert_eq!(
            adapter.poll().unwrap(),
            vec![TranscriptDelta::Append("\n\nThird paragraph.".to_string())]
        );
    }

    #[test]
    fn a_user_turn_resets_the_next_assistant_text_to_a_new_answer() {
        let path = temp_path("t.jsonl");
        write_jsonl(&path, &[&assistant_text_line("Old answer.")]);
        let mut adapter = TranscriptAdapter::open(&path).unwrap();
        assert_eq!(adapter.poll().unwrap(), Vec::new());

        append_jsonl(
            &path,
            &[
                &user_line(),
                &assistant_text_line("New answer starts here."),
            ],
        );
        assert_eq!(
            adapter.poll().unwrap(),
            vec![TranscriptDelta::Reset(
                "New answer starts here.".to_string()
            )]
        );
    }

    #[test]
    fn a_multi_message_answer_across_several_text_blocks_appends_each() {
        let path = temp_path("t.jsonl");
        write_jsonl(&path, &[&assistant_text_line("Part one.")]);
        let mut adapter = reopen_from_start(&path);
        adapter.poll().unwrap(); // consumes the initial Reset for "Part one."

        append_jsonl(
            &path,
            &[
                &assistant_thinking_line(), // ignored, not a text block
                &assistant_text_line("Part two."),
                &assistant_text_line("Part three."),
            ],
        );
        assert_eq!(
            adapter.poll().unwrap(),
            vec![
                TranscriptDelta::Append("\n\nPart two.".to_string()),
                TranscriptDelta::Append("\n\nPart three.".to_string()),
            ]
        );
    }

    #[test]
    fn non_text_blocks_and_unknown_types_are_ignored_without_error() {
        let path = temp_path("t.jsonl");
        write_jsonl(
            &path,
            &[
                r#"{"type":"mode","mode":"default"}"#,
                r#"{"type":"attachment","attachment":{}}"#,
                &assistant_thinking_line(),
                r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash"}]}}"#,
            ],
        );
        let mut adapter = TranscriptAdapter::open(&path).unwrap();
        drop(adapter);
        adapter = reopen_from_start(&path);
        assert_eq!(adapter.poll().unwrap(), Vec::new());
    }

    #[test]
    fn malformed_json_lines_are_skipped_without_crashing() {
        let path = temp_path("t.jsonl");
        write_jsonl(
            &path,
            &[
                "{not valid json",
                &assistant_text_line("Good line survives."),
                "{\"type\": \"assistant\", \"message\": {\"content\": \"not-an-array\"}}",
            ],
        );
        let adapter = reopen_from_start(&path);
        let mut adapter = adapter;
        let deltas = adapter.poll().unwrap();
        assert_eq!(
            deltas,
            vec![TranscriptDelta::Reset("Good line survives.".to_string())]
        );
    }

    // AT-3-602 supervisor fix 2: `String::truncate` panics if the cut point
    // is not on a char boundary. A block just over the byte cap, built so
    // the cap lands mid-way through a multibyte (3-byte) character, must
    // not crash `parse_transcript_line` and must produce a valid UTF-8
    // string truncated at or before the cap.
    #[test]
    fn a_block_over_the_byte_cap_truncates_without_panicking_on_a_multibyte_boundary() {
        // Fill up to one byte short of the cap with ASCII, then add a
        // 3-byte character (U+3042, hiragana "あ") straddling the cap: the
        // raw byte index `TRANSCRIPT_MAX_BLOCK_BYTES` lands inside it.
        let mut text = "a".repeat(TRANSCRIPT_MAX_BLOCK_BYTES - 1);
        text.push('あ');
        text.push_str("tail beyond the cap");
        let line = serde_json::json!({
            "type": "assistant",
            "message": {"content": [{"type": "text", "text": text}]}
        })
        .to_string();

        let mut awaiting_new_answer = true;
        let delta = parse_transcript_line(line.as_bytes(), &mut awaiting_new_answer);

        let TranscriptDelta::Reset(result) = delta.expect("a text block yields a delta") else {
            panic!("first block is always a Reset");
        };
        assert!(
            result.len() <= TRANSCRIPT_MAX_BLOCK_BYTES,
            "truncated to at or below the cap"
        );
        assert!(
            result.len() >= TRANSCRIPT_MAX_BLOCK_BYTES - 3,
            "cut lands on the nearest earlier char boundary, not far short of the cap"
        );
        assert!(
            result.starts_with(&"a".repeat(TRANSCRIPT_MAX_BLOCK_BYTES - 1)),
            "leading ASCII survives untouched"
        );
        assert!(
            !result.contains("tail beyond the cap"),
            "content past the cap is dropped"
        );
    }

    #[test]
    fn a_truncated_final_line_waits_for_more_bytes_rather_than_erroring() {
        let path = temp_path("t.jsonl");
        let mut file = File::create(&path).unwrap();
        // No trailing newline: an in-progress, incomplete write.
        write!(file, "{}", &assistant_text_line("Complete.")).unwrap();
        drop(file);
        let mut adapter = reopen_from_start(&path);
        assert_eq!(
            adapter.poll().unwrap(),
            Vec::new(),
            "an incomplete final line produces no delta yet"
        );

        // The line completes on a later write.
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(file).unwrap();
        drop(file);
        assert_eq!(
            adapter.poll().unwrap(),
            vec![TranscriptDelta::Reset("Complete.".to_string())]
        );
    }

    #[test]
    fn rotation_to_a_new_inode_reopens_from_the_start_and_drops_carry() {
        let path = temp_path("t.jsonl");
        write_jsonl(&path, &[&assistant_text_line("Before rotation.")]);
        let mut adapter = reopen_from_start(&path);
        assert_eq!(
            adapter.poll().unwrap(),
            vec![TranscriptDelta::Reset("Before rotation.".to_string())]
        );

        // Simulate log rotation: remove and recreate the file (a new inode
        // on most filesystems), with fresh content from byte 0.
        std::fs::remove_file(&path).unwrap();
        write_jsonl(&path, &[&assistant_text_line("After rotation.")]);
        assert_eq!(
            adapter.poll().unwrap(),
            vec![TranscriptDelta::Reset("After rotation.".to_string())],
            "rotation is detected and the new file is read from its own start"
        );
    }

    #[test]
    fn shrinking_below_the_tracked_offset_is_treated_as_rotation() {
        let path = temp_path("t.jsonl");
        write_jsonl(
            &path,
            &[
                &assistant_text_line("Long content that takes up some space."),
                &assistant_text_line("More content."),
            ],
        );
        let mut adapter = reopen_from_start(&path);
        adapter.poll().unwrap();

        // Truncate in place (same inode, smaller size) — some rotation
        // schemes do this instead of unlink+recreate.
        let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.set_len(0).unwrap();
        drop(file);
        write_jsonl(&path, &[&assistant_text_line("Fresh short content.")]);
        assert_eq!(
            adapter.poll().unwrap(),
            vec![TranscriptDelta::Reset("Fresh short content.".to_string())]
        );
    }

    #[test]
    fn an_absent_file_reports_an_error_rather_than_panicking() {
        let path = temp_path("does-not-exist.jsonl");
        assert!(TranscriptAdapter::open(&path).is_err());
    }

    #[test]
    fn empty_file_polls_cleanly_with_no_deltas() {
        let path = temp_path("t.jsonl");
        File::create(&path).unwrap();
        let mut adapter = reopen_from_start(&path);
        assert_eq!(adapter.poll().unwrap(), Vec::new());
    }

    #[test]
    fn project_transcript_dir_replaces_slashes_with_hyphens() {
        // The example home deliberately avoids macOS/Linux home-directory
        // prefixes so the no-absolute-home-path privacy gate stays clean.
        let home = Path::new("/opt/example");
        let cwd = Path::new("/opt/example/git/terminal-math");
        assert_eq!(
            project_transcript_dir(home, cwd).unwrap(),
            Path::new("/opt/example/.claude/projects/-opt-example-git-terminal-math")
        );
    }

    #[test]
    fn project_transcript_dir_rejects_a_relative_cwd() {
        let home = Path::new("/opt/example");
        let cwd = Path::new("relative/path");
        assert!(project_transcript_dir(home, cwd).is_none());
    }

    #[test]
    fn newest_transcript_file_picks_the_most_recently_modified_jsonl() {
        let dir = temp_path("project-dir");
        std::fs::create_dir_all(&dir).unwrap();
        let older = dir.join("older.jsonl");
        let newer = dir.join("newer.jsonl");
        let not_jsonl = dir.join("notes.txt");
        File::create(&older).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        File::create(&newer).unwrap();
        File::create(&not_jsonl).unwrap();
        assert_eq!(newest_transcript_file(&dir), Some(newer));
    }

    #[test]
    fn newest_transcript_file_is_none_for_a_missing_directory() {
        let dir = temp_path("missing").join("really-missing");
        assert_eq!(newest_transcript_file(&dir), None);
    }

    /// Test helper: opens `path` fresh with the offset pinned to 0, since
    /// `TranscriptAdapter::open` always starts at EOF (the adapter's
    /// documented "never re-scan history" behavior) and most tests want to
    /// read fixture content written before the adapter existed.
    fn reopen_from_start(path: &Path) -> TranscriptAdapter {
        let file = File::open(path).unwrap();
        let inode = file.metadata().unwrap().ino();
        TranscriptAdapter {
            path: path.to_path_buf(),
            file,
            inode,
            offset: 0,
            carry: Vec::new(),
            awaiting_new_answer: true,
        }
    }
}
