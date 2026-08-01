import { describe, expect, it } from "vitest";

import { PLUGIN_ID, PLUGIN_NAME } from "../../src/index.js";

describe("repository baseline", () => {
  it("exposes the canonical plugin identity", () => {
    expect(PLUGIN_ID).toBe("io.github.sodeyama.herdr-math");
    expect(PLUGIN_NAME).toBe("Herdr Math");
  });
});
