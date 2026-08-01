import { afterEach, describe, expect, it } from "vitest";
import sharp from "sharp";

import type { RenderedImage } from "../../src/core/contracts.js";
import { HerdrSocketClient } from "../../src/herdr/socket-client.js";
import { ViewerPresenter } from "../../src/viewer/presenter.js";
import { FakeHerdrServer } from "../support/fake-herdr-server.js";
import { createFakePane } from "../support/fake-herdr-types.js";

const servers = new Set<FakeHerdrServer>();

afterEach(async () => {
  await Promise.all([...servers].map((server) => server.close()));
  servers.clear();
});

describe("managed viewer presentation", () => {
  it("prebuilds a long response, scrolls from top to bottom, and keeps the final frame", async () => {
    const server = await start();
    server.setPaneRect("w1:p2", { x: 60, y: 0, width: 60, height: 10 });
    const intervals: number[] = [];
    const presenter = new ViewerPresenter(new HerdrSocketClient(server.socketPath), (milliseconds) => {
      intervals.push(milliseconds);
      return Promise.resolve();
    });

    const result = await presenter.present({
      viewerPaneId: "w1:p2",
      workspaceId: "w1",
      image: await png(480, 1600, "first")
    });
    expect(result.ok).toBe(true);
    expect(server.graphicsUpdates.length).toBeGreaterThan(1);
    expect(intervals).toHaveLength(server.graphicsUpdates.length - 1);
    expect(server.graphicsUpdates.every(({ image_height }) => image_height === 160)).toBe(true);
    expect(server.getGraphics("w1:p2")).toEqual(server.graphicsUpdates.at(-1));
    expect(server.requests.some(({ method }) => method === "pane.graphics.clear")).toBe(false);
  });

  it("uses one frame without delay when the response fits", async () => {
    const server = await start();
    const intervals: number[] = [];
    const presenter = new ViewerPresenter(new HerdrSocketClient(server.socketPath), (milliseconds) => {
      intervals.push(milliseconds);
      return Promise.resolve();
    });
    expect(
      (
        await presenter.present({
          viewerPaneId: "w1:p2",
          workspaceId: "w1",
          image: await png(480, 200, "short")
        })
      ).ok
    ).toBe(true);
    expect(server.graphicsUpdates).toHaveLength(1);
    expect(intervals).toHaveLength(0);
  });

  it("restores the previous final frame after a later animation commit fails", async () => {
    const server = await start();
    server.setPaneRect("w1:p2", { x: 60, y: 0, width: 60, height: 10 });
    const presenter = new ViewerPresenter(new HerdrSocketClient(server.socketPath), () => Promise.resolve());
    expect(
      (
        await presenter.present({
          viewerPaneId: "w1:p2",
          workspaceId: "w1",
          image: await png(480, 200, "previous")
        })
      ).ok
    ).toBe(true);
    const previous = server.getGraphics("w1:p2");

    server.queueResponse("pane.graphics.set", {});
    server.queueResponse("pane.graphics.set", { error: { code: "payload_rejected", message: "rejected" } });
    const failed = await presenter.present({
      viewerPaneId: "w1:p2",
      workspaceId: "w1",
      image: await png(480, 1200, "next")
    });
    expect(failed.ok).toBe(false);
    expect(server.getGraphics("w1:p2")).toEqual(previous);
  });
});

async function start(): Promise<FakeHerdrServer> {
  const server = await FakeHerdrServer.start({
    panes: [createFakePane(), createFakePane({ pane_id: "w1:p2", terminal_id: "term-2", agent: null, focused: false })]
  });
  servers.add(server);
  return server;
}

async function png(width: number, height: number, seed: string): Promise<RenderedImage> {
  const background = seed === "previous" ? { r: 20, g: 40, b: 80, alpha: 0.8 } : { r: 80, g: 30, b: 20, alpha: 0.8 };
  const buffer = await sharp({ create: { width, height, channels: 4, background } })
    .png()
    .toBuffer();
  return { buffer, width, height, bytes: buffer.byteLength, renderer: "test" };
}
