import { Buffer } from "node:buffer";
import { randomUUID } from "node:crypto";
import { createConnection, type Socket } from "node:net";
import { TextDecoder } from "node:util";

import { failure, success, type OperationResult } from "../core/contracts.js";
import { HerdrMathError, serializeError, type SafeErrorDetails } from "../core/errors.js";
import { POLICY_LIMITS } from "../core/limits.js";
import type { AgentStatus } from "../events/lifecycle.js";

const HERDR_IDENTIFIER = /^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$/;
const AGENT_STATUSES = new Set<AgentStatus>(["working", "blocked", "done", "idle", "unknown"]);
const UTF8_DECODER = new TextDecoder("utf-8", { fatal: true });

export const HERDR_CLIENT_LIMITS = Object.freeze({
  socketPathBytes: 4096,
  paneGetTimeoutMs: 2000,
  paneReadTimeoutMs: 2000,
  paneListTimeoutMs: 2000,
  paneMetadataTimeoutMs: 2000,
  pluginPaneOpenTimeoutMs: 5000,
  agentSessionValueBytes: 4096
});

export interface HerdrAgentSessionRef {
  source: string;
  agent: string;
  kind: "id" | "path";
  value: string;
}

export interface HerdrPaneSnapshot {
  paneId: string;
  workspaceId: string;
  tabId: string;
  focused: boolean;
  agent: string | null;
  agentSession: HerdrAgentSessionRef | null;
  status: AgentStatus;
  revision: number;
  title?: string | null;
  tokens?: Readonly<Record<string, string>>;
}

export interface HerdrPaneReadSnapshot {
  paneId: string;
  workspaceId: string;
  text: string;
  revision: number;
  truncated: boolean;
}

export interface HerdrSocketClientOptions {
  paneGetTimeoutMs?: number;
  paneReadTimeoutMs?: number;
  paneListTimeoutMs?: number;
  paneMetadataTimeoutMs?: number;
  pluginPaneOpenTimeoutMs?: number;
  responseBytes?: number;
}

export interface HerdrPaneMetadataReport {
  source: string;
  title: string;
  tokens: Readonly<Record<string, string>>;
}

export interface HerdrPluginPaneOpenRequest {
  pluginId: string;
  entrypointId: string;
  workspaceId: string;
  targetPaneId: string;
  placement: "split";
  direction: "right" | "down";
  focus: false;
  environment: Readonly<Record<string, string>>;
}

export interface HerdrPluginPaneSnapshot {
  pluginId: string;
  entrypointId: string;
  pane: HerdrPaneSnapshot;
}

type HerdrRequest =
  | { id: string; method: "pane.get"; params: { pane_id: string } }
  | { id: string; method: "pane.list"; params: { workspace_id: string } }
  | {
      id: string;
      method: "pane.read";
      params: { pane_id: string; source: "recent-unwrapped"; format: "text"; lines: number; strip_ansi: true };
    }
  | { id: string; method: "pane.report_metadata"; params: { pane_id: string } & HerdrPaneMetadataReport }
  | {
      id: string;
      method: "plugin.pane.open";
      params: {
        plugin_id: string;
        entrypoint: string;
        workspace_id: string;
        target_pane_id: string;
        placement: "split";
        direction: "right" | "down";
        focus: false;
        env: Record<string, string>;
      };
    };

export class HerdrSocketClient {
  readonly #paneGetTimeoutMs: number;
  readonly #paneReadTimeoutMs: number;
  readonly #paneListTimeoutMs: number;
  readonly #paneMetadataTimeoutMs: number;
  readonly #pluginPaneOpenTimeoutMs: number;
  readonly #responseBytes: number;

  constructor(
    readonly socketPath: string,
    options: HerdrSocketClientOptions = {}
  ) {
    this.#paneGetTimeoutMs = boundedOverride(
      options.paneGetTimeoutMs,
      HERDR_CLIENT_LIMITS.paneGetTimeoutMs,
      "paneGetTimeoutMs"
    );
    this.#paneReadTimeoutMs = boundedOverride(
      options.paneReadTimeoutMs,
      HERDR_CLIENT_LIMITS.paneReadTimeoutMs,
      "paneReadTimeoutMs"
    );
    this.#paneListTimeoutMs = boundedOverride(
      options.paneListTimeoutMs,
      HERDR_CLIENT_LIMITS.paneListTimeoutMs,
      "paneListTimeoutMs"
    );
    this.#paneMetadataTimeoutMs = boundedOverride(
      options.paneMetadataTimeoutMs,
      HERDR_CLIENT_LIMITS.paneMetadataTimeoutMs,
      "paneMetadataTimeoutMs"
    );
    this.#pluginPaneOpenTimeoutMs = boundedOverride(
      options.pluginPaneOpenTimeoutMs,
      HERDR_CLIENT_LIMITS.pluginPaneOpenTimeoutMs,
      "pluginPaneOpenTimeoutMs"
    );
    this.#responseBytes = boundedOverride(options.responseBytes, POLICY_LIMITS.socketResponseBytes, "responseBytes");
  }

  paneGet(paneId: string): Promise<OperationResult<HerdrPaneSnapshot>> {
    if (!isSocketPath(this.socketPath) || !isIdentifier(paneId)) {
      return Promise.resolve(protocolFailure());
    }

    const outbound: HerdrRequest = {
      id: randomUUID(),
      method: "pane.get",
      params: { pane_id: paneId }
    };
    return this.#request(outbound, this.#paneGetTimeoutMs, parsePaneGetResult);
  }

  paneGetIfPresent(paneId: string): Promise<OperationResult<HerdrPaneSnapshot | null>> {
    if (!isSocketPath(this.socketPath) || !isIdentifier(paneId)) {
      return Promise.resolve(protocolFailure());
    }
    const outbound: HerdrRequest = {
      id: randomUUID(),
      method: "pane.get",
      params: { pane_id: paneId }
    };
    return this.#request(outbound, this.#paneGetTimeoutMs, parsePaneGetResult, (remote) => {
      if (remote.code === "not_found") return null;
      throw new HerdrMathError("herdr_protocol_error");
    });
  }

  paneList(workspaceId: string): Promise<OperationResult<readonly HerdrPaneSnapshot[]>> {
    if (!isSocketPath(this.socketPath) || !isIdentifier(workspaceId)) {
      return Promise.resolve(protocolFailure());
    }
    const outbound: HerdrRequest = {
      id: randomUUID(),
      method: "pane.list",
      params: { workspace_id: workspaceId }
    };
    return this.#request(outbound, this.#paneListTimeoutMs, parsePaneListResult);
  }

  paneRead(paneId: string): Promise<OperationResult<HerdrPaneReadSnapshot>> {
    if (!isSocketPath(this.socketPath) || !isIdentifier(paneId)) {
      return Promise.resolve(protocolFailure());
    }
    const outbound: HerdrRequest = {
      id: randomUUID(),
      method: "pane.read",
      params: {
        pane_id: paneId,
        source: "recent-unwrapped",
        format: "text",
        lines: POLICY_LIMITS.paneReadLines,
        strip_ansi: true
      }
    };
    return this.#request(outbound, this.#paneReadTimeoutMs, parsePaneReadResult);
  }

  async paneReportMetadata(
    paneId: string,
    report: HerdrPaneMetadataReport
  ): Promise<OperationResult<HerdrPaneSnapshot>> {
    if (!isSocketPath(this.socketPath) || !isIdentifier(paneId) || !isMetadataReport(report)) {
      return protocolFailure();
    }
    const outbound: HerdrRequest = {
      id: randomUUID(),
      method: "pane.report_metadata",
      params: {
        pane_id: paneId,
        source: report.source,
        title: report.title,
        tokens: { ...report.tokens }
      }
    };
    const response = await this.#request(outbound, this.#paneMetadataTimeoutMs, parsePaneGetResult);
    if (!response.ok || response.value.paneId === paneId) return response;
    return protocolFailure();
  }

  pluginPaneOpen(request: HerdrPluginPaneOpenRequest): Promise<OperationResult<HerdrPluginPaneSnapshot>> {
    if (!isSocketPath(this.socketPath) || !isPluginPaneOpenRequest(request)) {
      return Promise.resolve(protocolFailure());
    }
    const outbound: HerdrRequest = {
      id: randomUUID(),
      method: "plugin.pane.open",
      params: {
        plugin_id: request.pluginId,
        entrypoint: request.entrypointId,
        workspace_id: request.workspaceId,
        target_pane_id: request.targetPaneId,
        placement: request.placement,
        direction: request.direction,
        focus: request.focus,
        env: { ...request.environment }
      }
    };
    return this.#request(outbound, this.#pluginPaneOpenTimeoutMs, parsePluginPaneOpenResult);
  }

  #request<T>(
    outbound: HerdrRequest,
    timeoutMs: number,
    parseResult: (value: unknown) => T,
    parseRemoteError?: (error: HerdrWireError) => T
  ): Promise<OperationResult<T>> {
    return new Promise((resolve) => {
      let socket: Socket;
      try {
        socket = createConnection({ path: this.socketPath });
      } catch {
        resolve(protocolFailure(true));
        return;
      }

      let settled = false;
      let receivedBytes = 0;
      const chunks: Buffer[] = [];
      const timer = setTimeout(() => {
        settle(failure(serializeError(new HerdrMathError("herdr_timeout", {}, true))));
      }, timeoutMs);
      timer.unref();

      const settle = (result: OperationResult<T>): void => {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        socket.destroy();
        resolve(result);
      };

      socket.once("connect", () => {
        try {
          socket.write(`${JSON.stringify(outbound)}\n`);
        } catch {
          settle(protocolFailure(true));
        }
      });

      socket.on("data", (chunk: Buffer) => {
        if (settled) return;
        receivedBytes += chunk.byteLength;
        if (receivedBytes > this.#responseBytes) {
          settle(
            protocolFailure(false, {
              limit_kind: "socket_response_bytes",
              limit: this.#responseBytes,
              actual: receivedBytes
            })
          );
          return;
        }

        const newlineOffset = chunk.indexOf(0x0a);
        if (newlineOffset === -1) {
          chunks.push(chunk);
          return;
        }
        if (newlineOffset !== chunk.byteLength - 1) {
          settle(protocolFailure());
          return;
        }

        chunks.push(chunk.subarray(0, newlineOffset));
        let line = Buffer.concat(chunks, receivedBytes - 1);
        if (line.at(-1) === 0x0d) line = line.subarray(0, -1);
        if (line.byteLength === 0) {
          settle(protocolFailure());
          return;
        }

        try {
          const parsed: unknown = JSON.parse(UTF8_DECODER.decode(line));
          settle(success(parseResponse(parsed, outbound.id, parseResult, parseRemoteError)));
        } catch (error) {
          settle(
            failure(
              serializeError(error instanceof HerdrMathError ? error : new HerdrMathError("herdr_protocol_error"))
            )
          );
        }
      });

      socket.once("error", () => settle(protocolFailure(true)));
      socket.once("end", () => settle(protocolFailure(true)));
      socket.once("close", () => settle(protocolFailure(true)));
    });
  }
}

interface HerdrWireError {
  code: string;
  message: string;
}

function parseResponse<T>(
  value: unknown,
  requestId: string,
  parseResult: (result: unknown) => T,
  parseRemoteError?: (error: HerdrWireError) => T
): T {
  if (!isRecord(value) || value.id !== requestId) {
    throw new HerdrMathError("herdr_protocol_error");
  }

  const hasResult = Object.hasOwn(value, "result");
  const hasError = Object.hasOwn(value, "error");
  if (hasResult === hasError) {
    throw new HerdrMathError("herdr_protocol_error");
  }
  if (hasError) {
    if (!isHerdrError(value.error)) throw new HerdrMathError("herdr_protocol_error");
    if (parseRemoteError !== undefined) return parseRemoteError(value.error);
    throw new HerdrMathError("herdr_protocol_error");
  }
  return parseResult(value.result);
}

function parsePaneGetResult(value: unknown): HerdrPaneSnapshot {
  if (!isRecord(value) || value.type !== "pane_info" || !isRecord(value.pane)) {
    throw new HerdrMathError("herdr_protocol_error");
  }
  return parsePaneInfo(value.pane);
}

function parsePaneInfo(pane: Record<string, unknown>): HerdrPaneSnapshot {
  const agentSession = parseAgentSession(pane.agent_session);
  const tokens = parseMetadataTokens(pane.tokens);
  if (
    !isIdentifier(pane.pane_id) ||
    !isIdentifier(pane.terminal_id) ||
    !isIdentifier(pane.workspace_id) ||
    !isIdentifier(pane.tab_id) ||
    typeof pane.focused !== "boolean" ||
    !isAgentStatus(pane.agent_status) ||
    (pane.agent !== undefined && pane.agent !== null && !isIdentifier(pane.agent)) ||
    agentSession === undefined ||
    tokens === undefined ||
    (pane.title !== undefined && pane.title !== null && typeof pane.title !== "string") ||
    !Number.isSafeInteger(pane.revision) ||
    (pane.revision as number) < 0
  ) {
    throw new HerdrMathError("herdr_protocol_error");
  }

  const snapshot: HerdrPaneSnapshot = {
    paneId: pane.pane_id,
    workspaceId: pane.workspace_id,
    tabId: pane.tab_id,
    focused: pane.focused,
    agent: typeof pane.agent === "string" ? pane.agent : null,
    agentSession,
    status: pane.agent_status,
    revision: pane.revision as number
  };
  if (pane.title !== undefined) snapshot.title = pane.title;
  if (tokens !== null) snapshot.tokens = tokens;
  return Object.freeze(snapshot);
}

function parsePaneListResult(value: unknown): readonly HerdrPaneSnapshot[] {
  if (!isRecord(value) || value.type !== "pane_list" || !Array.isArray(value.panes)) {
    throw new HerdrMathError("herdr_protocol_error");
  }
  if (value.panes.length > 4096) throw new HerdrMathError("herdr_protocol_error");
  return Object.freeze(
    value.panes.map((pane) => {
      if (!isRecord(pane)) throw new HerdrMathError("herdr_protocol_error");
      return parsePaneInfo(pane);
    })
  );
}

function parsePluginPaneOpenResult(value: unknown): HerdrPluginPaneSnapshot {
  if (
    !isRecord(value) ||
    value.type !== "plugin_pane_opened" ||
    !isRecord(value.plugin_pane) ||
    !isIdentifier(value.plugin_pane.plugin_id) ||
    !isIdentifier(value.plugin_pane.entrypoint) ||
    !isRecord(value.plugin_pane.pane)
  ) {
    throw new HerdrMathError("herdr_protocol_error");
  }
  return Object.freeze({
    pluginId: value.plugin_pane.plugin_id,
    entrypointId: value.plugin_pane.entrypoint,
    pane: parsePaneInfo(value.plugin_pane.pane)
  });
}

function parsePaneReadResult(value: unknown): HerdrPaneReadSnapshot {
  if (!isRecord(value) || value.type !== "pane_read" || !isRecord(value.read)) {
    throw new HerdrMathError("herdr_protocol_error");
  }
  const read = value.read;
  if (
    !isIdentifier(read.pane_id) ||
    !isIdentifier(read.workspace_id) ||
    !isIdentifier(read.tab_id) ||
    read.source !== "recent-unwrapped" ||
    read.format !== "text" ||
    typeof read.text !== "string" ||
    !Number.isSafeInteger(read.revision) ||
    (read.revision as number) < 0 ||
    typeof read.truncated !== "boolean"
  ) {
    throw new HerdrMathError("herdr_protocol_error");
  }
  const bytes = Buffer.byteLength(read.text, "utf8");
  if (bytes > POLICY_LIMITS.paneReadBytes) {
    throw new HerdrMathError("scanner_input_limit", {
      limit_kind: "pane_read_bytes",
      limit: POLICY_LIMITS.paneReadBytes,
      actual: bytes
    });
  }
  return Object.freeze({
    paneId: read.pane_id,
    workspaceId: read.workspace_id,
    text: read.text,
    revision: read.revision as number,
    truncated: read.truncated
  });
}

function parseAgentSession(value: unknown): HerdrAgentSessionRef | null | undefined {
  if (value === undefined || value === null) return null;
  if (
    !isRecord(value) ||
    !isIdentifier(value.source) ||
    !isIdentifier(value.agent) ||
    (value.kind !== "id" && value.kind !== "path") ||
    typeof value.value !== "string" ||
    value.value.length === 0 ||
    value.value.includes("\0") ||
    Buffer.byteLength(value.value, "utf8") > HERDR_CLIENT_LIMITS.agentSessionValueBytes
  ) {
    return undefined;
  }
  return Object.freeze({ source: value.source, agent: value.agent, kind: value.kind, value: value.value });
}

function protocolFailure<T>(retryable = false, details: SafeErrorDetails = {}): OperationResult<T> {
  return failure(serializeError(new HerdrMathError("herdr_protocol_error", details, retryable)));
}

function boundedOverride(value: number | undefined, maximum: number, name: string): number {
  if (value === undefined) return maximum;
  if (!Number.isSafeInteger(value) || value < 1 || value > maximum) {
    throw new TypeError(`${name} must be a positive integer no greater than its policy limit`);
  }
  return value;
}

function isSocketPath(value: unknown): value is string {
  return (
    typeof value === "string" &&
    value.length > 0 &&
    !value.includes("\0") &&
    Buffer.byteLength(value, "utf8") <= HERDR_CLIENT_LIMITS.socketPathBytes
  );
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isIdentifier(value: unknown): value is string {
  return typeof value === "string" && HERDR_IDENTIFIER.test(value);
}

function isAgentStatus(value: unknown): value is AgentStatus {
  return typeof value === "string" && AGENT_STATUSES.has(value as AgentStatus);
}

function isHerdrError(value: unknown): value is HerdrWireError {
  return isRecord(value) && isIdentifier(value.code) && typeof value.message === "string";
}

function isMetadataReport(value: unknown): value is HerdrPaneMetadataReport {
  if (
    !isRecord(value) ||
    typeof value.source !== "string" ||
    !/^[A-Za-z0-9:._-]{1,80}$/.test(value.source) ||
    typeof value.title !== "string" ||
    value.title.length === 0 ||
    [...value.title].length > 80 ||
    containsMetadataControl(value.title) ||
    !isRecord(value.tokens)
  ) {
    return false;
  }
  const entries = Object.entries(value.tokens);
  return (
    entries.length <= 16 &&
    entries.every(
      ([key, token]) =>
        /^[A-Za-z0-9_-]{1,32}$/.test(key) &&
        typeof token === "string" &&
        token.length > 0 &&
        [...token].length <= 80 &&
        !containsMetadataControl(token)
    )
  );
}

function parseMetadataTokens(value: unknown): Readonly<Record<string, string>> | null | undefined {
  if (value === undefined) return null;
  if (!isRecord(value)) return undefined;
  const entries = Object.entries(value);
  if (
    entries.length > 32 ||
    entries.some(
      ([key, token]) =>
        !/^[A-Za-z0-9_-]{1,32}$/.test(key) ||
        typeof token !== "string" ||
        token.length === 0 ||
        [...token].length > 80 ||
        containsMetadataControl(token)
    )
  ) {
    return undefined;
  }
  return Object.freeze(Object.fromEntries(entries) as Record<string, string>);
}

function isPluginPaneOpenRequest(value: unknown): value is HerdrPluginPaneOpenRequest {
  if (
    !isRecord(value) ||
    !isIdentifier(value.pluginId) ||
    !isIdentifier(value.entrypointId) ||
    !isIdentifier(value.workspaceId) ||
    !isIdentifier(value.targetPaneId) ||
    value.placement !== "split" ||
    (value.direction !== "right" && value.direction !== "down") ||
    value.focus !== false ||
    !isRecord(value.environment)
  ) {
    return false;
  }
  const environment = Object.entries(value.environment);
  return (
    environment.length <= 16 &&
    environment.every(
      ([key, entry]) =>
        /^[A-Z][A-Z0-9_]{0,63}$/.test(key) &&
        typeof entry === "string" &&
        !entry.includes("\0") &&
        Buffer.byteLength(entry, "utf8") <= 4096
    )
  );
}

function containsMetadataControl(value: string): boolean {
  return [...value].some((character) => {
    const codePoint = character.codePointAt(0);
    return codePoint !== undefined && (codePoint <= 0x1f || codePoint === 0x7f);
  });
}
