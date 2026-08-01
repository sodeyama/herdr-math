import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

import type { Formula } from "../../src/core/contracts.js";
import { POLICY_LIMITS } from "../../src/core/limits.js";

interface ValidCase {
  id: string;
  features: string[];
  formulas: FormulaInput[];
  expectedGlyphs: string[];
}

type FormulaInput = Pick<Formula, "latex" | "display">;

interface RendererCorpus {
  schemaVersion: number;
  validCases: ValidCase[];
  invalidCases: Array<{ id: string; formula: FormulaInput; expectedError: string }>;
  limitCases: Array<Record<string, string | number>>;
  faultCases: Array<Record<string, string | number>>;
  securityCases: Array<{ id: string; formula: FormulaInput; expectedError: string }>;
}

const corpus = JSON.parse(
  readFileSync(new URL("../fixtures/renderer/formula-corpus.json", import.meta.url), "utf8")
) as RendererCorpus;

describe("release renderer corpus", () => {
  it("covers every required representative formula family", () => {
    const features = new Set(corpus.validCases.flatMap((testCase) => testCase.features));
    expect(features).toEqual(
      new Set([
        "powers",
        "fractions",
        "roots",
        "sums",
        "integrals",
        "aligned",
        "matrices",
        "greek",
        "unicode",
        "multiline"
      ])
    );
    for (const testCase of corpus.validCases) {
      expect(testCase.formulas.length).toBeGreaterThan(0);
      expect(testCase.expectedGlyphs.length).toBeGreaterThan(0);
      expect(testCase.formulas.length).toBeLessThanOrEqual(POLICY_LIMITS.formulasPerAnswer);
      expect(testCase.formulas.reduce((total, formula) => total + [...formula.latex].length, 0)).toBeLessThanOrEqual(
        POLICY_LIMITS.aggregateFormulaCharacters
      );
      for (const formula of testCase.formulas) {
        expect(formula.latex.length).toBeGreaterThan(0);
        expect([...formula.latex].length).toBeLessThanOrEqual(POLICY_LIMITS.charactersPerFormula);
        expect(typeof formula.display).toBe("boolean");
      }
    }
  });

  it("keeps ids unique across valid and failure cases", () => {
    const ids = [
      ...corpus.validCases,
      ...corpus.invalidCases,
      ...corpus.limitCases,
      ...corpus.faultCases,
      ...corpus.securityCases
    ].map(({ id }) => id);
    expect(corpus.schemaVersion).toBe(1);
    expect(new Set(ids).size).toBe(ids.length);
  });

  it("fixes invalid, count, length, aggregate, and timeout boundaries", () => {
    expect(corpus.invalidCases).toHaveLength(3);
    expect(corpus.invalidCases.every(({ expectedError }) => expectedError === "invalid_latex")).toBe(true);
    expect(corpus.limitCases).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ kind: "formula_count", count: POLICY_LIMITS.formulasPerAnswer + 1 }),
        expect.objectContaining({
          kind: "formula_characters",
          characters: POLICY_LIMITS.charactersPerFormula + 1
        }),
        expect.objectContaining({
          kind: "aggregate_formula_characters",
          formulaCharacters: 1667,
          count: 6
        })
      ])
    );
    const aggregate = corpus.limitCases.find(({ kind }) => kind === "aggregate_formula_characters");
    expect(Number(aggregate?.formulaCharacters) * Number(aggregate?.count)).toBeGreaterThan(
      POLICY_LIMITS.aggregateFormulaCharacters
    );
    expect(corpus.faultCases).toContainEqual(
      expect.objectContaining({ kind: "timeout", delayMs: POLICY_LIMITS.renderDurationMs + 1 })
    );
  });

  it("uses non-routable targets for every link-capable security case", () => {
    expect(corpus.securityCases).toHaveLength(4);
    for (const testCase of corpus.securityCases) {
      expect(testCase.expectedError).toBe("invalid_latex");
      expect(testCase.formula.latex).toMatch(/\\(?:href|htmlClass|includegraphics|url)/);
      expect(testCase.formula.latex).not.toMatch(/https?:\/\/(?!renderer-test\.invalid)/);
      expect(testCase.formula.latex).not.toContain("localhost");
    }
  });
});
