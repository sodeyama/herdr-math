import type { Buffer } from "node:buffer";
import { chmod, lstat, rm } from "node:fs/promises";
import { createConnection, createServer, type Server, type Socket } from "node:net";

import { failure, success, type OperationResult } from "../core/contracts.js";
import { HerdrMathError, serializeError } from "../core/errors.js";
import { POLICY_LIMITS } from "../core/limits.js";
import type { ViewerPresenter } from "./presenter.js";
import {
  decodeViewerTransportRequest,
  encodeViewerTransportRequest,
  parseViewerTransportResponse,
  transportLimitFailure,
  validateViewerTransportIdentity,
  viewerSocketPath,
  viewerTransportFailure,
  type ViewerRenderDocument,
  type ViewerTransportRequest,
  type ViewerTransportResponse
} from "./transport-protocol.js";

const RESPONSE_BYTES = 16 * 1024;
const CONNECT_RETRY_MS = 25;
const CONNECT_WINDOW_MS = 1000;

export interface ViewerTransportServerOptions extends Pick<
  ViewerTransportRequest,
  "stateDirectory" | "sourceToken" | "viewerPaneId" | "workspaceId"
> {
  presenter: ViewerPresenter;
  onDocument?: (document: ViewerRenderDocument) => void;
}

export interface ViewerTransportServer {
  socketPath: string;
  close(): Promise<void>;
}

export async function sendViewerPresentation(
  request: ViewerTransportRequest
): Promise<OperationResult<{ viewerPaneId: string }>> {
  try {
    const encoded = encodeViewerTransportRequest(request);
    if (!encoded.ok) return failure(encoded.error);
    const response = await exchange(encoded.value.socketPath, encoded.value.payload);
    if (!response.ok) return failure(response.error);
    if (response.viewerPaneId !== request.viewerPaneId) return ownershipFailure();
    return success({ viewerPaneId: response.viewerPaneId });
  } catch (error) {
    return failure(serializeError(error));
  }
}

export async function startViewerTransport(options: ViewerTransportServerOptions): Promise<ViewerTransportServer> {
  validateViewerTransportIdentity({ ...options, generation: 0 });
  const socketPath = viewerSocketPath(options.stateDirectory, options.sourceToken);
  await removeStaleSocket(socketPath);
  const sockets = new Set<Socket>();
  let queue = Promise.resolve();
  const server = createServer((socket) => {
    sockets.add(socket);
    socket.once("close", () => sockets.delete(socket));
    receiveRequest(socket, options, (work) => {
      queue = queue.then(work, work).then(
        () => undefined,
        () => undefined
      );
    });
  });
  await listen(server, socketPath);
  try {
    await chmod(socketPath, 0o600);
  } catch (error) {
    await closeServer(server, sockets, socketPath);
    throw error;
  }
  return { socketPath, close: () => closeServer(server, sockets, socketPath) };
}

function receiveRequest(
  socket: Socket,
  options: ViewerTransportServerOptions,
  enqueue: (work: () => Promise<void>) => void
): void {
  socket.setTimeout(POLICY_LIMITS.viewerTransportTimeoutMs, () => socket.destroy());
  let source = "";
  let receivedBytes = 0;
  let handled = false;
  socket.on("data", (chunk: Buffer) => {
    if (handled) return;
    receivedBytes += chunk.byteLength;
    if (receivedBytes > POLICY_LIMITS.viewerTransportBytes) {
      handled = true;
      writeResponse(socket, transportLimitFailure(POLICY_LIMITS.viewerTransportBytes, receivedBytes));
      return;
    }
    source += chunk.toString("utf8");
    const newline = source.indexOf("\n");
    if (newline === -1) return;
    handled = true;
    if (source.slice(newline + 1) !== "") {
      writeResponse(socket, viewerTransportFailure("viewer_ownership_failed"));
      return;
    }
    enqueue(async () => {
      const decoded = decodeViewerTransportRequest(source.slice(0, newline), options);
      if (!decoded.ok) {
        writeResponse(socket, { ok: false, error: decoded.error });
        return;
      }
      if (decoded.value.document !== undefined) options.onDocument?.(decoded.value.document);
      const result = await options.presenter.present({
        viewerPaneId: options.viewerPaneId,
        workspaceId: options.workspaceId,
        image: decoded.value.image
      });
      writeResponse(
        socket,
        result.ok ? { ok: true, viewerPaneId: options.viewerPaneId } : { ok: false, error: result.error }
      );
    });
  });
  socket.once("error", () => undefined);
}

async function exchange(socketPath: string, payload: string): Promise<ViewerTransportResponse> {
  const deadline = Date.now() + POLICY_LIMITS.viewerTransportTimeoutMs;
  const socket = await connectWithRetry(socketPath, Math.min(deadline, Date.now() + CONNECT_WINDOW_MS));
  return new Promise((resolve, reject) => {
    let source = "";
    let bytes = 0;
    let settled = false;
    const finish = (callback: () => void): void => {
      if (settled) return;
      settled = true;
      socket.destroy();
      callback();
    };
    socket.setTimeout(Math.max(1, deadline - Date.now()), () =>
      finish(() => reject(new HerdrMathError("herdr_timeout", {}, true)))
    );
    socket.on("data", (chunk: Buffer) => {
      bytes += chunk.byteLength;
      if (bytes > RESPONSE_BYTES) {
        finish(() => reject(new HerdrMathError("viewer_ownership_failed")));
        return;
      }
      source += chunk.toString("utf8");
      const newline = source.indexOf("\n");
      if (newline !== -1) finish(() => resolve(parseViewerTransportResponse(source.slice(0, newline))));
    });
    socket.once("error", () => finish(() => reject(new HerdrMathError("viewer_open_failed", {}, true))));
    socket.once("close", () => {
      if (!settled) finish(() => reject(new HerdrMathError("viewer_open_failed", {}, true)));
    });
    socket.write(payload);
  });
}

async function connectWithRetry(socketPath: string, deadline: number): Promise<Socket> {
  while (true) {
    try {
      return await connectOnce(socketPath);
    } catch {
      if (Date.now() >= deadline) throw new HerdrMathError("viewer_open_failed", {}, true);
      await new Promise((resolve) => setTimeout(resolve, CONNECT_RETRY_MS));
    }
  }
}

function connectOnce(socketPath: string): Promise<Socket> {
  return new Promise((resolve, reject) => {
    const socket = createConnection(socketPath);
    socket.once("connect", () => resolve(socket));
    socket.once("error", (error) => {
      socket.destroy();
      reject(error);
    });
  });
}

async function removeStaleSocket(socketPath: string): Promise<void> {
  try {
    const status = await lstat(socketPath);
    if (!status.isSocket()) throw new HerdrMathError("viewer_ownership_failed");
    await rm(socketPath);
  } catch (error) {
    if (isNodeError(error) && error.code === "ENOENT") return;
    throw error;
  }
}

function listen(server: Server, socketPath: string): Promise<void> {
  return new Promise((resolve, reject) => {
    server.once("listening", resolve);
    server.once("error", reject);
    server.listen(socketPath);
  });
}

async function closeServer(server: Server, sockets: Set<Socket>, socketPath: string): Promise<void> {
  for (const socket of sockets) socket.destroy();
  await new Promise<void>((resolve) => server.close(() => resolve()));
  try {
    await rm(socketPath);
  } catch (error) {
    if (!isNodeError(error) || error.code !== "ENOENT") throw error;
  }
}

function writeResponse(socket: Socket, response: ViewerTransportResponse): void {
  if (!socket.destroyed) socket.end(`${JSON.stringify(response)}\n`);
}

function ownershipFailure<T>(): OperationResult<T> {
  return failure(serializeError(new HerdrMathError("viewer_ownership_failed")));
}

function isNodeError(value: unknown): value is NodeJS.ErrnoException {
  return value instanceof Error && "code" in value;
}
