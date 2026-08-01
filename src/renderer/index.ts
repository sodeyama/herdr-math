import { BrowserRendererBackend } from "./browser-backend.js";
import { renderWithBackend, type RendererFormula, type RendererOptions } from "./render.js";

export async function renderFormulas(formulas: readonly RendererFormula[], options: RendererOptions = {}) {
  return renderWithBackend(formulas, new BrowserRendererBackend(), options);
}

export type {
  RendererBackend,
  RendererBackendContext,
  RendererFormula,
  RendererLimits,
  RendererOptions
} from "./render.js";
