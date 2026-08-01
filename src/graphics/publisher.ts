import { failure, success, type OperationResult } from "../core/contracts.js";
import { HerdrMathError, serializeError } from "../core/errors.js";
import type { ImagePublishRequest, ImagePublishResult } from "../events/agent-status-worker.js";
import type { HerdrGraphicsInfo, HerdrGraphicsSetRequest, HerdrPaneLayoutSnapshot } from "../herdr/socket-client.js";
import { resolveViewer, type ViewerManagerClient } from "../viewer/manager.js";
import { computeGraphicsPlacement, encodeValidatedPng, validateGraphicsInfo } from "./placement.js";

export interface GraphicsPublisherClient extends ViewerManagerClient {
  paneGraphicsInfo(paneId: string): Promise<OperationResult<HerdrGraphicsInfo>>;
  paneLayout(paneId: string): Promise<OperationResult<HerdrPaneLayoutSnapshot>>;
  paneGraphicsSet(request: HerdrGraphicsSetRequest): Promise<OperationResult<void>>;
}

export interface GraphicsPublisherDependencies {
  client: GraphicsPublisherClient;
  sessionIdentity: string;
}

export async function publishImage(
  request: ImagePublishRequest,
  dependencies: GraphicsPublisherDependencies
): Promise<OperationResult<ImagePublishResult>> {
  try {
    if (!Number.isSafeInteger(request.generation) || request.generation < 0) return ownershipFailure();
    const encoded = encodeValidatedPng(request.image);
    if (!encoded.ok) return failure(encoded.error);

    const sourceInfo = await dependencies.client.paneGraphicsInfo(request.sourcePaneId);
    if (!sourceInfo.ok) return failure(sourceInfo.error);
    const sourceCapability = validateGraphicsInfo(sourceInfo.value);
    if (!sourceCapability.ok) return failure(sourceCapability.error);

    const viewer = await resolveViewer(
      {
        sessionIdentity: dependencies.sessionIdentity,
        workspaceId: request.workspaceId,
        sourcePaneId: request.sourcePaneId,
        ...(request.existingViewerPaneId === undefined ? {} : { existingViewerPaneId: request.existingViewerPaneId })
      },
      dependencies.client
    );
    if (!viewer.ok) return failure(viewer.error);

    const viewerInfo = await dependencies.client.paneGraphicsInfo(viewer.value.viewerPaneId);
    if (!viewerInfo.ok) return failure(viewerInfo.error);
    const layout = await dependencies.client.paneLayout(viewer.value.viewerPaneId);
    if (!layout.ok) return failure(layout.error);
    if (layout.value.workspaceId !== request.workspaceId) return ownershipFailure();
    const viewerCells = layout.value.panes.filter(({ paneId }) => paneId === viewer.value.viewerPaneId);
    if (viewerCells.length !== 1 || viewerCells[0] === undefined) return ownershipFailure();

    const placement = computeGraphicsPlacement(encoded.value, viewerInfo.value, viewerCells[0].rect);
    if (!placement.ok) return failure(placement.error);
    const updated = await dependencies.client.paneGraphicsSet({
      paneId: viewer.value.viewerPaneId,
      imageWidth: encoded.value.width,
      imageHeight: encoded.value.height,
      dataBase64: encoded.value.dataBase64,
      placement: placement.value
    });
    if (!updated.ok) return failure(updated.error);
    return success({ viewerPaneId: viewer.value.viewerPaneId });
  } catch (error) {
    return failure(serializeError(error));
  }
}

function ownershipFailure<T>(): OperationResult<T> {
  return failure(serializeError(new HerdrMathError("viewer_ownership_failed")));
}
