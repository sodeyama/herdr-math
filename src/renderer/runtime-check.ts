import { constants } from "node:fs";
import { access } from "node:fs/promises";
import { createRequire } from "node:module";
import { dirname, resolve } from "node:path";

const require = createRequire(import.meta.url);

export async function isRendererRuntimeAvailable(): Promise<boolean> {
  try {
    const katexCss = require.resolve("katex/dist/katex.min.css");
    const playwrightCore = dirname(require.resolve("playwright-core/package.json"));
    require.resolve("sharp");
    const architectureDirectory =
      process.platform === "darwin" && process.arch === "arm64"
        ? "chrome-headless-shell-mac-arm64"
        : process.platform === "darwin" && process.arch === "x64"
          ? "chrome-headless-shell-mac-x64"
          : undefined;
    if (architectureDirectory === undefined) return false;
    const browser = resolve(
      playwrightCore,
      ".local-browsers/chromium_headless_shell-1234",
      architectureDirectory,
      "chrome-headless-shell"
    );
    await Promise.all([
      access(katexCss, constants.R_OK),
      access(resolve(dirname(katexCss), "fonts"), constants.R_OK),
      access(browser, constants.R_OK | constants.X_OK)
    ]);
    return true;
  } catch {
    return false;
  }
}
