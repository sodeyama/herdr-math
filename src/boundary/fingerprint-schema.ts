export const FINGERPRINT_SCHEMA_VERSION = 1 as const;
export const FINGERPRINT_DIGEST_ALGORITHM = "hmac-sha256" as const;
export const FINGERPRINT_SECRET_BYTES = 32 as const;

export const FINGERPRINT_SCHEMA_LIMITS = Object.freeze({
  digestHexCharacters: 64,
  maxIdentifierCharacters: 128,
  maxPrefixCheckpoints: 16,
  maxSuffixWindows: 4,
  maxTailAnchors: 20,
  maxContextCharacters: 2048
});

export type FingerprintDigest = string;
export type SupportedAgent = "claude" | "codex" | "pi" | "opencode";
export type LifecycleAuthority = "screen_detection" | "integration_hook";

export interface PrefixCheckpointV1 {
  end_offset: number;
  digest: FingerprintDigest;
}

export interface SuffixWindowV1 {
  character_length: number;
  digest: FingerprintDigest;
}

export interface TailAnchorV1 {
  line_characters: number;
  line_digest: FingerprintDigest;
  context_characters: number;
  context_digest: FingerprintDigest;
  line_index_from_end: number;
}

export interface BaselineFingerprintV1 {
  character_count: number;
  utf8_bytes: number;
  line_count: number;
  digest: FingerprintDigest;
  prefix_checkpoints: PrefixCheckpointV1[];
  suffix_windows: SuffixWindowV1[];
  tail_anchors: TailAnchorV1[];
}

export interface ProcessedDigestV1 {
  content_digest: FingerprintDigest;
  pane_revision: number;
  processed_at: string;
}

export interface FingerprintStateV1 {
  schema_version: typeof FINGERPRINT_SCHEMA_VERSION;
  session_key: FingerprintDigest;
  workspace_id: string;
  source_pane_id: string;
  agent: SupportedAgent;
  lifecycle_authority: LifecycleAuthority;
  occupant_key: FingerprintDigest;
  pane_revision: number;
  event_sequence: number;
  generation: number;
  baseline: BaselineFingerprintV1;
  viewer_pane_id?: string;
  processed?: ProcessedDigestV1;
  created_at: string;
  expires_at: string;
}

export type FingerprintState = FingerprintStateV1;

export function isFingerprintDigest(value: unknown): value is FingerprintDigest {
  return typeof value === "string" && /^[a-f0-9]{64}$/.test(value);
}

export function isStateIdentifier(value: unknown): value is string {
  return (
    typeof value === "string" &&
    value.length > 0 &&
    value.length <= FINGERPRINT_SCHEMA_LIMITS.maxIdentifierCharacters &&
    !value.includes("/") &&
    !value.includes("\\") &&
    !value.includes("\0") &&
    value !== "." &&
    value !== ".."
  );
}

export function isIsoTimestamp(value: unknown): value is string {
  if (typeof value !== "string" || !/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$/.test(value)) {
    return false;
  }
  const timestamp = new Date(value);
  return !Number.isNaN(timestamp.getTime()) && timestamp.toISOString() === value;
}
