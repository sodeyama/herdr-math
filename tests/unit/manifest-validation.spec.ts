import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

import { type ManifestValidationInput, validateManifest } from "../../src/manifest/validate.js";

const manifestSource = readFileSync(new URL("../../herdr-plugin.toml", import.meta.url), "utf8");

const baseline: ManifestValidationInput = {
  manifestSource,
  packageVersion: "0.1.0",
  minimumHerdrVersion: "0.7.5",
  eventKinds: ["pane_agent_status_changed", "pane_closed"],
  pluginPlatforms: ["linux", "macos", "windows"],
  sourceEntrypoints: [
    "src/startup.ts",
    "src/on-agent-status.ts",
    "src/on-pane-closed.ts",
    "src/diagnose.ts",
    "src/viewer.ts"
  ]
};

describe("manifest validation", () => {
  it("accepts the repository manifest", () => {
    expect(validateManifest(baseline)).toEqual([]);
  });

  it("rejects version drift", () => {
    expect(validateManifest({ ...baseline, packageVersion: "0.1.1" })).toContain(
      "Manifest and package versions must agree."
    );
  });

  it("rejects an unknown event", () => {
    const changed = manifestSource.replace('on = "pane.closed"', 'on = "pane.unknown"');
    expect(validateManifest({ ...baseline, manifestSource: changed })).toEqual(
      expect.arrayContaining([
        "Manifest lifecycle events do not match the required event set.",
        "Unknown Herdr event: pane.unknown."
      ])
    );
  });

  it("rejects an unverified platform", () => {
    const changed = manifestSource.replace('platforms = ["macos"]', 'platforms = ["linux"]');
    expect(validateManifest({ ...baseline, manifestSource: changed })).toContain(
      "Manifest platforms must contain only the verified macos target."
    );
  });

  it("rejects absolute and prototype entrypoints", () => {
    const absolute = manifestSource.replace("dist/viewer.js", "/tmp/viewer.js");
    const prototype = manifestSource.replace("dist/viewer.js", "prototype/viewer.js");

    expect(validateManifest({ ...baseline, manifestSource: absolute }).join("\n")).toContain("escapes the plugin root");
    expect(validateManifest({ ...baseline, manifestSource: prototype }).join("\n")).toContain(
      "forbidden prototype path"
    );
  });
});
