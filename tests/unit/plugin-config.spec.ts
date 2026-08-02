import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterEach, describe, expect, it } from "vitest";

import { loadPluginConfig, parsePluginConfig } from "../../src/config/plugin-config.js";

const directories: string[] = [];

afterEach(async () => {
  await Promise.all(directories.splice(0).map((directory) => rm(directory, { recursive: true, force: true })));
});

describe("plugin config", () => {
  it("returns an empty allowlist when config is missing", async () => {
    const directory = await temporaryDirectory();
    await expect(loadPluginConfig(directory)).resolves.toEqual({ allowedDirectories: [] });
  });

  it("loads absolute allowed directories", async () => {
    const directory = await temporaryDirectory();
    await writeFile(
      join(directory, "config.json"),
      JSON.stringify({ allowed_directories: ["/Users/example/docs/obsidian"] })
    );
    await expect(loadPluginConfig(directory)).resolves.toEqual({
      allowedDirectories: ["/Users/example/docs/obsidian"]
    });
  });

  it("rejects relative paths and invalid shapes", () => {
    expect(() => parsePluginConfig({ allowed_directories: ["relative/path"] })).toThrowError(/plugin_config_invalid/);
    expect(() => parsePluginConfig({ allowed_directories: "bad" })).toThrowError(/plugin_config_invalid/);
  });
});

async function temporaryDirectory(): Promise<string> {
  const directory = await mkdtemp(join(tmpdir(), "herdr-math-config-"));
  directories.push(directory);
  return directory;
}
