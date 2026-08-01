import { BrowserRendererBackend } from "./browser-backend.js";
import type { Formula } from "../core/contracts.js";
import { renderResponseWithBackend, renderWithBackend, type RendererFormula, type RendererOptions } from "./render.js";

export async function renderFormulas(formulas: readonly RendererFormula[], options: RendererOptions = {}) {
  return renderWithBackend(formulas, new BrowserRendererBackend(), options);
}

export async function renderResponse(text: string, formulas: readonly Formula[], options: RendererOptions = {}) {
  return renderResponseWithBackend(text, formulas, new BrowserRendererBackend(), options);
}

export type {
  RendererBackend,
  RendererBackendContext,
  RendererFormula,
  RendererLimits,
  RendererOptions
} from "./render.js";
export type { RendererDocument, RendererDocumentSegment } from "./document.js";
