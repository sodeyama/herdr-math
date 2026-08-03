export const DEFAULT_RENDERER_FONT_SIZE_PX = 14;
export const DEFAULT_RENDERER_CONTENT_PADDING_PX = 0;
export const DEFAULT_RENDERER_CONTENT_WIDTH_PX = 480;
/** CSS-pixel layout is device-independent; render at this pixel density unless the terminal reports a higher one. */
export const DEFAULT_RENDERER_DEVICE_SCALE_FACTOR = 1;
export const MAX_RENDERER_DEVICE_SCALE_FACTOR = 4;

export interface RendererLayout {
  contentWidthPx: number;
  fontSizePx: number;
  paddingPx: number;
  /** Physical pixels per CSS pixel used to rasterize the PNG (HiDPI/Retina sharpness). */
  deviceScaleFactor: number;
}

export function resolveRendererLayout(overrides: Partial<RendererLayout> = {}): Readonly<RendererLayout> {
  const layout = {
    contentWidthPx: overrides.contentWidthPx ?? DEFAULT_RENDERER_CONTENT_WIDTH_PX,
    fontSizePx: overrides.fontSizePx ?? DEFAULT_RENDERER_FONT_SIZE_PX,
    paddingPx: overrides.paddingPx ?? DEFAULT_RENDERER_CONTENT_PADDING_PX,
    deviceScaleFactor: overrides.deviceScaleFactor ?? DEFAULT_RENDERER_DEVICE_SCALE_FACTOR
  };
  for (const [name, value] of Object.entries(layout) as Array<[keyof RendererLayout, number]>) {
    if (!Number.isSafeInteger(value) || value < 0) {
      throw new TypeError(`Renderer layout ${name} must be a non-negative safe integer`);
    }
    if (name !== "paddingPx" && value <= 0) {
      throw new TypeError(`Renderer layout ${name} must be a positive safe integer`);
    }
  }
  if (layout.deviceScaleFactor > MAX_RENDERER_DEVICE_SCALE_FACTOR) {
    throw new TypeError(`Renderer layout deviceScaleFactor must be at most ${MAX_RENDERER_DEVICE_SCALE_FACTOR}`);
  }
  return Object.freeze(layout);
}
