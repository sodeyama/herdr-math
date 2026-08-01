import { Buffer } from "node:buffer";
import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";
import sharp from "sharp";

import type { SupportedAgent } from "../../src/boundary/fingerprint-schema.js";
import { renderResponse } from "../../src/renderer/index.js";
import { scanLatex } from "../../src/scanner/scan-latex.js";
import { FullLifecycleRig } from "../support/full-lifecycle-rig.js";

interface PresentationCase {
  id: string;
  agent: SupportedAgent;
  plain: string;
  ansi: string;
  expected: string;
  expectedFormulas: string[];
}

const corpus = JSON.parse(
  readFileSync(new URL("../fixtures/presentation/final-response-corpus.json", import.meta.url), "utf8")
) as { cases: PresentationCase[] };

describe("integrated final response presentation", () => {
  it.each(corpus.cases)(
    "presents only the final response for $id",
    async (testCase) => {
      const rig = await FullLifecycleRig.start(testCase.agent);
      rig.renderer = ({ text, formulas }) => renderResponse(text, formulas);
      try {
        const baseline = `Synthetic ${testCase.agent} presentation baseline.\n`;
        const result = await rig.runStyledOutputs(
          baseline,
          `${baseline}${testCase.plain}`,
          `${baseline}${testCase.ansi}`
        );
        const expected = scanLatex(testCase.expected);
        if (expected.length === 0) {
          expect(result.completion).toMatchObject({
            ok: true,
            value: { kind: "completion_recorded", formulaCount: 0 }
          });
          expect(rig.renderedResponses).toHaveLength(0);
          expect(rig.server.graphicsUpdates).toHaveLength(0);
          return;
        }

        expect(result.completion).toMatchObject({
          ok: true,
          value: { kind: "image_published", formulaCount: expected.length }
        });
        expect(rig.renderedResponses).toEqual([testCase.expected]);
        expect(rig.renderedFormulas).toEqual([expected.map(({ latex, display }) => ({ latex, display }))]);
        expect(rig.server.graphicsUpdates).toHaveLength(1);
        const update = rig.server.graphicsUpdates[0];
        if (update === undefined) throw new Error("Expected one graphics update");
        const statistics = await sharp(Buffer.from(update.data_base64, "base64")).ensureAlpha().stats();
        expect(statistics.channels[3]?.min).toBe(0);
        expect(statistics.channels[3]?.max).toBe(255);
        expect(rig.server.getPane("w1:p1")?.focused).toBe(true);
      } finally {
        await rig.close();
      }
    },
    30_000
  );

  it("scrolls a long final message to its bottom through prevalidated frames", async () => {
    const rig = await FullLifecycleRig.start("codex");
    rig.renderer = ({ text, formulas }) => renderResponse(text, formulas);
    try {
      const prose = Array.from(
        { length: 36 },
        (_, index) =>
          `Paragraph ${index + 1} explains the result in enough detail to wrap across the response viewer width.`
      ).join("\n\n");
      const answer = `${prose}\n\nThe final relation is $$a^2+b^2=c^2$$.`;
      const result = await rig.runCycle("Long response baseline.\n", answer);

      expect(result.completion).toMatchObject({ ok: true, value: { kind: "image_published", formulaCount: 1 } });
      expect(rig.renderedResponses).toEqual([answer]);
      expect(rig.server.graphicsUpdates.length).toBeGreaterThan(1);
      expect(rig.server.getGraphics("w1:p2")).toEqual(rig.server.graphicsUpdates.at(-1));
      expect(rig.server.requests.some(({ method }) => method === "pane.graphics.clear")).toBe(false);
      expect(rig.server.getPane("w1:p1")?.focused).toBe(true);
    } finally {
      await rig.close();
    }
  }, 30_000);
});
