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
    const opened = await call(server, "plugin.pane.open", {
      plugin_id: "io.github.sodeyama.herdr-math",
      entrypoint: "viewer",
      placement: "split",
      direction: "right",
      target_pane_id: "w1:p1",
      focus: false
    });
    expect(validateSuccess(opened), JSON.stringify(validateSuccess.errors)).toBe(true);
    const viewerId = resultPaneId(opened);
    expect(viewerId).not.toBe("w1:p1");
    expect(server.paneCount).toBe(2);
    expect(server.getPane("w1:p1")?.focused).toBe(true);
    expect(server.getPane(viewerId)?.focused).toBe(false);

    const sourceToken = deriveViewerSourceToken(server.socketPath, "w1:p1");
    const annotated = await new HerdrSocketClient(server.socketPath).paneReportMetadata(
      viewerId,
      createViewerMetadata(sourceToken)
    );
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
    const info = await call(server, "pane.graphics.info", { pane_id: "w1:p2" });
    expect(validateSuccess(info), JSON.stringify(validateSuccess.errors)).toBe(true);
    expect(info).toMatchObject({ result: { cell_width_px: 8, cell_height_px: 16 } });

    for (const data of ["first-image", "second-image"]) {
      const response = await call(server, "pane.graphics.set", {
        pane_id: "w1:p2",
        format: "png",
        image_width: 640,
        image_height: 320,
        data_base64: data,
        placement: { viewport_col: 0, viewport_row: 0, grid_cols: 80, grid_rows: 20 }
      });
      expect(validateSuccess(response), JSON.stringify(validateSuccess.errors)).toBe(true);
    }
    expect(server.graphicsUpdates).toHaveLength(2);
    expect(server.getGraphics("w1:p2")?.data_base64).toBe("second-image");
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
    expect(server.getGraphics("w1:p2")?.data_base64).toBe("second-image");

    server.setGraphicsCapability({ enabled: false });
    const disabled = await call(server, "pane.graphics.info", { pane_id: "w1:p2" });
    expect(validateError(disabled), JSON.stringify(validateError.errors)).toBe(true);
    expect(disabled).toMatchObject({ error: { code: "feature_disabled" } });
    server.setGraphicsCapability({ enabled: true, cellWidthPx: 0, cellHeightPx: 0 });
    expect(await call(server, "pane.graphics.info", { pane_id: "w1:p2" })).toMatchObject({
      result: { cell_width_px: 0, cell_height_px: 0 }
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

function resultPaneId(value: unknown): string {
  if (
    typeof value !== "object" ||
    value === null ||
    !("result" in value) ||
    typeof value.result !== "object" ||
    value.result === null ||
    !("plugin_pane" in value.result) ||
    typeof value.result.plugin_pane !== "object" ||
    value.result.plugin_pane === null ||
    !("pane" in value.result.plugin_pane) ||
    typeof value.result.plugin_pane.pane !== "object" ||
    value.result.plugin_pane.pane === null ||
    !("pane_id" in value.result.plugin_pane.pane) ||
    typeof value.result.plugin_pane.pane.pane_id !== "string"
  ) {
    throw new Error("Expected a plugin pane response");
  }
  return value.result.plugin_pane.pane.pane_id;
}

function requireValidator(validator: ValidateFunction | undefined): ValidateFunction {
  if (validator === undefined) throw new Error("Expected schema validator");
  return validator;
}
