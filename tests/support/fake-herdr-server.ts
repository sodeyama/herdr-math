import { Buffer } from "node:buffer";
import { randomUUID } from "node:crypto";
import { mkdtemp, rm } from "node:fs/promises";
import { createServer, type Server, type Socket } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";

import type {
  FakeAgentStatus,
  FakeGraphicsCapability,
  FakeGraphicsUpdate,
  FakeHerdrServerOptions,
  FakeLayoutRect,
  FakeLayoutSnapshot,
  FakePaneOutput,
  FakePaneState,
  FakeResponsePlan,
  FakeStatusEvent,
  RecordedHerdrRequest
} from "./fake-herdr-types.js";

const REQUEST_BYTES = 4 * 1024 * 1024;
const DEFAULT_AREA: FakeLayoutRect = { x: 0, y: 0, width: 120, height: 40 };
const READ_SOURCES = new Set(["visible", "recent", "recent_unwrapped", "detection"]);

interface PendingDelay {
  timer: ReturnType<typeof setTimeout>;
  resolve: () => void;
}

interface PluginOwner {
  pluginId: string;
  entrypoint: string;
}

export class FakeHerdrServer {
  readonly socketPath: string;
  readonly #directory: string | null;
  readonly #server: Server;
  readonly #sockets = new Set<Socket>();
  readonly #panes = new Map<string, FakePaneState>();
  readonly #outputs = new Map<string, FakePaneOutput>();
  readonly #rects = new Map<string, FakeLayoutRect>();
  readonly #layouts = new Map<string, FakeLayoutSnapshot>();
  readonly #pluginOwners = new Map<string, PluginOwner>();
  readonly #plans = new Map<string, FakeResponsePlan[]>();
  readonly #recorded: RecordedHerdrRequest[] = [];
  readonly #graphicsUpdates: FakeGraphicsUpdate[] = [];
  readonly #graphicsByPane = new Map<string, FakeGraphicsUpdate>();
  readonly #pendingDelays = new Set<PendingDelay>();
  #graphics: FakeGraphicsCapability;
  #paneSequence = 1;
  #closed = false;

  private constructor(socketPath: string, directory: string | null, options: FakeHerdrServerOptions) {
    this.socketPath = socketPath;
    this.#directory = directory;
    this.#graphics = {
      enabled: options.graphics?.enabled ?? true,
      cellWidthPx: options.graphics?.cellWidthPx ?? 8,
      cellHeightPx: options.graphics?.cellHeightPx ?? 16
    };
    for (const pane of options.panes ?? []) this.addPane(pane);
    this.#server = createServer((socket) => this.#accept(socket));
  }

  static async start(options: FakeHerdrServerOptions = {}): Promise<FakeHerdrServer> {
    const directory = process.platform === "win32" ? null : await mkdtemp(join(tmpdir(), "herdr-math-fake-"));
    const socketPath =
      process.platform === "win32"
        ? String.raw`\\.\pipe\herdr-math-${randomUUID()}`
        : join(directory as string, "herdr.sock");
    const fake = new FakeHerdrServer(socketPath, directory, options);
    try {
      await listen(fake.#server, socketPath);
      return fake;
    } catch (error) {
      if (directory !== null) await rm(directory, { recursive: true, force: true });
      throw error;
    }
  }

  get requests(): readonly RecordedHerdrRequest[] {
    return this.#recorded.map(cloneRequest);
  }

  get graphicsUpdates(): readonly FakeGraphicsUpdate[] {
    return this.#graphicsUpdates.map(cloneGraphics);
  }

  get paneCount(): number {
    return this.#panes.size;
  }

  addPane(pane: FakePaneState): void {
    if (this.#panes.has(pane.pane_id)) throw new Error("Fake pane already exists");
    const copy = clonePane(pane);
    if (copy.focused) this.#clearTabFocus(copy.tab_id);
    this.#panes.set(copy.pane_id, copy);
    this.#outputs.set(copy.pane_id, { text: "", truncated: false });
    this.#paneSequence += 1;
  }

  getPane(paneId: string): FakePaneState | undefined {
    const pane = this.#panes.get(paneId);
    return pane === undefined ? undefined : clonePane(pane);
  }

  updatePane(paneId: string, changes: Partial<Omit<FakePaneState, "pane_id">>): FakePaneState {
    const pane = this.#requirePane(paneId);
    const next = clonePane({ ...pane, ...changes, pane_id: paneId });
    if (changes.revision === undefined) next.revision = pane.revision + 1;
    if (next.focused) this.#clearTabFocus(next.tab_id);
    this.#panes.set(paneId, next);
    return clonePane(next);
  }

  transitionPane(paneId: string, status: FakeAgentStatus, includeAgentHint = true): FakeStatusEvent {
    const current = this.#requirePane(paneId);
    const pane = this.updatePane(paneId, {
      agent_status: status,
      state_change_seq: (current.state_change_seq ?? current.revision) + 1
    });
    const data: FakeStatusEvent["data"] = {
      type: "pane_agent_status_changed",
      workspace_id: pane.workspace_id,
      pane_id: pane.pane_id,
      agent_status: status
    };
    if (includeAgentHint && typeof pane.agent === "string") data.agent = pane.agent;
    return { event: "pane_agent_status_changed", data };
  }

  closePane(paneId: string): boolean {
    if (!this.#panes.delete(paneId)) return false;
    this.#outputs.delete(paneId);
    this.#rects.delete(paneId);
    this.#pluginOwners.delete(paneId);
    this.#graphicsByPane.delete(paneId);
    for (const [tabId, layout] of this.#layouts) {
      if (layout.panes.some((pane) => pane.pane_id === paneId)) this.#layouts.delete(tabId);
    }
    return true;
  }

  setPaneOutput(paneId: string, text: string, truncated = false): void {
    this.#requirePane(paneId);
    this.#outputs.set(paneId, { text, truncated });
  }

  setPaneRect(paneId: string, rect: FakeLayoutRect): void {
    this.#requirePane(paneId);
    this.#rects.set(paneId, { ...rect });
    this.#layouts.delete(this.#requirePane(paneId).tab_id);
  }

  setLayout(layout: FakeLayoutSnapshot): void {
    this.#layouts.set(layout.tab_id, cloneLayout(layout));
  }

  setGraphicsCapability(changes: Partial<FakeGraphicsCapability>): void {
    this.#graphics = { ...this.#graphics, ...changes };
  }

  getGraphics(paneId: string): FakeGraphicsUpdate | undefined {
    const update = this.#graphicsByPane.get(paneId);
    return update === undefined ? undefined : cloneGraphics(update);
  }

  queueResponse(method: string, plan: FakeResponsePlan): void {
    const plans = this.#plans.get(method) ?? [];
    plans.push({ ...plan });
    this.#plans.set(method, plans);
  }

  async close(): Promise<void> {
    if (this.#closed) return;
    this.#closed = true;
    for (const pending of this.#pendingDelays) {
      clearTimeout(pending.timer);
      pending.resolve();
    }
    this.#pendingDelays.clear();
    for (const socket of this.#sockets) socket.destroy();
    await closeServer(this.#server);
    if (this.#directory !== null) await rm(this.#directory, { recursive: true, force: true });
  }

  #accept(socket: Socket): void {
    this.#sockets.add(socket);
    socket.once("close", () => this.#sockets.delete(socket));
    socket.on("error", () => undefined);
    let buffered = Buffer.alloc(0);
    let sequence = Promise.resolve();
    socket.on("data", (chunk: Buffer) => {
      buffered = Buffer.concat([buffered, chunk]);
      if (buffered.byteLength > REQUEST_BYTES) {
        socket.destroy();
        return;
      }
      let newlineOffset = buffered.indexOf(0x0a);
      while (newlineOffset !== -1) {
        const line = buffered.subarray(0, newlineOffset);
        buffered = buffered.subarray(newlineOffset + 1);
        sequence = sequence.then(() => this.#handleLine(socket, line));
        newlineOffset = buffered.indexOf(0x0a);
      }
    });
  }

  async #handleLine(socket: Socket, line: Buffer): Promise<void> {
    let value: unknown;
    try {
      value = JSON.parse(line.toString("utf8"));
    } catch {
      this.#sendError(socket, "unknown", "invalid_request", "invalid request");
      return;
    }
    if (
      !isRecord(value) ||
      typeof value.id !== "string" ||
      typeof value.method !== "string" ||
      !isRecord(value.params)
    ) {
      this.#sendError(socket, "unknown", "invalid_request", "invalid request");
      return;
    }

    const request: RecordedHerdrRequest = {
      id: value.id,
      method: value.method,
      params: structuredClone(value.params)
    };
    this.#recorded.push(request);
    const plan = this.#plans.get(request.method)?.shift();
    if (plan?.delayMs !== undefined) await this.#delay(plan.delayMs);
    if (socket.destroyed || this.#closed) return;
    if (plan?.disconnect === true) {
      socket.destroy();
      return;
    }
    if (plan?.raw !== undefined) {
      socket.end(plan.raw);
      return;
    }
    if (plan?.error !== undefined) {
      this.#sendError(socket, request.id, plan.error.code, plan.error.message);
      return;
    }
    this.#dispatch(socket, request);
  }

  #dispatch(socket: Socket, request: RecordedHerdrRequest): void {
    const paneId = stringParam(request.params, "pane_id");
    switch (request.method) {
      case "ping":
        return this.#sendResult(socket, request.id, { type: "pong", version: "0.7.5", protocol: 17 });
      case "agent.get": {
        const target = stringParam(request.params, "target");
        const pane = target === null ? undefined : this.#panes.get(target);
        if (pane === undefined) return this.#sendError(socket, request.id, "not_found", "agent not found");
        return this.#sendResult(socket, request.id, {
          type: "agent_info",
          agent: { ...clonePane(pane), state_change_seq: pane.state_change_seq ?? pane.revision }
        });
      }
      case "pane.get": {
        const pane = paneId === null ? undefined : this.#panes.get(paneId);
        if (pane === undefined) return this.#sendError(socket, request.id, "not_found", "pane not found");
        return this.#sendResult(socket, request.id, { type: "pane_info", pane: clonePane(pane) });
      }
      case "pane.list": {
        const workspaceId = optionalStringParam(request.params, "workspace_id");
        const panes = [...this.#panes.values()]
          .filter((pane) => workspaceId === null || pane.workspace_id === workspaceId)
          .map(clonePane);
        return this.#sendResult(socket, request.id, { type: "pane_list", panes });
      }
      case "pane.read": {
        const pane = paneId === null ? undefined : this.#panes.get(paneId);
        if (pane === undefined) return this.#sendError(socket, request.id, "not_found", "pane not found");
        if (!READ_SOURCES.has(String(request.params.source))) {
          return this.#sendError(socket, request.id, "invalid_params", "invalid pane read source");
        }
        const output = this.#outputs.get(pane.pane_id) ?? { text: "", truncated: false };
        return this.#sendResult(socket, request.id, {
          type: "pane_read",
          read: {
            pane_id: pane.pane_id,
            workspace_id: pane.workspace_id,
            tab_id: pane.tab_id,
            source: stringParam(request.params, "source") ?? "recent",
            format: stringParam(request.params, "format") ?? "text",
            text: output.text,
            revision: pane.revision,
            truncated: output.truncated
          }
        });
      }
      case "pane.layout": {
        const pane = paneId === null ? undefined : this.#panes.get(paneId);
        if (pane === undefined) return this.#sendError(socket, request.id, "not_found", "pane not found");
        return this.#sendResult(socket, request.id, { type: "pane_layout", layout: this.#layoutFor(pane) });
      }
      case "pane.graphics.info": {
        if (!this.#graphics.enabled)
          return this.#sendError(socket, request.id, "feature_disabled", "graphics disabled");
        if (paneId === null || !this.#panes.has(paneId)) {
          return this.#sendError(socket, request.id, "not_found", "pane not found");
        }
        return this.#sendResult(socket, request.id, {
          type: "pane_graphics_info",
          cell_width_px: this.#graphics.cellWidthPx,
          cell_height_px: this.#graphics.cellHeightPx
        });
      }
      case "pane.graphics.set": {
        if (!this.#graphics.enabled)
          return this.#sendError(socket, request.id, "feature_disabled", "graphics disabled");
        if (paneId === null || !this.#panes.has(paneId)) {
          return this.#sendError(socket, request.id, "not_found", "pane not found");
        }
        const update = parseGraphicsUpdate(request.params);
        if (update === null) return this.#sendError(socket, request.id, "invalid_params", "invalid graphics request");
        this.#graphicsUpdates.push(update);
        this.#graphicsByPane.set(update.pane_id, update);
        return this.#sendResult(socket, request.id, { type: "ok" });
      }
      case "pane.graphics.clear": {
        if (paneId === null || !this.#panes.has(paneId)) {
          return this.#sendError(socket, request.id, "not_found", "pane not found");
        }
        this.#graphicsByPane.delete(paneId);
        return this.#sendResult(socket, request.id, { type: "ok" });
      }
      case "pane.report_metadata":
        return this.#reportMetadata(socket, request, paneId);
      case "plugin.pane.open":
        return this.#openPluginPane(socket, request);
      case "plugin.pane.close":
        return this.#closePluginPane(socket, request, paneId);
      default:
        return this.#sendError(socket, request.id, "method_not_found", "method not found");
    }
  }

  #reportMetadata(socket: Socket, request: RecordedHerdrRequest, paneId: string | null): void {
    const pane = paneId === null ? undefined : this.#panes.get(paneId);
    if (pane === undefined) return this.#sendError(socket, request.id, "not_found", "pane not found");
    const next = clonePane(pane);
    for (const key of ["title", "display_agent"] as const) {
      const value = request.params[key];
      if (typeof value === "string" || value === null) next[key] = value;
    }
    if (isStringRecord(request.params.state_labels)) next.state_labels = { ...request.params.state_labels };
    if (isNullableStringRecord(request.params.tokens)) {
      const tokens = { ...(next.tokens ?? {}) };
      for (const [key, value] of Object.entries(request.params.tokens)) {
        if (value === null) delete tokens[key];
        else tokens[key] = value;
      }
      next.tokens = tokens;
    }
    next.revision += 1;
    this.#panes.set(next.pane_id, next);
    this.#sendResult(socket, request.id, { type: "ok" });
  }

  #openPluginPane(socket: Socket, request: RecordedHerdrRequest): void {
    const pluginId = stringParam(request.params, "plugin_id");
    const entrypoint = stringParam(request.params, "entrypoint");
    const targetId = optionalStringParam(request.params, "target_pane_id");
    const target =
      targetId === null ? [...this.#panes.values()].find(({ focused }) => focused) : this.#panes.get(targetId);
    if (pluginId === null || entrypoint === null || target === undefined || "workspace_id" in request.params) {
      return this.#sendError(socket, request.id, "invalid_params", "invalid plugin pane request");
    }

    const paneId = this.#nextPaneId(target.workspace_id);
    const focused = request.params.focus === true;
    const pane: FakePaneState = {
      pane_id: paneId,
      terminal_id: `term-${this.#paneSequence}`,
      workspace_id: target.workspace_id,
      tab_id: target.tab_id,
      focused,
      agent_status: "idle",
      revision: 1,
      agent: null
    };
    this.addPane(pane);
    this.#pluginOwners.set(paneId, { pluginId, entrypoint });
    this.#sendResult(socket, request.id, {
      type: "plugin_pane_opened",
      plugin_pane: { plugin_id: pluginId, entrypoint, pane: clonePane(this.#requirePane(paneId)) }
    });
  }

  #closePluginPane(socket: Socket, request: RecordedHerdrRequest, paneId: string | null): void {
    if (paneId === null || !this.#pluginOwners.has(paneId)) {
      return this.#sendError(socket, request.id, "not_found", "plugin pane not found");
    }
    this.closePane(paneId);
    this.#sendResult(socket, request.id, { type: "plugin_pane_closed", pane_id: paneId });
  }

  #layoutFor(pane: FakePaneState): FakeLayoutSnapshot {
    const configured = this.#layouts.get(pane.tab_id);
    if (configured !== undefined) return cloneLayout(configured);
    const panes = [...this.#panes.values()].filter(({ tab_id }) => tab_id === pane.tab_id);
    const width = Math.floor(DEFAULT_AREA.width / Math.max(1, panes.length));
    const focused = panes.find(({ focused }) => focused) ?? panes[0] ?? pane;
    return {
      workspace_id: pane.workspace_id,
      tab_id: pane.tab_id,
      zoomed: false,
      area: { ...DEFAULT_AREA },
      focused_pane_id: focused.pane_id,
      panes: panes.map((candidate, index) => ({
        pane_id: candidate.pane_id,
        focused: candidate.focused,
        rect: { ...(this.#rects.get(candidate.pane_id) ?? { x: index * width, y: 0, width, height: 40 }) }
      })),
      splits: []
    };
  }

  #nextPaneId(workspaceId: string): string {
    let candidate: string;
    do candidate = `${workspaceId}:p${this.#paneSequence++}`;
    while (this.#panes.has(candidate));
    return candidate;
  }

  #clearTabFocus(tabId: string): void {
    for (const pane of this.#panes.values()) {
      if (pane.tab_id === tabId) pane.focused = false;
    }
  }

  #requirePane(paneId: string): FakePaneState {
    const pane = this.#panes.get(paneId);
    if (pane === undefined) throw new Error("Fake pane does not exist");
    return pane;
  }

  #sendResult(socket: Socket, id: string, result: Record<string, unknown>): void {
    if (!socket.destroyed) socket.write(`${JSON.stringify({ id, result })}\n`);
  }

  #sendError(socket: Socket, id: string, code: string, message: string): void {
    if (!socket.destroyed) socket.write(`${JSON.stringify({ id, error: { code, message } })}\n`);
  }

  #delay(milliseconds: number): Promise<void> {
    if (!Number.isFinite(milliseconds) || milliseconds <= 0) return Promise.resolve();
    return new Promise((resolve) => {
      const pending: PendingDelay = {
        timer: setTimeout(() => {
          this.#pendingDelays.delete(pending);
          resolve();
        }, milliseconds),
        resolve
      };
      this.#pendingDelays.add(pending);
    });
  }
}

function parseGraphicsUpdate(params: Record<string, unknown>): FakeGraphicsUpdate | null {
  const paneId = stringParam(params, "pane_id");
  const format = stringParam(params, "format");
  const width = numberParam(params, "image_width");
  const height = numberParam(params, "image_height");
  const data = stringParam(params, "data_base64") ?? "";
  const placement = isRecord(params.placement) ? params.placement : {};
  if (paneId === null || !isGraphicsFormat(format) || width === null || height === null) return null;
  return {
    pane_id: paneId,
    format,
    image_width: width,
    image_height: height,
    data_base64: data,
    placement: {
      viewport_col: numberParam(placement, "viewport_col") ?? 0,
      viewport_row: numberParam(placement, "viewport_row") ?? 0,
      grid_cols: numberParam(placement, "grid_cols") ?? 0,
      grid_rows: numberParam(placement, "grid_rows") ?? 0
    }
  };
}

function clonePane(pane: FakePaneState): FakePaneState {
  return structuredClone(pane);
}

function cloneLayout(layout: FakeLayoutSnapshot): FakeLayoutSnapshot {
  return structuredClone(layout);
}

function cloneRequest(request: RecordedHerdrRequest): RecordedHerdrRequest {
  return { id: request.id, method: request.method, params: structuredClone(request.params) };
}

function cloneGraphics(update: FakeGraphicsUpdate): FakeGraphicsUpdate {
  return structuredClone(update);
}

function stringParam(params: Record<string, unknown>, key: string): string | null {
  return typeof params[key] === "string" ? params[key] : null;
}

function optionalStringParam(params: Record<string, unknown>, key: string): string | null {
  const value = params[key];
  return typeof value === "string" ? value : null;
}

function numberParam(params: Record<string, unknown>, key: string): number | null {
  const value = params[key];
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

function isGraphicsFormat(value: string | null): value is FakeGraphicsUpdate["format"] {
  return value === "png" || value === "rgb" || value === "rgba";
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isStringRecord(value: unknown): value is Record<string, string> {
  return isRecord(value) && Object.values(value).every((item) => typeof item === "string");
}

function isNullableStringRecord(value: unknown): value is Record<string, string | null> {
  return isRecord(value) && Object.values(value).every((item) => typeof item === "string" || item === null);
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
