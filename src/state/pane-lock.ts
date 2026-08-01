import { randomUUID } from "node:crypto";
import { constants } from "node:fs";
import { link, lstat, open, unlink } from "node:fs/promises";
import { join } from "node:path";

import { isIsoTimestamp, isStateIdentifier } from "../boundary/fingerprint-schema.js";
import { HerdrMathError } from "../core/errors.js";
import { POLICY_LIMITS } from "../core/limits.js";
import { ensurePaneStateDirectories, type PaneStatePaths } from "./paths.js";

const FILE_MODE = 0o600;
const EVENT_TYPES = new Set(["working", "blocked", "done", "idle", "unknown", "pane_closed", "startup"]);

interface LockRecord {
  schema_version: 1;
  process_id: number;
  started_at: string;
  event_type: string;
  pane_id: string;
}

export interface PaneLockOptions {
  eventType: string;
  now?: Date;
  processId?: number;
  isProcessAlive?: (processId: number) => boolean;
}

export interface PaneLock {
  release(): Promise<void>;
}

export async function acquirePaneLock(paths: PaneStatePaths, options: PaneLockOptions): Promise<PaneLock> {
  await ensurePaneStateDirectories(paths);
  const now = options.now ?? new Date();
  const processId = options.processId ?? process.pid;
  if (!EVENT_TYPES.has(options.eventType) || !isCount(processId) || Number.isNaN(now.getTime())) {
    throw new HerdrMathError("event_invalid");
  }

  for (let attempt = 0; attempt < 2; attempt += 1) {
    try {
      return await createLock(paths, {
        schema_version: 1,
        process_id: processId,
        started_at: now.toISOString(),
        event_type: options.eventType,
        pane_id: paths.sourcePaneId
      });
    } catch (error: unknown) {
      if (!isNodeError(error, "EEXIST")) throw error;
      const record = await readLock(paths);
      const age = now.getTime() - Date.parse(record.started_at);
      const alive = safelyCheckLiveness(options.isProcessAlive ?? defaultProcessLiveness, record.process_id);
      if (age <= POLICY_LIMITS.staleLockAgeMs || alive) {
        throw new HerdrMathError("state_locked", {}, true);
      }
      await unlink(paths.lockPath).catch((unlinkError: unknown) => {
        if (!isNodeError(unlinkError, "ENOENT")) throw unlinkError;
      });
    }
  }
  throw new HerdrMathError("state_locked", {}, true);
}

async function createLock(paths: PaneStatePaths, record: LockRecord): Promise<PaneLock> {
  const temporaryPath = join(paths.temporaryDirectory, `lock-${paths.paneKey}-${process.pid}-${randomUUID()}.tmp`);
  const handle = await open(temporaryPath, "wx", FILE_MODE);
  let identity;
  try {
    await handle.writeFile(JSON.stringify(record), "utf8");
    await handle.sync();
    identity = await handle.stat();
  } finally {
    await handle.close();
  }
  try {
    await link(temporaryPath, paths.lockPath);
  } finally {
    await unlink(temporaryPath).catch((error: unknown) => {
      if (!isNodeError(error, "ENOENT")) throw error;
    });
  }
  let released = false;
  return {
    async release() {
      if (released) return;
      released = true;
      const current = await lstat(paths.lockPath).catch((error: unknown) => {
        if (isNodeError(error, "ENOENT")) return undefined;
        throw error;
      });
      if (current?.dev === identity.dev && current.ino === identity.ino) await unlink(paths.lockPath);
    }
  };
}

async function readLock(paths: PaneStatePaths): Promise<LockRecord> {
  let handle;
  try {
    handle = await open(paths.lockPath, constants.O_RDONLY | constants.O_NOFOLLOW);
    const metadata = await handle.stat();
    if (!metadata.isFile() || metadata.size > POLICY_LIMITS.stateFileBytes) throw new HerdrMathError("state_corrupt");
    const value: unknown = JSON.parse(await handle.readFile("utf8"));
    if (
      !hasExactKeys(value, ["schema_version", "process_id", "started_at", "event_type", "pane_id"]) ||
      value.schema_version !== 1 ||
      !isCount(value.process_id) ||
      !isIsoTimestamp(value.started_at) ||
      typeof value.event_type !== "string" ||
      !EVENT_TYPES.has(value.event_type) ||
      !isStateIdentifier(value.pane_id) ||
      value.pane_id !== paths.sourcePaneId
    ) {
      throw new HerdrMathError("state_corrupt");
    }
    return value as unknown as LockRecord;
  } catch (error: unknown) {
    if (error instanceof HerdrMathError) throw error;
    throw new HerdrMathError("state_corrupt");
  } finally {
    await handle?.close();
  }
}

function defaultProcessLiveness(processId: number): boolean {
  try {
    process.kill(processId, 0);
    return true;
  } catch (error: unknown) {
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

function isCount(value: unknown): value is number {
  return Number.isSafeInteger(value) && (value as number) > 0;
}

function isNodeError(error: unknown, code: string): error is NodeJS.ErrnoException {
  return error instanceof Error && "code" in error && error.code === code;
}
