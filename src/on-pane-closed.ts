import { isAbsolute } from "node:path";
import { pathToFileURL } from "node:url";

import { loadOrCreateFingerprintSecret } from "./boundary/fingerprint-secret.js";
import { failure, type OperationResult } from "./core/contracts.js";
import { HerdrMathError, serializeError } from "./core/errors.js";
import { processDecodedPaneClosedEvent, type PaneCloseWorkerOutcome } from "./events/pane-close-worker.js";
import { decodePaneClosedEvent } from "./herdr/event-decoder.js";
import { HerdrSocketClient } from "./herdr/socket-client.js";

export interface PaneClosedHookEnvironment {
  HERDR_PLUGIN_EVENT_JSON?: string | undefined;
  HERDR_PLUGIN_STATE_DIR?: string | undefined;
  HERDR_SOCKET_PATH?: string | undefined;
}

export async function runPaneClosedHook(
  environment: PaneClosedHookEnvironment
): Promise<OperationResult<PaneCloseWorkerOutcome>> {
  const decoded = decodePaneClosedEvent(environment.HERDR_PLUGIN_EVENT_JSON ?? "");
  if (!decoded.ok) return failure(decoded.error);
  const stateDirectory = environment.HERDR_PLUGIN_STATE_DIR;
  const socketPath = environment.HERDR_SOCKET_PATH;
  if (
    stateDirectory === undefined ||
    stateDirectory.length === 0 ||
    !isAbsolute(stateDirectory) ||
    stateDirectory.includes("\0") ||
    socketPath === undefined ||
    socketPath.length === 0 ||
    socketPath.includes("\0")
  ) {
    return failure(serializeError(new HerdrMathError("event_invalid")));
  }

  try {
    const secret = await loadOrCreateFingerprintSecret(stateDirectory);
    return processDecodedPaneClosedEvent(decoded.value, {
      client: new HerdrSocketClient(socketPath),
      stateDirectory,
      sessionIdentity: socketPath,
      secret
    });
  } catch (error) {
    return failure(serializeError(error));
  }
}

async function main(): Promise<void> {
  const result = await runPaneClosedHook({
    HERDR_PLUGIN_EVENT_JSON: process.env.HERDR_PLUGIN_EVENT_JSON,
    HERDR_PLUGIN_STATE_DIR: process.env.HERDR_PLUGIN_STATE_DIR,
    HERDR_SOCKET_PATH: process.env.HERDR_SOCKET_PATH
  });
  const record = result.ok
    ? {
        timestamp: new Date().toISOString(),
        level: "info",
        outcome: result.value.kind,
        source_states_removed: result.value.kind === "cleaned" ? result.value.sourceStatesRemoved : 0,
        viewer_mappings_cleared: result.value.kind === "cleaned" ? result.value.viewerMappingsCleared : 0
      }
    : { timestamp: new Date().toISOString(), level: "error", code: result.error.code };
  const output = `${JSON.stringify(record)}\n`;
  if (result.ok) process.stdout.write(output);
  else {
    process.stderr.write(output);
    process.exitCode = 1;
  }
}

if (process.argv[1] !== undefined && import.meta.url === pathToFileURL(process.argv[1]).href) {
  await main();
}
