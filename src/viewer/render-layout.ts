import { failure, success, type OperationResult } from "../core/contracts.js";
import { HerdrMathError, serializeError } from "../core/errors.js";
import { validateGraphicsInfo } from "../graphics/placement.js";
import type { HerdrGraphicsInfo, HerdrPaneLayoutSnapshot, HerdrPaneSnapshot } from "../herdr/socket-client.js";
import { resolveRendererLayout, type RendererLayout } from "../renderer/layout.js";

export interface RenderLayoutClient {
  paneGetIfPresent(paneId: string): Promise<OperationResult<HerdrPaneSnapshot | null>>;
  paneLayout(paneId: string): Promise<OperationResult<HerdrPaneLayoutSnapshot>>;
  paneGraphicsInfo(paneId: string): Promise<OperationResult<HerdrGraphicsInfo>>;
}

export interface RenderLayoutRequest {
  sourcePaneId: string;
  existingViewerPaneId?: string;
}

export async function resolveRenderLayout(
  request: RenderLayoutRequest,
  client: RenderLayoutClient
): Promise<OperationResult<Readonly<RendererLayout>>> {
  try {
    if (request.existingViewerPaneId !== undefined) {
      const viewer = await client.paneGetIfPresent(request.existingViewerPaneId);
      if (!viewer.ok) return failure(viewer.error);
      if (viewer.value !== null) {
        const layout = await contentWidthForPane(request.existingViewerPaneId, client);
        if (layout.ok) return layout;
      }
    }
    return contentWidthForPane(request.sourcePaneId, client);
  } catch (error) {
    return failure(serializeError(error));
  }
}

async function contentWidthForPane(
  paneId: string,
  client: RenderLayoutClient
): Promise<OperationResult<Readonly<RendererLayout>>> {
  const [layout, info] = await Promise.all([client.paneLayout(paneId), client.paneGraphicsInfo(paneId)]);
  if (!layout.ok) return failure(layout.error);
  if (!info.ok) return failure(info.error);
  const capability = validateGraphicsInfo(info.value);
  if (!capability.ok) return failure(capability.error);

  const panes = layout.value.panes.filter((pane) => pane.paneId === paneId);
  const pane = panes[0];
  if (panes.length !== 1 || pane === undefined) {
    return failure(serializeError(new HerdrMathError("cell_size_unavailable")));
  }
  const contentWidthPx = pane.rect.width * capability.value.cellWidthPx;
  if (!Number.isSafeInteger(contentWidthPx) || contentWidthPx <= 0) {
    return failure(serializeError(new HerdrMathError("cell_size_unavailable")));
  }
  return success(resolveRendererLayout({ contentWidthPx }));
}
