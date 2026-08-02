import type { Buffer } from "node:buffer";
import { mkdtemp, readdir, rm, stat } from "node:fs/promises";
import { createConnection } from "node:net";
import { basename } from "node:path";

import { afterEach, describe, expect, it } from "vitest";
import sharp from "sharp";

import type { RenderedImage } from "../../src/core/contracts.js";
import { POLICY_LIMITS } from "../../src/core/limits.js";
import { publishImage } from "../../src/graphics/publisher.js";
import { HerdrSocketClient } from "../../src/herdr/socket-client.js";
import { deriveViewerSourceToken, VIEWER_IDENTITY } from "../../src/viewer/ownership.js";
import { ViewerPresenter } from "../../src/viewer/presenter.js";
import { startManagedViewer } from "../../src/viewer/runtime.js";
import {
  sendViewerPresentation,
  startViewerTransport,
  type ViewerTransportServer
} from "../../src/viewer/transport.js";
import type { ViewerRenderDocument } from "../../src/viewer/transport-protocol.js";
import { FakeHerdrServer } from "../support/fake-herdr-server.js";
import { createFakePane } from "../support/fake-herdr-types.js";

const servers = new Set<FakeHerdrServer>();
const transports = new Set<ViewerTransportServer>();
const directories: string[] = [];

afterEach(async () => {
  await Promise.all([...transports].map((transport) => transport.close()));
  transports.clear();
  await Promise.all([...servers].map((server) => server.close()));
  servers.clear();
  await Promise.all(directories.splice(0).map((directory) => rm(directory, { recursive: true, force: true })));
});

describe("private viewer transport", () => {
  it("connects the completion publisher to the managed viewer runtime", async () => {
    const directory = await mkdtemp("/tmp/hm-mv-");
    directories.push(directory);
    const server = await FakeHerdrServer.start({
      panes: [
        createFakePane(),
        createFakePane({ pane_id: "w1:p2", terminal_id: "term-2", agent: null, focused: false })
      ]
    });
    servers.add(server);
    const client = new HerdrSocketClient(server.socketPath);
    const sourceToken = deriveViewerSourceToken(server.socketPath, "w1:p1");
    const managed = await startManagedViewer(
      {
        HERDR_SOCKET_PATH: server.socketPath,
        HERDR_PLUGIN_ID: VIEWER_IDENTITY.pluginId,
        HERDR_PLUGIN_ENTRYPOINT_ID: VIEWER_IDENTITY.entrypointId,
        HERDR_PANE_ID: "w1:p2",
        HERDR_WORKSPACE_ID: "w1",
        HERDR_PLUGIN_STATE_DIR: directory,
        HERDR_MATH_SOURCE_TOKEN: sourceToken
      },
      client
    );
    expect(managed.ok).toBe(true);
    if (!managed.ok) return;
    transports.add(managed.value.transport);

    const result = await publishImage(
      {
        sourcePaneId: "w1:p1",
        workspaceId: "w1",
        generation: 1,
        existingViewerPaneId: "w1:p2",
        image: await png(480, 200)
      },
      { client, sessionIdentity: server.socketPath, stateDirectory: directory }
    );
    expect(result).toEqual({ ok: true, value: { viewerPaneId: "w1:p2" } });
    expect(server.graphicsUpdates).toHaveLength(1);
    expect(server.getPane("w1:p1")?.focused).toBe(true);
  });

  it("transfers bounded pixels over a user-only authenticated socket without durable image files", async () => {
    const directory = await mkdtemp("/tmp/hm-vt-");
    directories.push(directory);
    const server = await FakeHerdrServer.start({
      panes: [createFakePane(), createFakePane({ pane_id: "w1:p2", terminal_id: "term-2", agent: null })]
    });
    servers.add(server);
    server.setPaneRect("w1:p2", { x: 60, y: 0, width: 60, height: 10 });
    const sourceToken = deriveViewerSourceToken(server.socketPath, "w1:p1");
    const transport = await startViewerTransport({
      stateDirectory: directory,
      sourceToken,
      viewerPaneId: "w1:p2",
      workspaceId: "w1",
      presenter: new ViewerPresenter(new HerdrSocketClient(server.socketPath))
    });
    transports.add(transport);
    expect((await stat(transport.socketPath)).mode & 0o777).toBe(0o600);

    const image = await png(480, 1200);
    const result = await sendViewerPresentation({
      stateDirectory: directory,
      sourceToken,
      viewerPaneId: "w1:p2",
      workspaceId: "w1",
      generation: 1,
      image
    });
    expect(result).toEqual({ ok: true, value: { viewerPaneId: "w1:p2" } });
    expect(server.graphicsUpdates.length).toBe(1);
    expect((await stat(transport.socketPath)).isSocket()).toBe(true);
    expect(await readdir(directory)).toEqual([basename(transport.socketPath)]);

    const beforeUnauthorized = server.graphicsUpdates.length;
    const unauthorized = await rawExchange(transport.socketPath, requestRecord({ sourceToken: "0".repeat(64), image }));
    expect(unauthorized).toMatchObject({
      ok: false,
      error: { code: "viewer_ownership_failed" }
    });
    expect(server.graphicsUpdates).toHaveLength(beforeUnauthorized);

    const oversized = await rawExchange(
      transport.socketPath,
      `${"x".repeat(POLICY_LIMITS.viewerTransportBytes + 1)}\n`
    );
    expect(oversized).toMatchObject({
      ok: false,
      error: { code: "image_too_large", details: { limit_kind: "viewer_transport_bytes" } }
    });
    expect(server.graphicsUpdates).toHaveLength(beforeUnauthorized);

    await transport.close();
    transports.delete(transport);
    await expect(stat(transport.socketPath)).rejects.toMatchObject({ code: "ENOENT" });
  });

  it("carries the render document so the viewer can re-render after a resize", async () => {
    const directory = await mkdtemp("/tmp/hm-doc-");
    directories.push(directory);
    const server = await FakeHerdrServer.start({
      panes: [createFakePane(), createFakePane({ pane_id: "w1:p2", terminal_id: "term-2", agent: null })]
    });
    servers.add(server);
    const sourceToken = deriveViewerSourceToken(server.socketPath, "w1:p1");
    let receivedDocument: ViewerRenderDocument | undefined;
    const transport = await startViewerTransport({
      stateDirectory: directory,
      sourceToken,
      viewerPaneId: "w1:p2",
      workspaceId: "w1",
      presenter: new ViewerPresenter(new HerdrSocketClient(server.socketPath)),
      onDocument: (document) => {
        receivedDocument = document;
      }
    });
    transports.add(transport);

    const image = await png(480, 200);
    const document: ViewerRenderDocument = {
      text: "E=mc^2",
      formulas: [{ latex: "E=mc^2", display: true, start: 0, end: 6 }]
    };
    const result = await sendViewerPresentation({
      stateDirectory: directory,
      sourceToken,
      viewerPaneId: "w1:p2",
      workspaceId: "w1",
      generation: 1,
      image,
      document
    });
    expect(result).toEqual({ ok: true, value: { viewerPaneId: "w1:p2" } });
    expect(receivedDocument).toEqual(document);
    expect(server.graphicsUpdates).toHaveLength(1);
  });
});

async function png(width: number, height: number): Promise<RenderedImage> {
  const buffer = await sharp({
    create: { width, height, channels: 4, background: { r: 70, g: 40, b: 100, alpha: 0.75 } }
  })
    .png()
    .toBuffer();
  return { buffer, width, height, bytes: buffer.byteLength, renderer: "test" };
}

function requestRecord({ sourceToken, image }: { sourceToken: string; image: RenderedImage }): string {
  return `${JSON.stringify({
    version: 1,
    sourceToken,
    viewerPaneId: "w1:p2",
    workspaceId: "w1",
    generation: 2,
    image: {
      dataBase64: image.buffer.toString("base64"),
      width: image.width,
      height: image.height,
      bytes: image.bytes,
      renderer: image.renderer
    }
  })}\n`;
}

function rawExchange(socketPath: string, request: string): Promise<Record<string, unknown>> {
  return new Promise((resolve, reject) => {
    const socket = createConnection(socketPath);
    let source = "";
    socket.once("connect", () => socket.write(request));
    socket.on("data", (chunk: Buffer) => {
      source += chunk.toString("utf8");
      const newline = source.indexOf("\n");
      if (newline === -1) return;
      socket.destroy();
      resolve(JSON.parse(source.slice(0, newline)) as Record<string, unknown>);
    });
    socket.once("error", reject);
  });
}
