import {
  FINGERPRINT_SCHEMA_LIMITS,
  FINGERPRINT_SCHEMA_VERSION,
  type FingerprintStateV1,
  isFingerprintDigest,
  isIsoTimestamp,
  isStateIdentifier
} from "../boundary/fingerprint-schema.js";
import { HerdrMathError } from "../core/errors.js";
import { POLICY_LIMITS } from "../core/limits.js";

const AGENTS = new Set(["claude", "codex", "pi", "opencode"]);
const AUTHORITIES = new Set(["screen_detection", "integration_hook"]);

export function parseFingerprintState(value: unknown): FingerprintStateV1 {
  if (!isFingerprintState(value)) throw new HerdrMathError("state_corrupt");
  return value;
}

function isFingerprintState(value: unknown): value is FingerprintStateV1 {
  if (
    !hasKeys(
      value,
      [
        "schema_version",
        "session_key",
        "workspace_id",
        "source_pane_id",
        "agent",
        "lifecycle_authority",
        "occupant_key",
        "pane_revision",
        "event_sequence",
        "generation",
        "baseline",
        "created_at",
        "expires_at"
      ],
      ["viewer_pane_id", "processed"]
    ) ||
    value.schema_version !== FINGERPRINT_SCHEMA_VERSION ||
    !isFingerprintDigest(value.session_key) ||
    !isStateIdentifier(value.workspace_id) ||
    !isStateIdentifier(value.source_pane_id) ||
    typeof value.agent !== "string" ||
    !AGENTS.has(value.agent) ||
    typeof value.lifecycle_authority !== "string" ||
    !AUTHORITIES.has(value.lifecycle_authority) ||
    !isFingerprintDigest(value.occupant_key) ||
    !isCount(value.pane_revision) ||
    !isCount(value.event_sequence) ||
    !isCount(value.generation) ||
    !isBaseline(value.baseline) ||
    (value.viewer_pane_id !== undefined && !isStateIdentifier(value.viewer_pane_id)) ||
    (value.processed !== undefined && !isProcessed(value.processed)) ||
    !isIsoTimestamp(value.created_at) ||
    !isIsoTimestamp(value.expires_at)
  ) {
    return false;
  }
  const lifetime = Date.parse(value.expires_at) - Date.parse(value.created_at);
  return lifetime > 0 && lifetime <= POLICY_LIMITS.fingerprintExpiryMs;
}

function isBaseline(value: unknown): boolean {
  if (
    !hasKeys(value, [
      "character_count",
      "utf8_bytes",
      "line_count",
      "digest",
      "prefix_checkpoints",
      "suffix_windows",
      "tail_anchors"
    ]) ||
    !isCount(value.character_count, POLICY_LIMITS.paneReadBytes) ||
    !isCount(value.utf8_bytes, POLICY_LIMITS.paneReadBytes) ||
    !isCount(value.line_count, POLICY_LIMITS.paneReadLines) ||
    !isFingerprintDigest(value.digest) ||
    !isBoundedArray(value.prefix_checkpoints, FINGERPRINT_SCHEMA_LIMITS.maxPrefixCheckpoints) ||
    !isBoundedArray(value.suffix_windows, FINGERPRINT_SCHEMA_LIMITS.maxSuffixWindows) ||
    !isBoundedArray(value.tail_anchors, FINGERPRINT_SCHEMA_LIMITS.maxTailAnchors)
  ) {
    return false;
  }
  return (
    value.prefix_checkpoints.every(
      (item) =>
        hasKeys(item, ["end_offset", "digest"]) &&
        isPositiveCount(item.end_offset, value.character_count as number) &&
        isFingerprintDigest(item.digest)
    ) &&
    value.suffix_windows.every(
      (item) =>
        hasKeys(item, ["character_length", "digest"]) &&
        isPositiveCount(item.character_length, value.character_count as number) &&
        isFingerprintDigest(item.digest)
    ) &&
    value.tail_anchors.every(
      (item) =>
        hasKeys(
          item,
          ["line_characters", "line_digest", "context_characters", "context_digest", "line_index_from_end"],
          [
            "end_offset",
            "forward_context_characters",
            "forward_context_digest",
            "next_anchor_gap_digest",
            "next_anchor_gap_formula_digests",
            "prefix_formula_digests"
          ]
        ) &&
        isCount(
          item.line_characters,
          value.character_count as number,
          FINGERPRINT_SCHEMA_LIMITS.minTailAnchorCharacters
        ) &&
        (item.end_offset === undefined ||
          (isPositiveCount(item.end_offset, value.character_count as number) &&
            item.end_offset >= item.line_characters)) &&
        isFingerprintDigest(item.line_digest) &&
        ((item.forward_context_characters === undefined && item.forward_context_digest === undefined) ||
          (isCount(item.forward_context_characters, FINGERPRINT_SCHEMA_LIMITS.maxContextCharacters) &&
            isFingerprintDigest(item.forward_context_digest))) &&
        (item.next_anchor_gap_digest === undefined ||
          (item.end_offset !== undefined && isFingerprintDigest(item.next_anchor_gap_digest))) &&
        (item.next_anchor_gap_formula_digests === undefined ||
          (item.next_anchor_gap_digest !== undefined &&
            isBoundedArray(item.next_anchor_gap_formula_digests, FINGERPRINT_SCHEMA_LIMITS.maxGapFormulaDigests) &&
            item.next_anchor_gap_formula_digests.every(isFingerprintDigest))) &&
        (item.prefix_formula_digests === undefined ||
          (isBoundedArray(item.prefix_formula_digests, FINGERPRINT_SCHEMA_LIMITS.maxGapFormulaDigests) &&
            item.prefix_formula_digests.every(isFingerprintDigest))) &&
        isCount(item.context_characters, FINGERPRINT_SCHEMA_LIMITS.maxContextCharacters) &&
        isFingerprintDigest(item.context_digest) &&
        isCount(item.line_index_from_end, POLICY_LIMITS.paneReadLines - 1)
    )
  );
}

function isProcessed(value: unknown): boolean {
  return (
    hasKeys(value, ["content_digest", "pane_revision", "processed_at"]) &&
    isFingerprintDigest(value.content_digest) &&
    isCount(value.pane_revision) &&
    isIsoTimestamp(value.processed_at)
  );
}

function hasKeys(
  value: unknown,
  required: readonly string[],
  optional: readonly string[] = []
): value is Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
  const keys = Object.keys(value);
  return required.every((key) => key in value) && keys.every((key) => required.includes(key) || optional.includes(key));
}

function isBoundedArray(value: unknown, maximum: number): value is Record<string, unknown>[] {
  return Array.isArray(value) && value.length <= maximum;
}

function isCount(value: unknown, maximum = Number.MAX_SAFE_INTEGER, minimum = 0): value is number {
  return Number.isSafeInteger(value) && (value as number) >= minimum && (value as number) <= maximum;
}

function isPositiveCount(value: unknown, maximum: number): value is number {
  return isCount(value, maximum) && value > 0;
}
