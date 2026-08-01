import { Buffer } from "node:buffer";

import type { RenderedImage } from "../core/contracts.js";
import { failure, success, type OperationResult } from "../core/contracts.js";
import { HerdrMathError, serializeError, type SafeLimitKind } from "../core/errors.js";
import { POLICY_LIMITS } from "../core/limits.js";
import type { HerdrGraphicsInfo, HerdrLayoutRect } from "../herdr/socket-client.js";

const PNG_SIGNATURE = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);

export interface EncodedPng {
  dataBase64: string;
  width: number;
  height: number;
}

export interface GraphicsPlacement {
  viewportCol: 0;
  viewportRow: 0;
  gridCols: number;
  gridRows: number;
}

export function encodeValidatedPng(image: RenderedImage): OperationResult<EncodedPng> {
  try {
    if (
      !Buffer.isBuffer(image.buffer) ||
      image.bytes !== image.buffer.byteLength ||
      typeof image.renderer !== "string" ||
      image.renderer.length === 0 ||
      !image.buffer.subarray(0, PNG_SIGNATURE.length).equals(PNG_SIGNATURE)
    ) {
      throw new HerdrMathError("renderer_failed", {}, true);
    }
    assertDimension("image_width_px", image.width, POLICY_LIMITS.imageWidthPx);
    assertDimension("image_height_px", image.height, POLICY_LIMITS.imageHeightPx);
    const pixels = image.width * image.height;
    if (!Number.isSafeInteger(pixels) || pixels > POLICY_LIMITS.imagePixels) {
      throw limitError("image_pixels", POLICY_LIMITS.imagePixels, pixels);
    }
    assertBytes("raw_png_bytes", image.bytes, POLICY_LIMITS.rawPngBytes);
    const encodedBytes = 4 * Math.ceil(image.bytes / 3);
    assertBytes("base64_payload_bytes", encodedBytes, POLICY_LIMITS.base64PayloadBytes);
    const dataBase64 = image.buffer.toString("base64");
    assertBytes("base64_payload_bytes", Buffer.byteLength(dataBase64, "ascii"), POLICY_LIMITS.base64PayloadBytes);
    return success(Object.freeze({ dataBase64, width: image.width, height: image.height }));
  } catch (error) {
    return failure(serializeError(error));
  }
}

export function validateGraphicsInfo(info: HerdrGraphicsInfo): OperationResult<HerdrGraphicsInfo> {
  if (
    !Number.isSafeInteger(info.cellWidthPx) ||
    !Number.isSafeInteger(info.cellHeightPx) ||
    info.cellWidthPx <= 0 ||
    info.cellHeightPx <= 0
  ) {
    return failure(serializeError(new HerdrMathError("cell_size_unavailable")));
  }
  return success(info);
}

export function computeGraphicsPlacement(
  image: Pick<EncodedPng, "width" | "height">,
  info: HerdrGraphicsInfo,
  viewerRect: HerdrLayoutRect
): OperationResult<GraphicsPlacement> {
  const capability = validateGraphicsInfo(info);
  if (!capability.ok) return failure(capability.error);
  if (
    !Number.isSafeInteger(viewerRect.width) ||
    !Number.isSafeInteger(viewerRect.height) ||
    viewerRect.width <= 0 ||
    viewerRect.height <= 0
  ) {
    return failure(serializeError(new HerdrMathError("cell_size_unavailable")));
  }
  if (
    !Number.isSafeInteger(image.width) ||
    !Number.isSafeInteger(image.height) ||
    image.width <= 0 ||
    image.height <= 0
  ) {
    return failure(serializeError(new HerdrMathError("renderer_failed", {}, true)));
  }

  const naturalCols = Math.ceil(image.width / capability.value.cellWidthPx);
  const naturalRows = Math.ceil(image.height / capability.value.cellHeightPx);
  const scale = Math.min(1, viewerRect.width / naturalCols, viewerRect.height / naturalRows);
  return success(
    Object.freeze({
      viewportCol: 0,
      viewportRow: 0,
      gridCols: Math.max(1, Math.min(viewerRect.width, Math.floor(naturalCols * scale))),
      gridRows: Math.max(1, Math.min(viewerRect.height, Math.floor(naturalRows * scale)))
    })
  );
}

function assertDimension(kind: SafeLimitKind, actual: number, limit: number): void {
  if (!Number.isSafeInteger(actual) || actual <= 0) throw new HerdrMathError("renderer_failed", {}, true);
  if (actual > limit) throw limitError(kind, limit, actual);
}

function assertBytes(kind: SafeLimitKind, actual: number, limit: number): void {
  if (!Number.isSafeInteger(actual) || actual < 0 || actual > limit) throw limitError(kind, limit, actual);
}

function limitError(kind: SafeLimitKind, limit: number, actual: number): HerdrMathError {
  return new HerdrMathError("image_too_large", { limit_kind: kind, limit, actual });
}
