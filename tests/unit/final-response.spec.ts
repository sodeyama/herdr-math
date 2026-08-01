import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

import type { SupportedAgent } from "../../src/boundary/fingerprint-schema.js";
import { parseMatchingAnsiSnapshot } from "../../src/presentation/ansi-snapshot.js";
import { extractFinalResponse } from "../../src/presentation/final-response.js";
import { scanLatex } from "../../src/scanner/scan-latex.js";

interface PresentationCase {
  id: string;
  agent: SupportedAgent;
  plain: string;
  ansi: string;
  expected: string;
  expectedFormulas: string[];
}

interface PresentationCorpus {
  schemaVersion: number;
  cases: PresentationCase[];
}

const fixtureSource = readFileSync(
  new URL("../fixtures/presentation/final-response-corpus.json", import.meta.url),
  "utf8"
);
const corpus = JSON.parse(fixtureSource) as PresentationCorpus;

describe("coding-agent final response extraction", () => {
  it("covers every supported coding agent with unique synthetic cases", () => {
    expect(corpus.schemaVersion).toBe(1);
    expect(new Set(corpus.cases.map(({ id }) => id)).size).toBe(corpus.cases.length);
    expect(new Set(corpus.cases.map(({ agent }) => agent))).toEqual(new Set(["claude", "codex", "pi", "opencode"]));
  });

  it.each(corpus.cases)("extracts only the normalized final response for $id", (testCase) => {
    const snapshot = parseMatchingAnsiSnapshot(testCase.plain, testCase.ansi);
    expect(snapshot.ok).toBe(true);
    if (!snapshot.ok) return;

    const result = extractFinalResponse({
      agent: testCase.agent,
      answer: testCase.plain,
      answerStartOffset: 0,
      snapshot: snapshot.value
    });

    expect(result).toMatchObject({ ok: true, value: { text: testCase.expected } });
    if (!result.ok) return;
    expect(scanLatex(result.value.text).map(({ latex }) => latex)).toEqual(testCase.expectedFormulas);
  });

  it("extracts a verified answer slice from a larger matching snapshot", () => {
    const prefix = "Stable terminal history.\n";
    const answer = "Clean final response with $x=1$.";
    const snapshot = parseMatchingAnsiSnapshot(prefix + answer, prefix + answer);
    expect(snapshot.ok).toBe(true);
    if (!snapshot.ok) return;

    expect(
      extractFinalResponse({ agent: "claude", answer, answerStartOffset: prefix.length, snapshot: snapshot.value })
    ).toMatchObject({ ok: true, value: { text: answer, sourceStartOffset: prefix.length } });
  });

  it("fails closed for invalid offsets and unprovable agent chrome", () => {
    const clean = "Final $x$.";
    const snapshot = parseMatchingAnsiSnapshot(clean, clean);
    expect(snapshot.ok).toBe(true);
    if (!snapshot.ok) return;

    expect(
      extractFinalResponse({ agent: "codex", answer: clean, answerStartOffset: 1, snapshot: snapshot.value })
    ).toEqual({
      ok: false,
      error: { code: "conclusion_boundary_failed", retryable: false }
    });

    const ambiguous = "────────────────────────\nOnly one boundary with $x$.";
    const ambiguousSnapshot = parseMatchingAnsiSnapshot(ambiguous, ambiguous);
    expect(ambiguousSnapshot.ok).toBe(true);
    if (!ambiguousSnapshot.ok) return;
    expect(
      extractFinalResponse({
        agent: "codex",
        answer: ambiguous,
        answerStartOffset: 0,
        snapshot: ambiguousSnapshot.value
      })
    ).toEqual({ ok: false, error: { code: "conclusion_boundary_failed", retryable: false } });
  });

  it("requires a styled Pi boundary and footer for suffix recovery", () => {
    const noBoundary = "Final $x$.\n\n────────────────────────";
    const noBoundarySnapshot = parseMatchingAnsiSnapshot(noBoundary, noBoundary);
    expect(noBoundarySnapshot.ok).toBe(true);
    if (!noBoundarySnapshot.ok) return;
    expect(
      extractFinalResponse({
        agent: "pi",
        answer: noBoundary,
        answerStartOffset: 0,
        snapshot: noBoundarySnapshot.value,
        requirePiFooter: true
      })
    ).toMatchObject({ ok: false, error: { code: "conclusion_boundary_failed" } });

    const noFooter = "Reasoning $r$.\n\nFinal $x$.";
    const noFooterSnapshot = parseMatchingAnsiSnapshot(noFooter, "\u001b[3mReasoning $r$.\u001b[0m\n\nFinal $x$.");
    expect(noFooterSnapshot.ok).toBe(true);
    if (!noFooterSnapshot.ok) return;
    expect(
      extractFinalResponse({
        agent: "pi",
        answer: noFooter,
        answerStartOffset: 0,
        snapshot: noFooterSnapshot.value,
        requirePiFooter: true
      })
    ).toMatchObject({ ok: false, error: { code: "conclusion_boundary_failed" } });
  });

  it("keeps synthetic fixtures free of local paths and credentials", () => {
    expect(fixtureSource).not.toMatch(/\/Users\/|\\Users\\|api[_-]?key|access[_-]?token|bearer\s+/iu);
  });
});
