export type PrototypeBoundaryStrategy = "exact_prefix" | "stable_prefix" | "sliding_window" | "contextual_anchor";

export interface PrototypeAnswerDelta {
  text: string;
  strategy: PrototypeBoundaryStrategy;
}

export interface PrototypePaneRead {
  text: string;
  truncated: boolean;
}

export interface PrototypeBoundaryLimits {
  maxInputCharacters: number;
  tailLines: number;
  minimumAnchorCharacters: number;
  maxAnchorOccurrences: number;
  maxContextCharacters: number;
}

export const PROTOTYPE_BOUNDARY_LIMITS: Readonly<PrototypeBoundaryLimits> = Object.freeze({
  maxInputCharacters: 1024 * 1024,
  tailLines: 20,
  minimumAnchorCharacters: 24,
  maxAnchorOccurrences: 256,
  maxContextCharacters: 2048
});

export function computePrototypeAnswerDelta(
  before: string,
  after: string,
  overrides: Partial<PrototypeBoundaryLimits> = {}
): PrototypeAnswerDelta | null {
  const limits = resolveLimits(overrides);
  assertInputLimit(before, limits.maxInputCharacters);
  assertInputLimit(after, limits.maxInputCharacters);

  if (after.startsWith(before)) {
    return { text: after.slice(before.length), strategy: "exact_prefix" };
  }

  const prefixLength = longestCommonPrefix(before, after);
  const minimumStable = Math.min(before.length, Math.max(80, before.length * 0.8));
  if (prefixLength >= minimumStable) {
    return { text: after.slice(prefixLength), strategy: "stable_prefix" };
  }

  const overlapLength = longestSuffixPrefixOverlap(before, after);
  const minimumOverlap = Math.min(before.length, 256);
  if (overlapLength >= Math.max(80, minimumOverlap)) {
    return { text: after.slice(overlapLength), strategy: "sliding_window" };
  }

  const anchorBoundary = findTailAnchorBoundary(before, after, limits);
  if (anchorBoundary !== null) {
    return { text: after.slice(anchorBoundary), strategy: "contextual_anchor" };
  }

  return null;
}

export function isPrototypeReadTruncated(read: PrototypePaneRead, lineLimit: number): boolean {
  if (!Number.isSafeInteger(lineLimit) || lineLimit <= 0) {
    throw new RangeError("Line limit must be a positive safe integer.");
  }
  if (read.truncated) {
    return true;
  }
  if (read.text.length === 0) {
    return false;
  }

  let lines = 1;
  for (const character of read.text) {
    if (character === "\n") {
      lines += 1;
      if (lines >= lineLimit) {
        return true;
      }
    }
  }
  return lines >= lineLimit;
}

function resolveLimits(overrides: Partial<PrototypeBoundaryLimits>): PrototypeBoundaryLimits {
  const limits = { ...PROTOTYPE_BOUNDARY_LIMITS, ...overrides };
  for (const [name, value] of Object.entries(limits)) {
    if (!Number.isSafeInteger(value) || value <= 0) {
      throw new RangeError(`Prototype boundary limit ${name} must be a positive safe integer.`);
    }
  }
  return limits;
}

function assertInputLimit(input: string, limit: number): void {
  if (input.length > limit) {
    throw new RangeError("Prototype boundary input limit exceeded.");
  }
}

function longestCommonPrefix(left: string, right: string): number {
  const limit = Math.min(left.length, right.length);
  let index = 0;
  while (index < limit && left[index] === right[index]) {
    index += 1;
  }
  return index;
}

function findTailAnchorBoundary(before: string, after: string, limits: PrototypeBoundaryLimits): number | null {
  const lines = before.split("\n");
  const start = Math.max(0, lines.length - limits.tailLines);
  for (let index = lines.length - 1; index >= start; index -= 1) {
    const anchor = lines[index];
    if (anchor === undefined || anchor.trim().length < limits.minimumAnchorCharacters) {
      continue;
    }
    const position = bestContextualAnchorPosition(before, after, anchor, index, lines, limits);
    if (position !== null) {
      return position + anchor.length;
    }
  }
  return null;
}

function bestContextualAnchorPosition(
  before: string,
  after: string,
  anchor: string,
  lineIndex: number,
  beforeLines: string[],
  limits: PrototypeBoundaryLimits
): number | null {
  const beforeAnchorEnd = beforeLines.slice(0, lineIndex + 1).join("\n").length;
  const beforeContext = before.slice(0, beforeAnchorEnd);
  let cursor = 0;
  let bestPosition: number | null = null;
  let bestScore = -1;
  let occurrenceCount = 0;

  while (occurrenceCount < limits.maxAnchorOccurrences) {
    const position = after.indexOf(anchor, cursor);
    if (position === -1) {
      break;
    }
    const afterContext = after.slice(0, position + anchor.length);
    const score = commonSuffixLength(beforeContext, afterContext, limits.maxContextCharacters);
    if (score > bestScore) {
      bestScore = score;
      bestPosition = position;
    }
    cursor = position + Math.max(1, anchor.length);
    occurrenceCount += 1;
  }
  return bestPosition;
}

function commonSuffixLength(left: string, right: string, limit: number): number {
  const maxLength = Math.min(left.length, right.length, limit);
  let length = 0;
  while (length < maxLength && left[left.length - 1 - length] === right[right.length - 1 - length]) {
    length += 1;
  }
  return length;
}

function longestSuffixPrefixOverlap(before: string, after: string): number {
  if (before.length === 0 || after.length === 0) {
    return 0;
  }

  const prefix = buildPrefixTable(after);
  let matched = 0;
  for (let index = 0; index < before.length; index += 1) {
    const character = before[index];
    while (matched > 0 && character !== after[matched]) {
      matched = prefix[matched - 1] ?? 0;
    }
    if (character === after[matched]) {
      matched += 1;
    }
    if (matched === after.length) {
      if (index === before.length - 1) {
        return matched;
      }
      matched = prefix[matched - 1] ?? 0;
    }
  }
  return matched;
}

function buildPrefixTable(pattern: string): Uint32Array {
  const prefix = new Uint32Array(pattern.length);
  for (let index = 1; index < pattern.length; index += 1) {
    let matched = prefix[index - 1] ?? 0;
    while (matched > 0 && pattern[index] !== pattern[matched]) {
      matched = prefix[matched - 1] ?? 0;
    }
    if (pattern[index] === pattern[matched]) {
      matched += 1;
    }
    prefix[index] = matched;
  }
  return prefix;
}
