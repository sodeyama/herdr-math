import { constants } from "node:fs";
import { access, lstat } from "node:fs/promises";
import { pathToFileURL } from "node:url";

import type { OperationResult } from "./core/contracts.js";
import {
  createDiagnosticReport,
  createGraphicsChecks,
  createVersionCheck,
  diagnosticCheck,
  diagnosticNotChecked,
  parseDiagnosticEnvironment,
  type DiagnoseEnvironment,
  type DiagnosticCheck,
  type DiagnosticContext,
  type DiagnosticReport
} from "./diagnostics/model.js";
import {
  HerdrSocketClient,
  type HerdrGraphicsInfo,
  type HerdrPaneSnapshot,
  type HerdrServerInfo
} from "./herdr/socket-client.js";
import { isRendererRuntimeAvailable } from "./renderer/runtime-check.js";
import { deriveViewerSourceToken, VIEWER_IDENTITY } from "./viewer/ownership.js";

export type { DiagnoseEnvironment, DiagnosticCheck, DiagnosticReport } from "./diagnostics/model.js";

export interface DiagnosticClient {
  ping(): Promise<OperationResult<HerdrServerInfo>>;
  paneGet(paneId: string): Promise<OperationResult<HerdrPaneSnapshot>>;
  paneList(workspaceId: string): Promise<OperationResult<readonly HerdrPaneSnapshot[]>>;
  paneGraphicsInfo(paneId: string): Promise<OperationResult<HerdrGraphicsInfo>>;
}

export interface DiagnosticDependencies {
  client?: DiagnosticClient;
  rendererCheck?: () => Promise<boolean>;
}

export async function runDiagnostics(
  environment: DiagnoseEnvironment,
  dependencies: DiagnosticDependencies = {}
): Promise<DiagnosticReport> {
  const context = parseDiagnosticEnvironment(environment);
  if (context === null) {
    return createDiagnosticReport([
      diagnosticCheck(
        "environment",
        "fail",
        "environment_invalid",
        "Required Herdr plugin context is unavailable or invalid.",
        "Run diagnostics from a pane through the Herdr plugin action."
      ),
      ...diagnosticNotChecked()
    ]);
  }

  const client = dependencies.client ?? new HerdrSocketClient(context.socketPath);
  const rendererCheck = dependencies.rendererCheck ?? isRendererRuntimeAvailable;
  const [server, directories, renderer, source, graphics] = await Promise.all([
    safeCall(() => client.ping()),
    checkDirectories(context.configDirectory, context.stateDirectory),
    safeBoolean(rendererCheck),
    safeCall(() => client.paneGet(context.sourcePaneId)),
    safeCall(() => client.paneGraphicsInfo(context.sourcePaneId))
  ]);
  const checks: DiagnosticCheck[] = [
    diagnosticCheck("environment", "pass", "environment_ok", "Required Herdr plugin context is available."),
    createVersionCheck(server),
    directories
      ? diagnosticCheck("directories", "pass", "directories_ok", "Plugin config and state directories are accessible.")
      : diagnosticCheck(
          "directories",
          "fail",
          "directories_unavailable",
          "Plugin config or state directory access failed.",
          "Repair the plugin config and state directories, then run diagnostics again."
        ),
    renderer
      ? diagnosticCheck("renderer", "pass", "renderer_ok", "Locked renderer dependencies are available.")
      : diagnosticCheck(
          "renderer",
          "fail",
          "renderer_unavailable",
          "Locked renderer dependencies are unavailable.",
          "Reinstall the plugin so its locked renderer dependencies are built."
        )
  ];
  checks.push(...createGraphicsChecks(graphics));
  checks.push(await ownershipCheck(client, context, source));
  checks.push(
    diagnosticCheck(
      "terminal_support",
      "info",
      "terminal_unverified",
      "Cell dimensions do not verify the complete terminal graphics path.",
      "Use only terminals that pass the Herdr Math runtime graphics matrix."
    )
  );
  return createDiagnosticReport(checks, server.ok ? server.value : undefined);
}

async function ownershipCheck(
  client: DiagnosticClient,
  context: DiagnosticContext,
  source: OperationResult<HerdrPaneSnapshot>
): Promise<DiagnosticCheck> {
  if (!source.ok || source.value.workspaceId !== context.workspaceId) {
    return diagnosticCheck(
      "viewer_ownership",
      "fail",
      "viewer_ownership_failed",
      "The focused source pane could not be validated.",
      "Focus the coding-agent pane and run diagnostics again."
    );
  }
  const listed = await safeCall(() => client.paneList(context.workspaceId));
  if (!listed.ok) {
    return diagnosticCheck(
      "viewer_ownership",
      "fail",
      listed.error.code,
      "Viewer ownership could not be checked.",
      "Confirm the Herdr server is running, then run diagnostics again."
    );
  }
  const sourceToken = deriveViewerSourceToken(context.socketPath, context.sourcePaneId);
  const owned = listed.value.filter(
    (pane) =>
      pane.paneId !== context.sourcePaneId &&
      pane.workspaceId === context.workspaceId &&
      pane.tokens?.[VIEWER_IDENTITY.ownerTokenKey] === VIEWER_IDENTITY.ownerToken &&
      pane.tokens[VIEWER_IDENTITY.sourceTokenKey] === sourceToken
  );
  if (owned.length === 0) {
    return diagnosticCheck(
      "viewer_ownership",
      "info",
      "viewer_not_open",
      "No owned Herdr Math viewer is currently open."
    );
  }
  if (owned.length === 1) {
    return diagnosticCheck("viewer_ownership", "pass", "viewer_owned", "Exactly one owned Herdr Math viewer is open.");
  }
  return diagnosticCheck(
    "viewer_ownership",
    "fail",
    "viewer_ownership_failed",
    "Multiple owned Herdr Math viewers were found.",
    "Close duplicate Herdr Math viewer panes, then complete another formula response."
  );
}

async function checkDirectories(configDirectory: string, stateDirectory: string): Promise<boolean> {
  return (await Promise.all([isAccessibleDirectory(configDirectory), isAccessibleDirectory(stateDirectory)])).every(
    Boolean
  );
}

async function isAccessibleDirectory(path: string): Promise<boolean> {
  try {
    const metadata = await lstat(path);
    if (!metadata.isDirectory() || metadata.isSymbolicLink()) return false;
    await access(path, constants.R_OK | constants.W_OK);
    return true;
  } catch {
    return false;
  }
}

async function safeBoolean(operation: () => Promise<boolean>): Promise<boolean> {
  try {
    return (await operation()) === true;
  } catch {
    return false;
  }
}

async function safeCall<T>(operation: () => Promise<OperationResult<T>>): Promise<OperationResult<T>> {
  try {
    return await operation();
  } catch {
    return { ok: false, error: { code: "internal_error", retryable: false } };
  }
}

async function main(): Promise<void> {
  const result = await runDiagnostics({
    HERDR_SOCKET_PATH: process.env.HERDR_SOCKET_PATH,
    HERDR_BIN_PATH: process.env.HERDR_BIN_PATH,
    HERDR_PLUGIN_ID: process.env.HERDR_PLUGIN_ID,
    HERDR_PLUGIN_ROOT: process.env.HERDR_PLUGIN_ROOT,
    HERDR_PLUGIN_CONFIG_DIR: process.env.HERDR_PLUGIN_CONFIG_DIR,
    HERDR_PLUGIN_STATE_DIR: process.env.HERDR_PLUGIN_STATE_DIR,
    HERDR_PLUGIN_CONTEXT_JSON: process.env.HERDR_PLUGIN_CONTEXT_JSON
  });
  process.stdout.write(`${JSON.stringify(result)}\n`);
  if (result.outcome === "failed") process.exitCode = 1;
}

if (process.argv[1] !== undefined && import.meta.url === pathToFileURL(process.argv[1]).href) {
  await main();
}
