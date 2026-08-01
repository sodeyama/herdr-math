import { Buffer } from "node:buffer";

import { failure, success, type OperationResult } from "../core/contracts.js";
import { HerdrMathError, serializeError } from "../core/errors.js";
import { POLICY_LIMITS } from "../core/limits.js";

export interface TerminalStyleSummary {
  hasBold: boolean;
  hasDim: boolean;
  hasItalic: boolean;
  hasUnderline: boolean;
  hasInverse: boolean;
  hasForeground: boolean;
  hasBackground: boolean;
  nonWhitespaceCharacters: number;
  italicCharacters: number;
}

export interface StyledTerminalLine extends TerminalStyleSummary {
  text: string;
  startOffset: number;
  endOffset: number;
}

export interface StyledTerminalSnapshot {
  text: string;
  lines: readonly StyledTerminalLine[];
}

interface MutableStyle {
  bold: boolean;
  dim: boolean;
  italic: boolean;
  underline: boolean;
  inverse: boolean;
  foreground: boolean;
  background: boolean;
}

interface MutableLine extends TerminalStyleSummary {
  characters: string[];
}

const ESCAPE = "\u001b";
const SGR_END = "m";

export function parseMatchingAnsiSnapshot(
  plainText: string,
  ansiText: string
): OperationResult<StyledTerminalSnapshot> {
  try {
    assertBoundedInput(plainText);
    assertBoundedInput(ansiText);
    const snapshot = parseAnsiSnapshot(ansiText, plainText);
    return success(snapshot);
  } catch (error) {
    return failure(
      serializeError(error instanceof HerdrMathError ? error : new HerdrMathError("conclusion_boundary_failed"))
    );
  }
}

function parseAnsiSnapshot(source: string, expectedText: string): StyledTerminalSnapshot {
  const style = defaultStyle();
  const lines: StyledTerminalLine[] = [];
  const expectedLines = expectedText.split("\n");
  const expectedLineCount = expectedText.endsWith("\n") ? expectedLines.length - 1 : expectedLines.length;
  let line = emptyLine();
  let text = "";
  let lineIndex = 0;

  const finishLine = (withNewline: boolean): void => {
    const rawLine = line.characters.join("");
    const expectedLine = expectedLines[lineIndex];
    if (
      expectedLine === undefined ||
      (rawLine !== expectedLine &&
        (!rawLine.startsWith(expectedLine) || !/^[ \t]*$/u.test(rawLine.slice(expectedLine.length))))
    ) {
      throw new HerdrMathError("conclusion_boundary_failed");
    }
    const lineText = expectedLine;
    const startOffset = text.length;
    text += lineText;
    const endOffset = text.length;
    lines.push(
      Object.freeze({
        text: lineText,
        startOffset,
        endOffset,
        hasBold: line.hasBold,
        hasDim: line.hasDim,
        hasItalic: line.hasItalic,
        hasUnderline: line.hasUnderline,
        hasInverse: line.hasInverse,
        hasForeground: line.hasForeground,
        hasBackground: line.hasBackground,
        nonWhitespaceCharacters: line.nonWhitespaceCharacters,
        italicCharacters: line.italicCharacters
      })
    );
    if (withNewline) text += "\n";
    line = emptyLine();
    lineIndex += 1;
  };

  for (let index = 0; index < source.length;) {
    const character = source[index];
    if (character === ESCAPE) {
      index = consumeSgr(source, index, style);
      continue;
    }
    if (character === "\r") {
      if (source[index + 1] !== "\n") throw new HerdrMathError("conclusion_boundary_failed");
      index += 1;
      continue;
    }
    if (character === "\n") {
      finishLine(true);
      index += 1;
      continue;
    }
    if (character === undefined || isForbiddenControl(character)) {
      throw new HerdrMathError("conclusion_boundary_failed");
    }
    appendCharacter(line, character, style);
    index += 1;
  }
  if (line.characters.length > 0 || source.endsWith("\n") === false) finishLine(false);
  if (lineIndex !== expectedLineCount || text !== expectedText) {
    throw new HerdrMathError("conclusion_boundary_failed");
  }
  return Object.freeze({ text, lines: Object.freeze(lines) });
}

function consumeSgr(source: string, start: number, style: MutableStyle): number {
  if (source[start + 1] !== "[") throw new HerdrMathError("conclusion_boundary_failed");
  const end = source.indexOf(SGR_END, start + 2);
  if (end === -1 || end - start > 96) throw new HerdrMathError("conclusion_boundary_failed");
  const parameters = source.slice(start + 2, end);
  if (!/^[0-9;:]*$/.test(parameters)) throw new HerdrMathError("conclusion_boundary_failed");
  applySgr(parameters, style);
  return end + 1;
}

function applySgr(source: string, style: MutableStyle): void {
  const values = source === "" ? [0] : source.replaceAll(":", ";").split(";").map(Number);
  for (let index = 0; index < values.length; index += 1) {
    const value = values[index];
    if (value === 0) Object.assign(style, defaultStyle());
    else if (value === 1) style.bold = true;
    else if (value === 2) style.dim = true;
    else if (value === 3) style.italic = true;
    else if (value === 4) style.underline = true;
    else if (value === 7) style.inverse = true;
    else if (value === 22) {
      style.bold = false;
      style.dim = false;
    } else if (value === 23) style.italic = false;
    else if (value === 24) style.underline = false;
    else if (value === 27) style.inverse = false;
    else if (value === 39) style.foreground = false;
    else if (value === 49) style.background = false;
    else if (
      (value !== undefined && value >= 30 && value <= 37) ||
      (value !== undefined && value >= 90 && value <= 97)
    ) {
      style.foreground = true;
    } else if (
      (value !== undefined && value >= 40 && value <= 47) ||
      (value !== undefined && value >= 100 && value <= 107)
    ) {
      style.background = true;
    } else if (value === 38 || value === 48) {
      const consumed = consumeExtendedColor(values, index);
      if (value === 38) style.foreground = true;
      else style.background = true;
      index += consumed;
    } else {
      throw new HerdrMathError("conclusion_boundary_failed");
    }
  }
}

function consumeExtendedColor(values: number[], index: number): number {
  const mode = values[index + 1];
  if (mode === 5 && isColorByte(values[index + 2])) return 2;
  if (
    mode === 2 &&
    isColorByte(values[index + 2]) &&
    isColorByte(values[index + 3]) &&
    isColorByte(values[index + 4])
  ) {
    return 4;
  }
  throw new HerdrMathError("conclusion_boundary_failed");
}

function appendCharacter(line: MutableLine, character: string, style: MutableStyle): void {
  line.characters.push(character);
  if (/\s/u.test(character)) return;
  line.nonWhitespaceCharacters += 1;
  if (style.bold) line.hasBold = true;
  if (style.dim) line.hasDim = true;
  if (style.italic) {
    line.hasItalic = true;
    line.italicCharacters += 1;
  }
  if (style.underline) line.hasUnderline = true;
  if (style.inverse) line.hasInverse = true;
  if (style.foreground) line.hasForeground = true;
  if (style.background) line.hasBackground = true;
}

function emptyLine(): MutableLine {
  return {
    characters: [],
    hasBold: false,
    hasDim: false,
    hasItalic: false,
    hasUnderline: false,
    hasInverse: false,
    hasForeground: false,
    hasBackground: false,
    nonWhitespaceCharacters: 0,
    italicCharacters: 0
  };
}

function defaultStyle(): MutableStyle {
  return {
    bold: false,
    dim: false,
    italic: false,
    underline: false,
    inverse: false,
    foreground: false,
    background: false
  };
}

function assertBoundedInput(value: string): void {
  if (typeof value !== "string" || Buffer.byteLength(value, "utf8") > POLICY_LIMITS.paneReadBytes) {
    throw new HerdrMathError("conclusion_boundary_failed");
  }
}

function isForbiddenControl(character: string): boolean {
  const code = character.charCodeAt(0);
  return (code < 0x20 && character !== "\t") || code === 0x7f;
}

function isColorByte(value: number | undefined): boolean {
  return value !== undefined && Number.isInteger(value) && value >= 0 && value <= 255;
}
