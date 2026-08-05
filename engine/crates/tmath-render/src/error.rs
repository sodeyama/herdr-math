use serde::Serialize;
use std::error::Error;
use std::fmt;

/// Stable renderer error codes mirrored from the TypeScript contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    FormulaNotFound,
    ScannerInputLimit,
    InvalidLatex,
    RendererInputLimit,
    RendererTimeout,
    RendererFailed,
    ImageTooLarge,
    InternalError,
}

/// Stable safe-limit identifiers mirrored from the TypeScript contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SafeLimitKind {
    InputBytes,
    DelimiterRuns,
    DelimiterRunLength,
    FormulaCount,
    FormulaCharacters,
    AggregateFormulaCharacters,
    ResponseDocumentBytes,
    ResponseDocumentLines,
    ResponseDocumentBlocks,
    RenderDurationMs,
    ImageWidthPx,
    ImageHeightPx,
    ImagePixels,
    RawPngBytes,
    Base64PayloadBytes,
    /// Native-engine-only: bounds one formula's embedded SVG byte length
    /// (`Limits::math_svg_bytes`). No TypeScript/Node-engine counterpart
    /// exists yet, since that pipeline still rasterizes formulas to PNG.
    MathSvgBytes,
}

/// Numeric, input-free details safe to cross the public error boundary.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct SafeErrorDetails {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit_kind: Option<SafeLimitKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub formula_count: Option<u64>,
}

/// Public error record that is byte-compatible with the TypeScript JSON shape.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SafeErrorRecord {
    pub code: ErrorCode,
    pub retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<SafeErrorDetails>,
}

/// Crate error with a safe public record and a non-serialized internal message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderError {
    safe_record: Box<SafeErrorRecord>,
    internal_message: String,
}

impl RenderError {
    pub fn new(safe_record: SafeErrorRecord, internal_message: impl Into<String>) -> Self {
        Self {
            safe_record: Box::new(safe_record),
            internal_message: internal_message.into(),
        }
    }

    pub fn safe_record(&self) -> &SafeErrorRecord {
        self.safe_record.as_ref()
    }

    pub fn into_safe_record(self) -> SafeErrorRecord {
        *self.safe_record
    }

    pub(crate) fn limit_exceeded(
        code: ErrorCode,
        limit_kind: SafeLimitKind,
        limit: u64,
        actual: u64,
    ) -> Self {
        Self::new(
            SafeErrorRecord {
                code,
                retryable: false,
                details: Some(SafeErrorDetails {
                    limit_kind: Some(limit_kind),
                    limit: Some(limit),
                    actual: Some(actual),
                    ..SafeErrorDetails::default()
                }),
            },
            "render limit exceeded",
        )
    }

    pub(crate) fn deadline_exceeded(limit: u64, duration_ms: u64) -> Self {
        Self::new(
            SafeErrorRecord {
                code: ErrorCode::RendererTimeout,
                retryable: false,
                details: Some(SafeErrorDetails {
                    limit_kind: Some(SafeLimitKind::RenderDurationMs),
                    limit: Some(limit),
                    actual: Some(duration_ms),
                    duration_ms: Some(duration_ms),
                    ..SafeErrorDetails::default()
                }),
            },
            "render deadline exceeded",
        )
    }
}

impl fmt::Display for RenderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.internal_message)
    }
}

impl Error for RenderError {}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn error_codes_serialize_to_the_typescript_strings() {
        let cases = [
            (ErrorCode::FormulaNotFound, "formula_not_found"),
            (ErrorCode::ScannerInputLimit, "scanner_input_limit"),
            (ErrorCode::InvalidLatex, "invalid_latex"),
            (ErrorCode::RendererInputLimit, "renderer_input_limit"),
            (ErrorCode::RendererTimeout, "renderer_timeout"),
            (ErrorCode::RendererFailed, "renderer_failed"),
            (ErrorCode::ImageTooLarge, "image_too_large"),
            (ErrorCode::InternalError, "internal_error"),
        ];

        for (value, expected) in cases {
            assert_eq!(serde_json::to_value(value).unwrap(), json!(expected));
        }
    }

    #[test]
    fn safe_limit_kinds_serialize_to_the_typescript_strings() {
        let cases = [
            (SafeLimitKind::InputBytes, "input_bytes"),
            (SafeLimitKind::DelimiterRuns, "delimiter_runs"),
            (SafeLimitKind::DelimiterRunLength, "delimiter_run_length"),
            (SafeLimitKind::FormulaCount, "formula_count"),
            (SafeLimitKind::FormulaCharacters, "formula_characters"),
            (
                SafeLimitKind::AggregateFormulaCharacters,
                "aggregate_formula_characters",
            ),
            (
                SafeLimitKind::ResponseDocumentBytes,
                "response_document_bytes",
            ),
            (
                SafeLimitKind::ResponseDocumentLines,
                "response_document_lines",
            ),
            (
                SafeLimitKind::ResponseDocumentBlocks,
                "response_document_blocks",
            ),
            (SafeLimitKind::RenderDurationMs, "render_duration_ms"),
            (SafeLimitKind::ImageWidthPx, "image_width_px"),
            (SafeLimitKind::ImageHeightPx, "image_height_px"),
            (SafeLimitKind::ImagePixels, "image_pixels"),
            (SafeLimitKind::RawPngBytes, "raw_png_bytes"),
            (SafeLimitKind::Base64PayloadBytes, "base64_payload_bytes"),
            (SafeLimitKind::MathSvgBytes, "math_svg_bytes"),
        ];

        for (value, expected) in cases {
            assert_eq!(serde_json::to_value(value).unwrap(), json!(expected));
        }
    }

    #[test]
    fn safe_error_record_matches_the_typescript_json_shape() {
        let record = SafeErrorRecord {
            code: ErrorCode::ImageTooLarge,
            retryable: false,
            details: Some(SafeErrorDetails {
                limit_kind: Some(SafeLimitKind::RawPngBytes),
                limit: Some(524_288),
                actual: Some(600_000),
                ..SafeErrorDetails::default()
            }),
        };

        assert_eq!(
            serde_json::to_value(record).unwrap(),
            json!({
                "code": "image_too_large",
                "retryable": false,
                "details": {
                    "limit_kind": "raw_png_bytes",
                    "limit": 524288,
                    "actual": 600000
                }
            })
        );
    }

    #[test]
    fn absent_optional_fields_are_omitted() {
        let record = SafeErrorRecord {
            code: ErrorCode::InternalError,
            retryable: false,
            details: None,
        };

        assert_eq!(
            serde_json::to_value(record).unwrap(),
            json!({"code": "internal_error", "retryable": false})
        );
    }
}
