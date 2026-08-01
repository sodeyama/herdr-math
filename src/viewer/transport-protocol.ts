import { Buffer } from "node:buffer";
import { isAbsolute, join } from "node:path";

import type { RenderedImage } from "../core/contracts.js";
import { failure, success, type OperationResult } from "../core/contracts.js";
import { ERROR_CODES, HerdrMathError, serializeError, type SafeErrorRecord } from "../core/errors.js";
import { POLICY_LIMITS } from "../core/limits.js";
import { encodeValidatedPng } from "../graphics/placement.js";
import { isViewerSourceToken } from "./ownership.js";

const IDENTIFIER = /^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$/;
const RENDERER = /^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/;
const BASE64 = /^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/;
const SOCKET_PATH_BYTES = 103;

export interface ViewerTransportRequest {
  stateDirectory: string;
  sourceToken: string;
  viewerPaneId: string;
  workspaceId: string;
  generation: number;
  image: RenderedImage;
}

export interface ViewerTransportSuccess {
  ok: true;
  viewerPaneId: string;
}

export interface ViewerTransportFailure {
  ok: false;
  error: SafeErrorRecord;
}

export type ViewerTransportResponse = ViewerTransportSuccess | ViewerTransportFailure;

export function encodeViewerTransportRequest(
  request: ViewerTransportRequest
): OperationResult<{ socketPath: string; payload: string }> {
  try {
    validateViewerTransportIdentity({ ...request, renderer: request.image.renderer });
    const encoded = encodeValidatedPng(request.image);
    if (!encoded.ok) return failure(encoded.error);
    const payload = `${JSON.stringify({
      version: 1,
      sourceToken: request.sourceToken,
      viewerPaneId: request.viewerPaneId,
      workspaceId: request.workspaceId,
      generation: request.generation,
      image: {
        dataBase64: encoded.value.dataBase64,
        width: encoded.value.width,
        height: encoded.value.height,
        bytes: request.image.bytes,
        renderer: request.image.renderer
      }
    })}\n`;
    const payloadBytes = Buffer.byteLength(payload, "utf8");
    if (payloadBytes > POLICY_LIMITS.viewerTransportBytes) {
      return transportLimitFailure(POLICY_LIMITS.viewerTransportBytes, payloadBytes);
    }
    return success({ socketPath: viewerSocketPath(request.stateDirectory, request.sourceToken), payload });
  } catch (error) {
    return failure(serializeError(error));
  }
}

export function decodeViewerTransportRequest(
  source: string,
  expected: Pick<ViewerTransportRequest, "sourceToken" | "viewerPaneId" | "workspaceId">
): OperationResult<{ image: RenderedImage }> {
  try {
    const value: unknown = JSON.parse(source);
    if (!isRecord(value) || !isRecord(value.image)) throw new HerdrMathError("viewer_ownership_failed");
    if (
      value.version !== 1 ||
      value.sourceToken !== expected.sourceToken ||
      value.viewerPaneId !== expected.viewerPaneId ||
      value.workspaceId !== expected.workspaceId ||
      !Number.isSafeInteger(value.generation) ||
      (value.generation as number) < 0
    ) {
      throw new HerdrMathError("viewer_ownership_failed");
    }
    const image = value.image;
    if (
      typeof image.dataBase64 !== "string" ||
      image.dataBase64.length === 0 ||
      !BASE64.test(image.dataBase64) ||
      !Number.isSafeInteger(image.width) ||
      !Number.isSafeInteger(image.height) ||
      !Number.isSafeInteger(image.bytes) ||
      typeof image.renderer !== "string" ||
      !RENDERER.test(image.renderer)
    ) {
      throw new HerdrMathError("viewer_ownership_failed");
    }
    const buffer = Buffer.from(image.dataBase64, "base64");
    if (buffer.byteLength !== image.bytes || buffer.toString("base64") !== image.dataBase64) {
      throw new HerdrMathError("viewer_ownership_failed");
    }
    const rendered: RenderedImage = {
      buffer,
      width: image.width as number,
      height: image.height as number,
      bytes: image.bytes,
      renderer: image.renderer
    };
    const validated = encodeValidatedPng(rendered);
    return validated.ok ? success({ image: rendered }) : failure(validated.error);
  } catch (error) {
    return failure(serializeError(error));
  }
}

export function parseViewerTransportResponse(source: string): ViewerTransportResponse {
  try {
    const value: unknown = JSON.parse(source);
    if (!isRecord(value) || typeof value.ok !== "boolean") throw new Error("invalid response");
    if (value.ok === true && typeof value.viewerPaneId === "string" && IDENTIFIER.test(value.viewerPaneId)) {
      return { ok: true, viewerPaneId: value.viewerPaneId };
    }
    if (value.ok === false && isSafeError(value.error)) return { ok: false, error: value.error };
  } catch {
    // The caller receives the same closed ownership error for every invalid response.
  }
  return viewerTransportFailure("viewer_ownership_failed");
}

export function validateViewerTransportIdentity(
  request: Pick<
    ViewerTransportRequest,
    "stateDirectory" | "sourceToken" | "viewerPaneId" | "workspaceId" | "generation"
  > & { renderer?: string }
): void {
  viewerSocketPath(request.stateDirectory, request.sourceToken);
  if (
    !IDENTIFIER.test(request.viewerPaneId) ||
    !IDENTIFIER.test(request.workspaceId) ||
    !Number.isSafeInteger(request.generation) ||
    request.generation < 0 ||
    (request.renderer !== undefined && !RENDERER.test(request.renderer))
  ) {
    throw new HerdrMathError("viewer_ownership_failed");
  }
}

export function viewerSocketPath(stateDirectory: string, sourceToken: string): string {
  if (
    typeof stateDirectory !== "string" ||
    !isAbsolute(stateDirectory) ||
    stateDirectory.includes("\0") ||
    !isViewerSourceToken(sourceToken)
  ) {
    throw new HerdrMathError("viewer_ownership_failed");
  }
  const path = join(stateDirectory, `.v-${sourceToken.slice(0, 16)}.s`);
  if (Buffer.byteLength(path, "utf8") > SOCKET_PATH_BYTES) throw new HerdrMathError("viewer_open_failed", {}, true);
  return path;
}

export function viewerTransportFailure(code: HerdrMathError["code"]): ViewerTransportFailure {
  return { ok: false, error: serializeError(new HerdrMathError(code)) };
}

export function transportLimitFailure(limit: number, actual: number): ViewerTransportFailure {
  return {
    ok: false,
    error: serializeError(
      new HerdrMathError("image_too_large", { limit_kind: "viewer_transport_bytes", limit, actual })
    )
  };
}

function isSafeError(value: unknown): value is SafeErrorRecord {
  return (
    isRecord(value) &&
    typeof value.code === "string" &&
    ERROR_CODES.includes(value.code as (typeof ERROR_CODES)[number]) &&
    typeof value.retryable === "boolean" &&
    (value.details === undefined || isRecord(value.details))
  );
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
