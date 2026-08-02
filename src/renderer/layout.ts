export const DEFAULT_RENDERER_FONT_SIZE_PX = 14;
export const DEFAULT_RENDERER_CONTENT_PADDING_PX = 0;
export const DEFAULT_RENDERER_CONTENT_WIDTH_PX = 480;

export interface RendererLayout {
  contentWidthPx: number;
  fontSizePx: number;
  paddingPx: number;
}

export function resolveRendererLayout(overrides: Partial<RendererLayout> = {}): Readonly<RendererLayout> {
  const layout = {
    contentWidthPx: overrides.contentWidthPx ?? DEFAULT_RENDERER_CONTENT_WIDTH_PX,
    fontSizePx: overrides.fontSizePx ?? DEFAULT_RENDERER_FONT_SIZE_PX,
    paddingPx: overrides.paddingPx ?? DEFAULT_RENDERER_CONTENT_PADDING_PX
  };
  for (const [name, value] of Object.entries(layout) as Array<[keyof RendererLayout, number]>) {
    if (!Number.isSafeInteger(value) || value < 0) {
      throw new TypeError(`Renderer layout ${name} must be a non-negative safe integer`);
    }
    if (name !== "paddingPx" && value <= 0) {
      throw new TypeError(`Renderer layout ${name} must be a positive safe integer`);
    }
  }
  return Object.freeze(layout);
}
