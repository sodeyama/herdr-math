import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

import type { SupportedAgent } from "../../src/boundary/fingerprint-schema.js";
import { scanLatex } from "../../src/scanner/scan-latex.js";
import { FullLifecycleRig } from "../support/full-lifecycle-rig.js";

interface ScannerCase {
  id: string;
  agent: SupportedAgent;
  answer: string;
  expected: Array<{ latex: string; display: boolean }>;
}

interface BoundaryCase {
  id: string;
  agent: SupportedAgent;
  baseline: string;
  completion: string;
  expectedAnswer: string;
  readTruncated: boolean;
}

interface AnswerCorpus {
  scannerCases: ScannerCase[];
  boundaryCases: BoundaryCase[];
}

const corpus = JSON.parse(
  readFileSync(new URL("../fixtures/agents/answer-corpus.json", import.meta.url), "utf8")
) as AnswerCorpus;

describe("full formula lifecycle matrix", () => {
  it.each(corpus.scannerCases)("runs scanner case $id through graphics commit", async (testCase) => {
    const rig = await FullLifecycleRig.start(testCase.agent);
    try {
      const result = await rig.runCycle(`Synthetic baseline for ${testCase.id}.\n`, testCase.answer);

      expect(result.working).toMatchObject({
        ok: true,
        value: { kind: "baseline_stored", agent: testCase.agent, generation: 1 }
      });
      if (testCase.expected.length === 0) {
        expect(result.completion).toMatchObject({
          ok: true,
          value: { kind: "completion_recorded", formulaCount: 0 }
        });
        expect(rig.renderedFormulas).toHaveLength(0);
        expect(rig.server.paneCount).toBe(1);
        expect(rig.server.graphicsUpdates).toHaveLength(0);
      } else {
        expect(result.completion).toMatchObject({
          ok: true,
          value: { kind: "image_published", formulaCount: testCase.expected.length }
        });
        expect(rig.renderedFormulas).toEqual([testCase.expected]);
        expect(rig.server.paneCount).toBe(2);
        expect(rig.server.graphicsUpdates).toHaveLength(1);
        expect(rig.server.getPane("w1:p1")?.focused).toBe(true);
      }
    } finally {
      await rig.close();
    }
  });

  it.each(corpus.boundaryCases)("recovers boundary case $id through graphics commit", async (testCase) => {
    const rig = await FullLifecycleRig.start(testCase.agent);
    try {
      const result = await rig.runOutputs(testCase.baseline, testCase.completion, testCase.readTruncated);
      const expected = scanLatex(testCase.expectedAnswer).map(({ latex, display }) => ({ latex, display }));

      expect(result.completion).toMatchObject({
        ok: true,
        value: { kind: "image_published", formulaCount: expected.length }
      });
      expect(rig.renderedFormulas).toEqual([expected]);
      expect(rig.server.getPane("w1:p1")?.focused).toBe(true);
    } finally {
      await rig.close();
    }
  });
});
