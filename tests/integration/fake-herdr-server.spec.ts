import { readFile } from "node:fs/promises";
import { createConnection } from "node:net";
import { performance } from "node:perf_hooks";

import type { AnySchema } from "ajv";
import { Ajv2020, type ValidateFunction } from "ajv/dist/2020.js";
import { afterEach, describe, expect, it } from "vitest";

import { HerdrSocketClient } from "../../src/herdr/socket-client.js";
import { createViewerMetadata, deriveViewerSourceToken, VIEWER_IDENTITY } from "../../src/viewer/ownership.js";
import { FakeHerdrServer } from "../support/fake-herdr-server.js";
import { createFakePane, type FakeHerdrServerOptions } from "../support/fake-herdr-types.js";

const apiSchema = JSON.parse(
  await readFile(new URL("../fixtures/herdr/api-schema-0.7.5.json", import.meta.url), "utf8")
) as AnySchema;
const ajv = new Ajv2020({
  strict: false,
  formats: { float: true, int32: true, uint: true, uint16: true, uint32: true, uint64: true }
});
ajv.addSchema(apiSchema, "herdr-api");
const validateSuccess = requireValidator(ajv.getSchema("herdr-api#/schemas/success_response"));
const validateError = requireValidator(ajv.getSchema("herdr-api#/schemas/error_response"));
const validateEvent = requireValidator(ajv.getSchema("herdr-api#/schemas/event"));
const validateRequest = requireValidator(ajv.getSchema("herdr-api#/schemas/request"));

const servers = new Set<FakeHerdrServer>();
let requestSequence = 0;

afterEach(async () => {
  for (const server of servers) {
    for (const request of server.requests) {
      expect(validateRequest(request), JSON.stringify(validateRequest.errors)).toBe(true);
    }
  }
  await Promise.all([...servers].map((server) => server.close()));
  servers.clear();
});

describe("FakeHerdrServer", () => {
  it("simulates authoritative pane lifecycle, reads, lists, and layouts", async () => {
    const server = await startFake({ panes: [createFakePane({ agent: "claude", agent_status: "working" })] });
    server.setPaneOutput("w1:p1", "answer: $x^2$", false);
    server.setPaneRect("w1:p1", { x: 0, y: 0, width: 120, height: 40 });
    const event = server.transitionPane("w1:p1", "done");

    expect(validateEvent(event), JSON.stringify(validateEvent.errors)).toBe(true);
    const responses = [
      await call(server, "pane.get", { pane_id: "w1:p1" }),
      await call(server, "pane.list", { workspace_id: "w1" }),
      await call(server, "pane.read", {
        pane_id: "w1:p1",
        source: "recent",
        format: "text",
        lines: 100,
        strip_ansi: true
      }),
      await call(server, "pane.layout", { pane_id: "w1:p1" })
    ];
    for (const response of responses) {
      expect(validateSuccess(response), JSON.stringify(validateSuccess.errors)).toBe(true);
    }
    expect(responses[2]).toMatchObject({
      result: { type: "pane_read", read: { text: "answer: $x^2$", revision: 2, truncated: false } }
    });
    expect(responses[3]).toMatchObject({
      result: { type: "pane_layout", layout: { focused_pane_id: "w1:p1", panes: [{ pane_id: "w1:p1" }] } }
    });

    const clientResult = await new HerdrSocketClient(server.socketPath).paneGet("w1:p1");
    expect(clientResult).toEqual({
      ok: true,
      value: {
        paneId: "w1:p1",
        workspaceId: "w1",
        tabId: "w1:t1",
        focused: true,
        agent: "claude",
        agentSession: null,
        status: "done",
        revision: 2
      }
    });
    expect(server.requests.map(({ method }) => method)).toEqual([
      "pane.get",
      "pane.list",
      "pane.read",
      "pane.layout",
      "pane.get"
    ]);
  });

  it("opens, annotates, discovers, and closes an owned plugin viewer without stealing focus", async () => {
    const server = await startFake({ panes: [createFakePane()] });
    const client = new HerdrSocketClient(server.socketPath);
    const sourceToken = deriveViewerSourceToken(server.socketPath, "w1:p1");
    const opened = await client.pluginPaneOpen({
      pluginId: "io.github.sodeyama.herdr-math",
      entrypointId: "viewer",
      targetPaneId: "w1:p1",
      placement: "split",
      direction: "right",
      focus: false,
      environment: { HERDR_MATH_SOURCE_TOKEN: sourceToken }
    });
    expect(opened.ok).toBe(true);
    if (!opened.ok) throw new Error("Expected the plugin viewer to open");
    const viewerId = opened.value.pane.paneId;
    expect(viewerId).not.toBe("w1:p1");
    expect(server.paneCount).toBe(2);
    expect(server.getPane("w1:p1")?.focused).toBe(true);
    expect(server.getPane(viewerId)?.focused).toBe(false);

    const annotated = await client.paneReportMetadata(viewerId, createViewerMetadata(sourceToken));
    expect(annotated.ok).toBe(true);
    expect(server.getPane(viewerId)).toMatchObject({
      title: "Herdr Math",
      tokens: {
        herdr_math_source: sourceToken,
        herdr_math_owner: VIEWER_IDENTITY.ownerToken
      }
    });

    const listed = await call(server, "pane.list", { workspace_id: "w1" });
    expect(listed).toMatchObject({ result: { panes: [{ pane_id: "w1:p1" }, { pane_id: viewerId }] } });
    const closed = await call(server, "plugin.pane.close", { pane_id: viewerId });
    expect(validateSuccess(closed), JSON.stringify(validateSuccess.errors)).toBe(true);
    expect(server.paneCount).toBe(1);
    const missing = await call(server, "pane.get", { pane_id: viewerId });
    expect(validateError(missing), JSON.stringify(validateError.errors)).toBe(true);
    expect(missing).toMatchObject({ error: { code: "not_found" } });
  });

  it("tracks graphics capability and atomic replacement without an implicit clear", async () => {
    const viewer = createFakePane({ pane_id: "w1:p2", terminal_id: "term-2", focused: false, agent: null });
    const server = await startFake({ panes: [createFakePane(), viewer] });
    const client = new HerdrSocketClient(server.socketPath);
    expect(await client.paneGraphicsInfo("w1:p2")).toEqual({
      ok: true,
      value: { cellWidthPx: 8, cellHeightPx: 16 }
    });
    expect(await client.paneLayout("w1:p2")).toMatchObject({
      ok: true,
      value: { workspaceId: "w1", tabId: "w1:t1", panes: [{ paneId: "w1:p1" }, { paneId: "w1:p2" }] }
    });

    for (const data of ["first-image", "second-image"].map((value) => Buffer.from(value).toString("base64"))) {
      const response = await client.paneGraphicsSet({
        paneId: "w1:p2",
        imageWidth: 640,
        imageHeight: 320,
        dataBase64: data,
        placement: { viewportCol: 0, viewportRow: 0, gridCols: 80, gridRows: 20 }
      });
      expect(response).toEqual({ ok: true, value: undefined });
    }
    expect(server.graphicsUpdates).toHaveLength(2);
    expect(server.getGraphics("w1:p2")?.data_base64).toBe(Buffer.from("second-image").toString("base64"));
    expect(server.requests.some(({ method }) => method === "pane.graphics.clear")).toBe(false);

    server.queueResponse("pane.graphics.set", { error: { code: "payload_rejected", message: "rejected" } });
    const rejected = await call(server, "pane.graphics.set", {
      pane_id: "w1:p2",
      format: "png",
      image_width: 1,
      image_height: 1,
      data_base64: "invalid"
    });
    expect(validateError(rejected), JSON.stringify(validateError.errors)).toBe(true);
    expect(server.getGraphics("w1:p2")?.data_base64).toBe(Buffer.from("second-image").toString("base64"));

    server.setGraphicsCapability({ enabled: false });
    expect(await client.paneGraphicsInfo("w1:p2")).toEqual({
      ok: false,
      error: { code: "graphics_disabled", retryable: false }
    });
    server.setGraphicsCapability({ enabled: true, cellWidthPx: 0, cellHeightPx: 0 });
    expect(await client.paneGraphicsInfo("w1:p2")).toEqual({
      ok: true,
      value: { cellWidthPx: 0, cellHeightPx: 0 }
    });
    server.queueResponse("pane.graphics.info", {
      error: { code: "cell_size_unavailable", message: "host cell size is unavailable" }
    });
    expect(await client.paneGraphicsInfo("w1:p2")).toEqual({
      ok: false,
      error: { code: "cell_size_unavailable", retryable: false }
    });
  });

  it("injects bounded delays, errors, malformed frames, and disconnects in request order", async () => {
    const server = await startFake({ panes: [createFakePane()] });
    server.queueResponse("pane.get", { delayMs: 20 });
    server.queueResponse("pane.get", { error: { code: "busy", message: "retry later" } });
    server.queueResponse("pane.get", { raw: "not-json\n" });
    server.queueResponse("pane.get", { disconnect: true });

    const started = performance.now();
    expect(validateSuccess(await call(server, "pane.get", { pane_id: "w1:p1" }))).toBe(true);
    expect(performance.now() - started).toBeGreaterThanOrEqual(15);
    const error = await call(server, "pane.get", { pane_id: "w1:p1" });
    expect(validateError(error), JSON.stringify(validateError.errors)).toBe(true);
    expect(error).toMatchObject({ error: { code: "busy" } });
    expect((await exchangeLine(server, "pane.get", { pane_id: "w1:p1" }))?.toString("utf8")).toBe("not-json");
    expect(await exchangeLine(server, "pane.get", { pane_id: "w1:p1" })).toBeNull();
    expect(server.requests.map(({ method }) => method)).toEqual(["pane.get", "pane.get", "pane.get", "pane.get"]);
  });
});

async function startFake(options: FakeHerdrServerOptions): Promise<FakeHerdrServer> {
  const server = await FakeHerdrServer.start(options);
  servers.add(server);
  return server;
}

async function call(server: FakeHerdrServer, method: string, params: Record<string, unknown>): Promise<unknown> {
  const line = await exchangeLine(server, method, params);
  if (line === null) throw new Error("Expected a fake Herdr response");
  return JSON.parse(line.toString("utf8")) as unknown;
}

function exchangeLine(
  server: FakeHerdrServer,
  method: string,
  params: Record<string, unknown>
): Promise<Buffer | null> {
  requestSequence += 1;
  const outbound = JSON.stringify({ id: `request-${requestSequence}`, method, params });
  return new Promise((resolve, reject) => {
    const socket = createConnection({ path: server.socketPath });
    let connected = false;
    let settled = false;
    let buffered = Buffer.alloc(0);
    const timeout = setTimeout(() => settle(null), 1000);
    const settle = (value: Buffer | null, error?: Error): void => {
      if (settled) return;
      settled = true;
      clearTimeout(timeout);
      socket.destroy();
      if (error === undefined) resolve(value);
      else reject(error);
    };
    socket.once("connect", () => {
      connected = true;
      socket.write(`${outbound}\n`);
    });
    socket.on("data", (chunk: Buffer) => {
      buffered = Buffer.concat([buffered, chunk]);
      const newlineOffset = buffered.indexOf(0x0a);
      if (newlineOffset !== -1) settle(buffered.subarray(0, newlineOffset));
    });
    socket.once("close", () => settle(null));
    socket.once("end", () => settle(null));
    socket.once("error", (error) => settle(null, connected ? undefined : error));
  });
}

function requireValidator(validator: ValidateFunction | undefined): ValidateFunction {
  if (validator === undefined) throw new Error("Expected schema validator");
  return validator;
}
