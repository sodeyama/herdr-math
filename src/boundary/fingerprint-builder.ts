import { Buffer } from "node:buffer";

import { HerdrMathError } from "../core/errors.js";
import { POLICY_LIMITS } from "../core/limits.js";
import { scanLatex } from "../scanner/scan-latex.js";
import { assertFingerprintSecret, fingerprintDigest, formulaFingerprintDigest } from "./fingerprint-digest.js";
import {
  FINGERPRINT_SCHEMA_LIMITS,
  FINGERPRINT_SCHEMA_VERSION,
  type FingerprintDigest,
  type FingerprintStateV1,
  type LifecycleAuthority,
  type SupportedAgent,
  isStateIdentifier
} from "./fingerprint-schema.js";

export interface FingerprintBuildMetadata {
  sessionIdentity: string;
  occupantIdentity: string;
  workspaceId: string;
  sourcePaneId: string;
  agent: SupportedAgent;
  lifecycleAuthority: LifecycleAuthority;
  paneRevision: number;
  eventSequence: number;
  generation: number;
  createdAt: Date;
}

export const FINGERPRINT_EXPIRY_MS = POLICY_LIMITS.fingerprintExpiryMs;

const SUPPORTED_AGENTS = new Set<SupportedAgent>(["claude", "codex", "cursor", "pi", "opencode"]);
const LIFECYCLE_AUTHORITIES = new Set<LifecycleAuthority>(["screen_detection", "integration_hook"]);
const SUFFIX_WINDOW_LENGTHS = [80, 128, 256, 512] as const;

export function buildBaselineFingerprint(
  input: string,
  metadata: FingerprintBuildMetadata,
  secret: Uint8Array,
  expiryMs = FINGERPRINT_EXPIRY_MS
): FingerprintStateV1 {
  assertFingerprintSecret(secret);
  assertInput(input);
  assertMetadata(metadata);
  if (!Number.isSafeInteger(expiryMs) || expiryMs <= 0) {
    throw new RangeError("Fingerprint expiry must be a positive safe integer.");
  }

  const characterCount = input.length;
  const createdAt = new Date(metadata.createdAt.getTime());
  const expiresAt = new Date(createdAt.getTime() + expiryMs);
  if (Number.isNaN(expiresAt.getTime())) {
    throw new RangeError("Fingerprint expiry is outside the supported date range.");
  }

  return {
    schema_version: FINGERPRINT_SCHEMA_VERSION,
    session_key: deriveStateKey("session", metadata.sessionIdentity, secret),
    workspace_id: metadata.workspaceId,
    source_pane_id: metadata.sourcePaneId,
    agent: metadata.agent,
    lifecycle_authority: metadata.lifecycleAuthority,
    occupant_key: deriveStateKey("occupant", metadata.occupantIdentity, secret),
    pane_revision: metadata.paneRevision,
    event_sequence: metadata.eventSequence,
    generation: metadata.generation,
    baseline: {
      character_count: characterCount,
      utf8_bytes: Buffer.byteLength(input, "utf8"),
      line_count: countLines(input),
      digest: fingerprintDigest("baseline", input, secret),
      prefix_checkpoints: buildPrefixCheckpoints(input, secret),
      suffix_windows: buildSuffixWindows(input, secret),
      tail_anchors: buildTailAnchors(input, secret)
    },
    created_at: createdAt.toISOString(),
    expires_at: expiresAt.toISOString()
  };
}

export function deriveStateKey(
  purpose: "session" | "pane" | "occupant",
  identity: string,
  secret: Uint8Array
): FingerprintDigest {
  assertFingerprintSecret(secret);
  if (typeof identity !== "string" || identity.length === 0 || identity.length > 4096) {
    throw new HerdrMathError("event_invalid");
  }
  return fingerprintDigest(`state-key:${purpose}`, identity, secret);
}

function buildPrefixCheckpoints(input: string, secret: Uint8Array) {
  const checkpoints: FingerprintStateV1["baseline"]["prefix_checkpoints"] = [];
  const seen = new Set<number>();
  for (let index = 1; index <= FINGERPRINT_SCHEMA_LIMITS.maxPrefixCheckpoints; index += 1) {
    const offset = Math.floor((input.length * index) / (FINGERPRINT_SCHEMA_LIMITS.maxPrefixCheckpoints + 1));
    if (offset < 80 || offset >= input.length || seen.has(offset)) {
      continue;
    }
    seen.add(offset);
    checkpoints.push({ end_offset: offset, digest: fingerprintDigest("prefix", input.slice(0, offset), secret) });
  }
  return checkpoints;
}

function buildSuffixWindows(input: string, secret: Uint8Array) {
  return SUFFIX_WINDOW_LENGTHS.filter((length) => length <= input.length)
    .slice(0, FINGERPRINT_SCHEMA_LIMITS.maxSuffixWindows)
    .map((length) => ({
      character_length: length,
      digest: fingerprintDigest("suffix", input.slice(-length), secret)
    }));
}

function buildTailAnchors(input: string, secret: Uint8Array) {
  const anchors: FingerprintStateV1["baseline"]["tail_anchors"] = [];
  const baselineFormulas = fingerprintBaselineFormulas(input, secret);
  let lineEnd = input.length;
  let lineIndex = 0;

  while (lineIndex < POLICY_LIMITS.paneReadLines && anchors.length < FINGERPRINT_SCHEMA_LIMITS.maxTailAnchors) {
    const newline = input.lastIndexOf("\n", Math.max(0, lineEnd - 1));
    const lineStart = newline === -1 ? 0 : newline + 1;
    const line = input.slice(lineStart, lineEnd);
    if (line.trim().length >= FINGERPRINT_SCHEMA_LIMITS.minTailAnchorCharacters) {
      const previousNewline = input.lastIndexOf("\n", Math.max(0, lineStart - 2));
      const contextStart = Math.max(previousNewline + 1, lineStart - FINGERPRINT_SCHEMA_LIMITS.maxContextCharacters);
      const context = input.slice(contextStart, lineStart);
      const forwardContext = buildForwardContext(input, lineEnd);
      const prefixFormulaDigests =
        baselineFormulas === undefined
          ? undefined
          : [...new Set(baselineFormulas.filter(({ end }) => end <= lineStart).map(({ digest }) => digest))];
      const suffixFormulaDigests =
        baselineFormulas === undefined
          ? undefined
          : [...new Set(baselineFormulas.filter(({ start }) => start >= lineEnd).map(({ digest }) => digest))];
      anchors.push({
        end_offset: lineEnd,
        forward_context_characters: forwardContext.length,
        forward_context_digest: fingerprintDigest("anchor-forward-context", forwardContext, secret),
        line_characters: line.length,
        line_digest: fingerprintDigest("anchor-line", line, secret),
        context_characters: context.length,
        context_digest: fingerprintDigest("anchor-context", context, secret),
        line_index_from_end: lineIndex,
        ...(prefixFormulaDigests === undefined ? {} : { prefix_formula_digests: prefixFormulaDigests }),
        ...(suffixFormulaDigests === undefined ? {} : { suffix_formula_digests: suffixFormulaDigests })
      });
    }
    if (newline === -1) {
      break;
    }
    lineEnd = newline;
    lineIndex += 1;
  }

  const orderedAnchors = [...anchors].sort((left, right) => {
    return (left.end_offset ?? 0) - (right.end_offset ?? 0);
  });
  for (let index = 0; index < orderedAnchors.length - 1; index += 1) {
    const before = orderedAnchors[index];
    const after = orderedAnchors[index + 1];
    if (before?.end_offset === undefined || after?.end_offset === undefined) continue;
    const afterStart = after.end_offset - after.line_characters;
    if (afterStart < before.end_offset) continue;
    const gap = input.slice(before.end_offset, afterStart);
    before.next_anchor_gap_digest = fingerprintDigest("anchor-gap", gap, secret);
    try {
      const formulas = scanLatex(gap);
      before.next_anchor_gap_formula_digests = [
        ...new Set(formulas.map((formula) => formulaFingerprintDigest(formula, secret)))
      ];
    } catch {
      // Unscannable baseline gaps are not eligible for replacement recovery.
    }
  }
  return anchors;
}

function fingerprintBaselineFormulas(input: string, secret: Uint8Array) {
  try {
    return scanLatex(input).map((formula) => ({
      start: formula.start,
      end: formula.end,
      digest: formulaFingerprintDigest(formula, secret)
    }));
  } catch {
    return undefined;
  }
}

function buildForwardContext(input: string, lineEnd: number): string {
  let contextEnd = lineEnd;
  for (let line = 0; line < 2; line += 1) {
    const nextNewline = input.indexOf("\n", contextEnd + 1);
    contextEnd = nextNewline === -1 ? input.length : nextNewline;
    if (nextNewline === -1) break;
  }
  contextEnd = Math.min(contextEnd, lineEnd + FINGERPRINT_SCHEMA_LIMITS.maxContextCharacters);
  return input.slice(lineEnd, contextEnd);
}

function assertInput(input: string): void {
  const bytes = Buffer.byteLength(input, "utf8");
  if (bytes > POLICY_LIMITS.paneReadBytes) {
    throw new HerdrMathError("scanner_input_limit", {
      limit_kind: "pane_read_bytes",
      limit: POLICY_LIMITS.paneReadBytes,
      actual: bytes
    });
  }
}

function assertMetadata(metadata: FingerprintBuildMetadata): void {
  if (
    !isStateIdentifier(metadata.workspaceId) ||
    !isStateIdentifier(metadata.sourcePaneId) ||
    !SUPPORTED_AGENTS.has(metadata.agent) ||
    !LIFECYCLE_AUTHORITIES.has(metadata.lifecycleAuthority) ||
    typeof metadata.sessionIdentity !== "string" ||
    metadata.sessionIdentity.length === 0 ||
    metadata.sessionIdentity.length > 4096 ||
    typeof metadata.occupantIdentity !== "string" ||
    metadata.occupantIdentity.length === 0 ||
    metadata.occupantIdentity.length > 4096 ||
    !isNonNegativeSafeInteger(metadata.paneRevision) ||
    !isNonNegativeSafeInteger(metadata.eventSequence) ||
    !isNonNegativeSafeInteger(metadata.generation) ||
    !(metadata.createdAt instanceof Date) ||
    Number.isNaN(metadata.createdAt.getTime())
  ) {
    throw new HerdrMathError("event_invalid");
  }
}

function isNonNegativeSafeInteger(value: number): boolean {
  return Number.isSafeInteger(value) && value >= 0;
}

function countLines(input: string): number {
  if (input.length === 0) {
    return 0;
  }
  let lines = 1;
  for (const character of input) {
    if (character === "\n") lines += 1;
  }
  return lines;
}
