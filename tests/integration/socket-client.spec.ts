import { readFile } from "node:fs/promises";
import { mkdtemp, rm } from "node:fs/promises";
import { createServer, type Server, type Socket } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterEach, describe, expect, it, vi } from "vitest";

import { POLICY_LIMITS } from "../../src/core/limits.js";
import { HERDR_CLIENT_LIMITS, HerdrSocketClient, type HerdrPaneSnapshot } from "../../src/herdr/socket-client.js";

interface WireRequest {
  id: string;
  method: string;
  params: Record<string, unknown>;
}

interface PaneFixture {
  result: {
    pane: {
      pane_id: string;
      terminal_id: string;
      workspace_id: string;
      tab_id: string;
      focused: boolean;
      agent_status: "working" | "blocked" | "done" | "idle" | "unknown";
      agent?: string | null;
      revision: number;
    };
  };
}

interface RunningServer {
  path: string;
  close(): Promise<void>;
}

const cleanup: Array<() => Promise<void>> = [];

afterEach(async () => {
  await Promise.all(cleanup.splice(0).map((close) => close()));
  vi.restoreAllMocks();
});

describe("HerdrSocketClient", () => {
  it("requests and validates bounded server version information", async () => {
    let calls = 0;
    const requests: WireRequest[] = [];
    const server = await startServer((socket, request) => {
      requests.push(request);
      calls += 1;
      const result =
        calls === 1
          ? { type: "pong", version: "0.7.5", protocol: 17 }
          : { type: "pong", version: "SECRET_INVALID_VERSION", protocol: 17 };
      socket.end(`${JSON.stringify({ id: request.id, result })}\n`);
    });
    const client = new HerdrSocketClient(server.path);

    expect(await client.ping()).toEqual({ ok: true, value: { version: "0.7.5", protocol: 17 } });
    expect(await client.ping()).toEqual({
      ok: false,
      error: { code: "herdr_protocol_error", retryable: false }
    });
    expect(requests).toMatchObject([
      { method: "ping", params: {} },
      { method: "ping", params: {} }
    ]);
  });

  it("requests and validates authoritative pane snapshots for all supported agents", async () => {
    const fixtures = JSON.parse(
      await readFile(new URL("../fixtures/herdr/pane-info-responses.json", import.meta.url), "utf8")
    ) as PaneFixture[];
    const panes = new Map(fixtures.map(({ result }) => [result.pane.pane_id, result.pane]));
    const requests: WireRequest[] = [];
    const server = await startServer((socket, request) => {
      requests.push(request);
      const pane = panes.get(String(request.params.pane_id));
      socket.end(`${JSON.stringify({ id: request.id, result: { type: "pane_info", pane, future: true } })}\n`);
    });
    const client = new HerdrSocketClient(server.path);

    for (const { result } of fixtures) {
      const response = await client.paneGet(result.pane.pane_id);
      expect(response).toEqual({
        ok: true,
        value: {
          paneId: result.pane.pane_id,
          workspaceId: result.pane.workspace_id,
          tabId: result.pane.tab_id,
          focused: result.pane.focused,
          agent: result.pane.agent ?? null,
          agentSession: null,
          status: result.pane.agent_status,
          revision: result.pane.revision
        } satisfies HerdrPaneSnapshot
      });
      if (response.ok) expect(Object.isFrozen(response.value)).toBe(true);
    }

    expect(requests).toHaveLength(4);
    for (const [index, request] of requests.entries()) {
      expect(request).toMatchObject({ method: "pane.get", params: { pane_id: fixtures[index]?.result.pane.pane_id } });
      expect(request.id).toMatch(/^[0-9a-f-]{36}$/);
    }
    expect(new Set(requests.map(({ id }) => id))).toHaveLength(requests.length);
  });

  it("supports fragmented CRLF responses and reconnects after each request", async () => {
    let connections = 0;
    const server = await startServer(async (socket, request) => {
      connections += 1;
      const line = JSON.stringify({ id: request.id, result: paneResult("w1:p1") });
      const split = Math.floor(line.length / 2);
      socket.write(line.slice(0, split));
      await new Promise<void>((resolve) => setImmediate(resolve));
      socket.end(`${line.slice(split)}\r\n`);
    });
    const client = new HerdrSocketClient(server.path);

    expect((await client.paneGet("w1:p1")).ok).toBe(true);
    expect((await client.paneGet("w1:p1")).ok).toBe(true);
    expect(connections).toBe(2);
  });

  it("requests one bounded recent-unwrapped pane read", async () => {
    let captured: WireRequest | undefined;
    const server = await startServer((socket, request) => {
      captured = request;
      socket.end(
        `${JSON.stringify({
          id: request.id,
          result: {
            type: "pane_read",
            read: {
              pane_id: "w1:p1",
              workspace_id: "w1",
              tab_id: "w1:t1",
              source: "recent-unwrapped",
              format: "text",
              text: "answer $x$",
              revision: 9,
              truncated: false
            }
          }
        })}\n`
      );
    });

    expect(await new HerdrSocketClient(server.path).paneRead("w1:p1")).toEqual({
      ok: true,
      value: { paneId: "w1:p1", workspaceId: "w1", text: "answer $x$", revision: 9, truncated: false }
    });
    expect(captured).toMatchObject({
      method: "pane.read",
      params: { pane_id: "w1:p1", source: "recent-unwrapped", format: "text", lines: 1000, strip_ansi: true }
    });
  });

  it("maps malformed frames and server errors without exposing their contents", async () => {
    const secret = "SECRET_REQUEST_AND_PATH_SENTINEL";
    const responses: Array<(socket: Socket, request: WireRequest) => void> = [
      (socket) => socket.end("not-json\n"),
      (socket) => socket.end(Buffer.from([0xff, 0x0a])),
      (socket) => socket.end('{"id":"wrong","result":{}}\n'),
      (socket, request) =>
        socket.end(`${JSON.stringify({ id: request.id, result: paneResult("w1:p1"), error: {} })}\n`),
      (socket, request) =>
        socket.end(`${JSON.stringify({ id: request.id, error: { code: "not_found", message: secret } })}\n`),
      (socket, request) => socket.end(`${JSON.stringify({ id: request.id, result: { type: "pane_info" } })}\n`),
      (socket, request) => socket.end(`${JSON.stringify({ id: request.id, result: paneResult("w1:p1") })}\n{}\n`),
      (socket, request) => socket.end(JSON.stringify({ id: request.id, result: paneResult("w1:p1") }))
    ];
    let responseIndex = 0;
    const server = await startServer((socket, request) => {
      const respond = responses[responseIndex++];
      if (respond === undefined) throw new Error("Missing response fixture");
      respond(socket, request);
    });
    const client = new HerdrSocketClient(server.path, { paneGetTimeoutMs: 100 });
    const errorSpy = vi.spyOn(console, "error").mockImplementation(() => undefined);
    const logSpy = vi.spyOn(console, "log").mockImplementation(() => undefined);

    for (const [index] of responses.entries()) {
      const result = await client.paneGet("w1:p1");
      expect(result).toEqual({
        ok: false,
        error: { code: "herdr_protocol_error", retryable: index === responses.length - 1 }
      });
      expect(JSON.stringify(result)).not.toContain(secret);
    }
    expect(errorSpy).not.toHaveBeenCalled();
    expect(logSpy).not.toHaveBeenCalled();
  });

  it("bounds response bytes before parsing", async () => {
    const server = await startServer((socket) => {
      socket.end("x".repeat(1025));
    });
    const client = new HerdrSocketClient(server.path, { responseBytes: 1024 });

    const result = await client.paneGet("w1:p1");

    expect(result).toMatchObject({
      ok: false,
      error: {
        code: "herdr_protocol_error",
        retryable: false,
        details: { limit_kind: "socket_response_bytes", limit: 1024 }
      }
    });
    if (!result.ok) expect(result.error.details?.actual).toBeGreaterThan(1024);
  });

  it("times out a stalled method and recovers with a fresh connection", async () => {
    let connections = 0;
    const server = await startServer((socket, request) => {
      connections += 1;
      if (connections === 1) return;
      socket.end(`${JSON.stringify({ id: request.id, result: paneResult("w1:p1") })}\n`);
    });
    const client = new HerdrSocketClient(server.path, { paneGetTimeoutMs: 25 });

    expect(await client.paneGet("w1:p1")).toEqual({
      ok: false,
      error: { code: "herdr_timeout", retryable: true }
    });
    expect((await client.paneGet("w1:p1")).ok).toBe(true);
    expect(connections).toBe(2);
  });

  it("fails closed on disconnects and invalid inputs without socket fallback", async () => {
    const server = await startServer((socket) => {
      socket.end();
    });
    const client = new HerdrSocketClient(server.path);
    expect(await client.paneGet("w1:p1")).toEqual({
      ok: false,
      error: { code: "herdr_protocol_error", retryable: true }
    });

    const invalidInputs: Array<readonly [string, string]> = [
      ["", "w1:p1"],
      ["bad\0socket", "w1:p1"],
      ["x".repeat(HERDR_CLIENT_LIMITS.socketPathBytes + 1), "w1:p1"],
      [server.path, "../private-pane"]
    ];
    for (const [path, paneId] of invalidInputs) {
      expect(await new HerdrSocketClient(path).paneGet(paneId)).toEqual({
        ok: false,
        error: { code: "herdr_protocol_error", retryable: false }
      });
    }

    const namedPipe = String.raw`\\.\pipe\herdr-session`;
    expect(new HerdrSocketClient(namedPipe, { paneGetTimeoutMs: 1 }).socketPath).toBe(namedPipe);
  });

  it("rejects test overrides that weaken production policy", () => {
    expect(() => new HerdrSocketClient("socket", { pingTimeoutMs: HERDR_CLIENT_LIMITS.pingTimeoutMs + 1 })).toThrow(
      TypeError
    );
    expect(
      () => new HerdrSocketClient("socket", { paneGetTimeoutMs: HERDR_CLIENT_LIMITS.paneGetTimeoutMs + 1 })
    ).toThrow(TypeError);
    expect(
      () => new HerdrSocketClient("socket", { paneReadTimeoutMs: HERDR_CLIENT_LIMITS.paneReadTimeoutMs + 1 })
    ).toThrow(TypeError);
    expect(() => new HerdrSocketClient("socket", { responseBytes: POLICY_LIMITS.socketResponseBytes + 1 })).toThrow(
      TypeError
    );
  });
});

function paneResult(paneId: string): Record<string, unknown> {
  return {
    type: "pane_info",
    pane: {
      pane_id: paneId,
      terminal_id: "term-1",
      workspace_id: "w1",
      tab_id: "w1:t1",
      focused: true,
      agent_status: "done",
      agent: "codex",
      revision: 42
    }
  };
}

async function startServer(
  handler: (socket: Socket, request: WireRequest) => void | Promise<void>
): Promise<RunningServer> {
  const directory = await mkdtemp(join(tmpdir(), "herdr-math-socket-"));
  const path = join(directory, "herdr.sock");
  const sockets = new Set<Socket>();
  const server = createServer((socket) => {
    sockets.add(socket);
    socket.once("close", () => sockets.delete(socket));
    let source = "";
    socket.on("data", (chunk: Buffer) => {
      source += chunk.toString("utf8");
      const newlineOffset = source.indexOf("\n");
      if (newlineOffset === -1) return;
      const line = source.slice(0, newlineOffset);
      source = source.slice(newlineOffset + 1);
      void Promise.resolve(handler(socket, JSON.parse(line) as WireRequest)).catch(() => socket.destroy());
    });
  });
  await listen(server, path);

  let closed = false;
  const close = async (): Promise<void> => {
    if (closed) return;
    closed = true;
    for (const socket of sockets) socket.destroy();
    await closeServer(server);
    await rm(directory, { recursive: true, force: true });
  };
  cleanup.push(close);
  return { path, close };
}

function listen(server: Server, path: string): Promise<void> {
  return new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(path, () => {
      server.off("error", reject);
      resolve();
    });
  });
}

function closeServer(server: Server): Promise<void> {
  return new Promise((resolve, reject) => {
    server.close((error) => (error === undefined ? resolve() : reject(error)));
  });
}
