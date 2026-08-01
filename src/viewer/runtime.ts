import { Buffer } from "node:buffer";

import { failure, success, type OperationResult } from "../core/contracts.js";
import { HerdrMathError, serializeError } from "../core/errors.js";
import { HERDR_CLIENT_LIMITS, HerdrSocketClient, type HerdrPaneSnapshot } from "../herdr/socket-client.js";
import { createViewerMetadata, isViewerSourceToken, VIEWER_IDENTITY } from "./ownership.js";

const HERDR_IDENTIFIER = /^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$/;

export interface ViewerEnvironment {
  HERDR_SOCKET_PATH?: string | undefined;
  HERDR_PLUGIN_ID?: string | undefined;
  HERDR_PLUGIN_ENTRYPOINT_ID?: string | undefined;
  HERDR_PANE_ID?: string | undefined;
  HERDR_WORKSPACE_ID?: string | undefined;
  HERDR_MATH_SOURCE_TOKEN?: string | undefined;
}

export interface ViewerMetadataClient {
  paneReportMetadata(
    paneId: string,
    report: ReturnType<typeof createViewerMetadata>
  ): Promise<OperationResult<HerdrPaneSnapshot>>;
}

export interface ViewerReady {
  kind: "viewer_ready";
  paneId: string;
  workspaceId: string;
}

export async function registerViewer(
  environment: ViewerEnvironment,
  client?: ViewerMetadataClient
): Promise<OperationResult<ViewerReady>> {
  try {
    const decoded = decodeViewerEnvironment(environment);
    const response = await (client ?? new HerdrSocketClient(decoded.socketPath)).paneReportMetadata(
      decoded.paneId,
      createViewerMetadata(decoded.sourceToken)
    );
    if (!response.ok) return failure(response.error);
    if (response.value.paneId !== decoded.paneId || response.value.workspaceId !== decoded.workspaceId) {
      throw new HerdrMathError("viewer_ownership_failed");
    }
    return success({ kind: "viewer_ready", paneId: decoded.paneId, workspaceId: decoded.workspaceId });
  } catch (error) {
    return failure(serializeError(error));
  }
}

function decodeViewerEnvironment(environment: ViewerEnvironment): {
  socketPath: string;
  paneId: string;
  workspaceId: string;
  sourceToken: string;
} {
  const socketPath = environment.HERDR_SOCKET_PATH;
  const paneId = environment.HERDR_PANE_ID;
  const workspaceId = environment.HERDR_WORKSPACE_ID;
  const sourceToken = environment.HERDR_MATH_SOURCE_TOKEN;
  if (
    environment.HERDR_PLUGIN_ID !== VIEWER_IDENTITY.pluginId ||
    environment.HERDR_PLUGIN_ENTRYPOINT_ID !== VIEWER_IDENTITY.entrypointId ||
    typeof socketPath !== "string" ||
    socketPath.length === 0 ||
    socketPath.includes("\0") ||
    Buffer.byteLength(socketPath, "utf8") > HERDR_CLIENT_LIMITS.socketPathBytes ||
    typeof paneId !== "string" ||
    !HERDR_IDENTIFIER.test(paneId) ||
    typeof workspaceId !== "string" ||
    !HERDR_IDENTIFIER.test(workspaceId) ||
    !isViewerSourceToken(sourceToken)
  ) {
    throw new HerdrMathError("viewer_ownership_failed");
  }
  return { socketPath, paneId, workspaceId, sourceToken };
}
