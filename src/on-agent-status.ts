import { pathToFileURL } from "node:url";

import { failure, type OperationResult } from "./core/contracts.js";
import { HerdrMathError, serializeError } from "./core/errors.js";
import { processDecodedAgentStatusEvent, type AgentStatusWorkerOutcome } from "./events/agent-status-worker.js";
import { decodeAgentStatusEvent } from "./herdr/event-decoder.js";
import { HerdrSocketClient } from "./herdr/socket-client.js";
import { publishImage } from "./graphics/publisher.js";
import { renderFormulas } from "./renderer/index.js";
import { loadOrCreateFingerprintSecret } from "./boundary/fingerprint-secret.js";

export interface AgentStatusHookEnvironment {
  HERDR_PLUGIN_EVENT_JSON?: string | undefined;
  HERDR_PLUGIN_STATE_DIR?: string | undefined;
  HERDR_SOCKET_PATH?: string | undefined;
}

export async function runAgentStatusHook(
  environment: AgentStatusHookEnvironment
): Promise<OperationResult<AgentStatusWorkerOutcome>> {
  const source = environment.HERDR_PLUGIN_EVENT_JSON ?? "";
  const decoded = decodeAgentStatusEvent(source);
  if (!decoded.ok) return failure(decoded.error);
  const stateDirectory = environment.HERDR_PLUGIN_STATE_DIR;
  const socketPath = environment.HERDR_SOCKET_PATH;
  if (
    stateDirectory === undefined ||
    stateDirectory.length === 0 ||
    socketPath === undefined ||
    socketPath.length === 0
  ) {
    return failure(serializeError(new HerdrMathError("event_invalid")));
  }

  try {
    const secret = await loadOrCreateFingerprintSecret(stateDirectory);
    const client = new HerdrSocketClient(socketPath);
    return processDecodedAgentStatusEvent(decoded.value, {
      client,
      stateDirectory,
      sessionIdentity: socketPath,
      secret,
      render: renderFormulas,
      publish: (request) => publishImage(request, { client, sessionIdentity: socketPath })
    });
  } catch (error) {
    return failure(serializeError(error));
  }
}

async function main(): Promise<void> {
  const result = await runAgentStatusHook({
    HERDR_PLUGIN_EVENT_JSON: process.env.HERDR_PLUGIN_EVENT_JSON,
    HERDR_PLUGIN_STATE_DIR: process.env.HERDR_PLUGIN_STATE_DIR,
    HERDR_SOCKET_PATH: process.env.HERDR_SOCKET_PATH
  });
  const record = result.ok
    ? { timestamp: new Date().toISOString(), level: "info", outcome: result.value.kind }
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
