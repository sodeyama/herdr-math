import { Buffer } from "node:buffer";
import { createHash } from "node:crypto";
import { performance } from "node:perf_hooks";

import sharp from "sharp";
import { describe, expect, it } from "vitest";

import { buildBaselineFingerprint } from "../../src/boundary/fingerprint-builder.js";
import { resolveAnswerFromFingerprint } from "../../src/boundary/fingerprint-resolver.js";
import type { RenderedImage } from "../../src/core/contracts.js";
import { POLICY_LIMITS } from "../../src/core/limits.js";
import { BrowserRendererBackend } from "../../src/renderer/browser-backend.js";
import type { RendererDocument } from "../../src/renderer/document.js";
import {
  renderWithBackend,
  type RendererBackend,
  type RendererBackendContext,
  type RendererFormula
} from "../../src/renderer/render.js";
import { FullLifecycleRig } from "../support/full-lifecycle-rig.js";

const MEBIBYTE = 1024 * 1024;
const PERFORMANCE_BUDGETS = Object.freeze({
  boundaryResolutionMs: 1_000,
  workerHarnessStartupMs: 1_000,
  workerEventMs: 1_000,
  coldRenderMs: 4_000,
  warmRenderMedianMs: 2_000,
  representativePngBytes: 128 * 1024,
  nodeRssBytes: 1024 * MEBIBYTE,
  nodeRssGrowthBytes: 256 * MEBIBYTE
});
const SECRET = Buffer.alloc(32, 41);
const FORMULA: RendererFormula = { latex: "\\frac{x^2+y^2}{z^2}", display: true };

interface PerformanceMetrics {
  idle_node_rss_mib: number;
  worker_harness_startup_ms: number;
  worker_working_event_ms: number;
  worker_completion_event_ms: number;
  maximum_boundary_ms: number;
  cold_render_ms: number;
  warm_render_median_ms: number;
  maximum_render_ms: number;
  maximum_node_rss_mib: number;
  node_rss_growth_mib: number;
  representative_png_bytes: number;
  image_width_px: number;
  image_height_px: number;
  successful_renders: number;
  invalid_renders: number;
  timed_out_renders: number;
}

describe("worker and renderer performance budgets", () => {
  it("measures bounded lifecycle latency, memory, image size, and cleanup", async () => {
    const idleRss = process.memoryUsage().rss;
    const workerMeasurements = await measureWorkerLifecycle();
    const maximumBoundaryMs = measureMaximumBoundaryResolution();
    const renderMeasurements = await measureRepeatedRendering();
    const maximumRss = Math.max(idleRss, ...renderMeasurements.rssBytes);
    const metrics: PerformanceMetrics = {
      idle_node_rss_mib: toMebibytes(idleRss),
      worker_harness_startup_ms: rounded(workerMeasurements.startupMs),
      worker_working_event_ms: rounded(workerMeasurements.workingMs),
      worker_completion_event_ms: rounded(workerMeasurements.completionMs),
      maximum_boundary_ms: rounded(maximumBoundaryMs),
      cold_render_ms: rounded(renderMeasurements.durationsMs[0] ?? 0),
      warm_render_median_ms: rounded(median(renderMeasurements.durationsMs.slice(1))),
      maximum_render_ms: rounded(Math.max(...renderMeasurements.durationsMs)),
      maximum_node_rss_mib: toMebibytes(maximumRss),
      node_rss_growth_mib: toMebibytes(Math.max(0, maximumRss - idleRss)),
      representative_png_bytes: renderMeasurements.image.bytes,
      image_width_px: renderMeasurements.image.width,
      image_height_px: renderMeasurements.image.height,
      successful_renders: renderMeasurements.durationsMs.length,
      invalid_renders: 3,
      timed_out_renders: 3
    };

    process.stdout.write(`PERFORMANCE_METRICS ${JSON.stringify(metrics)}\n`);

    expect(workerMeasurements.startupMs).toBeLessThan(PERFORMANCE_BUDGETS.workerHarnessStartupMs);
    expect(workerMeasurements.workingMs).toBeLessThan(PERFORMANCE_BUDGETS.workerEventMs);
    expect(workerMeasurements.completionMs).toBeLessThan(PERFORMANCE_BUDGETS.workerEventMs);
    expect(maximumBoundaryMs).toBeLessThan(PERFORMANCE_BUDGETS.boundaryResolutionMs);
    expect(renderMeasurements.durationsMs[0]).toBeLessThan(PERFORMANCE_BUDGETS.coldRenderMs);
    expect(median(renderMeasurements.durationsMs.slice(1))).toBeLessThan(PERFORMANCE_BUDGETS.warmRenderMedianMs);
    expect(renderMeasurements.image.bytes).toBeLessThan(PERFORMANCE_BUDGETS.representativePngBytes);
    expect(maximumRss).toBeLessThan(PERFORMANCE_BUDGETS.nodeRssBytes);
    expect(maximumRss - idleRss).toBeLessThan(PERFORMANCE_BUDGETS.nodeRssGrowthBytes);
  }, 30_000);
});

async function measureWorkerLifecycle(): Promise<{ startupMs: number; workingMs: number; completionMs: number }> {
  const rigStarted = performance.now();
  const rig = await FullLifecycleRig.start("codex");
  const startupMs = performance.now() - rigStarted;
  try {
    rig.server.setPaneOutput("w1:p1", "Performance baseline.\n");
    const workingStarted = performance.now();
    const working = await rig.process(rig.server.transitionPane("w1:p1", "working"));
    const workingMs = performance.now() - workingStarted;

    rig.server.setPaneOutput("w1:p1", "Performance baseline.\nResult: $$x^2+y^2=z^2$$\n");
    const completionStarted = performance.now();
    const completion = await rig.process(rig.server.transitionPane("w1:p1", "done"));
    const completionMs = performance.now() - completionStarted;

    expect(working).toMatchObject({ ok: true, value: { kind: "baseline_stored" } });
    expect(completion).toMatchObject({ ok: true, value: { kind: "image_published" } });
    return { startupMs, workingMs, completionMs };
  } finally {
    await rig.close();
  }
}

function measureMaximumBoundaryResolution(): number {
  const anchor = "A".repeat(2048);
  const baseline = `${"B".repeat(POLICY_LIMITS.paneReadBytes - Buffer.byteLength(anchor) - 1)}\n${anchor}`;
  const repeatedLine = "R".repeat(anchor.length);
  let current = Array.from({ length: 500 }, () => repeatedLine).join("\n");
  current += "X".repeat(POLICY_LIMITS.paneReadBytes - Buffer.byteLength(current));
  const state = buildBaselineFingerprint(
    baseline,
    {
      sessionIdentity: "performance-session",
      occupantIdentity: "performance-occupant",
      workspaceId: "w1",
      sourcePaneId: "w1:p1",
      agent: "codex",
      lifecycleAuthority: "screen_detection",
      paneRevision: 1,
      eventSequence: 1,
      generation: 1,
      createdAt: new Date("2026-08-01T00:00:00.000Z")
    },
    SECRET
  );
  const durations: number[] = [];
  for (let index = 0; index < 5; index += 1) {
    const started = performance.now();
    const result = resolveAnswerFromFingerprint(state, current, SECRET, { readTruncated: true });
    durations.push(performance.now() - started);
    expect(result).toEqual({ ok: false, error: { code: "answer_truncated", retryable: false } });
  }
  return Math.max(...durations);
}

async function measureRepeatedRendering(): Promise<{
  durationsMs: number[];
  rssBytes: number[];
  image: RenderedImage;
}> {
  const durationsMs: number[] = [];
  const rssBytes: number[] = [];
  const pixelHashes: string[] = [];
  let image: RenderedImage | undefined;

  for (let index = 0; index < 4; index += 1) {
    const backend = new BrowserRendererBackend();
    const started = performance.now();
    const result = await renderWithBackend([FORMULA], backend);
    durationsMs.push(performance.now() - started);
    rssBytes.push(process.memoryUsage().rss);
    expect(result.ok).toBe(true);
    expect(backend.hasOpenResources()).toBe(false);
    if (!result.ok) continue;
    image = result.value;
    const pixels = await sharp(result.value.buffer).ensureAlpha().raw().toBuffer();
    pixelHashes.push(createHash("sha256").update(pixels).digest("hex"));
  }

  for (let index = 0; index < 3; index += 1) {
    const invalidBackend = new BrowserRendererBackend();
    const invalid = await renderWithBackend(
      [{ latex: "\\href{https://invalid.example}{x}", display: true }],
      invalidBackend
    );
    expect(invalid).toEqual({ ok: false, error: { code: "invalid_latex", retryable: false } });
    expect(invalidBackend.hasOpenResources()).toBe(false);

    const blockingBackend = new BlockingBackend();
    const timedOut = await renderWithBackend([FORMULA], blockingBackend, { limits: { renderDurationMs: 10 } });
    expect(timedOut).toMatchObject({ ok: false, error: { code: "renderer_timeout" } });
    expect(blockingBackend.closed).toBe(true);
  }

  expect(new Set(pixelHashes).size).toBe(1);
  if (image === undefined) throw new Error("Expected a successful performance render.");
  return { durationsMs, rssBytes, image };
}

class BlockingBackend implements RendererBackend {
  closed = false;

  render(_document: RendererDocument, context: RendererBackendContext): Promise<RenderedImage> {
    return new Promise((_resolve, reject) => {
      context.signal.addEventListener("abort", () => reject(new Error("aborted")), { once: true });
    });
  }

  close(): Promise<void> {
    this.closed = true;
    return Promise.resolve();
  }
}

function median(values: readonly number[]): number {
  const ordered = [...values].sort((left, right) => left - right);
  const middle = Math.floor(ordered.length / 2);
  return ordered[middle] ?? 0;
}

function rounded(value: number): number {
  return Math.round(value * 10) / 10;
}

function toMebibytes(bytes: number): number {
  return rounded(bytes / MEBIBYTE);
}
