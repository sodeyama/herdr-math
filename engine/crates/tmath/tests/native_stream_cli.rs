//! Incremental native CLI stream behavior (AT-3-402 and AT-3-403).

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

const DEADLINE: Duration = Duration::from_secs(20);

struct StreamProcess {
    child: Child,
    stdin: Option<std::process::ChildStdin>,
    lines: mpsc::Receiver<String>,
}

impl StreamProcess {
    fn spawn() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_tmath"))
            .args(["render", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().unwrap();
        let lines = line_reader(stdout);
        Self {
            child,
            stdin,
            lines,
        }
    }

    fn write(&mut self, bytes: &[u8]) {
        let stdin = self.stdin.as_mut().unwrap();
        stdin.write_all(bytes).unwrap();
        stdin.flush().unwrap();
    }

    fn close_stdin(&mut self) {
        self.stdin.take();
    }

    fn next_line(&self) -> String {
        self.lines
            .recv_timeout(DEADLINE)
            .expect("stream event deadline elapsed")
    }

    fn finish(mut self) -> Vec<String> {
        self.close_stdin();
        let mut lines = Vec::new();
        while let Ok(line) = self.lines.recv_timeout(DEADLINE) {
            lines.push(line);
        }
        let output = self.child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "stream failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        lines
    }
}

fn line_reader(stdout: ChildStdout) -> mpsc::Receiver<String> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else {
                return;
            };
            if sender.send(line).is_err() {
                return;
            }
        }
    });
    receiver
}

fn field<'a>(line: &'a str, name: &str) -> &'a str {
    line.split_whitespace()
        .find_map(|part| part.strip_prefix(&format!("{name}=")))
        .unwrap_or_else(|| panic!("missing {name} in {line:?}"))
}

fn one_shot_bytes(source: &str) -> usize {
    let root = std::env::temp_dir().join(format!(
        "tmath-native-stream-one-shot-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let input = root.join("input.md");
    std::fs::write(&input, source).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_tmath"))
        .args(["render", input.to_str().unwrap()])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(root);
    assert!(
        output.status.success(),
        "one-shot failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    field(&stdout, "bytes").parse().unwrap()
}

#[test]
fn at_3_402_emits_completed_block_while_stdin_remains_open() {
    let mut process = StreamProcess::spawn();
    process.write(b"First block.\n\n");

    let first = process.next_line();
    assert!(first.starts_with("event=append id=1 "), "{first}");

    process.write(b"Second block.\n");
    let rest = process.finish();
    assert!(
        rest.iter()
            .any(|line| line.starts_with("event=append id=2 ")),
        "{rest:?}"
    );
    assert_eq!(
        rest.last().map(String::as_str),
        Some("event=done blocks=2 formula_errors=0")
    );
}

#[test]
fn at_3_403_tail_updates_coalesce_and_match_one_shot_bytes() {
    let chunks = 120usize;
    let mut source = String::from("Growing");
    let mut process = StreamProcess::spawn();
    process.write(source.as_bytes());
    let first = process.next_line();
    assert!(first.starts_with("event=append id=1 "), "{first}");

    for index in 0..chunks {
        let chunk = format!(" {index}");
        source.push_str(&chunk);
        process.write(chunk.as_bytes());
    }

    let lines = process.finish();
    let replaces = lines
        .iter()
        .filter(|line| line.starts_with("event=replace "))
        .count();
    assert!(replaces >= 1, "tail was never replaced: {lines:?}");
    assert!(
        replaces < chunks,
        "latest-wins coalescing did not reduce renders: {replaces} replacements for {chunks} chunks"
    );
    let last_render = lines
        .iter()
        .rev()
        .find(|line| line.starts_with("event=append ") || line.starts_with("event=replace "))
        .unwrap();
    assert_eq!(
        field(last_render, "bytes").parse::<usize>().unwrap(),
        one_shot_bytes(&source)
    );
}

#[test]
fn unchanged_prefix_id_is_never_mentioned_again() {
    let mut process = StreamProcess::spawn();
    process.write(b"Stable prefix.\n\n");
    let first = process.next_line();
    assert!(first.starts_with("event=append id=1 "), "{first}");

    process.write(b"Tail");
    process.write(b" grows");
    let lines = process.finish();
    assert!(
        lines.iter().all(|line| !line
            .split_whitespace()
            .skip(1)
            .any(|field| { field == "id=1" || field == "old=1" })),
        "{lines:?}"
    );
}

#[test]
fn repeated_content_reports_a_cache_hit() {
    let output = Command::new(env!("CARGO_BIN_EXE_tmath"))
        .args(["render", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(b"Repeat.\n\nRepeat.\n\n")?;
            child.wait_with_output()
        })
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout
            .lines()
            .any(|line| line.starts_with("event=append id=2 ") && line.ends_with("cache=hit")),
        "{stdout}"
    );
}

#[test]
fn stream_error_is_safe_json_without_input() {
    let marker = "STREAM_PRIVATE_MARKER";
    let oversized = format!("{marker}{}", "x".repeat(70 * 1024));
    let output = Command::new(env!("CARGO_BIN_EXE_tmath"))
        .args(["render", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(oversized.as_bytes())?;
            child.wait_with_output()
        })
        .unwrap();
    assert!(!output.status.success());
    let mut stderr = String::new();
    output
        .stderr
        .as_slice()
        .read_to_string(&mut stderr)
        .unwrap();
    assert!(!stderr.contains(marker), "{stderr}");
    let record: serde_json::Value = serde_json::from_str(stderr.trim()).unwrap();
    assert!(
        matches!(
            record["code"].as_str(),
            Some("renderer_input_limit" | "renderer_timeout")
        ),
        "{record}"
    );
}

/// AT-3-103, stream-mode contract: a revision containing an invalid
/// display-math formula must apply normally — an `append`/`replace` event
/// for the block plus `event=done ... formula_errors=N` at N >= 1 — rather
/// than the whole run aborting with a hard JSON error record the way it did
/// before this fix (team-lead's exact repro:
/// `printf '$$x \badcmdxyz$$\n' | tmath render --engine native -`).
#[test]
fn at_3_103_invalid_display_math_applies_the_revision_with_formula_errors_incremented() {
    let mut process = StreamProcess::spawn();
    process.write(b"$$x \\badcmdxyz$$\n");

    let first = process.next_line();
    assert!(
        first.starts_with("event=append id=1 "),
        "bad display math must still place a (badge) block, not abort: {first}"
    );

    let rest = process.finish();
    assert_eq!(
        rest.last().map(String::as_str),
        Some("event=done blocks=1 formula_errors=1"),
        "{rest:?}"
    );
}

/// Sibling blocks around a bad display formula must render too — proves
/// AT-3-103's "sibling formulas and all other blocks render normally" at
/// the stream-event level, not just the direct `render_block` level
/// `lib.rs`'s unit tests already cover.
#[test]
fn at_3_103_sibling_blocks_around_a_bad_display_formula_still_place() {
    let mut process = StreamProcess::spawn();
    process.write(b"Before.\n\n$$x \\badcmdxyz$$\n\nAfter.\n");

    let events: Vec<String> = std::iter::from_fn(|| Some(process.next_line()))
        .take(2)
        .collect();
    assert!(
        events
            .iter()
            .any(|line| line.starts_with("event=append id=1 ")),
        "{events:?}"
    );
    assert!(
        events
            .iter()
            .any(|line| line.starts_with("event=append id=2 ")),
        "{events:?}"
    );

    let rest = process.finish();
    assert!(
        rest.iter()
            .any(|line| line.starts_with("event=append id=3 ")),
        "the block after the bad formula must still place: {rest:?}"
    );
    assert_eq!(
        rest.last().map(String::as_str),
        Some("event=done blocks=3 formula_errors=1")
    );
}
