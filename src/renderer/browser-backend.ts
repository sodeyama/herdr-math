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
  type RendererLimits
} from "./render.js";
import type { RendererDocument, RendererDocumentSegment } from "./document.js";
import { resolveRendererLayout, type RendererLayout } from "./layout.js";
import { MARKDOWN_CSS_PATH, renderMarkup } from "./markdown.js";

const require = createRequire(import.meta.url);
const KATEX_CSS_PATH = require.resolve("katex/dist/katex.min.css");
const KATEX_DIST_PATH = dirname(KATEX_CSS_PATH);
const KATEX_FONT_PATH = resolve(KATEX_DIST_PATH, "fonts");
const KATEX_BASE_URL = pathToFileURL(`${KATEX_DIST_PATH}${sep}`).href;
const PLAYWRIGHT_CORE_PATH = dirname(require.resolve("playwright-core/package.json"));
const BROWSER_ARCH_DIRECTORY =
  process.platform === "darwin" && process.arch === "arm64"
    ? "chrome-headless-shell-mac-arm64"
    : process.platform === "darwin" && process.arch === "x64"
      ? "chrome-headless-shell-mac-x64"
      : undefined;
const BROWSER_EXECUTABLE_PATH =
  BROWSER_ARCH_DIRECTORY === undefined
    ? undefined
    : resolve(
        PLAYWRIGHT_CORE_PATH,
        ".local-browsers/chromium_headless_shell-1234",
        BROWSER_ARCH_DIRECTORY,
        "chrome-headless-shell"
      );
const DENIED_COMMAND = /\\(?:href|url|includegraphics|htmlClass|htmlId|htmlStyle|htmlData)(?=[^A-Za-z]|$)/;
const ALLOWED_FONT_EXTENSIONS = new Set([".woff", ".woff2", ".ttf"]);

export class BrowserRendererBackend implements RendererBackend {
  private browser: Browser | undefined;
  private context: BrowserContext | undefined;
  private page: Page | undefined;
  private closed = false;
  private deniedRequests = 0;
  private closePromise: Promise<void> | undefined;

  async render(document: RendererDocument, renderContext: RendererBackendContext): Promise<RenderedImage> {
    const abort = (): void => void this.close();
    renderContext.signal.addEventListener("abort", abort, { once: true });
    try {
      const css = await readFile(KATEX_CSS_PATH, "utf8");
      const markdownCss = await readFile(MARKDOWN_CSS_PATH, "utf8");
      // Build and validate the math-only probe before launching the browser so invalid
      // input still fails before any browser startup. The probe measures the widest
      // unbroken math so the final canvas is never made wider than the content needs,
      // which prevents wide formulas from overflowing (and being clipped).
      const probeHtml = buildProbeHtml(document, css, renderContext.layout);
      if (BROWSER_EXECUTABLE_PATH === undefined) throw new HerdrMathError("renderer_failed", {}, true);
      await this.assertActive(renderContext);
      this.browser = await chromium.launch({
        executablePath: BROWSER_EXECUTABLE_PATH,
        headless: true,
        timeout: remainingMs(renderContext.deadlineMs)
      });
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
      const measuredWidth = await this.measure(probeHtml, renderContext);
      const layout = fittedLayout(renderContext.layout, renderContext.limits, measuredWidth);
      const html = buildRendererHtml(document, css, layout, markdownCss);
      await this.page.setContent(html, {
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
        omitBackground: true,
        timeout: remainingMs(renderContext.deadlineMs)
      });
      const sharpRemainingMs = remainingMs(renderContext.deadlineMs);
      if (sharpRemainingMs < 1000) throw new HerdrMathError("renderer_timeout", {}, true);
      const output = await sharp(screenshot, { limitInputPixels: renderContext.limits.imagePixels })
        .timeout({ seconds: Math.floor(sharpRemainingMs / 1000) })
        .png({
          adaptiveFiltering: true,
          compressionLevel: 9,
          palette: true,
          quality: 100,
          colours: 256,
          dither: 0
        })
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

  private async measure(probeHtml: string, renderContext: RendererBackendContext): Promise<number> {
    const page = this.page;
    if (page === undefined) throw new HerdrMathError("renderer_failed", {}, true);
    await page.setContent(probeHtml, {
      waitUntil: "load",
      timeout: remainingMs(renderContext.deadlineMs)
    });
    await page.evaluate("document.fonts.ready");
    await this.assertActive(renderContext);
    const box = await page.locator("#render").boundingBox({ timeout: remainingMs(renderContext.deadlineMs) });
    return Math.max(1, Math.ceil(box?.width ?? 0));
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

function buildProbeMarkup(segments: readonly RendererDocumentSegment[]): string {
  try {
    return segments
      .map((segment) => {
        if (segment.kind === "text") return escapeHtml(segment.text);
        if (DENIED_COMMAND.test(segment.latex) || segment.latex.includes("\0")) {
          throw new HerdrMathError("invalid_latex");
        }
        const rendered = katex.renderToString(segment.latex, {
          displayMode: segment.display,
          throwOnError: true,
          trust: false,
          strict: (code) => (code === "unicodeTextInMathMode" ? "ignore" : "error"),
          maxSize: 50,
          maxExpand: 1000,
          macros: {},
          output: "html"
        });
        return segment.display
          ? `<span class="math-display">${rendered}</span>`
          : `<span class="math-inline">${rendered}</span>`;
      })
      .join("");
  } catch {
    throw new HerdrMathError("invalid_latex");
  }
}

export function buildRendererHtml(
  document: RendererDocument,
  css: string,
  layout: Readonly<RendererLayout> = resolveRendererLayout(),
  markdownCss = ""
): string {
  const markup = renderMarkup(document.segments);
  const style = `${css}${markdownCss}\n${baseStyle(layout)}`;
  return `<!doctype html><html><head><base href="${KATEX_BASE_URL}"><style>${style}</style></head><body><div id="render">${markup}</div></body></html>`;
}

function buildProbeHtml(document: RendererDocument, css: string, layout: Readonly<RendererLayout>): string {
  const probes = document.segments.filter((segment) => segment.kind === "math");
  const markup = probes.map((segment) => `<div class="probe">${buildProbeMarkup([segment])}</div>`).join("");
  return `<!doctype html><html><head><base href="${KATEX_BASE_URL}"><style>${css}\nhtml,body{margin:0;background:transparent}#render{box-sizing:border-box;width:max-content;min-width:1px;padding:${layout.paddingPx}px;color:#e6edf3;background:transparent;font-family:ui-sans-serif,-apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif;font-size:${layout.fontSizePx}px;line-height:1.65}.probe{display:block;min-height:1.65em;white-space:nowrap}.katex{font-size:1em;line-height:normal}</style></head><body><div id="render">${markup}</div></body></html>`;
}

function baseStyle(layout: Readonly<RendererLayout>): string {
  return `html,body{margin:0;background:transparent}#render{box-sizing:border-box;width:${layout.contentWidthPx}px;min-height:1px;padding:${layout.paddingPx}px;color:#e6edf3;background:transparent;font-family:ui-sans-serif,-apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif;font-size:${layout.fontSizePx}px;line-height:1.65;word-break:break-word;overflow-wrap:break-word}.katex{font-size:1em;line-height:normal}.math-inline{display:inline-block;max-width:100%;vertical-align:baseline;white-space:nowrap}.math-display{display:block;margin:.65em 0;text-align:center;white-space:normal}.katex-display{margin:0}#render h1,#render h2,#render h3,#render h4,#render h5,#render h6{margin:0.9em 0 0.5em;line-height:1.3}#render h1{font-size:1.5em}#render h2{font-size:1.3em}#render h3{font-size:1.15em}#render h4{font-size:1em}#render p{margin:0.45em 0}#render ul,#render ol{padding-left:1.5em;margin:0.45em 0}#render li{margin:0.15em 0}#render blockquote{border-left:3px solid #6e7681;margin:0.45em 0;padding:0.1em 0.8em;color:#8b949e}#render hr{border:none;border-top:1px solid #30363d;margin:0.8em 0}#render code{font-family:ui-monospace,SFMono-Regular,Menlo,Consolas,monospace}#render :not(pre)>code{background:#161b22;border-radius:4px;padding:0.1em 0.35em;font-size:0.9em}#render pre{background:#161b22;border-radius:6px;padding:0.6em 0.8em;overflow-x:auto;line-height:1.45}#render pre code{background:transparent;padding:0;font-size:0.9em}#render table{border-collapse:collapse;margin:0.6em 0;max-width:100%;display:block;overflow-x:auto}#render th,#render td{border:1px solid #30363d;padding:0.3em 0.6em;text-align:left}#render th{background:#161b22;font-weight:600}#render a{color:inherit;text-decoration:none;cursor:default}.markdown-link-url{color:#58a6ff;font-size:0.85em}.markdown-image{color:#8b949e;font-style:italic}`;
}

function fittedLayout(
  requested: Readonly<RendererLayout>,
  limits: Pick<RendererLimits, "imageWidthPx">,
  probedWidth: number
): Readonly<RendererLayout> {
  const top = Math.min(requested.contentWidthPx, limits.imageWidthPx);
  const contentWidthPx = Math.min(limits.imageWidthPx, Math.max(top, probedWidth));
  return resolveRendererLayout({ ...requested, contentWidthPx });
}

function escapeHtml(value: string): string {
  return value.replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;");
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
