//! AT-3-405: event-driven changed-block rendering for `tmath watch`.

use std::fs;
use std::io::{BufRead as _, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use rustix::process::{kill_process, Pid, Signal};

const DEADLINE: Duration = Duration::from_secs(20);
const QUIET_WINDOW: Duration = Duration::from_millis(300);

struct WatchProcess {
    child: Option<Child>,
    lines: mpsc::Receiver<String>,
    stderr: mpsc::Receiver<String>,
}

impl WatchProcess {
    fn spawn(path: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_tmath"))
            .args(["watch", path.to_str().unwrap()])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let lines = line_reader(child.stdout.take().unwrap());
        let stderr = byte_reader(child.stderr.take().unwrap());
        Self {
            child: Some(child),
            lines,
            stderr,
        }
    }

    fn next_line(&self) -> String {
        self.lines
            .recv_timeout(DEADLINE)
            .expect("watch event deadline elapsed")
    }

    fn assert_quiet(&self) {
        match self.lines.recv_timeout(QUIET_WINDOW) {
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            other => panic!("watch emitted while the file was unchanged: {other:?}"),
        }
    }

    fn is_alive(&mut self) -> bool {
        self.child
            .as_mut()
            .expect("watch child already consumed")
            .try_wait()
            .unwrap()
            .is_none()
    }

    fn terminate(&mut self) {
        let child = self.child.take().expect("watch child already consumed");
        let pid = Pid::from_raw(child.id() as i32).unwrap();
        kill_process(pid, Signal::TERM).unwrap();
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(child.wait_with_output());
        });
        let output = receiver
            .recv_timeout(DEADLINE)
            .expect("watch did not exit after SIGTERM")
            .unwrap();
        assert!(
            output.status.success(),
            "SIGTERM did not produce a clean exit: {:?}",
            output.status
        );
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

fn byte_reader(mut stderr: impl std::io::Read + Send + 'static) -> mpsc::Receiver<String> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut output = String::new();
        let _ = stderr.read_to_string(&mut output);
        let _ = sender.send(output);
    });
    receiver
}

struct Sandbox {
    root: PathBuf,
    target: PathBuf,
    generation: usize,
}

impl Sandbox {
    fn new(name: &str, source: &str) -> Self {
        let root = std::env::temp_dir().join(format!("tmath-watch-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let target = root.join("document.md");
        fs::write(&target, source).unwrap();
        Self {
            root,
            target,
            generation: 0,
        }
    }

    fn atomic_save(&mut self, source: &str) {
        self.generation += 1;
        let temporary = self.root.join(format!("swap-{}.tmp", self.generation));
        fs::write(&temporary, source).unwrap();
        fs::rename(temporary, &self.target).unwrap();
    }

    fn remove(&self) {
        fs::remove_file(&self.target).unwrap();
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn initial_three(process: &WatchProcess) -> [String; 3] {
    let lines = [
        process.next_line(),
        process.next_line(),
        process.next_line(),
    ];
    for (index, line) in lines.iter().enumerate() {
        assert!(
            line.starts_with(&format!("event=append id={} ", index + 1)),
            "{lines:?}"
        );
    }
    lines
}

fn mentions_id(line: &str, id: u64) -> bool {
    line.split_whitespace()
        .any(|field| field == format!("id={id}") || field == format!("old={id}"))
}

#[test]
fn at_3_405_is_event_driven_and_renders_only_changed_blocks() {
    let mut sandbox = Sandbox::new(
        "minimality",
        "First block.\n\nSecond block.\n\nThird block.\n",
    );
    let mut process = WatchProcess::spawn(&sandbox.target);
    initial_three(&process);
    process.assert_quiet();

    sandbox.atomic_save("First block.\n\nSecond block.\n\nChanged third block.\n");
    let replace = process.next_line();
    assert!(
        replace.starts_with("event=replace old=3 id=4 "),
        "{replace}"
    );
    assert!(!mentions_id(&replace, 1), "{replace}");
    assert!(!mentions_id(&replace, 2), "{replace}");
    process.assert_quiet();

    sandbox.atomic_save("First block.\n\nSecond block.\n\nChanged third block.\n\nFourth block.\n");
    let append = process.next_line();
    assert!(append.starts_with("event=append id=5 "), "{append}");
    process.assert_quiet();
    process.terminate();
}

#[test]
fn at_3_405_survives_bad_atomic_save_and_recovers() {
    let mut sandbox = Sandbox::new(
        "recovery",
        "First block.\n\nSecond block.\n\nThird block.\n",
    );
    let mut process = WatchProcess::spawn(&sandbox.target);
    initial_three(&process);

    let oversized = format!(
        "First block.\n\nSecond block.\n\n{}",
        "x".repeat(64 * 1024 + 1)
    );
    sandbox.atomic_save(&oversized);
    let error = process.next_line();
    assert!(
        error.starts_with("event=error code=renderer_input_limit"),
        "{error}"
    );
    assert!(process.is_alive());

    sandbox.atomic_save("First block.\n\nSecond block.\n\nRecovered third block.\n");
    let replace = process.next_line();
    assert!(
        replace.starts_with("event=replace old=3 id=4 "),
        "{replace}"
    );
    assert!(process.is_alive());

    process.terminate();
    let stderr = process
        .stderr
        .recv_timeout(DEADLINE)
        .expect("stderr reader did not finish");
    let record = stderr.lines().next().expect("missing safe error record");
    let record: serde_json::Value = serde_json::from_str(record).unwrap();
    assert_eq!(record["code"], "renderer_input_limit");
}

#[test]
fn missing_file_waits_once_and_reappearing_file_is_diffed() {
    let mut sandbox = Sandbox::new("missing", "First block.\n\nSecond block.\n\nThird block.\n");
    let mut process = WatchProcess::spawn(&sandbox.target);
    initial_three(&process);

    sandbox.remove();
    assert_eq!(process.next_line(), "event=waiting");
    process.assert_quiet();

    sandbox.atomic_save("First block.\n\nSecond block.\n\nRestored third block.\n");
    let replace = process.next_line();
    assert!(
        replace.starts_with("event=replace old=3 id=4 "),
        "{replace}"
    );
    process.assert_quiet();
    process.terminate();
}

#[test]
fn unknown_engine_option_is_rejected_for_watch() {
    let sandbox = Sandbox::new("engine-error", "Only block.\n");
    let output = Command::new(env!("CARGO_BIN_EXE_tmath"))
        .args([
            "watch",
            "--engine",
            "node",
            sandbox.target.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unknown option"),
        "{stderr}"
    );
}
