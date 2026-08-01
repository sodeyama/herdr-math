import { Buffer } from "node:buffer";
import { readdirSync, readFileSync } from "node:fs";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { performance } from "node:perf_hooks";

import { afterEach, describe, expect, it } from "vitest";

import { buildBaselineFingerprint } from "../../src/boundary/fingerprint-builder.js";
import { resolveAnswerFromFingerprint } from "../../src/boundary/fingerprint-resolver.js";
import { FINGERPRINT_SCHEMA_LIMITS, type FingerprintStateV1 } from "../../src/boundary/fingerprint-schema.js";
import { HerdrMathError, serializeError, type SafeErrorDetails } from "../../src/core/errors.js";
import { POLICY_LIMITS } from "../../src/core/limits.js";
import { createPaneStatePaths } from "../../src/state/paths.js";
import { writePaneState } from "../../src/state/store.js";

const secret = Buffer.alloc(32, 13);
const createdAt = new Date("2026-08-01T00:00:00.000Z");
const temporaryDirectories: string[] = [];
const BOUNDARY_TIME_BUDGET_MS = 2000;

afterEach(async () => {
  await Promise.all(temporaryDirectories.splice(0).map((directory) => rm(directory, { recursive: true, force: true })));
});

function fingerprint(input: string): FingerprintStateV1 {
  return buildBaselineFingerprint(
    input,
    {
      sessionIdentity: "synthetic-session",
      occupantIdentity: "synthetic-occupant",
      workspaceId: "w1",
      sourcePaneId: "w1:p1",
      agent: "codex",
      lifecycleAuthority: "screen_detection",
      paneRevision: 10,
      eventSequence: 4,
      generation: 1,
      createdAt
    },
    secret
  );
}

describe("fingerprint persistence privacy", () => {
  it("writes only allowlisted metadata and non-reversible fingerprints", async () => {
    const sentinel = "H3RDR_PRIVATE_TRANSCRIPT_Q7Z9K2M4";
    const formula = "\\frac{PRIVATE_ALPHA_91}{PRIVATE_BETA_73}";
    const baseline = `Synthetic terminal history 0123456789\n${sentinel}\nThe result is $${formula}$.`;
    const state = fingerprint(baseline);
    const directory = await mkdtemp(join(tmpdir(), "herdr-math-privacy-"));
    temporaryDirectories.push(directory);
    const paths = createPaneStatePaths(directory, state.session_key, state.source_pane_id, secret);
    await writePaneState(paths, state, null, new Date("2026-08-01T00:01:00.000Z"));
    const bytes = await readFile(paths.statePath);
    const serialized = bytes.toString("utf8");

    for (const value of [baseline, sentinel, formula]) {
      expect(serialized).not.toContain(value);
      expect(serialized).not.toContain(Buffer.from(value).toString("base64"));
      expect(serialized).not.toContain(Buffer.from(value).toString("hex"));
      expect(serialized).not.toContain(encodeURIComponent(value));
    }
    for (let offset = 0; offset <= sentinel.length - 12; offset += 1) {
      expect(serialized).not.toContain(sentinel.slice(offset, offset + 12));
    }

    const allowedMetadata = new Set(["w1", "w1:p1", "codex", "screen_detection", state.created_at, state.expires_at]);
    for (const value of collectStringValues(JSON.parse(serialized))) {
      expect(allowedMetadata.has(value) || /^[a-f0-9]{64}$/.test(value)).toBe(true);
    }
  });

  it("does not serialize arbitrary error, event, path, or environment fields", () => {
    const sentinels = {
      answer: "SYNTHETIC_PRIVATE_ANSWER_Q7Z",
      formula: "SYNTHETIC_PRIVATE_FORMULA_Q7Z",
      path: "/synthetic/private/path/Q7Z",
      environment: "SYNTHETIC_ENV_SECRET_Q7Z"
    };
    const unsafeDetails = {
      ...sentinels,
      event: { raw: sentinels.answer },
      request: sentinels.path,
      limit: 10,
      actual: 11
    } as unknown as SafeErrorDetails;
    const records = [
      serializeError(new Error(JSON.stringify(sentinels))),
      serializeError(new HerdrMathError("internal_error", unsafeDetails))
    ];
    const serialized = JSON.stringify(records);

    expect(records).toEqual([
      { code: "internal_error", retryable: false },
      { code: "internal_error", retryable: false, details: { limit: 10, actual: 11 } }
    ]);
    for (const sentinel of Object.values(sentinels)) expect(serialized).not.toContain(sentinel);
    expect(serialized).not.toContain("event");
    expect(serialized).not.toContain("request");
    expect(serialized).not.toContain("environment");
  });

  it("statically rejects broad logging and environment serialization", () => {
    const unsafePatterns = [
      /\bconsole\.(?:debug|error|info|log|warn)\s*\(/,
      /JSON\.stringify\s*\(\s*(?:error|event|request|process\.env)\b/,
      /\.\.\.\s*process\.env\b/,
      /Object\.(?:entries|keys|values)\s*\(\s*process\.env\s*\)/,
      /for\s*\([^)]*\bin\s+process\.env\b/
    ];

    for (const source of readTypeScriptSources(new URL("../../src/", import.meta.url))) {
      for (const pattern of unsafePatterns) expect(source).not.toMatch(pattern);
    }
  });
});

describe("fingerprint privacy and complexity thresholds", () => {
  it("does not create or accept dictionary-like short anchors", () => {
    const shortLine = "build completed successfully";
    expect(shortLine.length).toBeLessThan(FINGERPRINT_SCHEMA_LIMITS.minTailAnchorCharacters);
    expect(fingerprint(shortLine).baseline.tail_anchors).toEqual([]);

    const valid = fingerprint("Unique contextual anchor line 1234567890 ABCDEF");
    expect(valid.baseline.tail_anchors).toHaveLength(1);
    const forged = structuredClone(valid);
    const anchor = forged.baseline.tail_anchors[0];
    if (anchor === undefined) throw new Error("Expected a valid anchor.");
    anchor.line_characters = FINGERPRINT_SCHEMA_LIMITS.minTailAnchorCharacters - 1;
    expect(resolveAnswerFromFingerprint(forged, "unrelated", secret)).toEqual({
      ok: false,
      error: { code: "state_corrupt", retryable: false }
    });
  });

  it("bounds maximum-size repeated-anchor resolution by candidates and time", () => {
    const anchor = "A".repeat(2048);
    const prefixBytes = POLICY_LIMITS.paneReadBytes - Buffer.byteLength(anchor) - 1;
    const baseline = `${"B".repeat(prefixBytes)}\n${anchor}`;
    const repeatedLine = "R".repeat(anchor.length);
    let current = Array.from({ length: 500 }, () => repeatedLine).join("\n");
    current += "X".repeat(POLICY_LIMITS.paneReadBytes - Buffer.byteLength(current));
    const state = fingerprint(baseline);

    expect(Buffer.byteLength(baseline)).toBe(POLICY_LIMITS.paneReadBytes);
    expect(Buffer.byteLength(current)).toBe(POLICY_LIMITS.paneReadBytes);
    expect(current.split("\n")).toHaveLength(500);
    expect(current.split("\n").length).toBeGreaterThan(POLICY_LIMITS.anchorOccurrences);
    const started = performance.now();
    const result = resolveAnswerFromFingerprint(state, current, secret, { readTruncated: true });
    const duration = performance.now() - started;

    expect(result).toEqual({ ok: false, error: { code: "answer_truncated", retryable: false } });
    expect(duration).toBeLessThan(BOUNDARY_TIME_BUDGET_MS);
  });
});

function collectStringValues(value: unknown): string[] {
  if (typeof value === "string") return [value];
  if (Array.isArray(value)) return value.flatMap(collectStringValues);
  if (typeof value !== "object" || value === null) return [];
  return Object.values(value).flatMap(collectStringValues);
}

function readTypeScriptSources(directory: URL): string[] {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const child = new URL(entry.isDirectory() ? `${entry.name}/` : entry.name, directory);
    if (entry.isDirectory()) return readTypeScriptSources(child);
    return entry.name.endsWith(".ts") ? [readFileSync(child, "utf8")] : [];
  });
}
