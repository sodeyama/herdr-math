import { constants } from "node:fs";
import { lstat, open, readdir, unlink } from "node:fs/promises";
import { isAbsolute, join } from "node:path";

import { assertFingerprintSecret } from "../boundary/fingerprint-digest.js";
import { deriveStateKey } from "../boundary/fingerprint-builder.js";
import { isFingerprintDigest, isIsoTimestamp, isStateIdentifier } from "../boundary/fingerprint-schema.js";
import { HerdrMathError } from "../core/errors.js";
import { POLICY_LIMITS } from "../core/limits.js";
import { parseFingerprintState } from "./validate.js";

const CLEANUP_LIMITS = Object.freeze({ sessions: 256, entriesPerDirectory: 4096 });
const LOCK_EVENTS = new Set(["working", "blocked", "done", "idle", "unknown", "pane_closed", "startup"]);
const PANE_STATE_NAME = /^([a-f0-9]{64})\.json$/;
const PANE_LOCK_NAME = /^([a-f0-9]{64})\.lock$/;
const PANE_TEMP_NAME = /^(?:state|lock)-([a-f0-9]{64})-[A-Za-z0-9-]+\.tmp$/;
const SECRET_TEMP_NAME = /^\.secret-[A-Za-z0-9-]+\.tmp$/;

interface FileIdentity {
  dev: number;
  ino: number;
}

interface BoundedJson {
  value: unknown;
  identity: FileIdentity;
  mtimeMs: number;
}

interface StartupLockRecord {
  schema_version: 1;
  process_id: number;
  started_at: string;
  event_type: string;
  pane_id: string;
}

export interface StartupCleanupResult {
  expiredStates: number;
  staleTemporaryFiles: number;
  staleLocks: number;
}

export interface StartupCleanupOptions {
  now?: Date;
  isProcessAlive?: (processId: number) => boolean;
}

export async function cleanupPluginState(
  stateDirectory: string,
  secret: Uint8Array,
  options: StartupCleanupOptions = {}
): Promise<StartupCleanupResult> {
  assertFingerprintSecret(secret);
  if (!isAbsolute(stateDirectory) || stateDirectory.includes("\0")) throw new HerdrMathError("event_invalid");
  const now = options.now ?? new Date();
  if (!(now instanceof Date) || Number.isNaN(now.getTime())) throw new HerdrMathError("event_invalid");

  const result: StartupCleanupResult = { expiredStates: 0, staleTemporaryFiles: 0, staleLocks: 0 };
  const versionDirectory = join(stateDirectory, "v1");
  result.staleTemporaryFiles += await cleanupSecretTemps(versionDirectory, now);
  const sessionsDirectory = join(versionDirectory, "sessions");
  const sessions = await boundedDirectory(sessionsDirectory, CLEANUP_LIMITS.sessions);
  for (const session of sessions) {
    if (!session.isDirectory() || !isFingerprintDigest(session.name)) continue;
    const sessionDirectory = join(sessionsDirectory, session.name);
    await assertRealDirectory(sessionDirectory);
    const protectedPanes = await cleanupSessionLocks(
      join(sessionDirectory, "locks"),
      secret,
      now,
      options.isProcessAlive ?? defaultProcessLiveness,
      result
    );
    result.expiredStates += await cleanupExpiredStates(
      join(sessionDirectory, "panes"),
      session.name,
      protectedPanes,
      secret,
      now
    );
    result.staleTemporaryFiles += await cleanupPaneTemps(join(sessionDirectory, "tmp"), protectedPanes, now);
  }
  return Object.freeze(result);
}

async function cleanupSessionLocks(
  locksDirectory: string,
  secret: Uint8Array,
  now: Date,
  isProcessAlive: (processId: number) => boolean,
  result: StartupCleanupResult
): Promise<Set<string>> {
  const protectedPanes = new Set<string>();
  for (const entry of await boundedDirectory(locksDirectory, CLEANUP_LIMITS.entriesPerDirectory)) {
    const match = PANE_LOCK_NAME.exec(entry.name);
    if (match === null || !entry.isFile()) continue;
    const paneKey = match[1];
    if (paneKey === undefined) continue;
    protectedPanes.add(paneKey);
    const path = join(locksDirectory, entry.name);
    let loaded: BoundedJson;
    try {
      loaded = await readBoundedJson(path);
    } catch {
      continue;
    }
    const record = parseStartupLock(loaded.value);
    if (record === undefined || deriveStateKey("pane", record.pane_id, secret) !== paneKey) continue;
    const age = now.getTime() - Date.parse(record.started_at);
    if (age <= POLICY_LIMITS.staleLockAgeMs || safelyCheckLiveness(isProcessAlive, record.process_id)) continue;
    if (await unlinkIfIdentityMatches(path, loaded.identity)) {
      protectedPanes.delete(paneKey);
      result.staleLocks += 1;
    }
  }
  return protectedPanes;
}

async function cleanupExpiredStates(
  panesDirectory: string,
  sessionKey: string,
  protectedPanes: ReadonlySet<string>,
  secret: Uint8Array,
  now: Date
): Promise<number> {
  let removed = 0;
  for (const entry of await boundedDirectory(panesDirectory, CLEANUP_LIMITS.entriesPerDirectory)) {
    const match = PANE_STATE_NAME.exec(entry.name);
    if (match === null || !entry.isFile()) continue;
    const paneKey = match[1];
    if (paneKey === undefined || protectedPanes.has(paneKey)) continue;
    const path = join(panesDirectory, entry.name);
    let loaded: BoundedJson;
    try {
      loaded = await readBoundedJson(path);
    } catch {
      continue;
    }
    let state;
    try {
      state = parseFingerprintState(loaded.value);
    } catch {
      continue;
    }
    if (
      state.session_key !== sessionKey ||
      deriveStateKey("pane", state.source_pane_id, secret) !== paneKey ||
      Date.parse(state.expires_at) > now.getTime()
    ) {
      continue;
    }
    if (await unlinkIfIdentityMatches(path, loaded.identity)) removed += 1;
  }
  return removed;
}

async function cleanupPaneTemps(
  temporaryDirectory: string,
  protectedPanes: ReadonlySet<string>,
  now: Date
): Promise<number> {
  let removed = 0;
  for (const entry of await boundedDirectory(temporaryDirectory, CLEANUP_LIMITS.entriesPerDirectory)) {
    const match = PANE_TEMP_NAME.exec(entry.name);
    if (match === null || !entry.isFile()) continue;
    const paneKey = match[1];
    if (paneKey === undefined || protectedPanes.has(paneKey)) continue;
    const path = join(temporaryDirectory, entry.name);
    const metadata = await lstat(path);
    if (
      !metadata.isFile() ||
      metadata.isSymbolicLink() ||
      now.getTime() - metadata.mtimeMs <= POLICY_LIMITS.staleLockAgeMs
    ) {
      continue;
    }
    if (await unlinkIfIdentityMatches(path, metadata)) removed += 1;
  }
  return removed;
}

async function cleanupSecretTemps(versionDirectory: string, now: Date): Promise<number> {
  let removed = 0;
  for (const entry of await boundedDirectory(versionDirectory, CLEANUP_LIMITS.entriesPerDirectory)) {
    if (!entry.isFile() || !SECRET_TEMP_NAME.test(entry.name)) continue;
    const path = join(versionDirectory, entry.name);
    const metadata = await lstat(path);
    if (
      metadata.isFile() &&
      !metadata.isSymbolicLink() &&
      now.getTime() - metadata.mtimeMs > POLICY_LIMITS.staleLockAgeMs &&
      (await unlinkIfIdentityMatches(path, metadata))
    ) {
      removed += 1;
    }
  }
  return removed;
}

async function readBoundedJson(path: string): Promise<BoundedJson> {
  let handle;
  try {
    handle = await open(path, constants.O_RDONLY | constants.O_NOFOLLOW);
    const metadata = await handle.stat();
    if (!metadata.isFile() || metadata.size > POLICY_LIMITS.stateFileBytes) {
      throw new HerdrMathError("state_corrupt");
    }
    const source = await handle.readFile("utf8");
    if (Buffer.byteLength(source, "utf8") > POLICY_LIMITS.stateFileBytes) {
      throw new HerdrMathError("state_corrupt");
    }
    return {
      value: JSON.parse(source) as unknown,
      identity: { dev: metadata.dev, ino: metadata.ino },
      mtimeMs: metadata.mtimeMs
    };
  } catch (error) {
    if (error instanceof HerdrMathError) throw error;
    throw new HerdrMathError("state_corrupt");
  } finally {
    await handle?.close();
  }
}

function parseStartupLock(value: unknown): StartupLockRecord | undefined {
  if (
    !hasExactKeys(value, ["schema_version", "process_id", "started_at", "event_type", "pane_id"]) ||
    value.schema_version !== 1 ||
    !Number.isSafeInteger(value.process_id) ||
    (value.process_id as number) <= 0 ||
    !isIsoTimestamp(value.started_at) ||
    typeof value.event_type !== "string" ||
    !LOCK_EVENTS.has(value.event_type) ||
    !isStateIdentifier(value.pane_id)
  ) {
    return undefined;
  }
  return value as unknown as StartupLockRecord;
}

async function boundedDirectory(path: string, maximum: number) {
  const metadata = await lstat(path).catch((error: unknown) => {
    if (isNodeError(error, "ENOENT")) return undefined;
    throw error;
  });
  if (metadata === undefined) return [];
  if (!metadata.isDirectory() || metadata.isSymbolicLink()) throw new HerdrMathError("state_corrupt");
  const entries = await readdir(path, { withFileTypes: true });
  if (entries.length > maximum) throw new HerdrMathError("state_corrupt");
  return entries;
}

async function assertRealDirectory(path: string): Promise<void> {
  const metadata = await lstat(path);
  if (!metadata.isDirectory() || metadata.isSymbolicLink()) throw new HerdrMathError("state_corrupt");
}

async function unlinkIfIdentityMatches(path: string, identity: FileIdentity): Promise<boolean> {
  const current = await lstat(path).catch((error: unknown) => {
    if (isNodeError(error, "ENOENT")) return undefined;
    throw error;
  });
  if (current === undefined) return true;
  if (current.dev !== identity.dev || current.ino !== identity.ino) return false;
  await unlink(path);
  return true;
}

function defaultProcessLiveness(processId: number): boolean {
  try {
    process.kill(processId, 0);
    return true;
  } catch (error) {
    return !isNodeError(error, "ESRCH");
  }
}

function safelyCheckLiveness(check: (processId: number) => boolean, processId: number): boolean {
  try {
    return check(processId);
  } catch {
    return true;
  }
}

function hasExactKeys(value: unknown, keys: readonly string[]): value is Record<string, unknown> {
  return (
    typeof value === "object" &&
    value !== null &&
    !Array.isArray(value) &&
    Object.keys(value).every((key) => keys.includes(key)) &&
    keys.every((key) => key in value)
  );
}

function isNodeError(error: unknown, code: string): error is NodeJS.ErrnoException {
  return error instanceof Error && "code" in error && error.code === code;
}
