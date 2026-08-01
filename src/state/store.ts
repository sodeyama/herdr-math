import { randomUUID } from "node:crypto";
import { constants } from "node:fs";
import { lstat, open, readdir, rename, unlink } from "node:fs/promises";
import { basename, join } from "node:path";

import type { FingerprintStateV1 } from "../boundary/fingerprint-schema.js";
import { isFingerprintDigest } from "../boundary/fingerprint-schema.js";
import { HerdrMathError } from "../core/errors.js";
import { POLICY_LIMITS } from "../core/limits.js";
import { ensurePaneStateDirectories, type PaneStatePaths } from "./paths.js";
import { parseFingerprintState } from "./validate.js";

const FILE_MODE = 0o600;

export async function loadPaneState(paths: PaneStatePaths, now = new Date()): Promise<FingerprintStateV1 | undefined> {
  if (Number.isNaN(now.getTime())) throw new HerdrMathError("event_invalid");
  await ensurePaneStateDirectories(paths);
  for (let attempt = 0; attempt < 2; attempt += 1) {
    let loaded: Awaited<ReturnType<typeof readCanonicalState>>;
    try {
      loaded = await readCanonicalState(paths);
    } catch (error: unknown) {
      await discardCanonicalState(paths);
      throw error instanceof HerdrMathError ? error : new HerdrMathError("state_corrupt");
    }
    if (loaded === undefined) return undefined;
    if (loaded.state.session_key !== paths.sessionKey || loaded.state.source_pane_id !== paths.sourcePaneId) {
      await discardCanonicalState(paths, loaded.identity);
      throw new HerdrMathError("state_corrupt");
    }
    if (Date.parse(loaded.state.expires_at) > now.getTime()) return loaded.state;
    if (await unlinkIfIdentityMatches(paths.statePath, loaded.identity)) return undefined;
  }
  throw new HerdrMathError("state_locked", {}, true);
}

export async function writePaneState(
  paths: PaneStatePaths,
  nextState: FingerprintStateV1,
  expectedGeneration: number | null,
  now = new Date()
): Promise<boolean> {
  const validated = parseFingerprintState(nextState);
  if (
    (expectedGeneration !== null && (!Number.isSafeInteger(expectedGeneration) || expectedGeneration < 0)) ||
    Number.isNaN(now.getTime()) ||
    validated.session_key !== paths.sessionKey ||
    validated.source_pane_id !== paths.sourcePaneId ||
    Date.parse(validated.expires_at) <= now.getTime()
  ) {
    throw new HerdrMathError("state_corrupt");
  }
  const current = await loadPaneState(paths, now);
  if (
    (expectedGeneration === null && current !== undefined) ||
    (expectedGeneration !== null && current?.generation !== expectedGeneration) ||
    (current !== undefined && validated.generation < current.generation)
  ) {
    return false;
  }

  const serialized = JSON.stringify(validated);
  const bytes = Buffer.byteLength(serialized, "utf8");
  if (bytes > POLICY_LIMITS.stateFileBytes) {
    throw new HerdrMathError("state_corrupt", {
      limit_kind: "state_file_bytes",
      limit: POLICY_LIMITS.stateFileBytes,
      actual: bytes
    });
  }
  const temporaryPath = join(paths.temporaryDirectory, `state-${paths.paneKey}-${process.pid}-${randomUUID()}.tmp`);
  let renamed = false;
  try {
    const handle = await open(temporaryPath, "wx", FILE_MODE);
    try {
      await handle.writeFile(serialized, "utf8");
      await handle.sync();
    } finally {
      await handle.close();
    }
    await rename(temporaryPath, paths.statePath);
    renamed = true;
    await syncDirectory(paths.panesDirectory);
  } finally {
    if (!renamed) {
      await unlink(temporaryPath).catch((cleanupError: unknown) => {
        if (!isNodeError(cleanupError, "ENOENT")) throw cleanupError;
      });
    }
  }
  return true;
}

export async function isCurrentGeneration(
  paths: PaneStatePaths,
  generation: number,
  occupantKey: string,
  now = new Date()
): Promise<boolean> {
  if (!Number.isSafeInteger(generation) || generation < 0 || !isFingerprintDigest(occupantKey)) return false;
  const state = await loadPaneState(paths, now);
  return state?.generation === generation && state.occupant_key === occupantKey;
}

export async function cleanupPaneTemporaryFiles(
  paths: PaneStatePaths,
  minimumAgeMs: number,
  now = new Date()
): Promise<number> {
  if (!Number.isSafeInteger(minimumAgeMs) || minimumAgeMs < 0 || Number.isNaN(now.getTime())) {
    throw new HerdrMathError("event_invalid");
  }
  await ensurePaneStateDirectories(paths);
  const prefixPattern = new RegExp(`^(?:state|lock)-${paths.paneKey}-[a-zA-Z0-9-]+\\.tmp$`);
  let removed = 0;
  for (const entry of await readdir(paths.temporaryDirectory, { withFileTypes: true })) {
    if (!prefixPattern.test(entry.name)) continue;
    const candidate = join(paths.temporaryDirectory, entry.name);
    const metadata = await lstat(candidate);
    if (now.getTime() - metadata.mtimeMs < minimumAgeMs) continue;
    await unlink(candidate);
    removed += 1;
  }
  return removed;
}

async function readCanonicalState(paths: PaneStatePaths) {
  let handle;
  try {
    handle = await open(paths.statePath, constants.O_RDONLY | constants.O_NOFOLLOW);
  } catch (error: unknown) {
    if (isNodeError(error, "ENOENT")) return undefined;
    throw new HerdrMathError("state_corrupt");
  }
  try {
    const metadata = await handle.stat();
    if (!metadata.isFile() || metadata.size > POLICY_LIMITS.stateFileBytes) {
      throw new HerdrMathError("state_corrupt");
    }
    const serialized = await handle.readFile("utf8");
    if (Buffer.byteLength(serialized, "utf8") > POLICY_LIMITS.stateFileBytes) {
      throw new HerdrMathError("state_corrupt");
    }
    let parsed: unknown;
    try {
      parsed = JSON.parse(serialized);
    } catch {
      throw new HerdrMathError("state_corrupt");
    }
    return { state: parseFingerprintState(parsed), identity: { dev: metadata.dev, ino: metadata.ino } };
  } finally {
    await handle.close();
  }
}

async function discardCanonicalState(paths: PaneStatePaths, identity?: { dev: number; ino: number }): Promise<void> {
  const current = await lstat(paths.statePath).catch((error: unknown) => {
    if (isNodeError(error, "ENOENT")) return undefined;
    throw error;
  });
  if (
    current === undefined ||
    (identity !== undefined && (current.dev !== identity.dev || current.ino !== identity.ino))
  )
    return;
  const quarantinePath = join(paths.temporaryDirectory, `corrupt-${paths.paneKey}-${randomUUID()}.json`);
  await rename(paths.statePath, quarantinePath);
  await unlink(quarantinePath);
}

async function unlinkIfIdentityMatches(path: string, identity: { dev: number; ino: number }): Promise<boolean> {
  const current = await lstat(path).catch((error: unknown) => {
    if (isNodeError(error, "ENOENT")) return undefined;
    throw error;
  });
  if (current === undefined) return true;
  if (current.dev !== identity.dev || current.ino !== identity.ino) return false;
  await unlink(path);
  return true;
}

async function syncDirectory(path: string): Promise<void> {
  const handle = await open(path, constants.O_RDONLY);
  try {
    await handle.sync();
  } finally {
    await handle.close();
  }
}

export function isCanonicalPaneStatePath(paths: PaneStatePaths, path: string): boolean {
  return basename(path) === `${paths.paneKey}.json` && path === paths.statePath;
}

function isNodeError(error: unknown, code: string): error is NodeJS.ErrnoException {
  return error instanceof Error && "code" in error && error.code === code;
}
