//! Shared renderer-subprocess transport for the `tmath` CLI.
//!
//! Both `tmath render` and `tmath agent-viewer` send a bounded `tmath-render/1`
//! document request to the one-shot TypeScript renderer and receive a bounded
//! response. This module owns locating the worker, building the request, and
//! enforcing the size and timeout bounds at that boundary.

use std::io::{self, Read as _, Write as _};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use tmath_core::ipc::{
    EitherKind, IpcError, RenderOptions, RenderRequest, RenderResponse, IPC_MAX_REQUEST_BYTES,
    IPC_MAX_RESPONSE_BYTES, IPC_PROTOCOL,
};

/// Wall-clock cap for a single renderer invocation.
pub const RENDER_TIMEOUT: Duration = Duration::from_secs(15);

/// The location of the built one-shot renderer subprocess.
///
/// Resolution order: the `TMATH_RENDER_WORKER` environment variable, then the
/// installed layout (`<install>/renderer/dist/renderer/subprocess.js` next to
/// `<install>/bin/tmath`). No environment setup is required after an install
/// via `scripts/install.sh`.
pub fn renderer_worker_path() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("TMATH_RENDER_WORKER") {
        return Ok(PathBuf::from(path));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join("../renderer/dist/renderer/subprocess.js");
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    Err("renderer subprocess not found; set TMATH_RENDER_WORKER or run scripts/install.sh".into())
}

/// Renders a full document (Markdown + `$...$`/`$$...$$` math) through the
/// one-shot renderer, returning the bounded response.
pub fn render_document_text(
    text: &str,
    options: Option<RenderOptions>,
) -> Result<RenderResponse, String> {
    if text.len() > IPC_MAX_REQUEST_BYTES {
        return Err(format!("document exceeds {IPC_MAX_REQUEST_BYTES} bytes"));
    }
    let request = RenderRequest {
        protocol: IPC_PROTOCOL.to_string(),
        kind: EitherKind::Document,
        text: Some(text.to_string()),
        formulas: None,
        options,
    };
    let worker = renderer_worker_path()?;
    let payload = request.encode().map_err(|error| error.to_string())?;
    spawn_renderer(&worker, &payload)
}

/// Spawns the renderer subprocess, writes one bounded request, reads the
/// bounded response, and reaps the child. Fails closed on timeout, size
/// overflow, a non-zero exit, or an unparseable response.
pub fn spawn_renderer(worker: &PathBuf, request: &[u8]) -> Result<RenderResponse, String> {
    let mut child = Command::new("node")
        .arg(worker)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("spawn renderer: {error}"))?;

    let mut stdin = child.stdin.take().ok_or("renderer stdin unavailable")?;
    stdin
        .write_all(request)
        .map_err(|error| format!("write request: {error}"))?;
    drop(stdin);

    let mut stdout = child.stdout.take().ok_or("renderer stdout unavailable")?;
    let mut bytes = Vec::new();
    let timed_out = std::sync::Arc::new(std::sync::Mutex::new(false));
    let timeout_flag = std::sync::Arc::clone(&timed_out);
    let read_result = std::thread::spawn(move || {
        let limit = IPC_MAX_RESPONSE_BYTES + 1;
        let mut chunk = [0u8; 4096];
        let mut read = 0usize;
        while read < limit {
            match stdout.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    bytes.extend_from_slice(&chunk[..n]);
                    read += n;
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(format!("read response: {error}")),
            }
        }
        Ok::<Vec<u8>, String>(bytes)
    });

    let started = Instant::now();
    loop {
        if read_result.is_finished() {
            break;
        }
        if started.elapsed() >= RENDER_TIMEOUT {
            *timeout_flag.lock().unwrap() = true;
            let _ = child.kill();
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    let response_bytes = match read_result.join() {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(message)) => return Err(message),
        Err(_) => return Err("renderer reader panicked".into()),
    };
    let status = child.wait().map_err(|error| format!("wait: {error}"))?;
    if *timed_out.lock().unwrap() {
        return Err(format!(
            "renderer timed out after {} ms",
            RENDER_TIMEOUT.as_millis()
        ));
    }
    if !status.success() {
        return Err(format!("renderer exited with {status}"));
    }
    if response_bytes.len() > IPC_MAX_RESPONSE_BYTES {
        return Err(format!("response exceeds {IPC_MAX_RESPONSE_BYTES} bytes"));
    }
    RenderResponse::parse(&response_bytes).map_err(|error: IpcError| error.to_string())
}
