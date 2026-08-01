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
export {
  DEFAULT_RENDERER_CONTENT_PADDING_PX,
  DEFAULT_RENDERER_CONTENT_WIDTH_PX,
  DEFAULT_RENDERER_FONT_SIZE_PX,
  resolveRendererLayout,
  type RendererLayout
} from "./layout.js";
