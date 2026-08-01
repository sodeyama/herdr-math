import { Buffer } from "node:buffer";

import { failure, success, type OperationResult } from "../core/contracts.js";
import { HerdrMathError, serializeError } from "../core/errors.js";
import {
  HERDR_CLIENT_LIMITS,
  type HerdrPaneSnapshot,
  type HerdrPluginPaneOpenRequest,
  type HerdrPluginPaneSnapshot
} from "../herdr/socket-client.js";
import { deriveViewerSourceToken, VIEWER_IDENTITY } from "./ownership.js";

const HERDR_IDENTIFIER = /^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$/;

export interface ViewerManagerClient {
  paneGet(paneId: string): Promise<OperationResult<HerdrPaneSnapshot>>;
  paneGetIfPresent(paneId: string): Promise<OperationResult<HerdrPaneSnapshot | null>>;
  paneList(workspaceId: string): Promise<OperationResult<readonly HerdrPaneSnapshot[]>>;
  pluginPaneOpen(request: HerdrPluginPaneOpenRequest): Promise<OperationResult<HerdrPluginPaneSnapshot>>;
}

export interface ViewerResolutionRequest {
  sessionIdentity: string;
  workspaceId: string;
  sourcePaneId: string;
  existingViewerPaneId?: string;
}

export interface ViewerResolution {
  viewerPaneId: string;
  disposition: "reused_state" | "recovered_metadata" | "created";
}

export async function resolveViewer(
  request: ViewerResolutionRequest,
  client: ViewerManagerClient
): Promise<OperationResult<ViewerResolution>> {
  try {
    validateRequest(request);
    const sourceToken = deriveViewerSourceToken(request.sessionIdentity, request.sourcePaneId);
    const source = await client.paneGet(request.sourcePaneId);
    if (!source.ok) return failure(source.error);
    if (source.value.workspaceId !== request.workspaceId) return ownershipFailure();

    if (request.existingViewerPaneId !== undefined) {
      const stored = await client.paneGetIfPresent(request.existingViewerPaneId);
      if (!stored.ok) return failure(stored.error);
      if (stored.value !== null && isOwnedViewer(stored.value, request, sourceToken)) {
        return success({ viewerPaneId: stored.value.paneId, disposition: "reused_state" });
      }
    }

    const listed = await client.paneList(request.workspaceId);
    if (!listed.ok) return failure(listed.error);
    const recovered = listed.value.filter((pane) => isOwnedViewer(pane, request, sourceToken));
    if (recovered.length > 1) return ownershipFailure();
    if (recovered[0] !== undefined) {
      return success({ viewerPaneId: recovered[0].paneId, disposition: "recovered_metadata" });
    }

    const confirmed = await client.paneGet(request.sourcePaneId);
    if (!confirmed.ok) return failure(confirmed.error);
    if (confirmed.value.workspaceId !== request.workspaceId) return ownershipFailure();

    const opened = await client.pluginPaneOpen({
      pluginId: VIEWER_IDENTITY.pluginId,
      entrypointId: VIEWER_IDENTITY.entrypointId,
      targetPaneId: request.sourcePaneId,
      placement: "split",
      direction: "right",
      focus: false,
      environment: { HERDR_MATH_SOURCE_TOKEN: sourceToken }
    });
    if (!opened.ok) {
      if (opened.error.code === "herdr_timeout") return failure(opened.error);
      return failure(serializeError(new HerdrMathError("viewer_open_failed", {}, opened.error.retryable)));
    }
    if (!isValidOpenedViewer(opened.value, request, confirmed.value)) return ownershipFailure();
    return success({ viewerPaneId: opened.value.pane.paneId, disposition: "created" });
  } catch (error) {
    return failure(serializeError(error));
  }
}

function isOwnedViewer(pane: HerdrPaneSnapshot, request: ViewerResolutionRequest, sourceToken: string): boolean {
  return (
    pane.paneId !== request.sourcePaneId &&
    pane.workspaceId === request.workspaceId &&
    pane.tokens?.[VIEWER_IDENTITY.ownerTokenKey] === VIEWER_IDENTITY.ownerToken &&
    pane.tokens[VIEWER_IDENTITY.sourceTokenKey] === sourceToken
  );
}

function isValidOpenedViewer(
  opened: HerdrPluginPaneSnapshot,
  request: ViewerResolutionRequest,
  source: HerdrPaneSnapshot
): boolean {
  return (
    opened.pluginId === VIEWER_IDENTITY.pluginId &&
    opened.entrypointId === VIEWER_IDENTITY.entrypointId &&
    opened.pane.paneId !== request.sourcePaneId &&
    opened.pane.workspaceId === request.workspaceId &&
    opened.pane.tabId === source.tabId &&
    !opened.pane.focused
  );
}

function validateRequest(request: ViewerResolutionRequest): void {
  if (
    typeof request.sessionIdentity !== "string" ||
    request.sessionIdentity.length === 0 ||
    request.sessionIdentity.includes("\0") ||
    Buffer.byteLength(request.sessionIdentity, "utf8") > HERDR_CLIENT_LIMITS.socketPathBytes ||
    !HERDR_IDENTIFIER.test(request.workspaceId) ||
    !HERDR_IDENTIFIER.test(request.sourcePaneId) ||
    (request.existingViewerPaneId !== undefined && !HERDR_IDENTIFIER.test(request.existingViewerPaneId))
  ) {
    throw new HerdrMathError("viewer_ownership_failed");
  }
}

function ownershipFailure<T>(): OperationResult<T> {
  return failure(serializeError(new HerdrMathError("viewer_ownership_failed")));
}
