import { Buffer } from "node:buffer";

import { describe, expect, it } from "vitest";

import type { RenderedImage } from "../../src/core/contracts.js";
import { POLICY_LIMITS } from "../../src/core/limits.js";
import { computeGraphicsPlacement, encodeValidatedPng, validateGraphicsInfo } from "../../src/graphics/placement.js";

const PNG_SIGNATURE = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);

describe("graphics image validation", () => {
  it("encodes a bounded PNG only after validating dimensions and bytes", () => {
    const image = validImage();
    expect(encodeValidatedPng(image)).toEqual({
      ok: true,
      value: {
        dataBase64: image.buffer.toString("base64"),
        width: 640,
        height: 320
      }
    });
  });

  it.each([
    ["width", validImage({ width: POLICY_LIMITS.imageWidthPx + 1 }), "image_width_px"],
    ["height", validImage({ height: POLICY_LIMITS.imageHeightPx + 1 }), "image_height_px"],
    ["pixels", validImage({ width: 4096, height: 8193 }), "image_pixels"],
    ["raw bytes", validImage({ buffer: pngBuffer(POLICY_LIMITS.rawPngBytes + 1) }), "raw_png_bytes"]
  ])("rejects excessive %s", (_name, image, limitKind) => {
    const result = encodeValidatedPng(image);
    expect(result.ok).toBe(false);
    if (result.ok) return;
    expect(result.error).toMatchObject({ code: "image_too_large", details: { limit_kind: limitKind } });
  });

  it("rejects malformed or inconsistent renderer output", () => {
    for (const image of [
      validImage({ buffer: Buffer.from("not a png") }),
      { ...validImage(), bytes: 1 },
      validImage({ width: 0 })
    ]) {
      expect(encodeValidatedPng(image)).toEqual({
        ok: false,
        error: { code: "renderer_failed", retryable: true }
      });
    }
  });
});

describe("graphics placement", () => {
  it("uses natural cell dimensions and scales within the current viewer rectangle", () => {
    const image = { width: 640, height: 320 };
    const info = { cellWidthPx: 8, cellHeightPx: 16 };
    expect(computeGraphicsPlacement(image, info, { x: 40, y: 0, width: 80, height: 20 })).toEqual({
      ok: true,
      value: { viewportCol: 0, viewportRow: 0, gridCols: 80, gridRows: 20 }
    });
    expect(computeGraphicsPlacement(image, info, { x: 40, y: 0, width: 40, height: 10 })).toEqual({
      ok: true,
      value: { viewportCol: 0, viewportRow: 0, gridCols: 40, gridRows: 10 }
    });
    expect(computeGraphicsPlacement({ width: 16, height: 16 }, info, { x: 0, y: 0, width: 80, height: 20 })).toEqual({
      ok: true,
      value: { viewportCol: 0, viewportRow: 0, gridCols: 2, gridRows: 1 }
    });
  });

  it("fails closed when cell or viewer dimensions are unavailable", () => {
    expect(validateGraphicsInfo({ cellWidthPx: 0, cellHeightPx: 16 })).toEqual({
      ok: false,
      error: { code: "cell_size_unavailable", retryable: false }
    });
    expect(
      computeGraphicsPlacement(
        { width: 640, height: 320 },
        { cellWidthPx: 8, cellHeightPx: 16 },
        { x: 0, y: 0, width: 0, height: 20 }
      )
    ).toEqual({
      ok: false,
      error: { code: "cell_size_unavailable", retryable: false }
    });
  });
});

function validImage(overrides: Partial<RenderedImage> = {}): RenderedImage {
  const buffer = overrides.buffer ?? pngBuffer(16);
  return {
    buffer,
    width: overrides.width ?? 640,
    height: overrides.height ?? 320,
    bytes: overrides.bytes ?? buffer.byteLength,
    renderer: overrides.renderer ?? "test-renderer"
  };
}

function pngBuffer(bytes: number): Buffer {
  const buffer = Buffer.alloc(bytes);
  PNG_SIGNATURE.copy(buffer);
  return buffer;
}
