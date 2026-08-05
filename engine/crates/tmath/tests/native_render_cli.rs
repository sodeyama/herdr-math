//! AT-3-204: the native CLI renderer does not depend on a child process.

use std::fs;
use std::io::Write as _;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

const DOCUMENT: &str = "# Native\n\nProse with $E=mc^2$.\n\n- one\n- two\n";

struct Sandbox {
    root: PathBuf,
    binary: PathBuf,
    empty_path: PathBuf,
}

impl Sandbox {
    fn new(test_name: &str, with_worker: bool) -> Self {
        let root = std::env::temp_dir().join(format!("tmath-{test_name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let binary_dir = root.join("bin");
        let empty_path = root.join("empty-path");
        fs::create_dir_all(&binary_dir).unwrap();
        fs::create_dir(&empty_path).unwrap();

        let binary = binary_dir.join("tmath");
        fs::copy(env!("CARGO_BIN_EXE_tmath"), &binary).unwrap();
        if with_worker {
            let worker = root.join("renderer/dist/renderer/subprocess.js");
            fs::create_dir_all(worker.parent().unwrap()).unwrap();
            fs::write(worker, "// Node is intentionally unavailable in PATH.\n").unwrap();
        }

        Self {
            root,
            binary,
            empty_path,
        }
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn run_render(engine: &str, sandbox: &Sandbox) -> Output {
    let mut command = Command::new(&sandbox.binary);
    command
        .args(["render", "--engine", engine, "-"])
        .env("PATH", &sandbox.empty_path)
        .env_remove("TMATH_RENDER_WORKER")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(DOCUMENT.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn native_engine_renders_without_node_or_worker_environment() {
    let sandbox = Sandbox::new("native-no-subprocess", false);
    let output = run_render("native", &sandbox);

    assert!(
        output.status.success(),
        "native render failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines = stdout.lines().collect::<Vec<_>>();
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("event=append id=")),
        "native stream must append at least one block: {stdout}"
    );
    assert_eq!(lines.last(), Some(&"event=done blocks=3 formula_errors=0"));
}

#[test]
fn node_engine_fails_when_node_is_absent_from_path() {
    let sandbox = Sandbox::new("node-subprocess-guard", true);
    let output = run_render("node", &sandbox);

    assert!(
        !output.status.success(),
        "node render unexpectedly succeeded"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("spawn renderer"),
        "failure did not reach subprocess spawn: {stderr}"
    );
}
