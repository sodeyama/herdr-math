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
  it("presents the full image as one frame with the bottom visible", async () => {
    const server = await start();
    server.setPaneRect("w1:p2", { x: 60, y: 0, width: 60, height: 10 });
    const presenter = new ViewerPresenter(new HerdrSocketClient(server.socketPath));

    const result = await presenter.present({
      viewerPaneId: "w1:p2",
      workspaceId: "w1",
      image: await png(480, 1600, "first")
    });
    expect(result.ok).toBe(true);
    expect(server.graphicsUpdates).toHaveLength(1);
    const update = server.graphicsUpdates[0];
    expect(update?.image_width).toBe(480);
    expect(update?.image_height).toBe(1600);
    // 480px / 8px = 60 cols, 1600px / 16px = 100 rows, pane is 10 rows -> overflow 90 rows.
    expect(update?.placement).toEqual({ viewport_col: 0, viewport_row: -90, grid_cols: 60, grid_rows: 100 });
    expect(server.requests.some(({ method }) => method === "pane.graphics.clear")).toBe(false);
  });

  it("uses one frame at the natural position when the response fits", async () => {
    const server = await start();
    const presenter = new ViewerPresenter(new HerdrSocketClient(server.socketPath));
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
    const update = server.graphicsUpdates[0];
    // 200px / 16px = 13 rows, pane is 40 rows -> no overflow, placed at top.
    expect(update?.placement.viewport_row).toBe(0);
  });

  it("scrolls the visible window up and down over the stacked image", async () => {
    const server = await start();
    server.setPaneRect("w1:p2", { x: 60, y: 0, width: 60, height: 10 });
    const presenter = new ViewerPresenter(new HerdrSocketClient(server.socketPath));
    expect(
      (
        await presenter.present({
          viewerPaneId: "w1:p2",
          workspaceId: "w1",
          image: await png(480, 1600, "first")
        })
      ).ok
    ).toBe(true);
    expect(server.graphicsUpdates.at(-1)?.placement.viewport_row).toBe(-90);

    // Scroll up 20 rows: viewport_row moves toward 0.
    expect((await presenter.scrollBy("w1:p2", "w1", 20)).ok).toBe(true);
    expect(server.graphicsUpdates.at(-1)?.placement.viewport_row).toBe(-70);

    // Scroll back down past the bottom clamps at the overflow.
    expect((await presenter.scrollBy("w1:p2", "w1", -50)).ok).toBe(true);
    expect(server.graphicsUpdates.at(-1)?.placement.viewport_row).toBe(-90);

    // Scrolling further up than the top clamps at 0.
    expect((await presenter.scrollBy("w1:p2", "w1", 1000)).ok).toBe(true);
    expect(server.graphicsUpdates.at(-1)?.placement.viewport_row).toBe(0);
  });

  it("resets to the bottom and stacks when a new response arrives", async () => {
    const server = await start();
    server.setPaneRect("w1:p2", { x: 60, y: 0, width: 60, height: 10 });
    const presenter = new ViewerPresenter(new HerdrSocketClient(server.socketPath));
    expect(
      (
        await presenter.present({
          viewerPaneId: "w1:p2",
          workspaceId: "w1",
          image: await png(480, 1600, "first")
        })
      ).ok
    ).toBe(true);
    await presenter.scrollBy("w1:p2", "w1", 1000);
    expect(server.graphicsUpdates.at(-1)?.placement.viewport_row).toBe(0);

    expect(
      (
        await presenter.present({
          viewerPaneId: "w1:p2",
          workspaceId: "w1",
          image: await png(480, 200, "second")
        })
      ).ok
    ).toBe(true);
    const update = server.graphicsUpdates.at(-1);
    // Stacked height: 1600 + 12 gap + 200 = 1812px -> 114 rows, overflow 104 rows.
    expect(update?.image_height).toBe(1812);
    expect(update?.placement.viewport_row).toBe(-104);
    expect(server.requests.some(({ method }) => method === "pane.graphics.clear")).toBe(false);
  });

  it("reflows a re-rendered image at the current pane width after a resize", async () => {
    const server = await start();
    server.setPaneRect("w1:p2", { x: 60, y: 0, width: 60, height: 10 });
    const presenter = new ViewerPresenter(new HerdrSocketClient(server.socketPath));
    expect(
      (
        await presenter.present({
          viewerPaneId: "w1:p2",
          workspaceId: "w1",
          image: await png(480, 1600, "first")
        })
      ).ok
    ).toBe(true);
    expect(server.graphicsUpdates.at(-1)?.placement.viewport_row).toBe(-90);

    // A resize re-renders the current response at a new width and replaces the stack.
    expect((await presenter.reflow("w1:p2", "w1", await png(400, 800, "resized"))).ok).toBe(true);
    const update = server.graphicsUpdates.at(-1);
    expect(update?.image_width).toBe(400);
    expect(update?.image_height).toBe(800);
    // 400px / 8px = 50 cols, 800px / 16px = 50 rows, pane is 10 rows -> overflow 40 rows, bottom visible.
    expect(update?.placement).toEqual({ viewport_col: 0, viewport_row: -40, grid_cols: 50, grid_rows: 50 });
  });

  it("leaves the layer unchanged when a presentation commit fails", async () => {
    const server = await start();
    server.setPaneRect("w1:p2", { x: 60, y: 0, width: 60, height: 10 });
    const presenter = new ViewerPresenter(new HerdrSocketClient(server.socketPath));
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
