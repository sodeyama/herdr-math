import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

import { buildBaselineFingerprint } from "../../src/boundary/fingerprint-builder.js";
import { resolveAnswerFromFingerprint } from "../../src/boundary/fingerprint-resolver.js";
import { FINGERPRINT_SCHEMA_LIMITS, isFingerprintDigest } from "../../src/boundary/fingerprint-schema.js";
import { POLICY_LIMITS } from "../../src/core/limits.js";
import { computePrototypeAnswerDelta, type PrototypeBoundaryStrategy } from "../reference/prototype-boundary.js";

interface BoundaryCase {
  id: string;
  agent: "claude" | "codex" | "pi" | "opencode";
  baseline: string;
  completion: string;
  expectedAnswer: string;
  expectedStrategy: PrototypeBoundaryStrategy;
  readTruncated: boolean;
}

interface AnswerCorpus {
  boundaryCases: BoundaryCase[];
}

const corpus = JSON.parse(
  readFileSync(new URL("../fixtures/agents/answer-corpus.json", import.meta.url), "utf8")
) as AnswerCorpus;
const secret = Buffer.alloc(32, 11);

function fingerprint(testCase: BoundaryCase) {
  return buildBaselineFingerprint(
    testCase.baseline,
    {
      sessionIdentity: "synthetic-session",
      occupantIdentity: `synthetic-${testCase.agent}-session`,
      workspaceId: "w1",
      sourcePaneId: "w1:p1",
      agent: testCase.agent,
      lifecycleAuthority:
        testCase.agent === "pi" || testCase.agent === "opencode" ? "integration_hook" : "screen_detection",
      paneRevision: 10,
      eventSequence: 4,
      generation: 1,
      createdAt: new Date("2026-08-01T00:00:00.000Z")
    },
    secret
  );
}

describe("fingerprint answer resolver", () => {
  it.each(corpus.boundaryCases)("matches the reference strategy for $id", (testCase) => {
    const state = fingerprint(testCase);
    const result = resolveAnswerFromFingerprint(state, testCase.completion, secret, {
      readTruncated: testCase.readTruncated
    });
    const reference = computePrototypeAnswerDelta(testCase.baseline, testCase.completion);

    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.value.strategy).toBe(testCase.expectedStrategy);
    expect(result.value.strategy).toBe(reference?.strategy);
    expect(result.value.answer).toContain(testCase.expectedAnswer);
    expect(result.value.startOffset).toBeGreaterThanOrEqual(0);
    expect(result.value.proof.kind).toBe(result.value.strategy);
    expect(result.value.recoveredTruncation).toBe(testCase.readTruncated);
    expect(isFingerprintDigest(result.value.currentDigest)).toBe(true);
  });

  it("fails closed with different codes for complete and truncated reads", () => {
    const testCase = corpus.boundaryCases[0];
    if (testCase === undefined) throw new Error("Boundary corpus is empty.");
    const state = fingerprint(testCase);

    expect(resolveAnswerFromFingerprint(state, "unrelated current screen", secret)).toEqual({
      ok: false,
      error: { code: "boundary_failed", retryable: false }
    });
    expect(resolveAnswerFromFingerprint(state, "unrelated current screen", secret, { readTruncated: true })).toEqual({
      ok: false,
      error: { code: "answer_truncated", retryable: false }
    });
  });

  it("rejects ambiguous contextual anchors", () => {
    const anchor = "synthetic repeated anchor value 1234567890";
    const testCase: BoundaryCase = {
      id: "ambiguous",
      agent: "pi",
      baseline: `ctx\n${anchor}`,
      completion: `repaint\nctx\n${anchor}\nctx\n${anchor}`,
      expectedAnswer: "",
      expectedStrategy: "contextual_anchor",
      readTruncated: true
    };
    const result = resolveAnswerFromFingerprint(fingerprint(testCase), testCase.completion, secret, {
      readTruncated: true
    });
    expect(result).toEqual({ ok: false, error: { code: "answer_truncated", retryable: false } });
  });

  it("recovers a sliding window whose baseline ended before an appended newline", () => {
    const baseline = Array.from({ length: 120 }, (_, index) => `token-${index.toString().padStart(3, "0")}|`).join("");
    const testCase: BoundaryCase = {
      id: "sliding-before-newline",
      agent: "opencode",
      baseline,
      completion: `${baseline.slice(80)}\nanswer $x$`,
      expectedAnswer: "answer $x$",
      expectedStrategy: "sliding_window",
      readTruncated: true
    };
    const result = resolveAnswerFromFingerprint(fingerprint(testCase), testCase.completion, secret, {
      readTruncated: true
    });
    expect(result.ok && result.value.strategy).toBe("sliding_window");
    expect(result.ok && result.value.answer).toContain("answer $x$");
  });

  it("bounds current input and malformed fingerprint collections", () => {
    const testCase = corpus.boundaryCases[0];
    if (testCase === undefined) throw new Error("Boundary corpus is empty.");
    const state = fingerprint(testCase);
    const oversized = "x".repeat(POLICY_LIMITS.paneReadBytes + 1);
    const oversizedResult = resolveAnswerFromFingerprint(state, oversized, secret);
    expect(oversizedResult).toEqual({
      ok: false,
      error: {
        code: "scanner_input_limit",
        retryable: false,
        details: {
          limit_kind: "pane_read_bytes",
          limit: POLICY_LIMITS.paneReadBytes,
          actual: POLICY_LIMITS.paneReadBytes + 1
        }
      }
    });

    const corrupt = structuredClone(state);
    corrupt.baseline.prefix_checkpoints = Array.from(
      { length: FINGERPRINT_SCHEMA_LIMITS.maxPrefixCheckpoints + 1 },
      () => ({ end_offset: 1, digest: "a".repeat(64) })
    );
    expect(resolveAnswerFromFingerprint(corrupt, testCase.completion, secret)).toEqual({
      ok: false,
      error: { code: "state_corrupt", retryable: false }
    });
  });

  it("rejects an invalid secret without exposing content", () => {
    const testCase = corpus.boundaryCases[0];
    if (testCase === undefined) throw new Error("Boundary corpus is empty.");
    const result = resolveAnswerFromFingerprint(fingerprint(testCase), testCase.completion, Buffer.alloc(31));
    expect(result).toEqual({ ok: false, error: { code: "state_corrupt", retryable: false } });
    expect(JSON.stringify(result)).not.toContain(testCase.completion);
  });
});
