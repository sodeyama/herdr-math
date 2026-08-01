import { afterEach, describe, expect, it, vi } from "vitest";

import { success } from "../../src/core/contracts.js";
import { HerdrSocketClient, type HerdrPaneSnapshot } from "../../src/herdr/socket-client.js";
import { deriveViewerSourceToken, VIEWER_IDENTITY } from "../../src/viewer/ownership.js";
import { registerViewer, type ViewerEnvironment, type ViewerMetadataClient } from "../../src/viewer/runtime.js";
import { FakeHerdrServer } from "../support/fake-herdr-server.js";
import { createFakePane } from "../support/fake-herdr-types.js";

const servers = new Set<FakeHerdrServer>();

afterEach(async () => {
  await Promise.all([...servers].map((server) => server.close()));
  servers.clear();
  vi.restoreAllMocks();
});

describe("viewer entrypoint", () => {
  it("reports title and ownership tokens for its authoritative Herdr pane", async () => {
    const viewer = createFakePane({ pane_id: "w1:p2", terminal_id: "term-2", focused: false, agent: null });
    const server = await FakeHerdrServer.start({ panes: [createFakePane(), viewer] });
    servers.add(server);
    const sourceToken = deriveViewerSourceToken(server.socketPath, "w1:p1");

    expect(await registerViewer(validEnvironment(server.socketPath, sourceToken))).toEqual({
      ok: true,
      value: { kind: "viewer_ready", paneId: "w1:p2", workspaceId: "w1" }
    });
    expect(server.getPane("w1:p2")).toMatchObject({
      title: "Herdr Math",
      tokens: {
        herdr_math_owner: VIEWER_IDENTITY.ownerToken,
        herdr_math_source: sourceToken
      }
    });
    expect(server.requests).toHaveLength(1);
    expect(server.requests[0]?.id).toMatch(/^[0-9a-f-]{36}$/);
    expect(server.requests[0]).toMatchObject({
      method: "pane.report_metadata",
      params: {
        pane_id: "w1:p2",
        source: VIEWER_IDENTITY.metadataSource,
        title: "Herdr Math",
        tokens: {
          herdr_math_owner: VIEWER_IDENTITY.ownerToken,
          herdr_math_source: sourceToken
        }
      }
    });
  });

  it.each([
    ["plugin", { HERDR_PLUGIN_ID: "other.plugin" }],
    ["entrypoint", { HERDR_PLUGIN_ENTRYPOINT_ID: "other" }],
    ["pane", { HERDR_PANE_ID: "../pane" }],
    ["workspace", { HERDR_WORKSPACE_ID: "" }],
    ["source token", { HERDR_MATH_SOURCE_TOKEN: "private-pane" }],
    ["socket", { HERDR_SOCKET_PATH: "bad\0socket" }]
  ])("rejects an invalid %s before metadata I/O", async (_name, override) => {
    const paneReportMetadata = vi.fn<ViewerMetadataClient["paneReportMetadata"]>();
    const client: ViewerMetadataClient = { paneReportMetadata };

    expect(
      await registerViewer({ ...validEnvironment("/runtime/herdr.sock", "a".repeat(64)), ...override }, client)
    ).toEqual({
      ok: false,
      error: { code: "viewer_ownership_failed", retryable: false }
    });
    expect(paneReportMetadata).not.toHaveBeenCalled();
  });

  it("rejects metadata responses for a different pane or workspace", async () => {
    const response: HerdrPaneSnapshot = {
      paneId: "w1:p3",
      workspaceId: "w2",
      tabId: "w2:t1",
      focused: false,
      agent: null,
      agentSession: null,
      status: "idle",
      revision: 2
    };
    const client: ViewerMetadataClient = {
      paneReportMetadata: vi.fn(() => Promise.resolve(success(response)))
    };

    expect(await registerViewer(validEnvironment("/runtime/herdr.sock", "a".repeat(64)), client)).toEqual({
      ok: false,
      error: { code: "viewer_ownership_failed", retryable: false }
    });
  });

  it("fails closed when the managed viewer pane no longer exists", async () => {
    const server = await FakeHerdrServer.start({ panes: [createFakePane()] });
    servers.add(server);
    const result = await registerViewer(
      validEnvironment(server.socketPath, deriveViewerSourceToken(server.socketPath, "w1:p1"))
    );

    expect(result).toEqual({
      ok: false,
      error: { code: "herdr_protocol_error", retryable: false }
    });
  });

  it("bounds a stalled metadata handshake", async () => {
    const viewer = createFakePane({ pane_id: "w1:p2", terminal_id: "term-2", focused: false, agent: null });
    const server = await FakeHerdrServer.start({ panes: [createFakePane(), viewer] });
    servers.add(server);
    server.queueResponse("pane.report_metadata", { delayMs: 100 });

    expect(
      await registerViewer(
        validEnvironment(server.socketPath, deriveViewerSourceToken(server.socketPath, "w1:p1")),
        new HerdrSocketClient(server.socketPath, { paneMetadataTimeoutMs: 25 })
      )
    ).toEqual({
      ok: false,
      error: { code: "herdr_timeout", retryable: true }
    });
  });
});

function validEnvironment(socketPath: string, sourceToken: string): ViewerEnvironment {
  return {
    HERDR_SOCKET_PATH: socketPath,
    HERDR_PLUGIN_ID: VIEWER_IDENTITY.pluginId,
    HERDR_PLUGIN_ENTRYPOINT_ID: VIEWER_IDENTITY.entrypointId,
    HERDR_PANE_ID: "w1:p2",
    HERDR_WORKSPACE_ID: "w1",
    HERDR_MATH_SOURCE_TOKEN: sourceToken
  };
}
