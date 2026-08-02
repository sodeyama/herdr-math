import { describe, expect, it } from "vitest";

import { renderResponse } from "../../src/renderer/index.js";
import { FullLifecycleRig, renderStatic, timeoutRenderer } from "../support/full-lifecycle-rig.js";

describe("full formula lifecycle recovery", () => {
  it("preserves a valid viewer across invalid, limit, timeout, and recovery generations", async () => {
    const rig = await FullLifecycleRig.start("codex");
    try {
      const first = await rig.runCycle("Initial response baseline.\n", "Valid formula $x=1$.");
      expect(first.completion).toMatchObject({
        ok: true,
        value: { kind: "image_published", viewerPaneId: "w1:p2" }
      });
      await rig.registerViewer("w1:p2");
      const previous = rig.server.getGraphics("w1:p2");

      rig.renderer = ({ text, formulas, layout }) => renderResponse(text, formulas, { layout });
      const invalid = await rig.runCycle("Invalid response baseline.\n", "Invalid $\\notARealCommand{x}$.");
      expect(invalid.completion).toEqual({
        ok: false,
        error: { code: "invalid_latex", retryable: false }
      });
      expect(rig.server.getGraphics("w1:p2")).toEqual(previous);

      rig.renderer = renderStatic;
      const countLimit = await rig.runCycle(
        "Count limit baseline.\n",
        Array.from({ length: 21 }, (_, index) => `$x_${index}$`).join(" ")
      );
      expect(countLimit.completion).toMatchObject({
        ok: false,
        error: { code: "scanner_input_limit", details: { limit_kind: "formula_count" } }
      });
      expect(rig.server.getGraphics("w1:p2")).toEqual(previous);

      const aggregateLimit = await rig.runCycle(
        "Aggregate limit baseline.\n",
        Array.from({ length: 6 }, () => `$${"x".repeat(1667)}$`).join(" ")
      );
      expect(aggregateLimit.completion).toMatchObject({
        ok: false,
        error: { code: "renderer_input_limit", details: { limit_kind: "aggregate_formula_characters" } }
      });
      expect(rig.server.getGraphics("w1:p2")).toEqual(previous);

      rig.renderer = timeoutRenderer();
      const timedOut = await rig.runCycle("Timeout baseline.\n", "Slow formula $y=2$.");
      expect(timedOut.completion).toMatchObject({ ok: false, error: { code: "renderer_timeout", retryable: true } });
      expect(rig.server.getGraphics("w1:p2")).toEqual(previous);

      rig.renderer = renderStatic;
      const recovered = await rig.runCycle("Recovery baseline.\n", "Recovered formula $z=3$.");
      expect(recovered.completion).toMatchObject({
        ok: true,
        value: { kind: "image_published", viewerPaneId: "w1:p2" }
      });
      expect(rig.server.requests.filter(({ method }) => method === "pane.graphics.set").length).toBeGreaterThanOrEqual(
        2
      );
      expect(rig.server.requests.some(({ method }) => method === "pane.graphics.clear")).toBe(false);
      expect(rig.server.paneCount).toBe(2);
      expect(rig.server.getPane("w1:p1")?.focused).toBe(true);
      expect(await rig.state()).toMatchObject({ generation: 6, viewer_pane_id: "w1:p2", processed: {} });
    } finally {
      await rig.close();
    }
  }, 30_000);

  it("recomputes placement after resize and recreates a closed viewer once", async () => {
    const rig = await FullLifecycleRig.start("claude");
    try {
      const first = await rig.runCycle("First baseline.\n", "First $x=1$.");
      expect(first.completion).toMatchObject({ ok: true, value: { viewerPaneId: "w1:p2" } });
      await rig.registerViewer("w1:p2");

      rig.server.setPaneRect("w1:p2", { x: 80, y: 0, width: 40, height: 10 });
      const resized = await rig.runCycle("Resize baseline.\n", "Second $x=2$.");
      expect(resized.completion).toMatchObject({ ok: true, value: { viewerPaneId: "w1:p2" } });
      expect(rig.server.getGraphics("w1:p2")?.placement).toEqual({
        viewport_col: 0,
        viewport_row: -31,
        grid_cols: 40,
        grid_rows: 41
      });

      expect(rig.server.closePane("w1:p2")).toBe(true);
      const recreated = await rig.runCycle("Closed viewer baseline.\n", "Third $x=3$.");
      expect(recreated.completion).toMatchObject({ ok: true, value: { kind: "image_published" } });
      if (!recreated.completion.ok || recreated.completion.value.kind !== "image_published") {
        throw new Error("Expected a recreated viewer");
      }
      expect(recreated.completion.value.viewerPaneId).not.toBe("w1:p2");
      expect(rig.server.getGraphics(recreated.completion.value.viewerPaneId)).toBeDefined();
      expect(rig.server.paneCount).toBe(2);
      expect(rig.server.requests.filter(({ method }) => method === "plugin.pane.open")).toHaveLength(2);
      expect(rig.server.getPane("w1:p1")?.focused).toBe(true);
    } finally {
      await rig.close();
    }
  });

  it("records no-formula completion and suppresses duplicate idle delivery", async () => {
    const rig = await FullLifecycleRig.start("opencode");
    try {
      const result = await rig.runCycle("No-formula baseline.\n", "Price $10 and shell $HOME remain plain text.");
      expect(result.completion).toMatchObject({
        ok: true,
        value: { kind: "completion_recorded", formulaCount: 0 }
      });
      const duplicate = await rig.process(rig.server.transitionPane("w1:p1", "idle"));
      expect(duplicate).toMatchObject({
        ok: true,
        value: { kind: "preserved", reason: "duplicate_completion" }
      });
      expect(rig.server.paneCount).toBe(1);
      expect(rig.server.graphicsUpdates).toHaveLength(0);
    } finally {
      await rig.close();
    }
  });
});
