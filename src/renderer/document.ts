import { Buffer } from "node:buffer";

import { failure, success, type Formula, type OperationResult } from "../core/contracts.js";
import { HerdrMathError, serializeError, type SafeLimitKind } from "../core/errors.js";
import { POLICY_LIMITS } from "../core/limits.js";

export type RendererDocumentSegment =
  Readonly<{ kind: "text"; text: string }> | Readonly<{ kind: "math"; latex: string; display: boolean }>;

export interface RendererDocument {
  segments: readonly RendererDocumentSegment[];
  textBytes: number;
  lineCount: number;
  blockCount: number;
  formulaCount: number;
  formulaCharacters: number;
}

export interface RendererDocumentLimits {
  responseDocumentBytes: number;
  responseDocumentLines: number;
  responseDocumentBlocks: number;
  formulasPerAnswer: number;
  charactersPerFormula: number;
  aggregateFormulaCharacters: number;
}

const DEFAULT_LIMITS: Readonly<RendererDocumentLimits> = Object.freeze({
  responseDocumentBytes: POLICY_LIMITS.responseDocumentBytes,
  responseDocumentLines: POLICY_LIMITS.responseDocumentLines,
  responseDocumentBlocks: POLICY_LIMITS.responseDocumentBlocks,
  formulasPerAnswer: POLICY_LIMITS.formulasPerAnswer,
  charactersPerFormula: POLICY_LIMITS.charactersPerFormula,
  aggregateFormulaCharacters: POLICY_LIMITS.aggregateFormulaCharacters
});

export function composeRendererDocument(
  text: string,
  formulas: readonly Formula[],
  overrides: Partial<RendererDocumentLimits> = {}
): OperationResult<RendererDocument> {
  try {
    const limits = resolveLimits(overrides);
    if (typeof text !== "string" || text.trim() === "") throw new HerdrMathError("formula_not_found");
    const textBytes = Buffer.byteLength(text, "utf8");
    assertLimit("response_document_bytes", textBytes, limits.responseDocumentBytes);
    const lineCount = text.split("\n").length;
    assertLimit("response_document_lines", lineCount, limits.responseDocumentLines);
    const blockCount = countBlocks(text);
    assertLimit("response_document_blocks", blockCount, limits.responseDocumentBlocks);
    assertLimit("formula_count", formulas.length, limits.formulasPerAnswer);
    if (formulas.length === 0) throw new HerdrMathError("formula_not_found");

    let formulaCharacters = 0;
    let cursor = 0;
    const segments: RendererDocumentSegment[] = [];
    for (const formula of formulas) {
      validateFormula(text, formula, cursor, limits.charactersPerFormula);
      formulaCharacters += [...formula.latex].length;
      assertLimit("aggregate_formula_characters", formulaCharacters, limits.aggregateFormulaCharacters);
      if (formula.start > cursor)
        segments.push(Object.freeze({ kind: "text", text: text.slice(cursor, formula.start) }));
      segments.push(Object.freeze({ kind: "math", latex: formula.latex, display: formula.display }));
      cursor = formula.end;
    }
    if (cursor < text.length) segments.push(Object.freeze({ kind: "text", text: text.slice(cursor) }));

    return success(
      Object.freeze({
        segments: Object.freeze(segments),
        textBytes,
        lineCount,
        blockCount,
        formulaCount: formulas.length,
        formulaCharacters
      })
    );
  } catch (error) {
    return failure(
      serializeError(error instanceof HerdrMathError ? error : new HerdrMathError("renderer_input_limit", {}, false))
    );
  }
}

function validateFormula(text: string, formula: Formula, cursor: number, characterLimit: number): void {
  if (
    typeof formula?.latex !== "string" ||
    formula.latex.trim() === "" ||
    typeof formula.display !== "boolean" ||
    !Number.isSafeInteger(formula.start) ||
    !Number.isSafeInteger(formula.end) ||
    formula.start < cursor ||
    formula.end <= formula.start ||
    formula.end > text.length
  ) {
    throw new HerdrMathError("invalid_latex");
  }
  const characters = [...formula.latex].length;
  assertLimit("formula_characters", characters, characterLimit);
  const source = text.slice(formula.start, formula.end);
  if (formula.delimiter === "paren" || formula.delimiter === "bracket") {
    const open = formula.delimiter === "paren" ? "\\(" : "\\[";
    const close = formula.delimiter === "paren" ? "\\)" : "\\]";
    if (!source.startsWith(open) || !source.endsWith(close)) throw new HerdrMathError("invalid_latex");
    if (source.slice(open.length, -close.length).trim() !== formula.latex) {
      throw new HerdrMathError("invalid_latex");
    }
  } else {
    const delimiter = formula.display ? "$$" : "$";
    if (!source.startsWith(delimiter) || !source.endsWith(delimiter)) throw new HerdrMathError("invalid_latex");
    if (source.slice(delimiter.length, -delimiter.length).trim() !== formula.latex) {
      throw new HerdrMathError("invalid_latex");
    }
  }
}

function countBlocks(text: string): number {
  const trimmed = text.trim();
  return trimmed === "" ? 0 : trimmed.split(/\n[ \t]*\n/u).length;
}

function resolveLimits(overrides: Partial<RendererDocumentLimits>): Readonly<RendererDocumentLimits> {
  const limits = { ...DEFAULT_LIMITS, ...overrides };
  for (const [name, value] of Object.entries(limits) as Array<[keyof RendererDocumentLimits, number]>) {
    if (!Number.isSafeInteger(value) || value <= 0 || value > DEFAULT_LIMITS[name]) {
      throw new TypeError("Renderer document limits must be positive safe integers within policy");
    }
  }
  return Object.freeze(limits);
}

function assertLimit(kind: SafeLimitKind, actual: number, limit: number): void {
  if (!Number.isSafeInteger(actual) || actual < 0 || actual > limit) {
    throw new HerdrMathError("renderer_input_limit", { limit_kind: kind, limit, actual });
  }
}
