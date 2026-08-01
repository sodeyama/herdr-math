import { Buffer } from "node:buffer";

import { failure, success, type Formula, type OperationResult, type RenderedImage } from "../core/contracts.js";
import { HerdrMathError, serializeError, type SafeLimitKind } from "../core/errors.js";
import { POLICY_LIMITS } from "../core/limits.js";
import {
  composeRendererDocument,
  type RendererDocument,
  type RendererDocumentLimits,
  type RendererDocumentSegment
} from "./document.js";

export type RendererFormula = Readonly<Pick<Formula, "latex" | "display">>;

export interface RendererLimits {
  formulasPerAnswer: number;
  charactersPerFormula: number;
  aggregateFormulaCharacters: number;
  responseDocumentBytes: number;
  responseDocumentLines: number;
  responseDocumentBlocks: number;
  renderDurationMs: number;
  imageWidthPx: number;
  imageHeightPx: number;
  imagePixels: number;
  rawPngBytes: number;
  base64PayloadBytes: number;
}

export interface RendererBackendContext {
  signal: AbortSignal;
  limits: Readonly<RendererLimits>;
  deadlineMs: number;
}

export interface RendererBackend {
  render(document: RendererDocument, context: RendererBackendContext): Promise<RenderedImage>;
  close(): Promise<void>;
}

export interface RendererOptions {
  limits?: Partial<RendererLimits>;
}

const DEFAULT_RENDERER_LIMITS: Readonly<RendererLimits> = Object.freeze({
  formulasPerAnswer: POLICY_LIMITS.formulasPerAnswer,
  charactersPerFormula: POLICY_LIMITS.charactersPerFormula,
  aggregateFormulaCharacters: POLICY_LIMITS.aggregateFormulaCharacters,
  responseDocumentBytes: POLICY_LIMITS.responseDocumentBytes,
  responseDocumentLines: POLICY_LIMITS.responseDocumentLines,
  responseDocumentBlocks: POLICY_LIMITS.responseDocumentBlocks,
  renderDurationMs: POLICY_LIMITS.renderDurationMs,
  imageWidthPx: POLICY_LIMITS.imageWidthPx,
  imageHeightPx: POLICY_LIMITS.imageHeightPx,
  imagePixels: POLICY_LIMITS.imagePixels,
  rawPngBytes: POLICY_LIMITS.rawPngBytes,
  base64PayloadBytes: POLICY_LIMITS.base64PayloadBytes
});

const PNG_SIGNATURE = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);

export async function renderWithBackend(
  formulas: readonly RendererFormula[],
  backend: RendererBackend,
  options: RendererOptions = {}
): Promise<OperationResult<RenderedImage>> {
  return renderPreparedWithBackend((limits) => success(composeFormulaDocument(formulas, limits)), backend, options);
}

export async function renderResponseWithBackend(
  text: string,
  formulas: readonly Formula[],
  backend: RendererBackend,
  options: RendererOptions = {}
): Promise<OperationResult<RenderedImage>> {
  return renderPreparedWithBackend(
    (limits) => composeRendererDocument(text, formulas, documentLimits(limits)),
    backend,
    options
  );
}

async function renderPreparedWithBackend(
  prepare: (limits: Readonly<RendererLimits>) => OperationResult<RendererDocument>,
  backend: RendererBackend,
  options: RendererOptions
): Promise<OperationResult<RenderedImage>> {
  const limits = resolveLimits(options.limits);
  const abortController = new AbortController();
  const startedAt = Date.now();
  let timedOut = false;
  let timer: NodeJS.Timeout | undefined;
  let outcome: OperationResult<RenderedImage> = failure(
    serializeError(new HerdrMathError("renderer_failed", {}, true))
  );

  try {
    const prepared = prepare(limits);
    if (!prepared.ok) {
      outcome = failure(prepared.error);
      return outcome;
    }
    const timeout = new Promise<never>((_resolve, reject) => {
      const timeoutHandle = setTimeout(() => {
        timedOut = true;
        abortController.abort();
        reject(limitError("renderer_timeout", "render_duration_ms", limits.renderDurationMs, limits.renderDurationMs));
      }, limits.renderDurationMs);
      timer = timeoutHandle;
    });
    const image = await Promise.race([
      backend.render(prepared.value, {
        signal: abortController.signal,
        limits,
        deadlineMs: startedAt + limits.renderDurationMs
      }),
      timeout
    ]);
    validateRenderedImage(image, limits);
    outcome = success(image);
  } catch (error) {
    const mapped = timedOut
      ? limitError("renderer_timeout", "render_duration_ms", limits.renderDurationMs, limits.renderDurationMs, true)
      : error instanceof HerdrMathError
        ? error
        : new HerdrMathError("renderer_failed", {}, true);
    outcome = failure(serializeError(mapped));
  } finally {
    if (timer !== undefined) clearTimeout(timer);
    try {
      await backend.close();
    } catch {
      if (outcome.ok) outcome = failure(serializeError(new HerdrMathError("renderer_failed", {}, true)));
    }
  }

  return outcome;
}

export function assertImageDimensions(width: number, height: number, limits: Readonly<RendererLimits>): void {
  assertDimension("image_width_px", width, limits.imageWidthPx);
  assertDimension("image_height_px", height, limits.imageHeightPx);
  const pixels = width * height;
  if (!Number.isSafeInteger(pixels) || pixels > limits.imagePixels) {
    throw limitError("image_too_large", "image_pixels", limits.imagePixels, pixels);
  }
}

function composeFormulaDocument(
  formulas: readonly RendererFormula[],
  limits: Readonly<RendererLimits>
): RendererDocument {
  if (formulas.length === 0) throw new HerdrMathError("formula_not_found");
  if (formulas.length > limits.formulasPerAnswer) {
    throw limitError("renderer_input_limit", "formula_count", limits.formulasPerAnswer, formulas.length);
  }

  let aggregate = 0;
  let textBytes = 0;
  const segments: RendererDocumentSegment[] = [];
  for (const formula of formulas) {
    if (typeof formula?.latex !== "string" || typeof formula.display !== "boolean" || formula.latex.trim() === "") {
      throw new HerdrMathError("invalid_latex");
    }
    const characters = [...formula.latex].length;
    textBytes += Buffer.byteLength(formula.latex, "utf8");
    if (characters > limits.charactersPerFormula) {
      throw limitError("renderer_input_limit", "formula_characters", limits.charactersPerFormula, characters);
    }
    aggregate += characters;
    if (aggregate > limits.aggregateFormulaCharacters) {
      throw limitError(
        "renderer_input_limit",
        "aggregate_formula_characters",
        limits.aggregateFormulaCharacters,
        aggregate
      );
    }
    if (segments.length > 0) segments.push(Object.freeze({ kind: "text", text: "\n\n" }));
    segments.push(Object.freeze({ kind: "math", latex: formula.latex, display: formula.display }));
  }
  return Object.freeze({
    segments: Object.freeze(segments),
    textBytes,
    lineCount: formulas.length,
    blockCount: formulas.length,
    formulaCount: formulas.length,
    formulaCharacters: aggregate
  });
}

function documentLimits(limits: Readonly<RendererLimits>): RendererDocumentLimits {
  return {
    responseDocumentBytes: limits.responseDocumentBytes,
    responseDocumentLines: limits.responseDocumentLines,
    responseDocumentBlocks: limits.responseDocumentBlocks,
    formulasPerAnswer: limits.formulasPerAnswer,
    charactersPerFormula: limits.charactersPerFormula,
    aggregateFormulaCharacters: limits.aggregateFormulaCharacters
  };
}

function validateRenderedImage(image: RenderedImage, limits: Readonly<RendererLimits>): void {
  if (
    !Buffer.isBuffer(image.buffer) ||
    image.bytes !== image.buffer.byteLength ||
    typeof image.renderer !== "string" ||
    image.renderer.length === 0 ||
    !image.buffer.subarray(0, PNG_SIGNATURE.length).equals(PNG_SIGNATURE)
  ) {
    throw new HerdrMathError("renderer_failed", {}, true);
  }
  assertImageDimensions(image.width, image.height, limits);
  assertBytes("raw_png_bytes", image.bytes, limits.rawPngBytes);
  assertBytes("base64_payload_bytes", 4 * Math.ceil(image.bytes / 3), limits.base64PayloadBytes);
}

function resolveLimits(overrides: Partial<RendererLimits> | undefined): Readonly<RendererLimits> {
  const limits = { ...DEFAULT_RENDERER_LIMITS, ...overrides };
  for (const [name, value] of Object.entries(limits) as Array<[keyof RendererLimits, number]>) {
    if (!Number.isSafeInteger(value) || value <= 0)
      throw new TypeError("Renderer limits must be positive safe integers");
    if (value > DEFAULT_RENDERER_LIMITS[name]) throw new TypeError("Renderer limits cannot exceed policy limits");
  }
  return Object.freeze(limits);
}

function assertDimension(kind: SafeLimitKind, actual: number, limit: number): void {
  if (!Number.isSafeInteger(actual) || actual <= 0) throw new HerdrMathError("renderer_failed", {}, true);
  if (actual > limit) throw limitError("image_too_large", kind, limit, actual);
}

function assertBytes(kind: SafeLimitKind, actual: number, limit: number): void {
  if (actual > limit) throw limitError("image_too_large", kind, limit, actual);
}

function limitError(
  code: "renderer_input_limit" | "renderer_timeout" | "image_too_large",
  limitKind: SafeLimitKind,
  limit: number,
  actual: number,
  retryable = false
): HerdrMathError {
  return new HerdrMathError(code, { limit_kind: limitKind, limit, actual }, retryable);
}
