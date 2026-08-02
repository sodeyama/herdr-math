//! Integration tests for the renderer subprocess transport.
//!
//! These drive a real `node` subprocess (the TypeScript renderer compiled to
//! `dist/renderer/subprocess.js`). They are skipped when the renderer build is
//! not present so a pure-Rust checkout still passes.

use std::env;
use std::path::PathBuf;

use tmath_core::ipc::{
    EitherKind, RenderRequest, RenderResponse, IPC_MAX_REQUEST_BYTES, IPC_PROTOCOL,
};

fn worker_path() -> Option<PathBuf> {
    env::var_os("TMATH_RENDER_WORKER")
        .map(PathBuf::from)
        .or_else(|| {
            let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join("..")
                .join("..");
            let candidate = repo.join("dist").join("renderer").join("subprocess.js");
            candidate.exists().then_some(candidate)
        })
}

fn spawn_and_send(worker: &PathBuf, request: &RenderRequest) -> Result<RenderResponse, String> {
    let payload = request.encode().map_err(|e| e.to_string())?;
    let mut child = std::process::Command::new("node")
        .arg(worker)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    use std::io::Write as _;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&payload)
        .map_err(|e| e.to_string())?;
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(format!("renderer exited {}", output.status));
    }
    RenderResponse::parse(&output.stdout).map_err(|e| e.to_string())
}

#[test]
fn renderer_subprocess_round_trips_a_document_when_built() {
    let Some(worker) = worker_path() else {
        eprintln!("skipping: render subprocess not built (set TMATH_RENDER_WORKER)");
        return;
    };
    let request = RenderRequest {
        protocol: IPC_PROTOCOL.to_string(),
        kind: EitherKind::Document,
        text: Some("The relation is $E=mc^2$.".to_string()),
        formulas: None,
        options: None,
    };
    match spawn_and_send(&worker, &request) {
        Ok(RenderResponse::Success(success)) => {
            assert_eq!(success.protocol, IPC_PROTOCOL);
            assert!(success.width > 0);
            assert!(success.height > 0);
            assert!(success.bytes > 0);
        }
        Ok(RenderResponse::Failure(failure)) => {
            panic!("unexpected failure: {}", failure.error.code)
        }
        Err(message) => panic!("transport error: {message}"),
    }
}

#[test]
fn renderer_subprocess_reports_invalid_latex_when_built() {
    let Some(worker) = worker_path() else {
        eprintln!("skipping: render subprocess not built (set TMATH_RENDER_WORKER)");
        return;
    };
    let request = RenderRequest {
        protocol: IPC_PROTOCOL.to_string(),
        kind: EitherKind::Document,
        text: Some("bad $\\href{https://example.com}{x}$".to_string()),
        formulas: None,
        options: None,
    };
    match spawn_and_send(&worker, &request) {
        Ok(RenderResponse::Failure(failure)) => {
            assert_eq!(failure.error.code, "invalid_latex");
            assert!(!failure.error.retryable);
        }
        Ok(RenderResponse::Success(_)) => panic!("expected failure, got success"),
        Err(message) => panic!("transport error: {message}"),
    }
}

#[test]
fn oversized_request_is_rejected_before_spawning() {
    let padding = "a".repeat(IPC_MAX_REQUEST_BYTES + 10);
    let request = RenderRequest {
        protocol: IPC_PROTOCOL.to_string(),
        kind: EitherKind::Document,
        text: Some(padding),
        formulas: None,
        options: None,
    };
    assert!(
        request.encode().is_err(),
        "oversized request must be rejected"
    );
}

#[test]
fn scroll_loop_consumes_wheel_input_and_exits_on_q_or_ctrl_c() {
    use tmath_core::input::{Event, InputDecoder};
    use tmath_core::mouse::MouseKind;
    use tmath_core::scroll_driver::{is_exit_signal, ScrollDriver};

    let mut decoder = InputDecoder::new();
    let mut driver = ScrollDriver::new(64.0);

    // Wheel up then down, then the fallback keys, then q.
    decoder.push(b"\x1b[<64;3;4M\x1b[<65;3;4M\x1b[B\x1b[6~\x1b[5~");
    let mut scrolled = 0u32;
    let mut exit = false;
    while let Some(event) = decoder.next_event() {
        match &event {
            Event::Mouse(mouse) if mouse.kind == MouseKind::ScrollUp => scrolled += 1,
            Event::Mouse(mouse) if mouse.kind == MouseKind::ScrollDown => scrolled += 1,
            _ => {}
        }
        let _ = driver.handle(&event, Some(24.0));
        let _ = driver.step(1.0 / 60.0);
        exit = is_exit_signal(&event);
    }
    assert_eq!(scrolled, 2, "both wheel events were consumed");
    assert!(!exit, "no exit key in this batch");

    decoder.push(b"q");
    let mut exit = false;
    while let Some(event) = decoder.next_event() {
        exit = is_exit_signal(&event);
    }
    assert!(exit, "q is a clean exit signal");

    assert!(driver.position() > 0.0);
}

#[test]
fn rendered_png_decodes_and_emits_a_placement_when_built() {
    use base64::engine::general_purpose::STANDARD as BASE64;
    use base64::Engine as _;
    use tmath_core::placement::{decode_png, emit_placed_block, CellSize};

    let Some(worker) = worker_path() else {
        eprintln!("skipping: render subprocess not built (set TMATH_RENDER_WORKER)");
        return;
    };
    let request = RenderRequest {
        protocol: IPC_PROTOCOL.to_string(),
        kind: EitherKind::Document,
        text: Some("The relation is $E=mc^2$.".to_string()),
        formulas: None,
        options: None,
    };
    let RenderResponse::Success(success) = spawn_and_send(&worker, &request).unwrap() else {
        panic!("expected render success")
    };
    let png = BASE64.decode(success.base64.as_bytes()).unwrap();
    let cell = CellSize {
        width: 7,
        height: 15,
    };
    let (width, height, rgba) = decode_png(&png, 64 * 1024 * 1024).unwrap();
    assert_eq!((width, height), (success.width, success.height));
    let (cols, rows) = tmath_core::placement::grid_for(width, height, cell);
    let placement = emit_placed_block(1, width, height, &rgba, cols, rows, 1);
    let text = String::from_utf8_lossy(&placement);
    assert!(text.starts_with("\x1b[1;1H"));
    assert!(text.contains("a=T,f=32,o=z,s="));
    assert!(text.contains("U=1,c="), "virtual placement keys present");
    assert!(
        text.ends_with("\x1b[39m"),
        "placeholder grid closes the color"
    );
}
