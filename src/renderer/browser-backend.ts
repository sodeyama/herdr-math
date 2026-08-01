import { readFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { dirname, extname, isAbsolute, relative, resolve, sep } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import katex from "katex";
import { chromium, type Browser, type BrowserContext, type Page, type Route } from "playwright";
import sharp from "sharp";

import type { RenderedImage } from "../core/contracts.js";
import { HerdrMathError } from "../core/errors.js";
import {
  assertImageDimensions,
  type RendererBackend,
  type RendererBackendContext,
  type RendererFormula
} from "./render.js";

const require = createRequire(import.meta.url);
const KATEX_CSS_PATH = require.resolve("katex/dist/katex.min.css");
const KATEX_DIST_PATH = dirname(KATEX_CSS_PATH);
const KATEX_FONT_PATH = resolve(KATEX_DIST_PATH, "fonts");
const KATEX_BASE_URL = pathToFileURL(`${KATEX_DIST_PATH}${sep}`).href;
const DENIED_COMMAND = /\\(?:href|url|includegraphics|htmlClass|htmlId|htmlStyle|htmlData)(?=[^A-Za-z]|$)/;
const ALLOWED_FONT_EXTENSIONS = new Set([".woff", ".woff2", ".ttf"]);

export class BrowserRendererBackend implements RendererBackend {
  private browser: Browser | undefined;
  private context: BrowserContext | undefined;
  private page: Page | undefined;
  private closed = false;
  private deniedRequests = 0;
  private closePromise: Promise<void> | undefined;

  async render(formulas: readonly RendererFormula[], renderContext: RendererBackendContext): Promise<RenderedImage> {
    const abort = (): void => void this.close();
    renderContext.signal.addEventListener("abort", abort, { once: true });
    try {
      const markup = buildMarkup(formulas);
      const css = await readFile(KATEX_CSS_PATH, "utf8");
      await this.assertActive(renderContext);
      this.browser = await chromium.launch({ headless: true, timeout: remainingMs(renderContext.deadlineMs) });
      await this.assertActive(renderContext);
      this.context = await this.browser.newContext({
        javaScriptEnabled: false,
        offline: true,
        serviceWorkers: "block",
        viewport: { width: renderContext.limits.imageWidthPx, height: 4096 }
      });
      await this.assertActive(renderContext);
      await this.context.route("**/*", async (route) => this.routeLocalFont(route));
      this.page = await this.context.newPage();
      await this.assertActive(renderContext);
      this.page.setDefaultTimeout(remainingMs(renderContext.deadlineMs));
      await this.page.setContent(pageHtml(css, markup), {
        waitUntil: "load",
        timeout: remainingMs(renderContext.deadlineMs)
      });
      await this.page.evaluate("document.fonts.ready");
      await this.assertActive(renderContext);
      if (this.deniedRequests > 0) throw new HerdrMathError("renderer_failed", {}, true);

      const target = this.page.locator("#render");
      const bounds = await target.boundingBox({ timeout: remainingMs(renderContext.deadlineMs) });
      if (bounds === null) throw new HerdrMathError("renderer_failed", {}, true);
      assertImageDimensions(Math.ceil(bounds.width), Math.ceil(bounds.height), renderContext.limits);
      const screenshot = await target.screenshot({
        type: "png",
        animations: "disabled",
        caret: "hide",
        timeout: remainingMs(renderContext.deadlineMs)
      });
      const sharpRemainingMs = remainingMs(renderContext.deadlineMs);
      if (sharpRemainingMs < 1000) throw new HerdrMathError("renderer_timeout", {}, true);
      const output = await sharp(screenshot, { limitInputPixels: renderContext.limits.imagePixels })
        .timeout({ seconds: Math.floor(sharpRemainingMs / 1000) })
        .png({ adaptiveFiltering: true, compressionLevel: 9 })
        .toBuffer({ resolveWithObject: true });
      return {
        buffer: output.data,
        width: output.info.width,
        height: output.info.height,
        bytes: output.data.byteLength,
        renderer: "katex-playwright-sharp"
      };
    } finally {
      renderContext.signal.removeEventListener("abort", abort);
    }
  }

  async close(): Promise<void> {
    this.closed = true;
    do {
      this.closePromise ??= this.closeCurrentResources();
      await this.closePromise;
    } while (this.hasOpenResources());
  }

  hasOpenResources(): boolean {
    return this.page !== undefined || this.context !== undefined || this.browser !== undefined;
  }

  private async closeCurrentResources(): Promise<void> {
    const page = this.page;
    const context = this.context;
    const browser = this.browser;
    this.page = undefined;
    this.context = undefined;
    this.browser = undefined;
    try {
      let failed = false;
      for (const closeResource of [
        async () => page?.close({ runBeforeUnload: false }),
        async () => context?.close(),
        async () => browser?.close()
      ]) {
        try {
          await closeResource();
        } catch {
          failed = true;
        }
      }
      if (failed) throw new Error("Renderer cleanup failed");
    } finally {
      this.closePromise = undefined;
    }
  }

  private async assertActive(renderContext: RendererBackendContext): Promise<void> {
    if (this.closed || renderContext.signal.aborted || Date.now() >= renderContext.deadlineMs) {
      await this.close();
      throw new HerdrMathError("renderer_timeout", {}, true);
    }
  }

  private async routeLocalFont(route: Route): Promise<void> {
    const url = route.request().url();
    if (isAllowedFontUrl(url)) {
      await route.continue();
      return;
    }
    this.deniedRequests += 1;
    await route.abort("blockedbyclient");
  }
}

function buildMarkup(formulas: readonly RendererFormula[]): string {
  try {
    return formulas
      .map((formula) => {
        if (DENIED_COMMAND.test(formula.latex) || formula.latex.includes("\0")) {
          throw new HerdrMathError("invalid_latex");
        }
        return katex.renderToString(formula.latex, {
          displayMode: formula.display,
          throwOnError: true,
          trust: false,
          strict: (code) => (code === "unicodeTextInMathMode" ? "ignore" : "error"),
          maxSize: 50,
          maxExpand: 1000,
          macros: {},
          output: "html"
        });
      })
      .join("");
  } catch {
    throw new HerdrMathError("invalid_latex");
  }
}

function pageHtml(css: string, markup: string): string {
  return `<!doctype html><html><head><base href="${KATEX_BASE_URL}"><style>${css}\nhtml,body{margin:0;background:#fff}#render{box-sizing:border-box;display:flex;flex-direction:column;gap:12px;width:max-content;max-width:4096px;padding:20px;color:#111;font-size:24px}.katex-display{margin:0}</style></head><body><div id="render">${markup}</div></body></html>`;
}

function isAllowedFontUrl(url: string): boolean {
  try {
    const parsed = new URL(url);
    if (parsed.protocol !== "file:") return false;
    const path = fileURLToPath(parsed);
    const relation = relative(KATEX_FONT_PATH, path);
    return (
      relation !== "" &&
      !relation.startsWith(`..${sep}`) &&
      !isAbsolute(relation) &&
      ALLOWED_FONT_EXTENSIONS.has(extname(path))
    );
  } catch {
    return false;
  }
}

function remainingMs(deadlineMs: number): number {
  return Math.max(1, deadlineMs - Date.now());
}
