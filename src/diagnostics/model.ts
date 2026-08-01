import { Buffer } from "node:buffer";
import { isAbsolute } from "node:path";

import type { OperationResult } from "../core/contracts.js";
import { POLICY_LIMITS } from "../core/limits.js";
import { HERDR_CLIENT_LIMITS, type HerdrGraphicsInfo, type HerdrServerInfo } from "../herdr/socket-client.js";
import { VIEWER_IDENTITY } from "../viewer/ownership.js";

const PLUGIN_VERSION = "0.1.0";
const MINIMUM_HERDR_VERSION = "0.7.5";
const HERDR_PROTOCOL = 17;
const IDENTIFIER = /^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$/;

export interface DiagnoseEnvironment {
  HERDR_SOCKET_PATH?: string | undefined;
  HERDR_BIN_PATH?: string | undefined;
  HERDR_PLUGIN_ID?: string | undefined;
  HERDR_PLUGIN_ROOT?: string | undefined;
  HERDR_PLUGIN_CONFIG_DIR?: string | undefined;
  HERDR_PLUGIN_STATE_DIR?: string | undefined;
  HERDR_PLUGIN_CONTEXT_JSON?: string | undefined;
}

export interface DiagnosticCheck {
  id: string;
  status: "pass" | "info" | "fail";
  code: string;
  message: string;
  action?: string;
}

export interface DiagnosticReport {
  schemaVersion: 1;
  plugin: "Herdr Math";
  pluginVersion: string;
  minimumHerdrVersion: string;
  expectedHerdrProtocol: number;
  herdrVersion?: string;
  herdrProtocol?: number;
  outcome: "ok" | "failed";
  checks: readonly DiagnosticCheck[];
}

export interface DiagnosticContext {
  socketPath: string;
  configDirectory: string;
  stateDirectory: string;
  sourcePaneId: string;
  workspaceId: string;
}

export function parseDiagnosticEnvironment(environment: DiagnoseEnvironment): DiagnosticContext | null {
  try {
    const socketPath = environment.HERDR_SOCKET_PATH;
    const configDirectory = environment.HERDR_PLUGIN_CONFIG_DIR;
    const stateDirectory = environment.HERDR_PLUGIN_STATE_DIR;
    const contextSource = environment.HERDR_PLUGIN_CONTEXT_JSON;
    if (
      environment.HERDR_PLUGIN_ID !== VIEWER_IDENTITY.pluginId ||
      !isAbsoluteSafePath(environment.HERDR_BIN_PATH) ||
      !isAbsoluteSafePath(environment.HERDR_PLUGIN_ROOT) ||
      !isAbsoluteSafePath(configDirectory) ||
      !isAbsoluteSafePath(stateDirectory) ||
      typeof socketPath !== "string" ||
      socketPath.length === 0 ||
      socketPath.includes("\0") ||
      Buffer.byteLength(socketPath, "utf8") > HERDR_CLIENT_LIMITS.socketPathBytes ||
      typeof contextSource !== "string" ||
      Buffer.byteLength(contextSource, "utf8") > POLICY_LIMITS.eventJsonBytes
    ) {
      return null;
    }
    const invocation: unknown = JSON.parse(contextSource);
    if (!isRecord(invocation) || !isIdentifier(invocation.focused_pane_id) || !isIdentifier(invocation.workspace_id)) {
      return null;
    }
    return {
      socketPath,
      configDirectory,
      stateDirectory,
      sourcePaneId: invocation.focused_pane_id,
      workspaceId: invocation.workspace_id
    };
  } catch {
    return null;
  }
}

export function createVersionCheck(server: OperationResult<HerdrServerInfo>): DiagnosticCheck {
  if (!server.ok) {
    return diagnosticCheck(
      "herdr_version",
      "fail",
      server.error.code,
      "Herdr version information is unavailable.",
      "Confirm the Herdr server is running, then run diagnostics again."
    );
  }
  if (compareVersions(server.value.version, MINIMUM_HERDR_VERSION) < 0) {
    return diagnosticCheck(
      "herdr_version",
      "fail",
      "herdr_version_unsupported",
      "The running Herdr version is below the supported minimum.",
      `Upgrade Herdr to version ${MINIMUM_HERDR_VERSION} or newer.`
    );
  }
  if (server.value.protocol !== HERDR_PROTOCOL) {
    return diagnosticCheck(
      "herdr_version",
      "fail",
      "herdr_protocol_unsupported",
      "The running Herdr protocol has not been validated.",
      `Use a Herdr release compatible with protocol ${HERDR_PROTOCOL}.`
    );
  }
  return diagnosticCheck("herdr_version", "pass", "herdr_version_ok", "Herdr version and protocol are supported.");
}

export function createGraphicsChecks(graphics: OperationResult<HerdrGraphicsInfo>): readonly DiagnosticCheck[] {
  if (!graphics.ok) {
    if (graphics.error.code === "graphics_disabled") {
      return [
        diagnosticCheck(
          "graphics",
          "fail",
          "graphics_disabled",
          "Herdr experimental Kitty graphics are disabled.",
          "Set [experimental].kitty_graphics = true in Herdr config, then run herdr server reload-config."
        ),
        diagnosticCheck("cell_size", "info", "cell_size_not_checked", "Cell dimensions were not checked.")
      ];
    }
    return [
      diagnosticCheck(
        "graphics",
        "fail",
        graphics.error.code,
        "Herdr graphics capability could not be checked.",
        "Confirm the Herdr server is running, then run diagnostics again."
      ),
      diagnosticCheck("cell_size", "info", "cell_size_not_checked", "Cell dimensions were not checked.")
    ];
  }
  if (graphics.value.cellWidthPx === 0 || graphics.value.cellHeightPx === 0) {
    return [
      diagnosticCheck("graphics", "pass", "graphics_enabled", "Herdr experimental Kitty graphics are enabled."),
      diagnosticCheck(
        "cell_size",
        "fail",
        "cell_size_unavailable",
        "The attached client does not provide usable cell dimensions.",
        "Reconnect Herdr from a compatible graphics-capable terminal, then run diagnostics again."
      )
    ];
  }
  return [
    diagnosticCheck("graphics", "pass", "graphics_enabled", "Herdr experimental Kitty graphics are enabled."),
    diagnosticCheck("cell_size", "pass", "cell_size_available", "The attached client provides usable cell dimensions.")
  ];
}

export function diagnosticCheck(
  id: string,
  status: DiagnosticCheck["status"],
  code: string,
  message: string,
  action?: string
): DiagnosticCheck {
  return action === undefined ? { id, status, code, message } : { id, status, code, message, action };
}

export function diagnosticNotChecked(): readonly DiagnosticCheck[] {
  return [
    "herdr_version",
    "directories",
    "renderer",
    "graphics",
    "cell_size",
    "viewer_ownership",
    "terminal_support"
  ].map((id) => diagnosticCheck(id, "info", `${id}_not_checked`, "This capability was not checked."));
}

export function createDiagnosticReport(checks: readonly DiagnosticCheck[], server?: HerdrServerInfo): DiagnosticReport {
  return {
    schemaVersion: 1,
    plugin: "Herdr Math",
    pluginVersion: PLUGIN_VERSION,
    minimumHerdrVersion: MINIMUM_HERDR_VERSION,
    expectedHerdrProtocol: HERDR_PROTOCOL,
    ...(server === undefined ? {} : { herdrVersion: server.version, herdrProtocol: server.protocol }),
    outcome: checks.some(({ status }) => status === "fail") ? "failed" : "ok",
    checks
  };
}

function compareVersions(left: string, right: string): number {
  const leftParts = left.split(/[+-]/u, 1)[0]?.split(".").map(Number) ?? [];
  const rightParts = right.split(/[+-]/u, 1)[0]?.split(".").map(Number) ?? [];
  for (let index = 0; index < 3; index += 1) {
    const difference = (leftParts[index] ?? 0) - (rightParts[index] ?? 0);
    if (difference !== 0) return difference;
  }
  const leftPrerelease = left.split("+", 1)[0]?.includes("-") === true;
  const rightPrerelease = right.split("+", 1)[0]?.includes("-") === true;
  return leftPrerelease === rightPrerelease ? 0 : leftPrerelease ? -1 : 1;
}

function isAbsoluteSafePath(value: unknown): value is string {
  return (
    typeof value === "string" &&
    value.length > 0 &&
    !value.includes("\0") &&
    Buffer.byteLength(value, "utf8") <= HERDR_CLIENT_LIMITS.socketPathBytes &&
    isAbsolute(value)
  );
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isIdentifier(value: unknown): value is string {
  return typeof value === "string" && IDENTIFIER.test(value);
}
