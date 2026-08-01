import { chmod, lstat, mkdir } from "node:fs/promises";
import { isAbsolute, join } from "node:path";

import { deriveStateKey } from "../boundary/fingerprint-builder.js";
import { assertFingerprintSecret } from "../boundary/fingerprint-digest.js";
import { isFingerprintDigest, isStateIdentifier } from "../boundary/fingerprint-schema.js";
import { HerdrMathError } from "../core/errors.js";

const DIRECTORY_MODE = 0o700;

export interface PaneStatePaths {
  readonly sessionKey: string;
  readonly sourcePaneId: string;
  readonly paneKey: string;
  readonly versionDirectory: string;
  readonly sessionsDirectory: string;
  readonly sessionDirectory: string;
  readonly panesDirectory: string;
  readonly locksDirectory: string;
  readonly temporaryDirectory: string;
  readonly statePath: string;
  readonly lockPath: string;
}

export function createPaneStatePaths(
  stateDirectory: string,
  sessionKey: string,
  sourcePaneId: string,
  secret: Uint8Array
): PaneStatePaths {
  assertFingerprintSecret(secret);
  if (!isAbsolute(stateDirectory) || stateDirectory.includes("\0")) {
    throw new HerdrMathError("event_invalid");
  }
  if (!isFingerprintDigest(sessionKey) || !isStateIdentifier(sourcePaneId)) {
    throw new HerdrMathError("event_invalid");
  }

  const paneKey = deriveStateKey("pane", sourcePaneId, secret);
  const versionDirectory = join(stateDirectory, "v1");
  const sessionsDirectory = join(versionDirectory, "sessions");
  const sessionDirectory = join(sessionsDirectory, sessionKey);
  const panesDirectory = join(sessionDirectory, "panes");
  const locksDirectory = join(sessionDirectory, "locks");
  const temporaryDirectory = join(sessionDirectory, "tmp");
  return {
    sessionKey,
    sourcePaneId,
    paneKey,
    versionDirectory,
    sessionsDirectory,
    sessionDirectory,
    panesDirectory,
    locksDirectory,
    temporaryDirectory,
    statePath: join(panesDirectory, `${paneKey}.json`),
    lockPath: join(locksDirectory, `${paneKey}.lock`)
  };
}

export async function ensurePaneStateDirectories(paths: PaneStatePaths): Promise<void> {
  for (const directory of [
    paths.versionDirectory,
    paths.sessionsDirectory,
    paths.sessionDirectory,
    paths.panesDirectory,
    paths.locksDirectory,
    paths.temporaryDirectory
  ]) {
    await mkdir(directory, { mode: DIRECTORY_MODE }).catch((error: unknown) => {
      if (!isNodeError(error, "EEXIST")) throw error;
    });
    const metadata = await lstat(directory);
    if (!metadata.isDirectory() || metadata.isSymbolicLink()) {
      throw new HerdrMathError("state_corrupt");
    }
    await chmod(directory, DIRECTORY_MODE);
  }
}

function isNodeError(error: unknown, code: string): error is NodeJS.ErrnoException {
  return error instanceof Error && "code" in error && error.code === code;
}
