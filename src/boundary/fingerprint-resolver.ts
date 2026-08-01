import { Buffer } from "node:buffer";

import { failure, success, type BoundaryProof, type BoundaryResult, type OperationResult } from "../core/contracts.js";
import { HerdrMathError, serializeError } from "../core/errors.js";
import { POLICY_LIMITS } from "../core/limits.js";
import { scanLatex } from "../scanner/scan-latex.js";
import {
  assertFingerprintSecret,
  fingerprintDigest,
  fingerprintDigestsEqual,
  formulaFingerprintDigest
} from "./fingerprint-digest.js";
import {
  FINGERPRINT_SCHEMA_LIMITS,
  FINGERPRINT_SCHEMA_VERSION,
  type FingerprintStateV1,
  type SuffixWindowV1,
  type TailAnchorV1,
  isFingerprintDigest
} from "./fingerprint-schema.js";

export interface FingerprintResolveOptions {
  readTruncated?: boolean;
}

interface LineRange {
  start: number;
  end: number;
}

export function resolveAnswerFromFingerprint(
  state: FingerprintStateV1,
  current: string,
  secret: Uint8Array,
  options: FingerprintResolveOptions = {}
): OperationResult<BoundaryResult> {
  try {
    assertFingerprintSecret(secret);
    assertCurrentInput(current);
    assertUsableFingerprint(state);
    const currentDigest = fingerprintDigest("current", current, secret);
    const readTruncated = options.readTruncated ?? false;

    const exact = resolveExact(state, current, secret);
    if (exact !== null) {
      return success(result(current, exact.startOffset, exact.proof, currentDigest, readTruncated));
    }

    const middle = resolveMiddleInsertion(state, current, secret, readTruncated);
    if (middle.status === "resolved") {
      return success(result(current, middle.startOffset, middle.proof, currentDigest, readTruncated, middle.endOffset));
    }
    const replacement = resolveMiddleReplacement(state, current, secret);
    if (replacement.status === "resolved") {
      return success(
        result(current, replacement.startOffset, replacement.proof, currentDigest, readTruncated, replacement.endOffset)
      );
    }
    const anchoredPrefix = resolveAnchoredPrefixReplacement(state, current, secret);
    if (anchoredPrefix.status === "resolved") {
      return success(
        result(
          current,
          anchoredPrefix.startOffset,
          anchoredPrefix.proof,
          currentDigest,
          readTruncated,
          anchoredPrefix.endOffset
        )
      );
    }
    const stable = resolveStablePrefix(state, current, secret);
    if (stable !== null) {
      return success(result(current, stable.startOffset, stable.proof, currentDigest, readTruncated));
    }

    if (middle.status === "conflict" || replacement.status === "conflict") {
      return failure(serializeError(new HerdrMathError(readTruncated ? "answer_truncated" : "boundary_failed")));
    }

    const sliding = resolveSlidingWindow(state, current, secret);
    if (sliding !== null) {
      return success(result(current, sliding.startOffset, sliding.proof, currentDigest, readTruncated));
    }
    if (anchoredPrefix.status === "conflict") {
      return failure(serializeError(new HerdrMathError(readTruncated ? "answer_truncated" : "boundary_failed")));
    }

    const contextual = resolveContextualAnchor(state, current, secret);
    if (contextual !== null) {
      return success(result(current, contextual.startOffset, contextual.proof, currentDigest, readTruncated));
    }

    return failure(serializeError(new HerdrMathError(readTruncated ? "answer_truncated" : "boundary_failed")));
  } catch (error: unknown) {
    return failure(serializeError(error));
  }
}

function result(
  current: string,
  startOffset: number,
  proof: BoundaryProof,
  currentDigest: string,
  recoveredTruncation: boolean,
  endOffset = current.length
): BoundaryResult {
  return {
    answer: current.slice(startOffset, endOffset),
    startOffset,
    strategy: proof.kind,
    recoveredTruncation,
    currentDigest,
    proof
  };
}

type MiddleInsertionResult =
  | { status: "none" | "conflict" }
  | {
      status: "resolved";
      startOffset: number;
      endOffset: number;
      proof: Extract<BoundaryProof, { kind: "middle_insertion" }>;
    };

type MiddleReplacementResult =
  | { status: "none" | "conflict" }
  | {
      status: "resolved";
      startOffset: number;
      endOffset: number;
      proof: Extract<BoundaryProof, { kind: "middle_replacement" }>;
    };

type AnchoredPrefixReplacementResult =
  | { status: "none" | "conflict" }
  | {
      status: "resolved";
      startOffset: 0;
      endOffset: number;
      proof: Extract<BoundaryProof, { kind: "anchored_prefix_replacement" }>;
    };

function resolveMiddleInsertion(
  state: FingerprintStateV1,
  current: string,
  secret: Uint8Array,
  readTruncated: boolean
): MiddleInsertionResult {
  const anchors = state.baseline.tail_anchors
    .filter((anchor): anchor is TailAnchorV1 & { end_offset: number } => anchor.end_offset !== undefined)
    .sort((left, right) => left.end_offset - right.end_offset);
  const lines = collectRecentLines(current, POLICY_LIMITS.boundaryCandidates);
  let candidatesExamined = 0;
  let conflict = false;

  for (let index = 0; index < anchors.length - 1; index += 1) {
    const before = anchors[index];
    const after = anchors[index + 1];
    if (before?.next_anchor_gap_digest === undefined || after === undefined) continue;
    if (fingerprintDigestsEqual(before.line_digest, after.line_digest)) {
      conflict = true;
      continue;
    }

    const beforeMatches = matchingLines(before, lines, current, secret, "preceding", () => {
      candidatesExamined += 1;
      return candidatesExamined <= POLICY_LIMITS.anchorOccurrences;
    });
    const afterMatches = matchingInsertionAfterLines(after, lines, current, secret, () => {
      candidatesExamined += 1;
      return candidatesExamined <= POLICY_LIMITS.anchorOccurrences;
    });
    if (candidatesExamined > POLICY_LIMITS.anchorOccurrences) return { status: "conflict" };
    if (beforeMatches.length !== 1 || afterMatches.length !== 1) {
      const bothSidesPresent = beforeMatches.length > 0 && afterMatches.length > 0;
      if (bothSidesPresent || (!readTruncated && (beforeMatches.length > 0 || afterMatches.length > 0))) {
        conflict = true;
      }
      continue;
    }

    const beforeMatch = beforeMatches[0];
    const afterMatch = afterMatches[0];
    if (beforeMatch === undefined || afterMatch === undefined || afterMatch.start < beforeMatch.end) {
      conflict = true;
      continue;
    }
    const baselineAfterStart = after.end_offset - after.line_characters;
    const baselineGapCharacters = baselineAfterStart - before.end_offset;
    if (baselineGapCharacters < 0) throw new HerdrMathError("state_corrupt");
    const currentGap = current.slice(beforeMatch.end, afterMatch.start);
    if (currentGap.length <= baselineGapCharacters) {
      const unchangedGap = currentGap.length === baselineGapCharacters && gapMatches(currentGap, before, secret);
      if (!unchangedGap) conflict = true;
      continue;
    }

    const preservedGap = currentGap.slice(currentGap.length - baselineGapCharacters);
    if (!gapMatches(preservedGap, before, secret)) {
      conflict = true;
      continue;
    }
    const insertedCharacters = currentGap.length - baselineGapCharacters;
    return {
      status: "resolved",
      startOffset: beforeMatch.end,
      endOffset: beforeMatch.end + insertedCharacters,
      proof: {
        kind: "middle_insertion",
        beforeMatchEndOffset: beforeMatch.end,
        afterMatchStartOffset: afterMatch.start,
        baselineGapCharacters,
        insertedCharacters
      }
    };
  }
  return { status: conflict ? "conflict" : "none" };
}

function resolveMiddleReplacement(
  state: FingerprintStateV1,
  current: string,
  secret: Uint8Array
): MiddleReplacementResult {
  const anchors = state.baseline.tail_anchors
    .filter((anchor): anchor is TailAnchorV1 & { end_offset: number } => anchor.end_offset !== undefined)
    .sort((left, right) => left.end_offset - right.end_offset);
  const lines = collectRecentLines(current, POLICY_LIMITS.boundaryCandidates);
  let candidatesExamined = 0;
  let conflict = false;
  let fallback: Extract<MiddleReplacementResult, { status: "resolved" }> | undefined;

  for (let index = 0; index < anchors.length - 1; index += 1) {
    const before = anchors[index];
    const after = anchors[index + 1];
    if (
      before?.next_anchor_gap_digest === undefined ||
      before.next_anchor_gap_formula_digests === undefined ||
      after === undefined
    ) {
      continue;
    }
    if (fingerprintDigestsEqual(before.line_digest, after.line_digest)) {
      conflict = true;
      continue;
    }

    const beforeMatches = matchingLines(before, lines, current, secret, "preceding", () => {
      candidatesExamined += 1;
      return candidatesExamined <= POLICY_LIMITS.anchorOccurrences;
    });
    const afterMatches = matchingInsertionAfterLines(after, lines, current, secret, () => {
      candidatesExamined += 1;
      return candidatesExamined <= POLICY_LIMITS.anchorOccurrences;
    });
    if (candidatesExamined > POLICY_LIMITS.anchorOccurrences) return { status: "conflict" };
    if (beforeMatches.length !== 1 || afterMatches.length !== 1) {
      const bothSidesPresent = beforeMatches.length > 0 && afterMatches.length > 0;
      if (bothSidesPresent) conflict = true;
      continue;
    }

    const beforeMatch = beforeMatches[0];
    const afterMatch = afterMatches[0];
    if (beforeMatch === undefined || afterMatch === undefined || afterMatch.start < beforeMatch.end) {
      conflict = true;
      continue;
    }
    const baselineAfterStart = after.end_offset - after.line_characters;
    const baselineGapCharacters = baselineAfterStart - before.end_offset;
    if (baselineGapCharacters < 0) throw new HerdrMathError("state_corrupt");
    const currentGap = current.slice(beforeMatch.end, afterMatch.start);
    if (gapMatches(currentGap, before, secret)) continue;

    let formulas: ReturnType<typeof scanLatex>;
    try {
      formulas = scanLatex(currentGap);
    } catch {
      conflict = true;
      continue;
    }
    const baselineFormulaDigests = before.next_anchor_gap_formula_digests;
    const hasNewFormula = formulas.some((formula) => {
      const digest = formulaFingerprintDigest(formula, secret);
      return !baselineFormulaDigests.some((baselineDigest) => fingerprintDigestsEqual(digest, baselineDigest));
    });
    const resolved: Extract<MiddleReplacementResult, { status: "resolved" }> = {
      status: "resolved",
      startOffset: beforeMatch.end,
      endOffset: afterMatch.start,
      proof: {
        kind: "middle_replacement",
        beforeMatchEndOffset: beforeMatch.end,
        afterMatchStartOffset: afterMatch.start,
        baselineGapCharacters,
        replacementCharacters: currentGap.length,
        baselineFormulaDigests
      }
    };
    if (hasNewFormula) return resolved;
    fallback = resolved;
  }
  if (fallback !== undefined) return fallback;
  return { status: conflict ? "conflict" : "none" };
}

function resolveAnchoredPrefixReplacement(
  state: FingerprintStateV1,
  current: string,
  secret: Uint8Array
): AnchoredPrefixReplacementResult {
  if (state.agent !== "opencode" || countLines(current) !== state.baseline.line_count) return { status: "none" };
  const lines = collectRecentLines(current, POLICY_LIMITS.paneReadLines);
  const candidates: Array<{
    match: LineRange;
    growth: number;
    lineIndexFromEnd: number;
    baselineFormulaDigests: string[];
  }> = [];
  let candidatesExamined = 0;
  let conflictingAnchor = false;

  for (const anchor of state.baseline.tail_anchors) {
    if (
      anchor.end_offset === undefined ||
      anchor.prefix_formula_digests === undefined ||
      anchor.forward_context_characters === undefined ||
      anchor.forward_context_digest === undefined ||
      anchor.context_characters === 0 ||
      anchor.forward_context_characters === 0
    ) {
      continue;
    }
    const rawMatches: LineRange[] = [];
    for (const line of lines) {
      if (line.end - line.start !== anchor.line_characters) continue;
      candidatesExamined += 1;
      if (candidatesExamined > POLICY_LIMITS.anchorOccurrences) return { status: "conflict" };
      if (
        fingerprintDigestsEqual(
          fingerprintDigest("anchor-line", current.slice(line.start, line.end), secret),
          anchor.line_digest
        )
      ) {
        rawMatches.push(line);
      }
      if (rawMatches.length > 1) break;
    }
    if (rawMatches.length === 0) continue;
    if (rawMatches.length > 1) {
      conflictingAnchor = true;
      continue;
    }
    const match = rawMatches[0];
    if (match === undefined) continue;
    const lineIndexFromEnd = lines.length - 1 - lines.indexOf(match);
    if (
      lineIndexFromEnd !== anchor.line_index_from_end ||
      !anchorContextMatches(anchor, match, current, secret) ||
      !anchorForwardContextMatches(anchor, match, current, secret)
    ) {
      conflictingAnchor = true;
      continue;
    }
    const baselineMatchStart = anchor.end_offset - anchor.line_characters;
    const growth = match.start - baselineMatchStart;
    if (growth <= 0) continue;
    candidates.push({
      match,
      growth,
      lineIndexFromEnd: anchor.line_index_from_end,
      baselineFormulaDigests: anchor.prefix_formula_digests
    });
  }
  if (candidates.length === 0) return { status: conflictingAnchor ? "conflict" : "none" };
  const growth = candidates[0]?.growth;
  if (growth === undefined || candidates.some((candidate) => candidate.growth !== growth))
    return { status: "conflict" };
  const candidate = [...candidates].sort((left, right) => left.match.start - right.match.start)[0];
  if (candidate === undefined) return { status: "none" };

  let formulas: ReturnType<typeof scanLatex>;
  try {
    formulas = scanLatex(current.slice(0, candidate.match.start));
  } catch {
    return { status: "conflict" };
  }
  const hasNewFormula = formulas.some((formula) => {
    const digest = formulaFingerprintDigest(formula, secret);
    return !candidate.baselineFormulaDigests.some((baselineDigest) => fingerprintDigestsEqual(digest, baselineDigest));
  });
  if (!hasNewFormula) return { status: "none" };
  return {
    status: "resolved",
    startOffset: 0,
    endOffset: candidate.match.start,
    proof: {
      kind: "anchored_prefix_replacement",
      anchorMatchStartOffset: candidate.match.start,
      lineIndexFromEnd: candidate.lineIndexFromEnd,
      replacementGrowthCharacters: growth,
      baselineFormulaDigests: candidate.baselineFormulaDigests
    }
  };
}

type AnchorMatchContext = "line" | "preceding" | "following";

function matchingInsertionAfterLines(
  anchor: TailAnchorV1,
  lines: LineRange[],
  current: string,
  secret: Uint8Array,
  consumeCandidate: () => boolean
): LineRange[] {
  const lineMatches = matchingLines(anchor, lines, current, secret, "line", consumeCandidate);
  if (lineMatches.length <= 1 || anchor.forward_context_characters === undefined) return lineMatches;
  return matchingLines(anchor, lines, current, secret, "following", consumeCandidate);
}

function matchingLines(
  anchor: TailAnchorV1,
  lines: LineRange[],
  current: string,
  secret: Uint8Array,
  context: AnchorMatchContext,
  consumeCandidate: () => boolean
): LineRange[] {
  const matches: LineRange[] = [];
  for (const line of lines) {
    if (line.end - line.start !== anchor.line_characters) continue;
    if (!consumeCandidate()) break;
    const lineMatched = fingerprintDigestsEqual(
      fingerprintDigest("anchor-line", current.slice(line.start, line.end), secret),
      anchor.line_digest
    );
    const matched =
      lineMatched &&
      (context === "line" ||
        (context === "preceding" && anchorContextMatches(anchor, line, current, secret)) ||
        (context === "following" && anchorForwardContextMatches(anchor, line, current, secret)));
    if (!matched) continue;
    matches.push(line);
    if (matches.length > 1) break;
  }
  return matches;
}

function anchorContextMatches(anchor: TailAnchorV1, line: LineRange, current: string, secret: Uint8Array): boolean {
  if (line.start < anchor.context_characters) return false;
  const context = current.slice(line.start - anchor.context_characters, line.start);
  return fingerprintDigestsEqual(fingerprintDigest("anchor-context", context, secret), anchor.context_digest);
}

function anchorForwardContextMatches(
  anchor: TailAnchorV1,
  line: LineRange,
  current: string,
  secret: Uint8Array
): boolean {
  if (anchor.forward_context_characters === undefined || anchor.forward_context_digest === undefined) return false;
  const end = line.end + anchor.forward_context_characters;
  if (end > current.length) return false;
  return fingerprintDigestsEqual(
    fingerprintDigest("anchor-forward-context", current.slice(line.end, end), secret),
    anchor.forward_context_digest
  );
}

function gapMatches(gap: string, before: TailAnchorV1, secret: Uint8Array): boolean {
  if (before.next_anchor_gap_digest === undefined) return false;
  return fingerprintDigestsEqual(fingerprintDigest("anchor-gap", gap, secret), before.next_anchor_gap_digest);
}

function resolveExact(state: FingerprintStateV1, current: string, secret: Uint8Array) {
  const length = state.baseline.character_count;
  if (current.length < length) return null;
  const candidate = fingerprintDigest("baseline", current.slice(0, length), secret);
  return fingerprintDigestsEqual(candidate, state.baseline.digest)
    ? { startOffset: length, proof: { kind: "exact_prefix", baselineCharacters: length } as const }
    : null;
}

function resolveStablePrefix(state: FingerprintStateV1, current: string, secret: Uint8Array) {
  const minimum = Math.min(state.baseline.character_count, Math.max(80, state.baseline.character_count * 0.8));
  const checkpoints = [...state.baseline.prefix_checkpoints].sort((left, right) => right.end_offset - left.end_offset);
  for (const checkpoint of checkpoints) {
    if (checkpoint.end_offset < minimum || current.length < checkpoint.end_offset) continue;
    const candidate = fingerprintDigest("prefix", current.slice(0, checkpoint.end_offset), secret);
    if (fingerprintDigestsEqual(candidate, checkpoint.digest)) {
      return {
        startOffset: checkpoint.end_offset,
        proof: { kind: "stable_prefix", checkpointOffset: checkpoint.end_offset } as const
      };
    }
  }
  return null;
}

function resolveSlidingWindow(state: FingerprintStateV1, current: string, secret: Uint8Array) {
  const candidateEnds = collectRecentLineEnds(current, POLICY_LIMITS.boundaryCandidates);
  const windows = [...state.baseline.suffix_windows].sort(
    (left, right) => right.character_length - left.character_length
  );

  for (const window of windows) {
    const matches = matchingWindowEnds(window, candidateEnds, current, secret);
    if (matches.length === 1) {
      const matchEndOffset = matches[0];
      if (matchEndOffset === undefined) continue;
      return {
        startOffset: matchEndOffset,
        proof: {
          kind: "sliding_window",
          windowCharacters: window.character_length,
          matchEndOffset
        } as const
      };
    }
  }
  return null;
}

function matchingWindowEnds(
  window: SuffixWindowV1,
  candidateEnds: number[],
  current: string,
  secret: Uint8Array
): number[] {
  const matches: number[] = [];
  for (const end of candidateEnds) {
    const start = end - window.character_length;
    if (start < 0) continue;
    const candidate = fingerprintDigest("suffix", current.slice(start, end), secret);
    if (fingerprintDigestsEqual(candidate, window.digest)) {
      matches.push(end);
      if (matches.length > 1) break;
    }
  }
  return matches;
}

function resolveContextualAnchor(state: FingerprintStateV1, current: string, secret: Uint8Array) {
  const lines = collectRecentLines(current, POLICY_LIMITS.boundaryCandidates);
  let candidatesExamined = 0;

  for (const anchor of state.baseline.tail_anchors) {
    const matches: number[] = [];
    for (const line of lines) {
      if (line.end - line.start !== anchor.line_characters) continue;
      candidatesExamined += 1;
      if (candidatesExamined > POLICY_LIMITS.anchorOccurrences) return null;
      if (!anchorMatches(anchor, line, current, secret)) continue;
      matches.push(line.end);
      if (matches.length > 1) break;
    }
    if (matches.length === 1) {
      const matchEndOffset = matches[0];
      if (matchEndOffset === undefined) continue;
      return {
        startOffset: matchEndOffset,
        proof: {
          kind: "contextual_anchor",
          lineCharacters: anchor.line_characters,
          contextCharacters: anchor.context_characters,
          matchEndOffset
        } as const
      };
    }
  }
  return null;
}

function anchorMatches(anchor: TailAnchorV1, line: LineRange, current: string, secret: Uint8Array): boolean {
  const lineDigest = fingerprintDigest("anchor-line", current.slice(line.start, line.end), secret);
  return fingerprintDigestsEqual(lineDigest, anchor.line_digest) && anchorContextMatches(anchor, line, current, secret);
}

function collectRecentLineEnds(input: string, limit: number): number[] {
  const ends = new Set<number>([input.length]);
  for (let index = input.length - 1; index >= 0 && ends.size < limit; index -= 1) {
    if (input[index] !== "\n") continue;
    ends.add(index);
    if (ends.size < limit) ends.add(index + 1);
  }
  return [...ends].sort((left, right) => left - right);
}

function collectRecentLines(input: string, limit: number): LineRange[] {
  const lines: LineRange[] = [];
  let end = input.length;
  while (lines.length < limit) {
    const newline = input.lastIndexOf("\n", Math.max(0, end - 1));
    const start = newline === -1 ? 0 : newline + 1;
    lines.push({ start, end });
    if (newline === -1) break;
    end = newline;
  }
  return lines.reverse();
}

function countLines(input: string): number {
  if (input.length === 0) return 0;
  let count = 1;
  for (const character of input) {
    if (character === "\n") count += 1;
  }
  return count;
}

function assertCurrentInput(current: string): void {
  const bytes = Buffer.byteLength(current, "utf8");
  if (bytes > POLICY_LIMITS.paneReadBytes) {
    throw new HerdrMathError("scanner_input_limit", {
      limit_kind: "pane_read_bytes",
      limit: POLICY_LIMITS.paneReadBytes,
      actual: bytes
    });
  }
}

function assertUsableFingerprint(state: FingerprintStateV1): void {
  const { baseline } = state;
  const invalid =
    state.schema_version !== FINGERPRINT_SCHEMA_VERSION ||
    !isNonNegativeInteger(baseline.character_count) ||
    baseline.character_count > POLICY_LIMITS.paneReadBytes ||
    !isFingerprintDigest(baseline.digest) ||
    baseline.prefix_checkpoints.length > FINGERPRINT_SCHEMA_LIMITS.maxPrefixCheckpoints ||
    baseline.suffix_windows.length > FINGERPRINT_SCHEMA_LIMITS.maxSuffixWindows ||
    baseline.tail_anchors.length > FINGERPRINT_SCHEMA_LIMITS.maxTailAnchors ||
    baseline.prefix_checkpoints.some(
      ({ end_offset, digest }) =>
        !isPositiveInteger(end_offset) || end_offset >= baseline.character_count || !isFingerprintDigest(digest)
    ) ||
    baseline.suffix_windows.some(
      ({ character_length, digest }) =>
        !isPositiveInteger(character_length) ||
        character_length > baseline.character_count ||
        !isFingerprintDigest(digest)
    ) ||
    baseline.tail_anchors.some(
      (anchor) =>
        !isPositiveInteger(anchor.line_characters) ||
        anchor.line_characters < FINGERPRINT_SCHEMA_LIMITS.minTailAnchorCharacters ||
        !isNonNegativeInteger(anchor.context_characters) ||
        anchor.context_characters > FINGERPRINT_SCHEMA_LIMITS.maxContextCharacters ||
        !isNonNegativeInteger(anchor.line_index_from_end) ||
        anchor.line_index_from_end >= POLICY_LIMITS.paneReadLines ||
        (anchor.end_offset !== undefined &&
          (!isPositiveInteger(anchor.end_offset) ||
            anchor.end_offset > baseline.character_count ||
            anchor.end_offset < anchor.line_characters)) ||
        (anchor.next_anchor_gap_digest !== undefined &&
          (anchor.end_offset === undefined || !isFingerprintDigest(anchor.next_anchor_gap_digest))) ||
        (anchor.forward_context_characters === undefined) !== (anchor.forward_context_digest === undefined) ||
        (anchor.forward_context_characters !== undefined &&
          (!isNonNegativeInteger(anchor.forward_context_characters) ||
            anchor.forward_context_characters > FINGERPRINT_SCHEMA_LIMITS.maxContextCharacters ||
            !isFingerprintDigest(anchor.forward_context_digest))) ||
        (anchor.next_anchor_gap_formula_digests !== undefined &&
          (anchor.next_anchor_gap_digest === undefined ||
            anchor.next_anchor_gap_formula_digests.length > FINGERPRINT_SCHEMA_LIMITS.maxGapFormulaDigests ||
            anchor.next_anchor_gap_formula_digests.some((digest) => !isFingerprintDigest(digest)))) ||
        (anchor.prefix_formula_digests !== undefined &&
          (anchor.prefix_formula_digests.length > FINGERPRINT_SCHEMA_LIMITS.maxGapFormulaDigests ||
            anchor.prefix_formula_digests.some((digest) => !isFingerprintDigest(digest)))) ||
        (anchor.suffix_formula_digests !== undefined &&
          (anchor.suffix_formula_digests.length > FINGERPRINT_SCHEMA_LIMITS.maxGapFormulaDigests ||
            anchor.suffix_formula_digests.some((digest) => !isFingerprintDigest(digest)))) ||
        !isFingerprintDigest(anchor.line_digest) ||
        !isFingerprintDigest(anchor.context_digest)
    );
  if (invalid) throw new HerdrMathError("state_corrupt");
}

function isPositiveInteger(value: number): boolean {
  return Number.isSafeInteger(value) && value > 0;
}

function isNonNegativeInteger(value: number): boolean {
  return Number.isSafeInteger(value) && value >= 0;
}
