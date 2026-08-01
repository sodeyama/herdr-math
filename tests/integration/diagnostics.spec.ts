import { mkdtemp, mkdir, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterEach, describe, expect, it, vi } from "vitest";

import { success } from "../../src/core/contracts.js";
import {
  runDiagnostics,
  type DiagnoseEnvironment,
  type DiagnosticCheck,
  type DiagnosticClient
} from "../../src/diagnose.js";
import { HerdrSocketClient } from "../../src/herdr/socket-client.js";
import { deriveViewerSourceToken, VIEWER_IDENTITY } from "../../src/viewer/ownership.js";
import { FakeHerdrServer } from "../support/fake-herdr-server.js";
import { createFakePane } from "../support/fake-herdr-types.js";

const servers = new Set<FakeHerdrServer>();
const directories = new Set<string>();

afterEach(async () => {
  await Promise.all([...servers].map((server) => server.close()));
  await Promise.all([...directories].map((directory) => rm(directory, { recursive: true, force: true })));
  servers.clear();
  directories.clear();
  vi.restoreAllMocks();
});

describe("privacy-safe diagnostics", () => {
  it("checks the supported runtime and reports one owned viewer without reading pane output", async () => {
    const server = await start();
    server.addPane(
      createFakePane({
        pane_id: "w1:p2",
        terminal_id: "term-2",
        focused: false,
        agent: null,
        tokens: {
          [VIEWER_IDENTITY.ownerTokenKey]: VIEWER_IDENTITY.ownerToken,
          [VIEWER_IDENTITY.sourceTokenKey]: deriveViewerSourceToken(server.socketPath, "w1:p1")
        }
      })
    );
    const secret = "SECRET_ANSWER_FORMULA_ENV_PATH";
    const environment = await validEnvironment(server, secret);
    Object.assign(environment, { PRIVATE_TOKEN: secret });

    const result = await runDiagnostics(environment, { rendererCheck: () => Promise.resolve(true) });

    expect(result).toMatchObject({
      schemaVersion: 1,
      plugin: "Herdr Math",
      pluginVersion: "0.1.0",
      minimumHerdrVersion: "0.7.5",
      expectedHerdrProtocol: 17,
      herdrVersion: "0.7.5",
      herdrProtocol: 17,
      outcome: "ok"
    });
    expect(result.checks.map(({ code }) => code)).toEqual([
      "environment_ok",
      "herdr_version_ok",
      "directories_ok",
      "renderer_ok",
      "graphics_enabled",
      "cell_size_available",
      "viewer_owned",
      "terminal_unverified"
    ]);
    const output = JSON.stringify(result);
    expect(output).not.toContain(secret);
    expect(output).not.toContain(server.socketPath);
    expect(output).not.toContain(environment.HERDR_PLUGIN_STATE_DIR);
    expect(output).not.toContain("w1:p1");
    expect(new Set(server.requests.map(({ method }) => method))).toEqual(
      new Set(["ping", "pane.get", "pane.graphics.info", "pane.list"])
    );
    expect(server.requests.some(({ method }) => method === "pane.read")).toBe(false);
  });

  it("gives the exact configuration action when graphics are disabled", async () => {
    const server = await start({ enabled: false });
    const result = await runDiagnostics(await validEnvironment(server), {
      rendererCheck: () => Promise.resolve(true)
    });

    expect(byId(result.checks, "graphics")).toEqual({
      id: "graphics",
      status: "fail",
      code: "graphics_disabled",
      message: "Herdr experimental Kitty graphics are disabled.",
      action: "Set [experimental].kitty_graphics = true in Herdr config, then run herdr server reload-config."
    });
    expect(byId(result.checks, "cell_size")).toMatchObject({ status: "info", code: "cell_size_not_checked" });
    expect(result.outcome).toBe("failed");
    expect(server.requests.some(({ method }) => method === "plugin.pane.open")).toBe(false);
  });

  it.each([
    [0, 16],
    [8, 0]
  ])("separates unavailable cell dimensions from disabled graphics", async (cellWidthPx, cellHeightPx) => {
    const server = await start({ enabled: true, cellWidthPx, cellHeightPx });
    const result = await runDiagnostics(await validEnvironment(server), {
      rendererCheck: () => Promise.resolve(true)
    });

    expect(byId(result.checks, "graphics")).toMatchObject({ status: "pass", code: "graphics_enabled" });
    expect(byId(result.checks, "cell_size")).toEqual({
      id: "cell_size",
      status: "fail",
      code: "cell_size_unavailable",
      message: "The attached client does not provide usable cell dimensions.",
      action: "Reconnect Herdr from a compatible graphics-capable terminal, then run diagnostics again."
    });
  });

  it("checks version, protocol, directories, and renderer with stable failures", async () => {
    const server = await start();
    const environment = await validEnvironment(server);
    environment.HERDR_PLUGIN_CONFIG_DIR = join(environment.HERDR_PLUGIN_STATE_DIR as string, "missing");
    const socket = new HerdrSocketClient(server.socketPath);
    const client: DiagnosticClient = {
      ping: () => Promise.resolve(success({ version: "0.7.4", protocol: 17 })),
      paneGet: (paneId) => socket.paneGet(paneId),
      paneList: (workspaceId) => socket.paneList(workspaceId),
      paneGraphicsInfo: (paneId) => socket.paneGraphicsInfo(paneId)
    };

    const result = await runDiagnostics(environment, { client, rendererCheck: () => Promise.resolve(false) });

    expect(byId(result.checks, "herdr_version")).toMatchObject({
      status: "fail",
      code: "herdr_version_unsupported",
      action: "Upgrade Herdr to version 0.7.5 or newer."
    });
    expect(byId(result.checks, "directories")).toMatchObject({ status: "fail", code: "directories_unavailable" });
    expect(byId(result.checks, "renderer")).toMatchObject({ status: "fail", code: "renderer_unavailable" });

    const protocolResult = await runDiagnostics(await validEnvironment(server), {
      client: { ...client, ping: () => Promise.resolve(success({ version: "0.7.5", protocol: 18 })) },
      rendererCheck: () => Promise.resolve(true)
    });
    expect(byId(protocolResult.checks, "herdr_version")).toMatchObject({
      status: "fail",
      code: "herdr_protocol_unsupported",
      action: "Use a Herdr release compatible with protocol 17."
    });

    const prereleaseResult = await runDiagnostics(await validEnvironment(server), {
      client: { ...client, ping: () => Promise.resolve(success({ version: "0.7.5-beta.1", protocol: 17 })) },
      rendererCheck: () => Promise.resolve(true)
    });
    expect(byId(prereleaseResult.checks, "herdr_version")).toMatchObject({
      status: "fail",
      code: "herdr_version_unsupported"
    });
  });

  it("fails before I/O for invalid action context and never serializes supplied values", async () => {
    const secret = "SECRET_INVALID_CONTEXT_AND_EXCEPTION";
    const client = {
      ping: vi.fn(() => Promise.reject(new Error(secret))),
      paneGet: vi.fn(() => Promise.reject(new Error(secret))),
      paneList: vi.fn(() => Promise.reject(new Error(secret))),
      paneGraphicsInfo: vi.fn(() => Promise.reject(new Error(secret)))
    } satisfies DiagnosticClient;
    const result = await runDiagnostics(
      {
        HERDR_SOCKET_PATH: secret,
        HERDR_BIN_PATH: secret,
        HERDR_PLUGIN_ID: secret,
        HERDR_PLUGIN_ROOT: secret,
        HERDR_PLUGIN_CONFIG_DIR: secret,
        HERDR_PLUGIN_STATE_DIR: secret,
        HERDR_PLUGIN_CONTEXT_JSON: JSON.stringify({ selected_text: secret })
      },
      { client, rendererCheck: () => Promise.resolve(true) }
    );

    expect(result.outcome).toBe("failed");
    expect(byId(result.checks, "environment")).toMatchObject({ status: "fail", code: "environment_invalid" });
    expect(JSON.stringify(result)).not.toContain(secret);
    expect(client.ping).not.toHaveBeenCalled();
    expect(client.paneGet).not.toHaveBeenCalled();
  });

  it("maps remote and dependency exceptions without serializing messages", async () => {
    const secret = "SECRET_REMOTE_ERROR_MESSAGE";
    const server = await start();
    server.queueResponse("ping", { error: { code: "unknown", message: secret } });
    const result = await runDiagnostics(await validEnvironment(server, secret), {
      rendererCheck: () => Promise.reject(new Error(secret))
    });

    expect(byId(result.checks, "herdr_version")).toMatchObject({ status: "fail", code: "herdr_protocol_error" });
    expect(byId(result.checks, "renderer")).toMatchObject({ status: "fail", code: "renderer_unavailable" });
    expect(JSON.stringify(result)).not.toContain(secret);
  });
});

async function start(
  graphics: { enabled?: boolean; cellWidthPx?: number; cellHeightPx?: number } = {}
): Promise<FakeHerdrServer> {
  const server = await FakeHerdrServer.start({
    panes: [createFakePane({ agent_status: "done" })],
    graphics
  });
  servers.add(server);
  return server;
}

async function validEnvironment(server: FakeHerdrServer, secret = ""): Promise<DiagnoseEnvironment> {
  const root = await mkdtemp(join(tmpdir(), "herdr-math-diagnose-"));
  directories.add(root);
  const config = join(root, "config");
  const state = join(root, "state");
  await Promise.all([mkdir(config), mkdir(state)]);
  return {
    HERDR_SOCKET_PATH: server.socketPath,
    HERDR_BIN_PATH: process.execPath,
    HERDR_PLUGIN_ID: VIEWER_IDENTITY.pluginId,
    HERDR_PLUGIN_ROOT: process.cwd(),
    HERDR_PLUGIN_CONFIG_DIR: config,
    HERDR_PLUGIN_STATE_DIR: state,
    HERDR_PLUGIN_CONTEXT_JSON: JSON.stringify({
      focused_pane_id: "w1:p1",
      workspace_id: "w1",
      selected_text: secret,
      focused_pane_cwd: secret,
      workspace_cwd: secret
    })
  };
}

function byId(checks: readonly DiagnosticCheck[], id: string): DiagnosticCheck | undefined {
  return checks.find((candidate) => candidate.id === id);
}
