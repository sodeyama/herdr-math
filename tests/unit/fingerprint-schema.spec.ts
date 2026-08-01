import { Buffer } from "node:buffer";

import { describe, expect, it } from "vitest";

import {
  FINGERPRINT_DIGEST_ALGORITHM,
  FINGERPRINT_SCHEMA_LIMITS,
  FINGERPRINT_SCHEMA_VERSION,
  FINGERPRINT_SECRET_BYTES,
  type FingerprintStateV1,
  isFingerprintDigest,
  isIsoTimestamp,
  isStateIdentifier
} from "../../src/boundary/fingerprint-schema.js";
import { POLICY_LIMITS } from "../../src/core/limits.js";

const digest = "a".repeat(64);

function sampleState(): FingerprintStateV1 {
  return {
    schema_version: FINGERPRINT_SCHEMA_VERSION,
    session_key: digest,
    workspace_id: "w1",
    source_pane_id: "w1:p1",
    agent: "codex",
    lifecycle_authority: "screen_detection",
    occupant_key: "b".repeat(64),
    pane_revision: 12,
    event_sequence: 7,
    generation: 2,
    baseline: {
      character_count: 500,
      utf8_bytes: 520,
      line_count: 20,
      digest,
      prefix_checkpoints: [{ end_offset: 400, digest }],
      suffix_windows: [{ character_length: 256, digest }],
      tail_anchors: [
        {
          end_offset: 480,
          forward_context_characters: 20,
          forward_context_digest: digest,
          next_anchor_gap_digest: digest,
          next_anchor_gap_formula_digests: [digest],
          prefix_formula_digests: [digest],
          line_characters: 40,
          line_digest: digest,
          context_characters: 256,
          context_digest: digest,
          line_index_from_end: 1
        }
      ]
    },
    viewer_pane_id: "w1:p2",
    processed: {
      content_digest: digest,
      pane_revision: 13,
      processed_at: "2026-08-01T00:01:00.000Z"
    },
    created_at: "2026-08-01T00:00:00.000Z",
    expires_at: "2026-08-02T00:00:00.000Z"
  };
}

describe("fingerprint state v1 schema", () => {
  it("is versioned and JSON serializable within the state limit", () => {
    const state = sampleState();
    const serialized = JSON.stringify(state);

    expect(state.schema_version).toBe(1);
    expect(Buffer.byteLength(serialized, "utf8")).toBeLessThan(POLICY_LIMITS.stateFileBytes);
    expect(JSON.parse(serialized)).toEqual(state);
  });

  it("contains only fingerprints and bounded metadata", () => {
    const keys = new Set<string>();
    JSON.stringify(sampleState(), (key, value: unknown) => {
      if (key.length > 0) keys.add(key);
      return value;
    });

    expect(keys).not.toContain("text");
    expect(keys).not.toContain("answer");
    expect(keys).not.toContain("latex");
    expect(keys).not.toContain("content");
    expect(keys).not.toContain("environment");
  });

  it("defines bounded checkpoint, window, anchor, and context counts", () => {
    expect(FINGERPRINT_DIGEST_ALGORITHM).toBe("hmac-sha256");
    expect(FINGERPRINT_SECRET_BYTES).toBe(32);
    expect(FINGERPRINT_SCHEMA_LIMITS).toEqual({
      digestHexCharacters: 64,
      maxIdentifierCharacters: 128,
      maxPrefixCheckpoints: 16,
      maxSuffixWindows: 4,
      maxTailAnchors: 20,
      maxGapFormulaDigests: 20,
      minTailAnchorCharacters: 32,
      maxContextCharacters: 2048
    });
    for (const value of Object.values(FINGERPRINT_SCHEMA_LIMITS)) {
      expect(Number.isSafeInteger(value)).toBe(true);
      expect(value).toBeGreaterThan(0);
    }
  });

  it("accepts only lowercase SHA-256-sized digests", () => {
    expect(isFingerprintDigest(digest)).toBe(true);
    expect(isFingerprintDigest("A".repeat(64))).toBe(false);
    expect(isFingerprintDigest("a".repeat(63))).toBe(false);
    expect(isFingerprintDigest("g".repeat(64))).toBe(false);
    expect(isFingerprintDigest(123)).toBe(false);
  });

  it("rejects path-like or oversized state identifiers", () => {
    expect(isStateIdentifier("w1:p1")).toBe(true);
    expect(isStateIdentifier("session_key-01")).toBe(true);
    expect(isStateIdentifier("../pane")).toBe(false);
    expect(isStateIdentifier("pane\\child")).toBe(false);
    expect(isStateIdentifier("a".repeat(FINGERPRINT_SCHEMA_LIMITS.maxIdentifierCharacters + 1))).toBe(false);
    expect(isStateIdentifier("")).toBe(false);
  });

  it("accepts canonical UTC timestamps only", () => {
    expect(isIsoTimestamp("2026-08-01T00:00:00.000Z")).toBe(true);
    expect(isIsoTimestamp("2026-08-01T00:00:00Z")).toBe(false);
    expect(isIsoTimestamp("2026-02-30T00:00:00.000Z")).toBe(false);
    expect(isIsoTimestamp("not-a-date")).toBe(false);
  });
});
