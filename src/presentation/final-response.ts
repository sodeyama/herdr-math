import type { SupportedAgent } from "../boundary/fingerprint-schema.js";
import { failure, success, type OperationResult } from "../core/contracts.js";
import { HerdrMathError, serializeError } from "../core/errors.js";
import type { StyledTerminalLine, StyledTerminalSnapshot } from "./ansi-snapshot.js";

export interface FinalResponseRequest {
  agent: SupportedAgent;
  answer: string;
  answerStartOffset: number;
  snapshot: StyledTerminalSnapshot;
  requirePiFooter?: boolean;
  requireOpenCodeChrome?: boolean;
}

export interface FinalResponse {
  text: string;
  sourceStartOffset: number;
  sourceEndOffset: number;
}

interface ResponseLine extends StyledTerminalLine {
  sourceIndex: number;
}

const SEPARATOR = /^\s*[─━═]{20,}\s*$/u;
const CODEX_WORKED_FOOTER = /^\s*─+\s*Worked for\b/u;
const CODEX_PROMPT = /^\s*›(?:\s|$)/u;
const CURSOR_TOOL =
  /^(?:Grepped|Searched|Read|Listed|Globbed|Wrote|Applied|Ran|Deleted|Updated|Built|Checked|Called|Explored|Task|Shell|Edited|Created|Used)\b/u;
const CURSOR_SHELL = /^\$\s+\S/u;
const CURSOR_STATUS_BAR = /^[\s▄▀▌▐▆▇█]+$/u;
const CURSOR_FOOTER =
  /^(?:→\s*Add a follow-up|\d+\s+task(?:\s|$)|(?:Composer|GPT|Claude|Sonnet|Opus|Fast|ctrl\+c to stop)\b|~\/|Tip:)/u;
const CURSOR_ACTIVITY = /^[\u2800-\u28FF\u2500-\u257F].*(?:Reading|Running)\b/u;
const LIST_ITEM = /^\s*(?:[-*+] |\d+[.)] )/u;
const CJK_END = /[\p{Script=Han}\p{Script=Hiragana}\p{Script=Katakana}ー々〆ヵヶ]$/u;
const CJK_START = /^[\p{Script=Han}\p{Script=Hiragana}\p{Script=Katakana}ー々〆ヵヶ]/u;
const NO_LEADING_SPACE = /^[,.;:!?%)\]}、。，．！？：；）］｝」』】]/u;

export function extractFinalResponse(request: FinalResponseRequest): OperationResult<FinalResponse> {
  try {
    validateRequest(request);
    const lines = answerLines(request);
    const selected = trimKnownTailChrome(
      request.agent,
      selectFinalLines(request.agent, lines, request.requirePiFooter === true, request.requireOpenCodeChrome === true)
    );
    const trimmed = trimBlankLines(selected);
    if (trimmed.length === 0) throw new HerdrMathError("conclusion_boundary_failed");
    const text = normalizeResponseLines(trimmed);
    if (text === "") throw new HerdrMathError("conclusion_boundary_failed");
    return success(
      Object.freeze({
        text,
        sourceStartOffset: trimmed[0]?.startOffset ?? request.answerStartOffset,
        sourceEndOffset: trimmed.at(-1)?.endOffset ?? request.answerStartOffset + request.answer.length
      })
    );
  } catch (error) {
    return failure(
      serializeError(error instanceof HerdrMathError ? error : new HerdrMathError("conclusion_boundary_failed"))
    );
  }
}

function selectFinalLines(
  agent: SupportedAgent,
  lines: readonly ResponseLine[],
  requirePiFooter: boolean,
  requireOpenCodeChrome = false
): ResponseLine[] {
  if (agent === "claude") return selectClaude(lines);
  if (agent === "codex") return selectCodex(lines);
  if (agent === "cursor") return selectCursor(lines);
  if (agent === "pi") return selectPi(lines, requirePiFooter);
  return selectOpenCode(lines, requireOpenCodeChrome);
}

function selectClaude(lines: readonly ResponseLine[]): ResponseLine[] {
  const footer = lines.findIndex(({ text }) => /^\s*[✻✽✢]\s+\S/u.test(text));
  if (footer === -1) return [...lines];
  const beforeFooter = lines.slice(0, footer);
  const lastActivity = findLastIndex(beforeFooter, ({ text }) => /^\s*(?:⏺|●)\s+\S/u.test(text));
  if (lastActivity === -1) return beforeFooter;
  const nextBlank = beforeFooter.findIndex(({ text }, index) => index > lastActivity && text.trim() === "");
  return beforeFooter.slice(nextBlank === -1 ? lastActivity + 1 : nextBlank + 1);
}

function selectCodex(lines: readonly ResponseLine[]): ResponseLine[] {
  const separators = lines.flatMap((line, index) => (SEPARATOR.test(line.text) ? [index] : []));
  if (separators.length === 0) return [...lines];
  if (separators.length === 1) {
    const start = separators[0];
    if (start === undefined) throw new HerdrMathError("conclusion_boundary_failed");
    const block = trimBlankLines(lines.slice(start + 1, findCodexTailStart(lines, start + 1)));
    if (block.some(({ text }) => text.trim() !== "")) return stripCodexAnswerMarker(block);
    throw new HerdrMathError("conclusion_boundary_failed");
  }
  for (let index = separators.length - 2; index >= 0; index -= 1) {
    const start = separators[index];
    const end = separators[index + 1];
    if (start === undefined || end === undefined) continue;
    const block = trimBlankLines(lines.slice(start + 1, end));
    if (block.some(({ text }) => text.trim() !== "")) return stripCodexAnswerMarker(block);
  }
  throw new HerdrMathError("conclusion_boundary_failed");
}

function findCodexTailStart(lines: readonly ResponseLine[], fromIndex: number): number {
  for (let index = fromIndex; index < lines.length; index += 1) {
    const line = lines[index];
    if (line === undefined) break;
    if (SEPARATOR.test(line.text) || CODEX_WORKED_FOOTER.test(line.text) || CODEX_PROMPT.test(line.text)) {
      return index;
    }
  }
  return lines.length;
}

function selectCursor(lines: readonly ResponseLine[]): ResponseLine[] {
  const tailIndex = lines.findIndex(({ text }) => isCursorTailLine(text));
  const scoped = tailIndex === -1 ? lines : lines.slice(0, tailIndex);
  const lastTool = findLastIndex(scoped, ({ text }) => isCursorToolLine(text));
  if (lastTool === -1) return stripCursorAnswerMarker([...scoped]);
  let start = lastTool + 1;
  while (start < scoped.length && scoped[start]?.text.trim() === "") start += 1;
  return stripCursorAnswerMarker(trimBlankLines(scoped.slice(start)));
}

function isCursorToolLine(text: string): boolean {
  const trimmed = text.trim();
  return CURSOR_SHELL.test(trimmed) || CURSOR_TOOL.test(trimmed);
}

function isCursorTailLine(text: string): boolean {
  const trimmed = text.trim();
  if (trimmed === "") return false;
  return (
    CURSOR_STATUS_BAR.test(trimmed) ||
    CURSOR_FOOTER.test(trimmed) ||
    CURSOR_ACTIVITY.test(trimmed) ||
    /^\s*Run Every(?:thing|…)/u.test(trimmed)
  );
}

function stripCursorAnswerMarker(lines: readonly ResponseLine[]): ResponseLine[] {
  return lines.map((line) => ({
    ...line,
    text: line.text.replace(/^\s*•\s+/u, "")
  }));
}

function selectPi(lines: readonly ResponseLine[], requireFooter: boolean): ResponseLine[] {
  const lastReasoning = findLastIndex(lines, (line) => {
    if (line.nonWhitespaceCharacters === 0) return false;
    return line.italicCharacters / line.nonWhitespaceCharacters >= 0.8;
  });
  const lastTool = findLastIndex(lines, (line) => line.nonWhitespaceCharacters > 0 && line.hasBackground);
  const lastChrome = Math.max(lastReasoning, lastTool);
  if (lastChrome === -1) {
    if (requireFooter) throw new HerdrMathError("conclusion_boundary_failed");
    if (lines.some(({ text }) => SEPARATOR.test(text))) throw new HerdrMathError("conclusion_boundary_failed");
    return [...lines];
  }
  const footer = lines.findIndex(({ text }, index) => index > lastChrome && SEPARATOR.test(text));
  if (requireFooter && footer === -1) throw new HerdrMathError("conclusion_boundary_failed");
  return lines.slice(lastChrome + 1, footer === -1 ? lines.length : footer);
}

function selectOpenCode(lines: readonly ResponseLine[], requireChrome: boolean): ResponseLine[] {
  const summary = findLastIndex(lines, ({ text }) => /^\s*▣\s+\S/u.test(text));
  const hasToolChrome = lines.some(({ text }) => /^\s*→\s*/u.test(text));
  if (summary === -1) {
    if (hasToolChrome || requireChrome) throw new HerdrMathError("conclusion_boundary_failed");
    return stripOpenCodeAnswerMarker([...lines]);
  }
  const beforeSummary = lines.slice(0, summary);
  const lastTool = findLastIndex(beforeSummary, ({ text }) => /^\s*→\s+\S/u.test(text));
  if (lastTool !== -1) return stripOpenCodeAnswerMarker(beforeSummary.slice(lastTool + 1));
  if (requireChrome) throw new HerdrMathError("conclusion_boundary_failed");
  const lastPromptChrome = findLastIndex(beforeSummary, ({ text }) => /^\s*[┃╹]/u.test(text));
  return stripOpenCodeAnswerMarker(beforeSummary.slice(lastPromptChrome + 1));
}

function normalizeResponseLines(source: readonly ResponseLine[]): string {
  const lines = removeCommonIndent(source.map(({ text }) => text));
  const blocks: string[] = [];
  let prose: string[] = [];
  let list: string[] = [];
  let display: string[] = [];
  let code: string[] = [];
  let insideDisplay = false;
  let codeFence: string | undefined;

  const flushProse = (): void => {
    if (prose.length > 0) blocks.push(joinSoftWrappedLines(prose));
    prose = [];
  };
  const flushList = (): void => {
    if (list.length > 0) blocks.push(list.join("\n"));
    list = [];
  };
  const flushDisplay = (): void => {
    if (display.length > 0) blocks.push(display.join("\n"));
    display = [];
  };
  const flushCode = (): void => {
    if (code.length > 0) blocks.push(code.join("\n"));
    code = [];
  };

  for (const rawLine of lines) {
    const line = rawLine.trimEnd();
    const fence = line.trim().match(/^(`{3,}|~{3,})/u)?.[1];
    if (codeFence !== undefined) {
      code.push(line.trim());
      if (fence !== undefined && fence[0] === codeFence[0] && fence.length >= codeFence.length) {
        codeFence = undefined;
        flushCode();
      }
      continue;
    }
    if (fence !== undefined) {
      flushProse();
      flushList();
      flushDisplay();
      codeFence = fence;
      code.push(line.trim());
      continue;
    }
    if (insideDisplay) {
      display.push(line.trim());
      if (countDoubleDollarRuns(line) % 2 === 1) {
        insideDisplay = false;
        flushDisplay();
      }
      continue;
    }
    if (line.trim() === "") {
      flushProse();
      flushList();
      continue;
    }
    const doubleDollarRuns = countDoubleDollarRuns(line);
    if (doubleDollarRuns > 0) {
      flushProse();
      flushList();
      display.push(line.trim());
      insideDisplay = doubleDollarRuns % 2 === 1;
      if (!insideDisplay) flushDisplay();
      continue;
    }
    if (LIST_ITEM.test(line)) {
      flushProse();
      list.push(line.trim());
      continue;
    }
    flushList();
    prose.push(line.trim());
  }
  flushProse();
  flushList();
  flushDisplay();
  flushCode();
  return blocks.filter(Boolean).join("\n\n").trim();
}

function joinSoftWrappedLines(lines: readonly string[]): string {
  let result = "";
  for (const line of lines) {
    const value = line.trim();
    if (result === "") {
      result = value;
      continue;
    }
    result += softWrapSeparator(result, value) + value;
  }
  return result;
}

function softWrapSeparator(previous: string, next: string): string {
  if (CJK_END.test(previous) && CJK_START.test(next)) return "";
  if (NO_LEADING_SPACE.test(next)) return "";
  return " ";
}

function countDoubleDollarRuns(line: string): number {
  let count = 0;
  for (let index = 0; index < line.length - 1; index += 1) {
    if (line[index] !== "$" || line[index + 1] !== "$" || isEscaped(line, index)) continue;
    count += 1;
    index += 1;
  }
  return count;
}

function isEscaped(source: string, offset: number): boolean {
  let backslashes = 0;
  for (let index = offset - 1; index >= 0 && source[index] === "\\"; index -= 1) backslashes += 1;
  return backslashes % 2 === 1;
}

function answerLines(request: FinalResponseRequest): ResponseLine[] {
  const result: ResponseLine[] = [];
  let localOffset = 0;
  const values = request.answer.split("\n");
  for (let index = 0; index < values.length; index += 1) {
    if (index === values.length - 1 && values[index] === "" && request.answer.endsWith("\n")) break;
    const text = values[index] ?? "";
    const startOffset = request.answerStartOffset + localOffset;
    const endOffset = startOffset + text.length;
    const source = findSourceLine(request.snapshot.lines, startOffset, endOffset);
    result.push({ ...source, text, startOffset, endOffset, sourceIndex: index });
    localOffset += text.length + 1;
  }
  return result;
}

function findSourceLine(lines: readonly StyledTerminalLine[], start: number, end: number): StyledTerminalLine {
  const line = lines.find(
    (candidate) =>
      (candidate.startOffset <= start && candidate.endOffset >= end) ||
      (start === end && candidate.startOffset === start && candidate.endOffset === end)
  );
  if (line === undefined) throw new HerdrMathError("conclusion_boundary_failed");
  return line;
}

function stripCodexAnswerMarker(lines: ResponseLine[]): ResponseLine[] {
  const first = lines.findIndex(({ text }) => text.trim() !== "");
  if (first === -1) return lines;
  const line = lines[first];
  if (line === undefined || !/^\s*•\s+/u.test(line.text)) return lines;
  const text = line.text.replace(/^\s*•\s+/u, "");
  return lines.map((value, index) => (index === first ? { ...value, text } : value));
}

function stripOpenCodeAnswerMarker(lines: ResponseLine[]): ResponseLine[] {
  const first = lines.findIndex(({ text }) => text.trim() !== "");
  if (first === -1) return lines;
  const line = lines[first];
  if (line === undefined || !/^\s*┃\s*answer:\s*/iu.test(line.text)) return lines;
  const text = line.text.replace(/^\s*┃\s*answer:\s*/iu, "");
  return lines.map((value, index) => (index === first ? { ...value, text } : value));
}

function trimKnownTailChrome(agent: SupportedAgent, lines: readonly ResponseLine[]): ResponseLine[] {
  const pattern =
    agent === "claude"
      ? /^\s*❯(?:\s|$)/u
      : agent === "codex"
        ? /^\s*›(?:\s|$)/u
        : agent === "pi"
          ? /^\s*(?:Current prompt\s*>|└)/u
          : agent === "cursor"
            ? /^(?:→\s*Add a follow-up|\d+\s+task\s*$)/u
            : /^\s*┃\s*prompt\s*:/iu;
  let start = 0;
  if (agent === "claude" || agent === "codex") {
    while (start < lines.length && pattern.test(lines[start]?.text ?? "")) {
      start += 1;
    }
  }
  const scoped = lines.slice(start);
  const end = scoped.findIndex(({ text }) => pattern.test(text));
  return end === -1 ? [...scoped] : scoped.slice(0, end);
}

function removeCommonIndent(lines: readonly string[]): string[] {
  const indents = lines.filter((line) => line.trim() !== "").map((line) => line.match(/^ */u)?.[0].length ?? 0);
  const common = indents.length === 0 ? 0 : Math.min(...indents);
  return lines.map((line) => line.slice(Math.min(common, line.length)));
}

function trimBlankLines(lines: readonly ResponseLine[]): ResponseLine[] {
  let start = 0;
  let end = lines.length;
  while (start < end && lines[start]?.text.trim() === "") start += 1;
  while (end > start && lines[end - 1]?.text.trim() === "") end -= 1;
  return lines.slice(start, end);
}

function findLastIndex<T>(values: readonly T[], predicate: (value: T, index: number) => boolean): number {
  for (let index = values.length - 1; index >= 0; index -= 1) {
    const value = values[index];
    if (value !== undefined && predicate(value, index)) return index;
  }
  return -1;
}

function validateRequest(request: FinalResponseRequest): void {
  const endOffset = request.answerStartOffset + request.answer.length;
  if (
    !Number.isSafeInteger(request.answerStartOffset) ||
    request.answerStartOffset < 0 ||
    (request.requirePiFooter === true && request.agent !== "pi") ||
    (request.requireOpenCodeChrome === true && request.agent !== "opencode") ||
    endOffset > request.snapshot.text.length ||
    request.snapshot.text.slice(request.answerStartOffset, endOffset) !== request.answer
  ) {
    throw new HerdrMathError("conclusion_boundary_failed");
  }
}
