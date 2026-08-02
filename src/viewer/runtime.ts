import { Buffer } from "node:buffer";

import { failure, success, type OperationResult } from "../core/contracts.js";
import { HerdrMathError, serializeError } from "../core/errors.js";
import { renderResponse } from "../renderer/index.js";
import {
  HERDR_CLIENT_LIMITS,
  HerdrSocketClient,
  type HerdrEventSubscription,
  type HerdrPaneScrollChangedEvent,
  type HerdrPaneSnapshot
} from "../herdr/socket-client.js";
import { createViewerMetadata, isViewerSourceToken, VIEWER_IDENTITY } from "./ownership.js";
import { ViewerPresenter, type ViewerPresenterClient } from "./presenter.js";
import { startViewerTransport, type ViewerTransportServer } from "./transport.js";
import type { ViewerRenderDocument } from "./transport-protocol.js";

const HERDR_IDENTIFIER = /^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$/;
const RESIZE_DEBOUNCE_MS = 200;

export interface ViewerEnvironment {
  HERDR_SOCKET_PATH?: string | undefined;
  HERDR_PLUGIN_ID?: string | undefined;
  HERDR_PLUGIN_ENTRYPOINT_ID?: string | undefined;
  HERDR_PANE_ID?: string | undefined;
  HERDR_WORKSPACE_ID?: string | undefined;
  HERDR_PLUGIN_STATE_DIR?: string | undefined;
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

export interface ManagedViewerReady extends ViewerReady {
  transport: ViewerTransportServer;
  subscription?: HerdrEventSubscription | undefined;
  layoutSubscription?: HerdrEventSubscription | undefined;
  presenter: ViewerPresenter;
}

export type { ViewerTransportServer };

export interface ManagedViewerClient extends ViewerMetadataClient, ViewerPresenterClient {
  subscribePaneScroll?(
    paneId: string,
    onEvent: (event: HerdrPaneScrollChangedEvent) => void
  ): OperationResult<HerdrEventSubscription>;
  subscribePaneLayout?(
    paneId: string,
    workspaceId: string,
    onEvent: (pane: { width: number; height: number }) => void
  ): OperationResult<HerdrEventSubscription>;
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

export async function startManagedViewer(
  environment: ViewerEnvironment,
  client?: ManagedViewerClient
): Promise<OperationResult<ManagedViewerReady>> {
  try {
    const decoded = decodeViewerEnvironment(environment);
    const stateDirectory = environment.HERDR_PLUGIN_STATE_DIR;
    if (typeof stateDirectory !== "string" || stateDirectory.length === 0) {
      throw new HerdrMathError("viewer_ownership_failed");
    }
    const runtimeClient = client ?? new HerdrSocketClient(decoded.socketPath);
    const registered = await registerViewer(environment, runtimeClient);
    if (!registered.ok) return failure(registered.error);
    const presenter = new ViewerPresenter(runtimeClient);
    let currentDocument: ViewerRenderDocument | undefined;
    const transport = await startViewerTransport({
      stateDirectory,
      sourceToken: decoded.sourceToken,
      viewerPaneId: decoded.paneId,
      workspaceId: decoded.workspaceId,
      presenter,
      onDocument: (document) => {
        currentDocument = document;
      }
    });
    const subscription = startScrollSubscription(runtimeClient, presenter, decoded.paneId, decoded.workspaceId);
    const layoutSubscription = startResizeSubscription(
      runtimeClient,
      presenter,
      decoded.paneId,
      decoded.workspaceId,
      () => currentDocument
    );
    return success({ ...registered.value, transport, subscription, layoutSubscription, presenter });
  } catch (error) {
    return failure(serializeError(error));
  }
}

function startScrollSubscription(
  client: ManagedViewerClient,
  presenter: ViewerPresenter,
  paneId: string,
  workspaceId: string
): HerdrEventSubscription | undefined {
  if (typeof client.subscribePaneScroll !== "function") return undefined;
  let lastOffsetFromBottom: number | undefined;
  const result = client.subscribePaneScroll(paneId, (event: HerdrPaneScrollChangedEvent) => {
    if (event.workspaceId !== workspaceId) return;
    const current = event.scroll.offsetFromBottom;
    if (lastOffsetFromBottom === undefined) {
      lastOffsetFromBottom = current;
      return;
    }
    const delta = current - lastOffsetFromBottom;
    lastOffsetFromBottom = current;
    if (delta === 0) return;
    void presenter.scrollBy(paneId, workspaceId, delta);
  });
  return result.ok ? result.value : undefined;
}

function startResizeSubscription(
  client: ManagedViewerClient,
  presenter: ViewerPresenter,
  viewerPaneId: string,
  workspaceId: string,
  getDocument: () => ViewerRenderDocument | undefined
): HerdrEventSubscription | undefined {
  if (typeof client.subscribePaneLayout !== "function") return undefined;
  let debounceTimer: NodeJS.Timeout | undefined;
  let rendering = false;
  let lastWidthPx = -1;
  const handle = (pane: { width: number; height: number }): void => {
    if (debounceTimer !== undefined) clearTimeout(debounceTimer);
    debounceTimer = setTimeout(() => {
      debounceTimer = undefined;
      void renderForPane(pane);
    }, RESIZE_DEBOUNCE_MS);
  };

  const renderForPane = async (pane: { width: number; height: number }): Promise<void> => {
    if (rendering || pane.width <= 0) return;
    const document = getDocument();
    if (document === undefined) return;
    const info = await client.paneGraphicsInfo(viewerPaneId);
    if (!info.ok) return;
    const contentWidthPx = pane.width * info.value.cellWidthPx;
    if (!Number.isSafeInteger(contentWidthPx) || contentWidthPx <= 0 || contentWidthPx === lastWidthPx) return;
    lastWidthPx = contentWidthPx;
    rendering = true;
    try {
      const rendered = await renderResponse(document.text, document.formulas, { layout: { contentWidthPx } });
      if (rendered.ok) {
        await presenter.reflow(viewerPaneId, workspaceId, rendered.value);
      }
    } catch {
      // Ignore render failures on resize; the previous image stays intact.
    } finally {
      rendering = false;
    }
  };

  const result = client.subscribePaneLayout(viewerPaneId, workspaceId, handle);
  return result.ok ? result.value : undefined;
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
