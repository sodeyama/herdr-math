import { isAbsolute } from "node:path";
import { pathToFileURL } from "node:url";

import { loadOrCreateFingerprintSecret } from "./boundary/fingerprint-secret.js";
import { failure, success, type OperationResult } from "./core/contracts.js";
import { HerdrMathError, serializeError } from "./core/errors.js";
import { cleanupPluginState, type StartupCleanupOptions, type StartupCleanupResult } from "./state/startup-cleanup.js";

export interface StartupHookEnvironment {
  HERDR_PLUGIN_STATE_DIR?: string | undefined;
}

export async function runStartupHook(
  environment: StartupHookEnvironment,
  options: StartupCleanupOptions = {}
): Promise<OperationResult<StartupCleanupResult>> {
  const stateDirectory = environment.HERDR_PLUGIN_STATE_DIR;
  if (
    stateDirectory === undefined ||
    stateDirectory.length === 0 ||
    stateDirectory.includes("\0") ||
    !isAbsolute(stateDirectory)
  ) {
    return failure(serializeError(new HerdrMathError("event_invalid")));
  }
  try {
    const secret = await loadOrCreateFingerprintSecret(stateDirectory);
    return success(await cleanupPluginState(stateDirectory, secret, options));
  } catch (error) {
    return failure(serializeError(error));
  }
}

async function main(): Promise<void> {
  const result = await runStartupHook({ HERDR_PLUGIN_STATE_DIR: process.env.HERDR_PLUGIN_STATE_DIR });
  const record = result.ok
    ? {
        timestamp: new Date().toISOString(),
        level: "info",
        outcome: "startup_cleanup_complete",
        expired_states: result.value.expiredStates,
        stale_temporary_files: result.value.staleTemporaryFiles,
        stale_locks: result.value.staleLocks
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
