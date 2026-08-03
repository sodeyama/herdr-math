//! Versioned JSON IPC contract between the Rust CLI (`tmath`) and the one-shot
//! TypeScript renderer subprocess (`tmath-render`).
//!
//! One bounded json request is written to the subprocess stdin and one bounded
//! json response is read from its stdout before it exits. Both sides share the
//! protocol version and the size limits declared here; the renderer subprocess
//! never stays alive waiting for a second request.

/// Protocol version shared with `src/renderer/ipc-contract.ts`.
pub const IPC_PROTOCOL: &str = "tmath-render/1";

/// Maximum bytes accepted for a request payload. Sized above
/// `POLICY_LIMITS.scannerInputBytes` (160 MiB) to leave room for JSON envelope
/// overhead (escaping, field names) around the source document.
pub const IPC_MAX_REQUEST_BYTES: usize = 192 * 1024 * 1024;

/// Maximum bytes accepted for a response payload (PNG + envelope).
pub const IPC_MAX_RESPONSE_BYTES: usize = 768 * 1024;

/// A single formula as transmitted over the wire.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WireFormula {
    pub latex: String,
    pub display: bool,
}

/// Optional render options forwarded to the renderer subprocess.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct RenderOptions {
    pub limits: Option<serde_json::Value>,
    pub layout: Option<serde_json::Value>,
}

/// A render request addressed to the subprocess.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RenderRequest {
    pub protocol: String,
    #[serde(rename = "kind")]
    pub kind: EitherKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub formulas: Option<Vec<WireFormula>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<RenderOptions>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EitherKind {
    Document,
    Formulas,
}

/// A render success response from the subprocess.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RenderSuccess {
    pub protocol: String,
    pub ok: bool,
    pub width: u32,
    pub height: u32,
    pub bytes: u32,
    pub renderer: String,
    pub base64: String,
}

/// A stable error record carried in a render failure response.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SafeErrorRecord {
    pub code: String,
    pub retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

/// A render failure response from the subprocess.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RenderFailure {
    pub protocol: String,
    #[serde(rename = "ok")]
    pub ok: bool,
    pub error: SafeErrorRecord,
}

/// Enum-style view over success and failure responses.
#[derive(Debug, Clone)]
pub enum RenderResponse {
    Success(RenderSuccess),
    Failure(RenderFailure),
}

impl RenderResponse {
    /// Parses a response payload, rejecting payloads over the size limit.
    pub fn parse(payload: &[u8]) -> Result<Self, IpcError> {
        if payload.len() > IPC_MAX_RESPONSE_BYTES {
            return Err(IpcError::OversizedResponse(payload.len()));
        }
        let value: serde_json::Value =
            serde_json::from_slice(payload).map_err(|_| IpcError::Malformed("response json"))?;
        let ok = value.get("ok").and_then(serde_json::Value::as_bool);
        match ok {
            Some(true) => serde_json::from_value(value)
                .map(RenderResponse::Success)
                .map_err(|_| IpcError::Malformed("success response")),
            Some(false) => serde_json::from_value(value)
                .map(RenderResponse::Failure)
                .map_err(|_| IpcError::Malformed("failure response")),
            _ => Err(IpcError::Malformed("response ok flag")),
        }
    }
}

/// Reasons the IPC boundary rejects a payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IpcError {
    Malformed(&'static str),
    OversizedRequest(usize),
    OversizedResponse(usize),
}

impl std::fmt::Display for IpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IpcError::Malformed(kind) => write!(f, "malformed {kind}"),
            IpcError::OversizedRequest(bytes) => {
                write!(f, "request of {bytes} bytes exceeds limit")
            }
            IpcError::OversizedResponse(bytes) => {
                write!(f, "response of {bytes} bytes exceeds limit")
            }
        }
    }
}

impl std::error::Error for IpcError {}

impl RenderRequest {
    /// Encodes the request, rejecting oversized payloads.
    pub fn encode(&self) -> Result<Vec<u8>, IpcError> {
        let mut out = serde_json::to_vec(self).map_err(|_| IpcError::Malformed("request json"))?;
        out.push(b'\n');
        if out.len() > IPC_MAX_REQUEST_BYTES {
            return Err(IpcError::OversizedRequest(out.len()));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_round_trips_through_json() {
        let request = RenderRequest {
            protocol: IPC_PROTOCOL.to_string(),
            kind: EitherKind::Document,
            text: Some("The relation is $E=mc^2$.".to_string()),
            formulas: None,
            options: None,
        };
        let bytes = request.encode().unwrap();
        let decoded: RenderRequest = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(decoded.protocol, IPC_PROTOCOL);
        assert_eq!(decoded.kind, EitherKind::Document);
        assert_eq!(decoded.text.as_deref(), Some("The relation is $E=mc^2$."));
    }

    #[test]
    fn success_response_parses() {
        let payload = br#"{"protocol":"tmath-render/1","ok":true,"width":640,"height":120,"bytes":2048,"renderer":"katex-playwright-sharp","base64":"aGk="}"#;
        match RenderResponse::parse(payload).unwrap() {
            RenderResponse::Success(success) => {
                assert_eq!(success.width, 640);
                assert_eq!(success.height, 120);
                assert_eq!(success.base64, "aGk=");
            }
            RenderResponse::Failure(_) => panic!("expected success"),
        }
    }

    #[test]
    fn failure_response_parses() {
        let payload = br#"{"protocol":"tmath-render/1","ok":false,"error":{"code":"invalid_latex","retryable":false}}"#;
        match RenderResponse::parse(payload).unwrap() {
            RenderResponse::Failure(failure) => {
                assert_eq!(failure.error.code, "invalid_latex");
                assert!(!failure.error.retryable);
            }
            RenderResponse::Success(_) => panic!("expected failure"),
        }
    }

    #[test]
    fn oversized_response_is_rejected() {
        let payload = [b' '; IPC_MAX_RESPONSE_BYTES + 1];
        assert!(matches!(
            RenderResponse::parse(&payload),
            Err(IpcError::OversizedResponse(n)) if n == IPC_MAX_RESPONSE_BYTES + 1
        ));
    }

    #[test]
    fn malformed_response_is_rejected() {
        assert!(matches!(
            RenderResponse::parse(b"not json"),
            Err(IpcError::Malformed("response json"))
        ));
        assert!(matches!(
            RenderResponse::parse(b"{\"protocol\":\"tmath-render/1\"}"),
            Err(IpcError::Malformed("response ok flag"))
        ));
    }
}
