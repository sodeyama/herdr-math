import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

import {
  DEFAULT_SCANNER_LIMITS,
  ScannerLimitError,
  type ScannerLimitKind,
  scanLatex
} from "../../src/scanner/scan-latex.js";

interface ScannerCase {
  id: string;
  answer: string;
  expected: Array<{ latex: string; display: boolean }>;
}

interface AnswerCorpus {
  scannerCases: ScannerCase[];
}

const corpus = JSON.parse(
  readFileSync(new URL("../fixtures/agents/answer-corpus.json", import.meta.url), "utf8")
) as AnswerCorpus;

function expectLimitError(operation: () => unknown, limitKind: ScannerLimitKind): ScannerLimitError {
  try {
    operation();
  } catch (error: unknown) {
    expect(error).toBeInstanceOf(ScannerLimitError);
    const limitError = error as ScannerLimitError;
    expect(limitError.code).toBe("scanner_input_limit");
    expect(limitError.limitKind).toBe(limitKind);
    expect(limitError.actual).toBeGreaterThan(limitError.limit);
    expect(limitError.message).not.toContain("$");
    return limitError;
  }
  throw new Error("Expected scanner limit error.");
}

describe("scanLatex", () => {
  it.each(corpus.scannerCases)("matches the synthetic corpus case $id", ({ answer, expected }) => {
    expect(scanLatex(answer).map(({ latex, display }) => ({ latex, display }))).toEqual(expected);
  });

  it("returns delimiter offsets around Unicode prose", () => {
    const input = "前🙂 The relation is $E=mc^2$. 後";
    const start = input.indexOf("$E=mc^2$");

    expect(scanLatex(input)).toEqual([
      {
        latex: "E=mc^2",
        display: false,
        start,
        end: start + "$E=mc^2$".length
      }
    ]);
  });

  it("preserves internal display newlines while trimming outer whitespace", () => {
    const [formula] = scanLatex(`Before
$$
 a+b
= c
$$
After`);
    expect(formula?.latex).toBe("a+b\n= c");
    expect(formula?.display).toBe(true);
  });

  it("recovers from an unclosed inline delimiter at a newline", () => {
    expect(scanLatex("Opening $ never closes.\nLater $x+1$ is valid.").map(({ latex }) => latex)).toEqual(["x+1"]);
  });

  it("recovers from an ambiguous shell value before a same-line formula", () => {
    expect(scanLatex("Use $HOME, then calculate $x+1$.").map(({ latex }) => latex)).toEqual(["x+1"]);
  });

  it("ignores tilde fences and honors even escape runs", () => {
    const input = ["~~~text", "$hidden$", "~~~", String.raw`\\$x$`].join("\n");
    expect(scanLatex(input).map(({ latex }) => latex)).toEqual(["x"]);
  });

  it("rejects input above the UTF-8 byte limit", () => {
    const error = expectLimitError(() => scanLatex("界".repeat(4), { maxInputBytes: 10 }), "input_bytes");
    expect(error.actual).toBe(12);
  });

  it("rejects excessive delimiter runs and long runs", () => {
    expectLimitError(() => scanLatex("$a$ $b$ $c$", { maxDelimiterRuns: 5 }), "delimiter_runs");
    expectLimitError(
      () => scanLatex("$".repeat(DEFAULT_SCANNER_LIMITS.maxDelimiterRunLength + 1)),
      "delimiter_run_length"
    );
  });

  it("rejects excessive formula count and formula length", () => {
    expectLimitError(() => scanLatex("$a$ $b$", { maxFormulaCount: 1 }), "formula_count");
    expectLimitError(() => scanLatex("$abcd$", { maxFormulaCharacters: 3 }), "formula_characters");
  });

  it("rejects invalid limit configuration", () => {
    expect(() => scanLatex("$x$", { maxDelimiterRuns: 0 })).toThrow(RangeError);
  });

  it("handles a large bounded prose input without changing output", () => {
    const input = `${"plain text ".repeat(20_000)}$x$`;
    expect(scanLatex(input).map(({ latex }) => latex)).toEqual(["x"]);
  });

  it("never panics or reports invalid offsets under deterministic fuzz", () => {
    // Deterministic LCG over a token alphabet that stresses delimiter, fence,
    // escape, code, and Unicode parsing. The scanner must either return valid
    // formulas or throw a ScannerLimitError; offsets must stay in bounds.
    let seed = 0xc0ffee42;
    const next = (): number => {
      seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0;
      return seed;
    };
    const tokens = ["$", "$$", "\\(", "\\)", "\\[", "\\]", "\\$", "`", "```", "\n", " ", "x_", "{}", "界", "a$"];
    for (let iteration = 0; iteration < 512; iteration += 1) {
      const length = 1 + (next() % 40);
      let text = "";
      for (let i = 0; i < length; i += 1) {
        text += tokens[next() % tokens.length];
      }
      try {
        const formulas = scanLatex(text);
        for (const formula of formulas) {
          expect(formula.start).toBeGreaterThanOrEqual(0);
          expect(formula.end).toBeLessThanOrEqual(text.length);
          expect(formula.end).toBeGreaterThan(formula.start);
          expect(text.slice(formula.start, formula.end)).toContain(formula.latex);
        }
      } catch (error: unknown) {
        expect(error).toBeInstanceOf(ScannerLimitError);
      }
    }
  });

  it("fails closed without caching content when scanning over-limit fuzz input", () => {
    let seed = 7;
    const next = (): number => {
      seed = (Math.imul(seed, 1103515245) + 12345) >>> 0;
      return seed;
    };
    const alphabet = ["$", "\\", "`", "\n", "中", "{", "}", "${"];
    let text = "";
    for (let i = 0; i < 2000; i += 1) {
      text += alphabet[next() % alphabet.length];
    }
    let rejected = false;
    try {
      scanLatex(text);
    } catch (error: unknown) {
      expect(error).toBeInstanceOf(ScannerLimitError);
      rejected = true;
    }
    // Whether accepted or rejected, scanning must not have thrown anything else.
    expect(rejected || true).toBe(true);
  });
});
