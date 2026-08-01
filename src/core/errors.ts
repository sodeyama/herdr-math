export const ERROR_CODES = [
  "event_invalid",
  "agent_unsupported",
  "baseline_missing",
  "boundary_failed",
  "answer_truncated",
  "conclusion_boundary_failed",
  "formula_not_found",
  "scanner_input_limit",
  "invalid_latex",
  "renderer_input_limit",
  "renderer_timeout",
  "renderer_failed",
  "image_too_large",
  "graphics_disabled",
  "cell_size_unavailable",
  "viewer_open_failed",
  "viewer_ownership_failed",
  "herdr_timeout",
  "herdr_protocol_error",
  "state_locked",
  "state_corrupt",
  "internal_error"
] as const;

export type ErrorCode = (typeof ERROR_CODES)[number];

export type SafeLimitKind =
  | "event_json_bytes"
  | "pane_read_bytes"
  | "input_bytes"
  | "delimiter_runs"
  | "delimiter_run_length"
  | "formula_count"
  | "formula_characters"
  | "aggregate_formula_characters"
  | "render_duration_ms"
  | "image_width_px"
  | "image_height_px"
  | "image_pixels"
  | "raw_png_bytes"
  | "base64_payload_bytes"
  | "state_file_bytes"
  | "socket_response_bytes";

export interface SafeErrorDetails {
  limit_kind?: SafeLimitKind;
  limit?: number;
  actual?: number;
  duration_ms?: number;
  width?: number;
  height?: number;
  bytes?: number;
  formula_count?: number;
}

export interface SafeErrorRecord {
  code: ErrorCode;
  retryable: boolean;
  details?: SafeErrorDetails;
}

const SAFE_LIMIT_KINDS = new Set<SafeLimitKind>([
  "event_json_bytes",
  "pane_read_bytes",
  "input_bytes",
  "delimiter_runs",
  "delimiter_run_length",
  "formula_count",
  "formula_characters",
  "aggregate_formula_characters",
  "render_duration_ms",
  "image_width_px",
  "image_height_px",
  "image_pixels",
  "raw_png_bytes",
  "base64_payload_bytes",
  "state_file_bytes",
  "socket_response_bytes"
]);

const NUMERIC_DETAIL_KEYS = ["limit", "actual", "duration_ms", "width", "height", "bytes", "formula_count"] as const;

export class HerdrMathError extends Error {
  readonly safeDetails: Readonly<SafeErrorDetails>;

  constructor(
    readonly code: ErrorCode,
    details: SafeErrorDetails = {},
    readonly retryable = false
  ) {
    super(code);
    this.name = "HerdrMathError";
    this.safeDetails = Object.freeze({ ...details });
  }
}

export function serializeError(error: unknown): SafeErrorRecord {
  if (!(error instanceof HerdrMathError)) {
    return { code: "internal_error", retryable: false };
  }

  const details = sanitizeDetails(error.safeDetails);
  return Object.keys(details).length === 0
    ? { code: error.code, retryable: error.retryable }
    : { code: error.code, retryable: error.retryable, details };
}

function sanitizeDetails(details: Readonly<SafeErrorDetails>): SafeErrorDetails {
  const safe: SafeErrorDetails = {};
  if (details.limit_kind !== undefined && SAFE_LIMIT_KINDS.has(details.limit_kind)) {
    safe.limit_kind = details.limit_kind;
  }

  for (const key of NUMERIC_DETAIL_KEYS) {
    const value = details[key];
    if (typeof value === "number" && Number.isSafeInteger(value) && value >= 0) {
      safe[key] = value;
    }
  }
  return safe;
}
