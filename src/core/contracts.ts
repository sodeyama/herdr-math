import type { Buffer } from "node:buffer";

import type { SafeErrorRecord } from "./errors.js";

export interface Formula {
  latex: string;
  display: boolean;
  start: number;
  end: number;
  delimiter?: "dollar" | "paren" | "bracket";
}

export type BoundaryStrategy =
  | "exact_prefix"
  | "middle_insertion"
  | "middle_replacement"
  | "anchored_prefix_replacement"
  | "anchored_suffix_replacement"
  | "stable_prefix"
  | "sliding_window"
  | "contextual_anchor";

export type BoundaryProof =
  | { kind: "exact_prefix"; baselineCharacters: number }
  | {
      kind: "middle_insertion";
      beforeMatchEndOffset: number;
      afterMatchStartOffset: number;
      baselineGapCharacters: number;
      insertedCharacters: number;
    }
  | {
      kind: "middle_replacement";
      beforeMatchEndOffset: number;
      afterMatchStartOffset: number;
      baselineGapCharacters: number;
      replacementCharacters: number;
      baselineFormulaDigests: readonly string[];
    }
  | {
      kind: "anchored_prefix_replacement";
      anchorMatchStartOffset: number;
      lineIndexFromEnd: number;
      replacementGrowthCharacters: number;
      baselineFormulaDigests: readonly string[];
    }
  | {
      kind: "anchored_suffix_replacement";
      anchorMatchEndOffset: number;
      lineIndexFromEnd: number;
      replacementGrowthCharacters: number;
      baselineFormulaDigests: readonly string[];
    }
  | { kind: "stable_prefix"; checkpointOffset: number }
  | { kind: "sliding_window"; windowCharacters: number; matchEndOffset: number }
  | {
      kind: "contextual_anchor";
      lineCharacters: number;
      contextCharacters: number;
      matchEndOffset: number;
    };

export interface BoundaryResult {
  answer: string;
  startOffset: number;
  strategy: BoundaryStrategy;
  recoveredTruncation: boolean;
  currentDigest: string;
  proof: BoundaryProof;
}

export interface RenderedImage {
  buffer: Buffer;
  width: number;
  height: number;
  bytes: number;
  renderer: string;
}

export type OperationResult<T> = { ok: true; value: T } | { ok: false; error: SafeErrorRecord };

export function success<T>(value: T): OperationResult<T> {
  return { ok: true, value };
}

export function failure<T>(error: SafeErrorRecord): OperationResult<T> {
  return { ok: false, error };
}
