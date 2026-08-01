import sharp from "sharp";

import type { RenderedImage } from "../core/contracts.js";
import { HerdrMathError } from "../core/errors.js";
import { POLICY_LIMITS } from "../core/limits.js";

export const STACK_GAP_PX = 12;

export async function stackRenderedImages(
  previous: RenderedImage | undefined,
  next: RenderedImage
): Promise<RenderedImage> {
  if (previous === undefined) return next;
  if (previous.width !== next.width) return next;

  let top = previous;
  let height = previous.height + STACK_GAP_PX + next.height;
  if (height > POLICY_LIMITS.imageHeightPx) {
    const maxTopHeight = POLICY_LIMITS.imageHeightPx - STACK_GAP_PX - next.height;
    if (maxTopHeight <= 0) return next;
    top = await cropFromTop(previous, maxTopHeight);
    height = top.height + STACK_GAP_PX + next.height;
  }

  const output = await sharp({
    create: {
      width: next.width,
      height,
      channels: 4,
      background: { r: 0, g: 0, b: 0, alpha: 0 }
    }
  })
    .composite([
      { input: top.buffer, top: 0, left: 0 },
      { input: next.buffer, top: top.height + STACK_GAP_PX, left: 0 }
    ])
    .png({
      adaptiveFiltering: true,
      compressionLevel: 9,
      palette: true,
      quality: 100,
      colours: 256,
      dither: 0
    })
    .toBuffer({ resolveWithObject: true });

  return {
    buffer: output.data,
    width: output.info.width,
    height: output.info.height,
    bytes: output.data.byteLength,
    renderer: next.renderer
  };
}

async function cropFromTop(image: RenderedImage, height: number): Promise<RenderedImage> {
  if (height >= image.height) return image;
  try {
    const top = image.height - height;
    const output = await sharp(image.buffer, { limitInputPixels: POLICY_LIMITS.imagePixels })
      .extract({ left: 0, top, width: image.width, height })
      .png({
        adaptiveFiltering: true,
        compressionLevel: 9,
        palette: true,
        quality: 100,
        colours: 256,
        dither: 0
      })
      .toBuffer({ resolveWithObject: true });
    return {
      buffer: output.data,
      width: output.info.width,
      height: output.info.height,
      bytes: output.data.byteLength,
      renderer: image.renderer
    };
  } catch {
    throw new HerdrMathError("renderer_failed", {}, true);
  }
}
