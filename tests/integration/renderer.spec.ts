import { Buffer } from "node:buffer";
import { readFileSync } from "node:fs";

import { chromium } from "playwright";
import sharp from "sharp";
import { describe, expect, it, vi } from "vitest";

import type { Formula, RenderedImage } from "../../src/core/contracts.js";
import { POLICY_LIMITS } from "../../src/core/limits.js";
import { BrowserRendererBackend, buildRendererHtml } from "../../src/renderer/browser-backend.js";
import { composeRendererDocument, type RendererDocument } from "../../src/renderer/document.js";
import { renderFormulas, renderResponse } from "../../src/renderer/index.js";
import {
  renderWithBackend,
  type RendererBackend,
  type RendererBackendContext,
  type RendererFormula,
  type RendererLimits
} from "../../src/renderer/render.js";
import { scanLatex } from "../../src/scanner/scan-latex.js";

interface RendererCorpus {
  validCases: Array<{ formulas: RendererFormula[] }>;
  invalidCases: Array<{ formula: RendererFormula }>;
  limitCases: Array<Record<string, string | number>>;
  securityCases: Array<{ formula: RendererFormula }>;
}

const corpus = JSON.parse(
  readFileSync(new URL("../fixtures/renderer/formula-corpus.json", import.meta.url), "utf8")
) as RendererCorpus;
const simpleFormula: RendererFormula = { latex: "x^2+y^2=z^2", display: true };
const pngSignature = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);

describe("bounded local renderer", () => {
  it("renders escaped prose and math on a transparent canvas at one base font size", async () => {
    const text = "Before <unsafe> $E=mc^2$.\n\nThen:\n$$a^2+b^2=c^2$$\nAfter.";
    const formulas = scanLatex(text);
    const document = composeRendererDocument(text, formulas);
    expect(document.ok).toBe(true);
    if (!document.ok) return;

    const html = buildRendererHtml(document.value, "/* katex */");
    expect(html).toContain("Before &lt;unsafe&gt;");
    expect(html).not.toContain("Before <unsafe>");
    expect(html).toContain("background:transparent");
    expect(html).toContain("font-size:20px");
    expect(html).toContain(".katex{font-size:1em}");

    const result = await renderResponse(text, formulas);
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    const statistics = await sharp(result.value.buffer).ensureAlpha().stats();
    const alpha = statistics.channels[3];
    expect(alpha?.min).toBe(0);
    expect(alpha?.max).toBe(255);
  }, 30_000);

  it("renders the release corpus into a meaningful bounded PNG and closes browser resources", async () => {
    const formulas = corpus.validCases.flatMap((testCase) => testCase.formulas);
    const backend = new BrowserRendererBackend();
    const result = await renderWithBackend(formulas, backend);

    expect(result.ok).toBe(true);
    if (!result.ok) return;
    const metadata = await sharp(result.value.buffer).metadata();
    const statistics = await sharp(result.value.buffer).stats();
    expect(metadata.format).toBe("png");
    expect(result.value).toMatchObject({
      width: metadata.width,
      height: metadata.height,
      bytes: result.value.buffer.byteLength,
      renderer: "katex-playwright-sharp"
    });
    expect(result.value.width).toBeLessThanOrEqual(POLICY_LIMITS.imageWidthPx);
    expect(result.value.height).toBeLessThanOrEqual(POLICY_LIMITS.imageHeightPx);
    expect(result.value.bytes).toBeLessThanOrEqual(POLICY_LIMITS.rawPngBytes);
    expect(statistics.channels.some((channel) => channel.stdev > 1)).toBe(true);
    expect(backend.hasOpenResources()).toBe(false);
  }, 30_000);

  it("rejects malformed and link-capable input before browser startup without leaking source", async () => {
    const launch = vi.spyOn(chromium, "launch");
    const failures = [...corpus.invalidCases, ...corpus.securityCases];
    for (const testCase of failures) {
      const backend = new BrowserRendererBackend();
      const result = await renderWithBackend([testCase.formula], backend);
      expect(result).toEqual({ ok: false, error: { code: "invalid_latex", retryable: false } });
      expect(JSON.stringify(result)).not.toContain(testCase.formula.latex);
      expect(backend.hasOpenResources()).toBe(false);
    }
    expect(launch).not.toHaveBeenCalled();
    launch.mockRestore();
  });

  it("rejects count, per-formula, and aggregate limits before backend work", async () => {
    const cases: RendererFormula[][] = [
      Array.from({ length: POLICY_LIMITS.formulasPerAnswer + 1 }, () => simpleFormula),
      [{ latex: "x".repeat(POLICY_LIMITS.charactersPerFormula + 1), display: true }],
      Array.from({ length: 6 }, () => ({ latex: "x".repeat(1667), display: true }))
    ];

    for (const formulas of cases) {
      const backend = new StaticBackend(validImage());
      const result = await renderWithBackend(formulas, backend);
      expect(result.ok).toBe(false);
      if (result.ok) continue;
      expect(result.error.code).toBe("renderer_input_limit");
      expect(backend.renderCalls).toBe(0);
      expect(backend.closeCalls).toBe(1);
    }
  });

  it("cancels a timed-out backend, closes it, and permits the next real render", async () => {
    const backend = new BlockingBackend();
    const timedOut = await renderWithBackend([simpleFormula], backend, { limits: { renderDurationMs: 20 } });
    expect(timedOut).toEqual({
      ok: false,
      error: {
        code: "renderer_timeout",
        retryable: true,
        details: { limit_kind: "render_duration_ms", limit: 20, actual: 20 }
      }
    });
    expect(backend.aborted).toBe(true);
    expect(backend.closeCalls).toBe(1);

    const recovery = await renderFormulas([simpleFormula]);
    expect(recovery.ok).toBe(true);
  }, 30_000);

  it("rejects dimension, raw-byte, encoded-byte, and malformed backend output", async () => {
    const cases: Array<{ image: RenderedImage; limits?: Partial<RendererLimits>; code: string; kind?: string }> = [
      {
        image: validImage({ width: POLICY_LIMITS.imageWidthPx + 1 }),
        code: "image_too_large",
        kind: "image_width_px"
      },
      {
        image: validImage({ buffer: pngBuffer(POLICY_LIMITS.rawPngBytes + 1) }),
        code: "image_too_large",
        kind: "raw_png_bytes"
      },
      {
        image: validImage({ buffer: pngBuffer(20) }),
        limits: { rawPngBytes: 100, base64PayloadBytes: 27 },
        code: "image_too_large",
        kind: "base64_payload_bytes"
      },
      { image: validImage({ buffer: Buffer.from("not a png") }), code: "renderer_failed" }
    ];

    for (const testCase of cases) {
      const backend = new StaticBackend(testCase.image);
      const result =
        testCase.limits === undefined
          ? await renderWithBackend([simpleFormula], backend)
          : await renderWithBackend([simpleFormula], backend, { limits: testCase.limits });
      expect(result.ok).toBe(false);
      if (result.ok) continue;
      expect(result.error.code).toBe(testCase.code);
      if (testCase.kind !== undefined) expect(result.error.details?.limit_kind).toBe(testCase.kind);
      expect(backend.closeCalls).toBe(1);
    }
  });
});

class StaticBackend implements RendererBackend {
  renderCalls = 0;
  closeCalls = 0;

  constructor(private readonly image: RenderedImage) {}

  render(): Promise<RenderedImage> {
    this.renderCalls += 1;
    return Promise.resolve(this.image);
  }

  close(): Promise<void> {
    this.closeCalls += 1;
    return Promise.resolve();
  }
}

class BlockingBackend implements RendererBackend {
  closeCalls = 0;
  aborted = false;

  render(_document: RendererDocument, context: RendererBackendContext): Promise<RenderedImage> {
    return new Promise((_resolve, reject) => {
      context.signal.addEventListener(
        "abort",
        () => {
          this.aborted = true;
          reject(new Error("aborted"));
        },
        { once: true }
      );
    });
  }

  close(): Promise<void> {
    this.closeCalls += 1;
    return Promise.resolve();
  }
}

function validImage(overrides: Partial<RenderedImage> = {}): RenderedImage {
  const buffer = overrides.buffer ?? pngBuffer(16);
  return {
    buffer,
    width: overrides.width ?? 1,
    height: overrides.height ?? 1,
    bytes: buffer.byteLength,
    renderer: overrides.renderer ?? "test"
  };
}

function pngBuffer(bytes: number): Buffer {
  const buffer = Buffer.alloc(bytes);
  pngSignature.copy(buffer);
  return buffer;
}
