import { describe, expect, it } from "vitest";

import { isRendererRuntimeAvailable } from "../../src/renderer/runtime-check.js";

describe("renderer runtime check", () => {
  it("detects the locked renderer artifacts on the declared local platform", async () => {
    expect(await isRendererRuntimeAvailable()).toBe(true);
  });
});
