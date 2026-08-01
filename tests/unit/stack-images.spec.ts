import { describe, expect, it } from "vitest";
import sharp from "sharp";

import { stackRenderedImages } from "../../src/viewer/stack-images.js";

describe("stackRenderedImages", () => {
  it("returns the next image when there is no previous history", async () => {
    const next = await png(480, 120, { r: 200, g: 10, b: 10, alpha: 1 });
    expect(await stackRenderedImages(undefined, next)).toEqual(next);
  });

  it("appends the next image below the previous one at the same width", async () => {
    const previous = await png(480, 100, { r: 10, g: 10, b: 200, alpha: 1 });
    const next = await png(480, 80, { r: 10, g: 200, b: 10, alpha: 1 });
    const stacked = await stackRenderedImages(previous, next);
    expect(stacked.width).toBe(480);
    expect(stacked.height).toBe(192);
    const top = await sample(stacked, 240, 10);
    const bottom = await sample(stacked, 240, 150);
    expect(top).toMatchObject({ r: 10, g: 10, b: 200 });
    expect(bottom).toMatchObject({ r: 10, g: 200, b: 10 });
  });

  it("resets history when the render width changes", async () => {
    const previous = await png(480, 100, { r: 10, g: 10, b: 200, alpha: 1 });
    const next = await png(640, 80, { r: 10, g: 200, b: 10, alpha: 1 });
    expect(await stackRenderedImages(previous, next)).toEqual(next);
  });
});

async function png(width: number, height: number, background: { r: number; g: number; b: number; alpha: number }) {
  const buffer = await sharp({ create: { width, height, channels: 4, background } })
    .png()
    .toBuffer();
  return { buffer, width, height, bytes: buffer.byteLength, renderer: "test" };
}

async function sample(image: { buffer: Buffer; width: number; height: number }, x: number, y: number) {
  const pixel = await sharp(image.buffer).extract({ left: x, top: y, width: 1, height: 1 }).raw().toBuffer();
  return { r: pixel[0], g: pixel[1], b: pixel[2] };
}
