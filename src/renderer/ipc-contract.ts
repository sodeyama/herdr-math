import { Buffer } from "node:buffer";

import type { SafeErrorRecord } from "../core/errors.js";
import type { RendererLimits } from "./render.js";
import type { RendererLayout } from "./layout.js";

/**
 * Threads the versioned JSON contract between the Rust `tmath` binary and the
 * one-shot TypeScript renderer subprocess (`tmath-render`).
 *
 * A single request is read from stdin and a single response is written to
 * stdout. The subprocess renders, writes its response, and exits; it never
 * stays alive waiting for more input.
 */

export const IPC_PROTOCOL = "tmath-render/1";

/**
 * Maximum bytes read for a request payload before the boundary rejects it.
 * Sized above `POLICY_LIMITS.scannerInputBytes` (160 MiB) to leave room for
 * JSON envelope overhead (escaping, field names) around the source document.
 */
export const IPC_MAX_REQUEST_BYTES = 192 * 1024 * 1024;

/** Maximum bytes accepted for a response payload (PNG + envelope). */
export const IPC_MAX_RESPONSE_BYTES = 768 * 1024;

export { IPC_PROTOCOL as RENDER_IPC_PROTOCOL_VERSION };

/** A formula as transmitted over the wire (no offsets required). */
export interface WireFormula {
  latex: string;
  display: boolean;
}

export interface RenderIpcRequest {
  protocol: string;
  kind: "document" | "formulas";
  /** Source document for `document` requests (scanned by the subprocess). */
  text?: string;
  /** Pre-scanned formulas for `formulas` requests. */
  formulas?: readonly WireFormula[];
  options?: {
    limits?: Partial<RendererLimits>;
    layout?: Partial<RendererLayout>;
  };
}

export type RenderIpcResponse =
  | {
      protocol: string;
      ok: true;
      width: number;
      height: number;
      bytes: number;
      renderer: string;
      base64: string;
    }
  | { protocol: string; ok: false; error: SafeErrorRecord };

export function encodeRequest(message: RenderIpcRequest): Buffer {
  const payload = Buffer.from(JSON.stringify(message), "utf8");
  if (Buffer.byteLength(payload) > IPC_MAX_REQUEST_BYTES) {
    throw new RangeError(`Request exceeds ${IPC_MAX_REQUEST_BYTES} bytes`);
  }
  return payload;
}

export function decodeRequest(payload: Buffer): RenderIpcRequest {
  if (Buffer.byteLength(payload) > IPC_MAX_REQUEST_BYTES) {
    throw new RangeError(`Request exceeds ${IPC_MAX_REQUEST_BYTES} bytes`);
  }
  const parsed: unknown = JSON.parse(payload.toString("utf8"));
  if (
    typeof parsed !== "object" ||
    parsed === null ||
    typeof (parsed as RenderIpcRequest).protocol !== "string" ||
    ((parsed as RenderIpcRequest).kind !== "document" && (parsed as RenderIpcRequest).kind !== "formulas")
  ) {
    throw new TypeError("Malformed render request");
  }
  return parsed as RenderIpcRequest;
}

export function validateRequest(request: RenderIpcRequest): string | undefined {
  if (request.protocol !== IPC_PROTOCOL) return `Unsupported protocol ${request.protocol}`;
  if (request.kind === "formulas") {
    if (!Array.isArray(request.formulas) || request.formulas.length === 0) return "No formulas";
    for (const formula of request.formulas) {
      if (
        typeof formula !== "object" ||
        formula === null ||
        typeof (formula as WireFormula).latex !== "string" ||
        typeof (formula as WireFormula).display !== "boolean"
      ) {
        return "Malformed formula";
      }
    }
    return undefined;
  }
  if (typeof request.text !== "string" || request.text.trim() === "") return "No source text";
  return undefined;
}

export function encodeResponse(response: RenderIpcResponse): Buffer {
  const payload = Buffer.from(JSON.stringify(response), "utf8");
  if (Buffer.byteLength(payload) > IPC_MAX_RESPONSE_BYTES) {
    throw new RangeError(`Response exceeds ${IPC_MAX_RESPONSE_BYTES} bytes`);
  }
  return payload;
}
