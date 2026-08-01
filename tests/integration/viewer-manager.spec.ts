import { afterEach, describe, expect, it } from "vitest";

import { HerdrSocketClient } from "../../src/herdr/socket-client.js";
import { resolveViewer } from "../../src/viewer/manager.js";
import { createViewerMetadata, deriveViewerSourceToken, VIEWER_IDENTITY } from "../../src/viewer/ownership.js";
import { FakeHerdrServer } from "../support/fake-herdr-server.js";
import { createFakePane, type FakePaneState } from "../support/fake-herdr-types.js";

const servers = new Set<FakeHerdrServer>();

afterEach(async () => {
  await Promise.all([...servers].map((server) => server.close()));
  servers.clear();
});

describe("viewer manager", () => {
  it("reuses a stored viewer only when its ownership metadata matches", async () => {
    const server = await start();
    server.addPane(ownedViewer(server, "w1:p2"));

    expect(await resolveViewer(request(server, "w1:p2"), new HerdrSocketClient(server.socketPath))).toEqual({
      ok: true,
      value: { viewerPaneId: "w1:p2", disposition: "reused_state" }
    });
    expect(server.requests.map(({ method }) => method)).toEqual(["pane.get", "pane.get"]);
    expect(server.paneCount).toBe(2);
  });

  it("ignores an unowned stored pane and recovers the metadata-owned viewer", async () => {
    const userPane = createFakePane({
      pane_id: "w1:p2",
      terminal_id: "term-2",
      agent: null,
      focused: false,
      title: "User notes"
    });
    const server = await start([userPane]);
    server.addPane(ownedViewer(server, "w1:p3"));

    expect(await resolveViewer(request(server, "w1:p2"), new HerdrSocketClient(server.socketPath))).toEqual({
      ok: true,
      value: { viewerPaneId: "w1:p3", disposition: "recovered_metadata" }
    });
    expect(server.getPane("w1:p2")).toMatchObject({ title: "User notes" });
    expect(server.requests.map(({ method }) => method)).toEqual(["pane.get", "pane.get", "pane.list"]);
  });

  it("opens one right split without focus and reuses it after the viewer reports metadata", async () => {
    const server = await start();
    const client = new HerdrSocketClient(server.socketPath);
    const first = await resolveViewer(request(server), client);

    expect(first).toEqual({
      ok: true,
      value: { viewerPaneId: "w1:p2", disposition: "created" }
    });
    expect(server.paneCount).toBe(2);
    expect(server.getPane("w1:p1")?.focused).toBe(true);
    expect(server.getPane("w1:p2")?.focused).toBe(false);
    const openRequest = server.requests.find(({ method }) => method === "plugin.pane.open");
    expect(openRequest?.params).toEqual({
      plugin_id: VIEWER_IDENTITY.pluginId,
      entrypoint: VIEWER_IDENTITY.entrypointId,
      target_pane_id: "w1:p1",
      placement: "split",
      direction: "right",
      focus: false,
      env: { HERDR_MATH_SOURCE_TOKEN: deriveViewerSourceToken(server.socketPath, "w1:p1") }
    });

    await client.paneReportMetadata("w1:p2", createViewerMetadata(deriveViewerSourceToken(server.socketPath, "w1:p1")));
    expect(await resolveViewer(request(server, "w1:p2"), client)).toEqual({
      ok: true,
      value: { viewerPaneId: "w1:p2", disposition: "reused_state" }
    });
    expect(server.paneCount).toBe(2);
    expect(server.requests.filter(({ method }) => method === "plugin.pane.open")).toHaveLength(1);
  });

  it("creates a replacement after the stored viewer closes", async () => {
    const server = await start();
    server.addPane(ownedViewer(server, "w1:p2"));
    server.closePane("w1:p2");

    expect(await resolveViewer(request(server, "w1:p2"), new HerdrSocketClient(server.socketPath))).toEqual({
      ok: true,
      value: { viewerPaneId: "w1:p3", disposition: "created" }
    });
    expect(server.paneCount).toBe(2);
    expect(server.getPane("w1:p1")?.focused).toBe(true);
  });

  it("never modifies or closes an unowned stored pane", async () => {
    const userPane = createFakePane({
      pane_id: "w1:p2",
      terminal_id: "term-2",
      agent: null,
      focused: false,
      title: "Keep me"
    });
    const server = await start([userPane]);

    expect(await resolveViewer(request(server, "w1:p2"), new HerdrSocketClient(server.socketPath))).toEqual({
      ok: true,
      value: { viewerPaneId: "w1:p3", disposition: "created" }
    });
    expect(server.getPane("w1:p2")).toMatchObject({ title: "Keep me" });
    expect(server.requests.some(({ method }) => method === "plugin.pane.close")).toBe(false);
  });

  it("fails closed instead of choosing between duplicate metadata owners", async () => {
    const server = await start();
    server.addPane(ownedViewer(server, "w1:p2"));
    server.addPane(ownedViewer(server, "w1:p3"));

    expect(await resolveViewer(request(server), new HerdrSocketClient(server.socketPath))).toEqual({
      ok: false,
      error: { code: "viewer_ownership_failed", retryable: false }
    });
    expect(server.requests.some(({ method }) => method === "plugin.pane.open")).toBe(false);
  });
});

async function start(extraPanes: FakePaneState[] = []): Promise<FakeHerdrServer> {
  const server = await FakeHerdrServer.start({ panes: [createFakePane({ agent_status: "done" }), ...extraPanes] });
  servers.add(server);
  return server;
}

function ownedViewer(server: FakeHerdrServer, paneId: string): FakePaneState {
  return createFakePane({
    pane_id: paneId,
    terminal_id: `term-${paneId}`,
    focused: false,
    agent: null,
    title: VIEWER_IDENTITY.title,
    tokens: {
      [VIEWER_IDENTITY.ownerTokenKey]: VIEWER_IDENTITY.ownerToken,
      [VIEWER_IDENTITY.sourceTokenKey]: deriveViewerSourceToken(server.socketPath, "w1:p1")
    }
  });
}

function request(server: FakeHerdrServer, existingViewerPaneId?: string) {
  return {
    sessionIdentity: server.socketPath,
    workspaceId: "w1",
    sourcePaneId: "w1:p1",
    ...(existingViewerPaneId === undefined ? {} : { existingViewerPaneId })
  };
}
