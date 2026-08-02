import type { Buffer } from "node:buffer";

import type { SafeErrorRecord } from "./errors.js";

export interface Formula {
  latex: string;
  display: boolean;
  start: number;
  end: number;
  delimiter?: "dollar" | "paren" | "bracket";
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
