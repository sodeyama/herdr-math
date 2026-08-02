//! `tmath` — standalone terminal math/document renderer CLI.
//!
//! Phase 1 placeholder: `tmath render <file | ->` reads a document, forwards it
//! to the one-shot TypeScript renderer subprocess over stdin/stdout, and reports
//! the bounded response. Terminal placement and the input loop land in later
//! phases.

use std::env;
use std::io::{self, Read as _, Write as _};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use tmath_core::ipc::{
    EitherKind, IpcError, RenderRequest, RenderResponse, IPC_MAX_REQUEST_BYTES,
    IPC_MAX_RESPONSE_BYTES, IPC_PROTOCOL,
};

const RENDER_TIMEOUT: Duration = Duration::from_secs(15);

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
        return Err("usage: tmath render <file | ->".into());
    };
    match command.as_str() {
        "render" => render(&args[1..]),
        "--help" | "-h" => {
            println!("usage: tmath render <file | ->");
            println!("  -  read the document from stdin");
            Ok(0)
        }
        "--version" | "-V" => {
            println!("tmath {}", env!("CARGO_PKG_VERSION"));
            Ok(0)
        }
        other => Err(format!("unknown command {other:?}; use 'render'")),
    }
}

fn render(args: &[String]) -> Result<i32, String> {
    if args.len() != 1 {
        return Err("usage: tmath render <file | ->".into());
    }
    let source = read_document(&args[0])?;
    let request = RenderRequest {
        protocol: IPC_PROTOCOL.to_string(),
        kind: EitherKind::Document,
        text: Some(source),
        formulas: None,
        options: None,
    };

    let worker = renderer_worker_path()?;
    let payload = request.encode().map_err(|error| error.to_string())?;
    let response = spawn_renderer(&worker, &payload)?;
    match response {
        RenderResponse::Success(success) => {
            println!(
                "ok width={} height={} bytes={} renderer={}",
                success.width, success.height, success.bytes, success.renderer
            );
            Ok(0)
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

fn read_document(path: &str) -> Result<String, String> {
    let mut text = String::new();
    if path == "-" {
        io::stdin()
            .take((IPC_MAX_REQUEST_BYTES + 1) as u64)
            .read_to_string(&mut text)
            .map_err(|error| format!("read stdin: {error}"))?;
    } else {
        std::fs::read_to_string(path).map_err(|error| format!("read {path}: {error}"))?;
    }
    if text.len() > IPC_MAX_REQUEST_BYTES {
        return Err(format!("document exceeds {IPC_MAX_REQUEST_BYTES} bytes"));
    }
    Ok(text)
}

fn renderer_worker_path() -> Result<PathBuf, String> {
    if let Some(path) = env::var_os("TMATH_RENDER_WORKER") {
        return Ok(PathBuf::from(path));
    }
    Err("TMATH_RENDER_WORKER must point at the built render subprocess".into())
}

fn spawn_renderer(worker: &PathBuf, request: &[u8]) -> Result<RenderResponse, String> {
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
    let response =
        RenderResponse::parse(&response_bytes).map_err(|error: IpcError| error.to_string())?;
    Ok(response)
}
