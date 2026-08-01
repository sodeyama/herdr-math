import { Buffer } from "node:buffer";

import { failure, success, type BoundaryProof, type BoundaryResult, type OperationResult } from "../core/contracts.js";
import { HerdrMathError, serializeError } from "../core/errors.js";
import { POLICY_LIMITS } from "../core/limits.js";
import { assertFingerprintSecret, fingerprintDigest, fingerprintDigestsEqual } from "./fingerprint-digest.js";
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

    const stable = resolveStablePrefix(state, current, secret);
    if (stable !== null) {
      return success(result(current, stable.startOffset, stable.proof, currentDigest, readTruncated));
    }

    const sliding = resolveSlidingWindow(state, current, secret);
    if (sliding !== null) {
      return success(result(current, sliding.startOffset, sliding.proof, currentDigest, readTruncated));
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
  recoveredTruncation: boolean
): BoundaryResult {
  return {
    answer: current.slice(startOffset),
    startOffset,
    strategy: proof.kind,
    recoveredTruncation,
    currentDigest,
    proof
  };
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
  if (!fingerprintDigestsEqual(lineDigest, anchor.line_digest) || line.start < anchor.context_characters) {
    return false;
  }
  const context = current.slice(line.start - anchor.context_characters, line.start);
  return fingerprintDigestsEqual(fingerprintDigest("anchor-context", context, secret), anchor.context_digest);
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
