import { Buffer } from "node:buffer";
import { mkdtemp, readFile, rm, stat, symlink, utimes, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterEach, describe, expect, it } from "vitest";

import { buildBaselineFingerprint } from "../../src/boundary/fingerprint-builder.js";
import { loadOrCreateFingerprintSecret } from "../../src/boundary/fingerprint-secret.js";
import { POLICY_LIMITS } from "../../src/core/limits.js";
import { runStartupHook } from "../../src/startup.js";
import { acquirePaneLock } from "../../src/state/pane-lock.js";
import { createPaneStatePaths } from "../../src/state/paths.js";
import { cleanupPluginState } from "../../src/state/startup-cleanup.js";
import { loadPaneState, writePaneState } from "../../src/state/store.js";

const SECRET = Buffer.alloc(32, 13);
const directories: string[] = [];

afterEach(async () => {
  await Promise.all(directories.splice(0).map((directory) => rm(directory, { recursive: true, force: true })));
});

describe("one-shot startup cleanup", () => {
  it("removes only expired state, stale temporary files, and dead stale locks", async () => {
    const directory = await temporaryDirectory();
    const created = new Date("2026-08-01T00:00:00.000Z");
    const cleanupTime = new Date("2026-08-02T00:00:01.000Z");
    const expired = state("w1:p1", 1, created);
    const current = state("w1:p2", 1, new Date("2026-08-01T12:00:02.000Z"));
    const protectedExpired = state("w1:p3", 1, created);
    const expiredPaths = paths(directory, expired);
    const currentPaths = paths(directory, current);
    const protectedPaths = paths(directory, protectedExpired);
    await writePaneState(expiredPaths, expired, null, new Date("2026-08-01T01:00:00.000Z"));
    await writePaneState(currentPaths, current, null, new Date("2026-08-01T13:00:00.000Z"));
    await writePaneState(protectedPaths, protectedExpired, null, new Date("2026-08-01T01:00:00.000Z"));

    const old = new Date(cleanupTime.getTime() - POLICY_LIMITS.staleLockAgeMs - 1);
    const staleTemp = join(expiredPaths.temporaryDirectory, `state-${expiredPaths.paneKey}-123-old.tmp`);
    const youngTemp = join(currentPaths.temporaryDirectory, `state-${currentPaths.paneKey}-123-young.tmp`);
    const protectedTemp = join(protectedPaths.temporaryDirectory, `lock-${protectedPaths.paneKey}-123-live.tmp`);
    await writeFile(staleTemp, "stale", { mode: 0o600 });
    await writeFile(youngTemp, "young", { mode: 0o600 });
    await writeFile(protectedTemp, "protected", { mode: 0o600 });
    await utimes(staleTemp, old, old);
    await utimes(youngTemp, cleanupTime, cleanupTime);
    await utimes(protectedTemp, old, old);

    const deadLock = await acquirePaneLock(currentPaths, { eventType: "working", now: old, processId: 41001 });
    const liveLock = await acquirePaneLock(protectedPaths, { eventType: "working", now: old, processId: 41002 });
    const result = await cleanupPluginState(directory, SECRET, {
      now: cleanupTime,
      isProcessAlive: (processId) => processId === 41002
    });

    expect(result).toEqual({ expiredStates: 1, staleTemporaryFiles: 1, staleLocks: 1 });
    expect(await loadPaneState(expiredPaths, cleanupTime)).toBeUndefined();
    expect(await loadPaneState(currentPaths, cleanupTime)).toEqual(current);
    expect(JSON.parse(await readFile(protectedPaths.statePath, "utf8"))).toEqual(protectedExpired);
    await expect(stat(staleTemp)).rejects.toMatchObject({ code: "ENOENT" });
    expect(await stat(youngTemp)).toBeDefined();
    expect(await stat(protectedTemp)).toBeDefined();
    await expect(stat(currentPaths.lockPath)).rejects.toMatchObject({ code: "ENOENT" });
    expect(await stat(protectedPaths.lockPath)).toBeDefined();

    const recovered = await acquirePaneLock(currentPaths, {
      eventType: "done",
      now: cleanupTime,
      processId: 41003,
      isProcessAlive: () => false
    });
    await recovered.release();
    await deadLock.release();
    await liveLock.release();
  });

  it("leaves corrupt, symlinked, unknown, and liveness-uncertain artifacts untouched", async () => {
    const directory = await temporaryDirectory();
    const created = new Date("2026-08-01T00:00:00.000Z");
    const cleanupTime = new Date("2026-08-02T00:00:01.000Z");
    const current = state("w1:p1", 1, created);
    const panePaths = paths(directory, current);
    await writePaneState(panePaths, current, null, new Date("2026-08-01T01:00:00.000Z"));
    await writeFile(panePaths.statePath, "CORRUPT_STATE_SENTINEL", { mode: 0o600 });
    const old = new Date(cleanupTime.getTime() - POLICY_LIMITS.staleLockAgeMs - 1);
    const lock = await acquirePaneLock(panePaths, { eventType: "working", now: old, processId: 42001 });
    const outside = join(directory, "outside-sentinel");
    const linkedTemp = join(panePaths.temporaryDirectory, `state-${panePaths.paneKey}-123-link.tmp`);
    const unknown = join(panePaths.temporaryDirectory, "unrelated.tmp");
    await writeFile(outside, "OUTSIDE_SENTINEL");
    await symlink(outside, linkedTemp);
    await writeFile(unknown, "UNKNOWN_SENTINEL");

    expect(
      await cleanupPluginState(directory, SECRET, {
        now: cleanupTime,
        isProcessAlive: () => {
          throw new Error("liveness unavailable");
        }
      })
    ).toEqual({ expiredStates: 0, staleTemporaryFiles: 0, staleLocks: 0 });
    expect(await readFile(panePaths.statePath, "utf8")).toBe("CORRUPT_STATE_SENTINEL");
    expect(await readFile(outside, "utf8")).toBe("OUTSIDE_SENTINEL");
    expect(await readFile(linkedTemp, "utf8")).toBe("OUTSIDE_SENTINEL");
    expect(await readFile(unknown, "utf8")).toBe("UNKNOWN_SENTINEL");
    expect(await stat(panePaths.lockPath)).toBeDefined();
    await lock.release();
  });

  it("runs through the manifest entrypoint contract and returns without a controller", async () => {
    const directory = await temporaryDirectory();
    const secret = await loadOrCreateFingerprintSecret(directory);
    const expired = buildState("w1:p1", 1, new Date("2026-08-01T00:00:00.000Z"), secret);
    const panePaths = createPaneStatePaths(directory, expired.session_key, expired.source_pane_id, secret);
    await writePaneState(panePaths, expired, null, new Date("2026-08-01T01:00:00.000Z"));

    const first = await runStartupHook(
      { HERDR_PLUGIN_STATE_DIR: directory },
      { now: new Date("2026-08-02T00:00:01.000Z"), isProcessAlive: () => false }
    );
    expect(first).toEqual({
      ok: true,
      value: { expiredStates: 1, staleTemporaryFiles: 0, staleLocks: 0 }
    });
    expect(
      await runStartupHook(
        { HERDR_PLUGIN_STATE_DIR: directory },
        { now: new Date("2026-08-02T00:00:01.000Z"), isProcessAlive: () => false }
      )
    ).toEqual({
      ok: true,
      value: { expiredStates: 0, staleTemporaryFiles: 0, staleLocks: 0 }
    });

    const source = await readFile(new URL("../../src/startup.ts", import.meta.url), "utf8");
    expect(source).not.toMatch(/child_process|spawn\s*\(|setInterval\s*\(|daemon/i);
    expect(await runStartupHook({})).toEqual({
      ok: false,
      error: { code: "event_invalid", retryable: false }
    });
  });
});

async function temporaryDirectory(): Promise<string> {
  const directory = await mkdtemp(join(tmpdir(), "herdr-math-startup-"));
  directories.push(directory);
  return directory;
}

function state(sourcePaneId: string, generation: number, createdAt: Date) {
  return buildState(sourcePaneId, generation, createdAt, SECRET);
}

function buildState(sourcePaneId: string, generation: number, createdAt: Date, secret: Uint8Array) {
  return buildBaselineFingerprint(
    `Fingerprint-only baseline for ${sourcePaneId}`,
    {
      sessionIdentity: "isolated-test-session",
      occupantIdentity: `occupant-${sourcePaneId}`,
      workspaceId: "w1",
      sourcePaneId,
      agent: "codex",
      lifecycleAuthority: "screen_detection",
      paneRevision: generation,
      eventSequence: generation,
      generation,
      createdAt
    },
    secret
  );
}

function paths(directory: string, value: ReturnType<typeof state>) {
  return createPaneStatePaths(directory, value.session_key, value.source_pane_id, SECRET);
}
