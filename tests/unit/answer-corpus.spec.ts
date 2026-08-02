import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

interface CorpusCase {
  id: string;
  agent: string;
  coverage: string[];
}

interface ScannerCase extends CorpusCase {
  answer: string;
  expected: Array<{ latex: string; display: boolean }>;
}

interface BoundaryCase extends CorpusCase {
  baseline: string;
  completion: string;
  expectedAnswer: string;
  expectedStrategy: string;
  readTruncated: boolean;
}

interface AnswerCorpus {
  schemaVersion: number;
  scannerCases: ScannerCase[];
  boundaryCases: BoundaryCase[];
}

const fixtureUrl = new URL("../fixtures/agents/answer-corpus.json", import.meta.url);
const fixtureSource = readFileSync(fixtureUrl, "utf8");
const corpus = JSON.parse(fixtureSource) as AnswerCorpus;
const cases: CorpusCase[] = [...corpus.scannerCases, ...corpus.boundaryCases];

describe("synthetic coding-agent answer corpus", () => {
  it("has a stable schema and unique case ids", () => {
    expect(corpus.schemaVersion).toBe(1);
    expect(new Set(cases.map(({ id }) => id)).size).toBe(cases.length);
  });

  it("covers every supported agent", () => {
    expect([...new Set(cases.map(({ agent }) => agent))].sort()).toEqual([
      "claude",
      "codex",
      "cursor",
      "opencode",
      "pi"
    ]);
  });

  it("covers the required scanner and boundary families", () => {
    const coverage = new Set(cases.flatMap(({ coverage: tags }) => tags));
    expect([...coverage]).toEqual(
      expect.arrayContaining([
        "valid_math",
        "code",
        "prices",
        "shell_variables",
        "escaped",
        "repeated_prompt",
        "repaint",
        "alternate_screen",
        "truncated_window",
        "unicode",
        "limits"
      ])
    );
  });

  it("keeps expected formulas inside their synthetic answers", () => {
    for (const testCase of corpus.scannerCases) {
      for (const formula of testCase.expected) {
        expect(testCase.answer).toContain(formula.latex);
      }
    }
  });

  it("contains no private transcript or local path markers", () => {
    for (const homeDirectory of ["Users", "home"]) {
      expect(fixtureSource).not.toContain(`/${homeDirectory}/`);
    }
    expect(fixtureSource).not.toMatch(/(?:api[_-]?key|access[_-]?token|bearer)\s*[:=]/iu);
    expect(fixtureSource).not.toContain("BEGIN PRIVATE");
  });
});
