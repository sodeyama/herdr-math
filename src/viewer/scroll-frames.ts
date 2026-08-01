import { failure, success, type OperationResult } from "../core/contracts.js";
import { HerdrMathError, serializeError } from "../core/errors.js";
import { POLICY_LIMITS } from "../core/limits.js";

export interface ScrollFramePlan {
  imageWidthPx: number;
  imageHeightPx: number;
  frameHeightPx: number;
  offsetsPx: readonly number[];
  intervalMs: number;
  totalDurationMs: number;
}

export interface ScrollViewport {
  widthPx: number;
  heightPx: number;
}

export interface ScrollFrameOptions {
  startOffsetPx?: number;
}

export function planScrollFrames(
  image: { width: number; height: number },
  viewport: ScrollViewport,
  options: ScrollFrameOptions = {}
): OperationResult<ScrollFramePlan> {
  try {
    assertDimension(image.width, POLICY_LIMITS.imageWidthPx);
    assertDimension(image.height, POLICY_LIMITS.imageHeightPx);
    assertDimension(viewport.widthPx, POLICY_LIMITS.imageWidthPx);
    assertDimension(viewport.heightPx, POLICY_LIMITS.imageHeightPx);

    const scale = Math.min(1, viewport.widthPx / image.width);
    const frameHeightPx = Math.min(image.height, Math.max(1, Math.floor(viewport.heightPx / scale)));
    const maximumOffset = image.height - frameHeightPx;
    const startOffset = Math.min(maximumOffset, Math.max(0, options.startOffsetPx ?? 0));
    const maximumAdvance = Math.max(1, Math.floor(frameHeightPx * 0.75));
    const remainingOffset = maximumOffset - startOffset;
    const frameCount = remainingOffset === 0 ? 1 : Math.ceil(remainingOffset / maximumAdvance) + 1;
    if (frameCount > POLICY_LIMITS.scrollFrameCount) {
      throw limitError("scroll_frame_count", POLICY_LIMITS.scrollFrameCount, frameCount);
    }

    const offsets = Array.from({ length: frameCount }, (_, index) => {
      if (index === frameCount - 1) return maximumOffset;
      return Math.min(maximumOffset, startOffset + index * maximumAdvance);
    });
    const totalDurationMs = (frameCount - 1) * POLICY_LIMITS.scrollFrameIntervalMs;
    if (totalDurationMs > POLICY_LIMITS.scrollAnimationDurationMs) {
      throw limitError("scroll_animation_duration_ms", POLICY_LIMITS.scrollAnimationDurationMs, totalDurationMs);
    }
    return success(
      Object.freeze({
        imageWidthPx: image.width,
        imageHeightPx: image.height,
        frameHeightPx,
        offsetsPx: Object.freeze(offsets),
        intervalMs: POLICY_LIMITS.scrollFrameIntervalMs,
        totalDurationMs
      })
    );
  } catch (error) {
    return failure(serializeError(error));
  }
}

function assertDimension(value: number, maximum: number): void {
  if (!Number.isSafeInteger(value) || value <= 0) throw new HerdrMathError("image_too_large");
  if (value > maximum) throw new HerdrMathError("image_too_large");
}

function limitError(
  limitKind: "scroll_frame_count" | "scroll_animation_duration_ms",
  limit: number,
  actual: number
): HerdrMathError {
  return new HerdrMathError("image_too_large", { limit_kind: limitKind, limit, actual });
}
