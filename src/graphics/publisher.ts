import { failure, success, type OperationResult } from "../core/contracts.js";
import { HerdrMathError, serializeError } from "../core/errors.js";
import type { ImagePublishRequest, ImagePublishResult } from "../events/agent-status-worker.js";
import type { HerdrGraphicsInfo } from "../herdr/socket-client.js";
import { resolveViewer, type ViewerManagerClient } from "../viewer/manager.js";
import { deriveViewerSourceToken } from "../viewer/ownership.js";
import { sendViewerPresentation } from "../viewer/transport.js";
import { encodeValidatedPng, validateGraphicsInfo } from "./placement.js";

export interface GraphicsPublisherClient extends ViewerManagerClient {
  paneGraphicsInfo(paneId: string): Promise<OperationResult<HerdrGraphicsInfo>>;
}

export interface GraphicsPublisherDependencies {
  client: GraphicsPublisherClient;
  sessionIdentity: string;
  stateDirectory?: string;
  present?: (request: {
    viewerPaneId: string;
    workspaceId: string;
    image: ImagePublishRequest["image"];
  }) => Promise<OperationResult<void>>;
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

    const presentation =
      dependencies.present === undefined
        ? await sendThroughManagedViewer(request, viewer.value.viewerPaneId, dependencies)
        : await dependencies.present({
            viewerPaneId: viewer.value.viewerPaneId,
            workspaceId: request.workspaceId,
            image: request.image
          });
    if (!presentation.ok) return failure(presentation.error);
    return success({ viewerPaneId: viewer.value.viewerPaneId });
  } catch (error) {
    return failure(serializeError(error));
  }
}

async function sendThroughManagedViewer(
  request: ImagePublishRequest,
  viewerPaneId: string,
  dependencies: GraphicsPublisherDependencies
): Promise<OperationResult<void>> {
  if (dependencies.stateDirectory === undefined) return ownershipFailure();
  const sent = await sendViewerPresentation({
    stateDirectory: dependencies.stateDirectory,
    sourceToken: deriveViewerSourceToken(dependencies.sessionIdentity, request.sourcePaneId),
    viewerPaneId,
    workspaceId: request.workspaceId,
    generation: request.generation,
    image: request.image,
    ...(request.document === undefined ? {} : { document: request.document })
  });
  return sent.ok ? success(undefined) : failure(sent.error);
}

function ownershipFailure<T>(): OperationResult<T> {
  return failure(serializeError(new HerdrMathError("viewer_ownership_failed")));
}
