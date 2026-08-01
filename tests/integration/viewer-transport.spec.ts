import type { Buffer } from "node:buffer";
import { mkdtemp, readdir, rm, stat } from "node:fs/promises";
import { createConnection } from "node:net";
import { basename } from "node:path";

import { afterEach, describe, expect, it } from "vitest";
import sharp from "sharp";

import type { RenderedImage } from "../../src/core/contracts.js";
import { POLICY_LIMITS } from "../../src/core/limits.js";
import { HerdrSocketClient } from "../../src/herdr/socket-client.js";
import { deriveViewerSourceToken } from "../../src/viewer/ownership.js";
import { ViewerPresenter } from "../../src/viewer/presenter.js";
import {
  sendViewerPresentation,
  startViewerTransport,
  type ViewerTransportServer
} from "../../src/viewer/transport.js";
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
      presenter: new ViewerPresenter(new HerdrSocketClient(server.socketPath), () => Promise.resolve())
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
    expect(server.graphicsUpdates.length).toBeGreaterThan(1);
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
