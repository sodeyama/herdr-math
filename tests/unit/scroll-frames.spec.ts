import { describe, expect, it } from "vitest";

import { POLICY_LIMITS } from "../../src/core/limits.js";
import { planScrollFrames } from "../../src/viewer/scroll-frames.js";

describe("bounded viewer scroll planning", () => {
  it("uses one immediate frame when the response fits the viewport", () => {
    expect(planScrollFrames({ width: 480, height: 300 }, { widthPx: 480, heightPx: 400 })).toEqual({
      ok: true,
      value: {
        imageWidthPx: 480,
        imageHeightPx: 300,
        frameHeightPx: 300,
        offsetsPx: [0],
        intervalMs: POLICY_LIMITS.scrollFrameIntervalMs,
        totalDurationMs: 0
      }
    });
  });

  it("plans monotonic overlapping frames that finish at the bottom", () => {
    const result = planScrollFrames({ width: 480, height: 1800 }, { widthPx: 480, heightPx: 400 });
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.value.offsetsPx[0]).toBe(0);
    expect(result.value.offsetsPx.at(-1)).toBe(1400);
    expect(result.value.offsetsPx.length).toBeGreaterThan(1);
    for (let index = 1; index < result.value.offsetsPx.length; index += 1) {
      const previous = result.value.offsetsPx[index - 1] ?? -1;
      const current = result.value.offsetsPx[index] ?? -1;
      expect(current).toBeGreaterThan(previous);
      expect(current - previous).toBeLessThan(result.value.frameHeightPx);
    }
    expect(result.value.totalDurationMs).toBeLessThanOrEqual(POLICY_LIMITS.scrollAnimationDurationMs);
  });

  it("accounts for width scaling when deriving the source crop height", () => {
    const result = planScrollFrames({ width: 480, height: 1200 }, { widthPx: 240, heightPx: 300 });
    expect(result).toMatchObject({ ok: true, value: { frameHeightPx: 600, offsetsPx: [0, 450, 600] } });
  });

  it("rejects a document that cannot retain overlap within the frame budget", () => {
    const result = planScrollFrames(
      { width: 480, height: POLICY_LIMITS.imageHeightPx },
      { widthPx: 480, heightPx: 200 }
    );
    expect(result).toMatchObject({
      ok: false,
      error: { code: "image_too_large", details: { limit_kind: "scroll_frame_count" } }
    });
  });

  it("continues from a prior resting offset when appending more content", () => {
    const result = planScrollFrames(
      { width: 480, height: 1800 },
      { widthPx: 480, heightPx: 400 },
      { startOffsetPx: 1400 }
    );
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.value.offsetsPx).toEqual([1400]);
    expect(result.value.totalDurationMs).toBe(0);
  });
});
