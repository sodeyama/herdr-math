import { Buffer } from "node:buffer";
import { readFile, readdir } from "node:fs/promises";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

import { renderFormulas } from "../../src/renderer/index.js";
import { FullLifecycleRig, timeoutRenderer } from "../support/full-lifecycle-rig.js";

describe("privacy gates across state, output, and request logs", () => {
  it("keeps answer, formula, path, and environment sentinels out of durable and observable output", async () => {
    const rig = await FullLifecycleRig.start("claude");
    const sentinels = [
      "PRIVATE_ANSWER_SENTINEL_Q7Z9",
      "PRIVATE_FORMULA_SENTINEL_Q7Z9",
      "/synthetic/private/home/Q7Z9",
      "PRIVATE_ENVIRONMENT_SECRET_Q7Z9"
    ];
    try {
      const baseline = `Synthetic history ${sentinels[0]} ${sentinels[2]} ${sentinels[3]}\n`;
      const result = await rig.runCycle(baseline, `Result $\\frac{${sentinels[1]}}{2}$.`);
      expect(result.completion).toMatchObject({ ok: true, value: { kind: "image_published" } });

      const durable = await readTree(rig.stateDirectory);
      const observable = JSON.stringify({ result, requests: rig.server.requests });
      for (const sentinel of sentinels) {
        for (const encoded of [
          sentinel,
          Buffer.from(sentinel).toString("base64"),
          Buffer.from(sentinel).toString("hex"),
          encodeURIComponent(sentinel)
        ]) {
          expect(durable).not.toContain(encoded);
          expect(observable).not.toContain(encoded);
        }
      }
    } finally {
      await rig.close();
    }
  });

  it("keeps sentinel formula text out of invalid and timeout results", async () => {
    const rig = await FullLifecycleRig.start("codex");
    const invalidSentinel = "PRIVATE_INVALID_FORMULA_Q7Z9";
    const timeoutSentinel = "PRIVATE_TIMEOUT_FORMULA_Q7Z9";
    try {
      rig.renderer = renderFormulas;
      const invalid = await rig.runCycle(
        "Invalid privacy baseline.\n",
        `Invalid $\\notARealCommand{${invalidSentinel}}$.`
      );
      expect(invalid.completion).toMatchObject({ ok: false, error: { code: "invalid_latex" } });

      rig.renderer = timeoutRenderer();
      const timeout = await rig.runCycle("Timeout privacy baseline.\n", `Slow $x_{${timeoutSentinel}}$.`);
      expect(timeout.completion).toMatchObject({ ok: false, error: { code: "renderer_timeout" } });

      const serialized = JSON.stringify({ invalid, timeout, requests: rig.server.requests });
      expect(serialized).not.toContain(invalidSentinel);
      expect(serialized).not.toContain(timeoutSentinel);
      const durable = await readTree(rig.stateDirectory);
      expect(durable).not.toContain(invalidSentinel);
      expect(durable).not.toContain(timeoutSentinel);
    } finally {
      await rig.close();
    }
  }, 30_000);
});

async function readTree(directory: string): Promise<string> {
  const contents: string[] = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) contents.push(await readTree(path));
    else if (entry.isFile()) contents.push(await readFile(path, "utf8"));
  }
  return contents.join("\n");
}
