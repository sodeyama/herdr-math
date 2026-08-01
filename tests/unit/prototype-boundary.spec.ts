import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

import {
  computePrototypeAnswerDelta,
  isPrototypeReadTruncated,
  type PrototypeBoundaryStrategy
} from "../reference/prototype-boundary.js";

interface BoundaryCase {
  id: string;
  baseline: string;
  completion: string;
  expectedAnswer: string;
  expectedStrategy: PrototypeBoundaryStrategy;
}

interface AnswerCorpus {
  boundaryCases: BoundaryCase[];
}

const corpus = JSON.parse(
  readFileSync(new URL("../fixtures/agents/answer-corpus.json", import.meta.url), "utf8")
) as AnswerCorpus;

describe("prototype boundary reference", () => {
  it.each(corpus.boundaryCases)("resolves the synthetic corpus case $id", (testCase) => {
    const result = computePrototypeAnswerDelta(testCase.baseline, testCase.completion);
    expect(result?.strategy).toBe(testCase.expectedStrategy);
    expect(result?.text).toContain(testCase.expectedAnswer);
  });

  it("keeps the full appended region for a prompt-only baseline", () => {
    const prompt = "synthetic-user project % 1234567890";
    const after = `${prompt}\nAnswer $x$\n${prompt}`;
    expect(computePrototypeAnswerDelta(prompt, after)).toEqual({
      text: `\nAnswer $x$\n${prompt}`,
      strategy: "exact_prefix"
    });
  });

  it("selects the context-qualified occurrence of a repeated prompt", () => {
    const prompt = "synthetic prompt with unique context 1234567890";
    const before = `Earlier answer\n${prompt}`;
    const after = `Repainted\n${before}\nNew answer $E=mc^2$\n${prompt}`;
    const result = computePrototypeAnswerDelta(before, after);
    expect(result?.strategy).toBe("contextual_anchor");
    expect(result?.text).toContain("New answer $E=mc^2$");
  });

  it("fails closed when no boundary can be proved", () => {
    expect(computePrototypeAnswerDelta("unrelated baseline", "completely new screen")).toBeNull();
  });

  it("detects explicit and line-window truncation", () => {
    expect(isPrototypeReadTruncated({ text: "a", truncated: true }, 3)).toBe(true);
    expect(isPrototypeReadTruncated({ text: "a\nb\nc", truncated: false }, 3)).toBe(true);
    expect(isPrototypeReadTruncated({ text: "a\nb", truncated: false }, 3)).toBe(false);
    expect(isPrototypeReadTruncated({ text: "", truncated: false }, 1)).toBe(false);
  });

  it("bounds raw inputs and contextual anchor candidates", () => {
    expect(() => computePrototypeAnswerDelta("a".repeat(11), "a", { maxInputCharacters: 10 })).toThrow(
      "Prototype boundary input limit exceeded."
    );

    const anchor = "repeated synthetic anchor 1234567890";
    const before = `context that should match\n${anchor}`;
    const after = `${`${anchor}\n`.repeat(300)}answer`;
    const result = computePrototypeAnswerDelta(before, after, { maxAnchorOccurrences: 16 });
    expect(result?.strategy).toBe("contextual_anchor");
  });

  it("rejects invalid reference limits", () => {
    expect(() => computePrototypeAnswerDelta("a", "ab", { maxAnchorOccurrences: 0 })).toThrow(RangeError);
    expect(() => isPrototypeReadTruncated({ text: "a", truncated: false }, 0)).toThrow(RangeError);
  });
});
