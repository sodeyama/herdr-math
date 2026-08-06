import type { Formula } from "../core/contracts.js";
import { HerdrMathError, type SafeLimitKind } from "../core/errors.js";
import { POLICY_LIMITS } from "../core/limits.js";

export type { Formula } from "../core/contracts.js";

export interface ScannerLimits {
  maxInputBytes: number;
  maxDelimiterRuns: number;
  maxDelimiterRunLength: number;
  maxFormulaCount: number;
  maxFormulaCharacters: number;
}

export type ScannerLimitKind = Extract<
  SafeLimitKind,
  "input_bytes" | "delimiter_runs" | "delimiter_run_length" | "formula_count" | "formula_characters"
>;

export const DEFAULT_SCANNER_LIMITS: Readonly<ScannerLimits> = Object.freeze({
  maxInputBytes: POLICY_LIMITS.scannerInputBytes,
  maxDelimiterRuns: POLICY_LIMITS.delimiterRuns,
  maxDelimiterRunLength: POLICY_LIMITS.delimiterRunCharacters,
  maxFormulaCount: POLICY_LIMITS.formulasPerAnswer,
  maxFormulaCharacters: POLICY_LIMITS.charactersPerFormula
});

export class ScannerLimitError extends HerdrMathError {
  constructor(
    readonly limitKind: ScannerLimitKind,
    readonly limit: number,
    readonly actual: number
  ) {
    super("scanner_input_limit", { limit_kind: limitKind, limit, actual });
    this.name = "ScannerLimitError";
  }
}

interface Fence {
  marker: string;
  length: number;
}

interface DelimiterBudget {
  runs: number;
}

interface CandidateResult {
  formula?: Formula;
  nextIndex: number;
}

export function scanLatex(input: string, overrides: Partial<ScannerLimits> = {}): Formula[] {
  const limits = resolveLimits(overrides);
  assertInputBytes(input, limits.maxInputBytes);

  const formulas: Formula[] = [];
  const budget: DelimiterBudget = { runs: 0 };
  let index = 0;
  let fence: Fence | undefined;
  let inlineTicks = 0;

  while (index < input.length) {
    if (isLineStart(input, index)) {
      const detectedFence = readFence(input, index);
      if (detectedFence !== undefined) {
        const { marker, length } = detectedFence;
        if (fence === undefined) {
          fence = { marker, length };
        } else if (fence.marker === marker && length >= fence.length) {
          fence = undefined;
        }
        index = skipLine(input, index);
        continue;
      }
    }

    if (fence !== undefined) {
      index += 1;
      continue;
    }

    if (input[index] === "`" && !isEscaped(input, index)) {
      const ticks = countRun(input, index, "`");
      if (inlineTicks === 0) {
        inlineTicks = ticks;
      } else if (ticks === inlineTicks) {
        inlineTicks = 0;
      }
      index += ticks;
      continue;
    }

    if (inlineTicks > 0) {
      index += 1;
      continue;
    }

    // \( ... \) and \[ ... \] delimiters
    if (input[index] === "\\" && !isEscaped(input, index)) {
      const next = input[index + 1];
      if (next === "(" || next === "[") {
        const closing = next === "(" ? "\\)" : "\\]";
        const contentStart = index + 2;
        let cursor = contentStart;
        while (cursor < input.length) {
          if (input[cursor] === "\n" && next === "(") {
            break;
          }
          if (input.startsWith(closing, cursor)) {
            const latex = input.slice(contentStart, cursor).trim();
            const formulaCharacters = [...latex].length;
            if (formulaCharacters > limits.maxFormulaCharacters) {
              throw new ScannerLimitError("formula_characters", limits.maxFormulaCharacters, formulaCharacters);
            }
            if (latex.length > 0) {
              formulas.push({
                latex,
                display: next === "[",
                start: index,
                end: cursor + closing.length,
                delimiter: next === "(" ? "paren" : "bracket"
              });
              if (formulas.length > limits.maxFormulaCount) {
                throw new ScannerLimitError("formula_count", limits.maxFormulaCount, formulas.length);
              }
            }
            cursor += closing.length;
            break;
          }
          cursor += 1;
        }
        index = cursor;
        continue;
      }
    }

    // Bare `[ ... ]` block math: some agents drop the backslash and emit
    // `[ \boldsymbol{x} ]` instead of `\[ ... \]`. Only treat this as math at
    // the start of a line, and only when the content looks like LaTeX and the
    // closing bracket isn't immediately followed by `(` (a Markdown link).
    if (input[index] === "[" && !isEscaped(input, index) && isLineStart(input, index)) {
      const bareMatch = scanBareBracketMath(input, index, limits);
      if (bareMatch !== undefined) {
        formulas.push(bareMatch.formula);
        if (formulas.length > limits.maxFormulaCount) {
          throw new ScannerLimitError("formula_count", limits.maxFormulaCount, formulas.length);
        }
        index = bareMatch.nextIndex;
        continue;
      }
    }

    if (input[index] !== "$" || isEscaped(input, index)) {
      index += 1;
      continue;
    }

    const runLength = countRun(input, index, "$");
    consumeDelimiterRun(budget, runLength, limits);
    if (runLength > 2) {
      index += runLength;
      continue;
    }

    const delimiterLength: 1 | 2 = runLength === 2 ? 2 : 1;
    const candidate = scanCandidate(input, index, delimiterLength, budget, limits);
    if (candidate.formula !== undefined) {
      formulas.push(candidate.formula);
      if (formulas.length > limits.maxFormulaCount) {
        throw new ScannerLimitError("formula_count", limits.maxFormulaCount, formulas.length);
      }
    }
    index = candidate.nextIndex;
  }

  return formulas;
}

function scanCandidate(
  input: string,
  start: number,
  delimiterLength: 1 | 2,
  budget: DelimiterBudget,
  limits: ScannerLimits
): CandidateResult {
  const contentStart = start + delimiterLength;
  let cursor = contentStart;

  while (cursor < input.length) {
    if (delimiterLength === 1 && input[cursor] === "\n") {
      return { nextIndex: cursor + 1 };
    }
    if (input[cursor] !== "$" || isEscaped(input, cursor)) {
      cursor += 1;
      continue;
    }

    const runLength = countRun(input, cursor, "$");
    if (runLength > limits.maxDelimiterRunLength) {
      throw new ScannerLimitError("delimiter_run_length", limits.maxDelimiterRunLength, runLength);
    }
    if (runLength > 2) {
      consumeDelimiterRun(budget, runLength, limits);
      cursor += runLength;
      continue;
    }
    if (runLength !== delimiterLength) {
      return { nextIndex: cursor };
    }

    const latex = input.slice(contentStart, cursor).trim();
    const formulaCharacters = [...latex].length;
    if (formulaCharacters > limits.maxFormulaCharacters) {
      throw new ScannerLimitError("formula_characters", limits.maxFormulaCharacters, formulaCharacters);
    }
    if (delimiterLength === 1 && looksLikeNextDollarValue(input, contentStart, cursor)) {
      return { nextIndex: cursor };
    }
    if (latex.length === 0 || (delimiterLength === 1 && !isPlausibleInlineLatex(latex))) {
      return { nextIndex: cursor };
    }

    consumeDelimiterRun(budget, runLength, limits);
    return {
      formula: {
        latex,
        display: delimiterLength === 2,
        start,
        end: cursor + delimiterLength
      },
      nextIndex: cursor + delimiterLength
    };
  }

  return { nextIndex: input.length };
}

function resolveLimits(overrides: Partial<ScannerLimits>): ScannerLimits {
  const limits = { ...DEFAULT_SCANNER_LIMITS, ...overrides };
  for (const [name, value] of Object.entries(limits)) {
    if (!Number.isSafeInteger(value) || value <= 0) {
      throw new RangeError(`Scanner limit ${name} must be a positive safe integer.`);
    }
  }
  return limits;
}

function assertInputBytes(input: string, limit: number): void {
  let bytes = 0;
  for (const character of input) {
    const codePoint = character.codePointAt(0);
    if (codePoint === undefined) {
      continue;
    }
    bytes += codePoint <= 0x7f ? 1 : codePoint <= 0x7ff ? 2 : codePoint <= 0xffff ? 3 : 4;
    if (bytes > limit) {
      throw new ScannerLimitError("input_bytes", limit, bytes);
    }
  }
}

function consumeDelimiterRun(budget: DelimiterBudget, runLength: number, limits: ScannerLimits): void {
  if (runLength > limits.maxDelimiterRunLength) {
    throw new ScannerLimitError("delimiter_run_length", limits.maxDelimiterRunLength, runLength);
  }
  budget.runs += 1;
  if (budget.runs > limits.maxDelimiterRuns) {
    throw new ScannerLimitError("delimiter_runs", limits.maxDelimiterRuns, budget.runs);
  }
}

function isPlausibleInlineLatex(latex: string): boolean {
  if (latex.includes("\n") || latex.includes("$")) {
    return false;
  }
  return !/[、。\p{Script=Hiragana}\p{Script=Katakana}\p{Script=Han}]/u.test(latex);
}

const BARE_BRACKET_LATEX_HINT = /\\[A-Za-z]+|\\(?:mid|cdot|top|left|right)|[_^]\{|\\[[({]/;

function isPlausibleBlockLatex(latex: string): boolean {
  if (latex.length === 0 || latex.includes("[") || latex.includes("]")) {
    return false;
  }
  if (/[、。\p{Script=Hiragana}\p{Script=Katakana}\p{Script=Han}]/u.test(latex)) {
    return false;
  }
  return BARE_BRACKET_LATEX_HINT.test(latex);
}

interface BareBracketMatch {
  formula: Formula;
  nextIndex: number;
}

function scanBareBracketMath(input: string, start: number, limits: ScannerLimits): BareBracketMatch | undefined {
  const contentStart = start + 1;
  let cursor = contentStart;
  while (cursor < input.length) {
    if (input[cursor] === "]" && !isEscaped(input, cursor)) {
      if (input[cursor + 1] === "(") {
        // Looks like a Markdown link `[label](url)`, not math.
        return undefined;
      }
      const latex = input.slice(contentStart, cursor).trim();
      if (!isPlausibleBlockLatex(latex)) {
        return undefined;
      }
      const formulaCharacters = [...latex].length;
      if (formulaCharacters > limits.maxFormulaCharacters) {
        throw new ScannerLimitError("formula_characters", limits.maxFormulaCharacters, formulaCharacters);
      }
      return {
        formula: {
          latex,
          display: true,
          start,
          end: cursor + 1,
          delimiter: "bracket"
        },
        nextIndex: cursor + 1
      };
    }
    cursor += 1;
  }
  return undefined;
}

function looksLikeNextDollarValue(input: string, contentStart: number, candidateIndex: number): boolean {
  const next = input[candidateIndex + 1];
  if (next === undefined || !/[A-Za-z0-9_]/.test(next)) {
    return false;
  }
  const content = input.slice(contentStart, candidateIndex).trim();
  if (content.length === 0 || /[\\^_=+*/<>()[\]{}]/.test(content)) {
    return false;
  }
  return /\s/.test(content) || /^\d/.test(content) || /^[A-Z_][A-Z0-9_]*$/.test(content);
}

function isEscaped(input: string, index: number): boolean {
  let slashes = 0;
  for (let cursor = index - 1; cursor >= 0 && input[cursor] === "\\"; cursor -= 1) {
    slashes += 1;
  }
  return slashes % 2 === 1;
}

function isLineStart(input: string, index: number): boolean {
  return index === 0 || input[index - 1] === "\n";
}

function readFence(input: string, index: number): Fence | undefined {
  let cursor = index;
  let spaces = 0;
  while (spaces < 4 && input[cursor] === " ") {
    spaces += 1;
    cursor += 1;
  }
  if (spaces > 3) {
    return undefined;
  }

  const marker = input[cursor];
  if (marker !== "`" && marker !== "~") {
    return undefined;
  }
  const length = countRun(input, cursor, marker);
  return length >= 3 ? { marker, length } : undefined;
}

function skipLine(input: string, index: number): number {
  const newline = input.indexOf("\n", index);
  return newline === -1 ? input.length : newline + 1;
}

function countRun(input: string, index: number, character: string): number {
  let length = 0;
  while (input[index + length] === character) {
    length += 1;
  }
  return length;
}
