import { Buffer } from "node:buffer";

import { afterEach, describe, expect, it } from "vitest";

import type { RenderedImage } from "../../src/core/contracts.js";
import { POLICY_LIMITS } from "../../src/core/limits.js";
import { publishImage } from "../../src/graphics/publisher.js";
import { HerdrSocketClient } from "../../src/herdr/socket-client.js";
import { deriveViewerSourceToken, VIEWER_IDENTITY } from "../../src/viewer/ownership.js";
import { ViewerPresenter } from "../../src/viewer/presenter.js";
import { FakeHerdrServer } from "../support/fake-herdr-server.js";
import { createFakePane, type FakeHerdrServerOptions } from "../support/fake-herdr-types.js";

const PNG_SIGNATURE = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
const servers = new Set<FakeHerdrServer>();

afterEach(async () => {
  await Promise.all([...servers].map((server) => server.close()));
  servers.clear();
});

describe("graphics publisher", () => {
  it("replaces one owned viewer layer without clear and recomputes placement after resize", async () => {
    const server = await start({ withViewer: true });
    const client = new HerdrSocketClient(server.socketPath);
    server.setPaneRect("w1:p2", { x: 60, y: 0, width: 80, height: 20 });

    expect(await publish(server, client, image(), "w1:p2")).toEqual({
      ok: true,
      value: { viewerPaneId: "w1:p2" }
    });
    expect(server.getGraphics("w1:p2")).toMatchObject({
      image_width: 640,
      image_height: 320,
      placement: { viewport_col: 0, viewport_row: 0, grid_cols: 80, grid_rows: 20 }
    });

    server.setPaneRect("w1:p2", { x: 60, y: 0, width: 40, height: 10 });
    expect((await publish(server, client, image(), "w1:p2")).ok).toBe(true);
    // 640px / 8px = 80 natural cols clamped to 40, 320px / 16px = 20 natural rows,
    // pane shows 10 rows -> overflow 10 rows, bottom visible at viewport_row -10.
    expect(server.getGraphics("w1:p2")?.placement).toEqual({
      viewport_col: 0,
      viewport_row: -10,
      grid_cols: 40,
      grid_rows: 20
    });
    expect(server.requests.filter(({ method }) => method === "pane.graphics.set")).toHaveLength(2);
    expect(server.requests.some(({ method }) => method === "pane.graphics.clear")).toBe(false);
    expect(server.paneCount).toBe(2);
    expect(server.getPane("w1:p1")?.focused).toBe(true);
  });

  it("preflights graphics before creating the first viewer", async () => {
    const server = await start();
    const client = new HerdrSocketClient(server.socketPath);

    expect(await publish(server, client, image())).toEqual({
      ok: true,
      value: { viewerPaneId: "w1:p2" }
    });
    expect(server.paneCount).toBe(2);
    expect(server.getPane("w1:p1")?.focused).toBe(true);
    expect(server.getPane("w1:p2")?.focused).toBe(false);
    expect(server.getGraphics("w1:p2")).toBeDefined();
  });

  it.each([
    ["disabled", { enabled: false }, "graphics_disabled"],
    ["zero width", { cellWidthPx: 0 }, "cell_size_unavailable"],
    ["zero height", { cellHeightPx: 0 }, "cell_size_unavailable"]
  ])("does not create a viewer when graphics are %s", async (_name, graphics, code) => {
    const server = await start({ graphics });
    const result = await publish(server, new HerdrSocketClient(server.socketPath), image());

    expect(result).toEqual({ ok: false, error: { code, retryable: false } });
    expect(server.paneCount).toBe(1);
    expect(server.requests.some(({ method }) => method === "plugin.pane.open")).toBe(false);
  });

  it("preserves the previous image when a later image fails validation", async () => {
    const server = await start({ withViewer: true });
    const client = new HerdrSocketClient(server.socketPath);
    expect((await publish(server, client, image(), "w1:p2")).ok).toBe(true);
    const previous = server.getGraphics("w1:p2");

    const invalid = image({ buffer: pngBuffer(POLICY_LIMITS.rawPngBytes + 1) });
    const result = await publish(server, client, invalid, "w1:p2");
    expect(result.ok).toBe(false);
    if (!result.ok) expect(result.error).toMatchObject({ code: "image_too_large" });
    expect(server.getGraphics("w1:p2")).toEqual(previous);
    expect(server.requests.filter(({ method }) => method === "pane.graphics.set")).toHaveLength(1);
    expect(server.requests.some(({ method }) => method === "pane.graphics.clear")).toBe(false);
  });

  it("preserves the previous image when cell dimensions become unavailable", async () => {
    const server = await start({ withViewer: true });
    const client = new HerdrSocketClient(server.socketPath);
    expect((await publish(server, client, image(), "w1:p2")).ok).toBe(true);
    const previous = server.getGraphics("w1:p2");
    server.setGraphicsCapability({ cellWidthPx: 0 });

    expect(await publish(server, client, image(), "w1:p2")).toEqual({
      ok: false,
      error: { code: "cell_size_unavailable", retryable: false }
    });
    expect(server.getGraphics("w1:p2")).toEqual(previous);
    expect(server.requests.filter(({ method }) => method === "pane.graphics.set")).toHaveLength(1);
    expect(server.requests.some(({ method }) => method === "pane.graphics.clear")).toBe(false);
  });

  it("preserves the previous layer when graphics are disabled between validation and commit", async () => {
    const server = await start({ withViewer: true });
    const client = new HerdrSocketClient(server.socketPath);
    expect((await publish(server, client, image(), "w1:p2")).ok).toBe(true);
    const previous = server.getGraphics("w1:p2");
    server.queueResponse("pane.graphics.set", { error: { code: "feature_disabled", message: "disabled" } });

    expect(await publish(server, client, image(), "w1:p2")).toEqual({
      ok: false,
      error: { code: "graphics_disabled", retryable: false }
    });
    expect(server.getGraphics("w1:p2")).toEqual(previous);
    expect(server.requests.some(({ method }) => method === "pane.graphics.clear")).toBe(false);
  });
});

async function start(
  options: { withViewer?: boolean; graphics?: FakeHerdrServerOptions["graphics"] } = {}
): Promise<FakeHerdrServer> {
  const server = await FakeHerdrServer.start({
    panes: [createFakePane({ agent_status: "done" })],
    ...(options.graphics === undefined ? {} : { graphics: options.graphics })
  });
  servers.add(server);
  if (options.withViewer === true) {
    server.addPane(
      createFakePane({
        pane_id: "w1:p2",
        terminal_id: "term-2",
        agent: null,
        focused: false,
        title: VIEWER_IDENTITY.title,
        tokens: {
          [VIEWER_IDENTITY.ownerTokenKey]: VIEWER_IDENTITY.ownerToken,
          [VIEWER_IDENTITY.sourceTokenKey]: deriveViewerSourceToken(server.socketPath, "w1:p1")
        }
      })
    );
  }
  return server;
}

function publish(
  server: FakeHerdrServer,
  client: HerdrSocketClient,
  renderedImage: RenderedImage,
  existingViewerPaneId?: string
) {
  const presenter = new ViewerPresenter(client);
  return publishImage(
    {
      sourcePaneId: "w1:p1",
      workspaceId: "w1",
      generation: 1,
      image: renderedImage,
      ...(existingViewerPaneId === undefined ? {} : { existingViewerPaneId })
    },
    {
      client,
      sessionIdentity: server.socketPath,
      present: (presentation) => presenter.present(presentation)
    }
  );
}

function image(overrides: Partial<RenderedImage> = {}): RenderedImage {
  const buffer = overrides.buffer ?? pngBuffer(16);
  return {
    buffer,
    width: overrides.width ?? 640,
    height: overrides.height ?? 320,
    bytes: overrides.bytes ?? buffer.byteLength,
    renderer: overrides.renderer ?? "test-renderer"
  };
}

function pngBuffer(bytes: number): Buffer {
  const buffer = Buffer.alloc(bytes);
  PNG_SIGNATURE.copy(buffer);
  return buffer;
}
