import { Buffer } from "node:buffer";
import { mkdtemp, readFile, readdir, rm, stat, symlink, utimes, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterEach, describe, expect, it } from "vitest";

import { buildBaselineFingerprint } from "../../src/boundary/fingerprint-builder.js";
import { acquirePaneLock } from "../../src/state/pane-lock.js";
import { createPaneStatePaths } from "../../src/state/paths.js";
import {
  cleanupPaneTemporaryFiles,
  isCanonicalPaneStatePath,
  isCurrentGeneration,
  loadPaneState,
  writePaneState
} from "../../src/state/store.js";
import { POLICY_LIMITS } from "../../src/core/limits.js";

const temporaryDirectories: string[] = [];
const secret = Buffer.alloc(32, 5);

async function temporaryDirectory(): Promise<string> {
  const directory = await mkdtemp(join(tmpdir(), "herdr-math-state-"));
  temporaryDirectories.push(directory);
  return directory;
}

afterEach(async () => {
  await Promise.all(temporaryDirectories.splice(0).map((directory) => rm(directory, { recursive: true, force: true })));
});

function state(generation = 1, createdAt = new Date("2026-08-01T00:00:00.000Z"), sourcePaneId = "w1:p1") {
  return buildBaselineFingerprint(
    "Synthetic baseline with enough content for a fingerprint.",
    {
      sessionIdentity: "session",
      occupantIdentity: "occupant",
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

function paths(directory: string, sessionKey = state().session_key, sourcePaneId = "w1:p1") {
  return createPaneStatePaths(directory, sessionKey, sourcePaneId, secret);
}

describe("session-scoped state paths", () => {
  it("isolates identical pane ids by non-reversible session keys", async () => {
    const directory = await temporaryDirectory();
    const first = paths(directory, "a".repeat(64));
    const second = paths(directory, "b".repeat(64));

    expect(first.statePath).not.toBe(second.statePath);
    expect(first.lockPath).not.toBe(second.lockPath);
    expect(first.statePath).not.toContain("w1:p1");
    expect(isCanonicalPaneStatePath(first, first.statePath)).toBe(true);
    expect(isCanonicalPaneStatePath(first, join(first.panesDirectory, "../unsafe.json"))).toBe(false);
  });

  it("rejects unsafe roots, session keys, and pane ids", async () => {
    const directory = await temporaryDirectory();
    expect(() => createPaneStatePaths("relative", "a".repeat(64), "w1:p1", secret)).toThrowError();
    expect(() => createPaneStatePaths(directory, "not-a-digest", "w1:p1", secret)).toThrowError();
    expect(() => createPaneStatePaths(directory, "a".repeat(64), "../pane", secret)).toThrowError();
  });
});

describe("atomic pane state", () => {
  it("writes and loads user-only JSON with a generation guard", async () => {
    const directory = await temporaryDirectory();
    const initial = state();
    const panePaths = paths(directory, initial.session_key);
    const now = new Date("2026-08-01T01:00:00.000Z");
    const next = { ...state(2), session_key: initial.session_key };

    expect(await writePaneState(panePaths, initial, null, now)).toBe(true);
    expect(await writePaneState(panePaths, next, 1, now)).toBe(true);
    expect(await writePaneState(panePaths, initial, 1, now)).toBe(false);
    expect(await loadPaneState(panePaths, now)).toEqual(next);
    expect(await isCurrentGeneration(panePaths, 2, next.occupant_key, now)).toBe(true);
    expect(await isCurrentGeneration(panePaths, 1, next.occupant_key, now)).toBe(false);
    expect((await stat(panePaths.statePath)).mode & 0o777).toBe(0o600);
    expect((await stat(panePaths.sessionDirectory)).mode & 0o777).toBe(0o700);
    expect((await readdir(panePaths.temporaryDirectory)).length).toBe(0);
    expect(JSON.parse(await readFile(panePaths.statePath, "utf8"))).toEqual(next);
  });

  it("removes only the expired pane generation", async () => {
    const directory = await temporaryDirectory();
    const expired = state();
    const current = {
      ...state(2, new Date("2026-08-02T00:00:00.000Z")),
      session_key: "b".repeat(64)
    };
    const expiredPaths = paths(directory, expired.session_key);
    const currentPaths = paths(directory, current.session_key);
    await writePaneState(expiredPaths, expired, null, new Date("2026-08-01T01:00:00.000Z"));
    await writePaneState(currentPaths, current, null, new Date("2026-08-02T01:00:00.000Z"));

    expect(await loadPaneState(expiredPaths, new Date("2026-08-02T00:00:00.001Z"))).toBeUndefined();
    expect(await loadPaneState(currentPaths, new Date("2026-08-02T01:00:00.000Z"))).toEqual(current);
  });

  it.each([
    ["malformed", "not-json"],
    ["unknown version", JSON.stringify({ ...state(), schema_version: 2 })],
    ["path field", JSON.stringify({ ...state(), source_pane_id: "../outside" })],
    ["oversized", "x".repeat(POLICY_LIMITS.stateFileBytes + 1)]
  ])("rejects and discards %s state", async (_name, content) => {
    const directory = await temporaryDirectory();
    const panePaths = paths(directory);
    await writePaneState(panePaths, state(), null, new Date("2026-08-01T01:00:00.000Z"));
    await writeFile(panePaths.statePath, content);

    await expect(loadPaneState(panePaths)).rejects.toMatchObject({ code: "state_corrupt" });
    expect(await readdir(panePaths.panesDirectory)).toEqual([]);
    expect(await readdir(panePaths.temporaryDirectory)).toEqual([]);
  });

  it("discards a symbolic link without reading its target", async () => {
    const directory = await temporaryDirectory();
    const panePaths = paths(directory);
    await writePaneState(panePaths, state(), null, new Date("2026-08-01T01:00:00.000Z"));
    await rm(panePaths.statePath);
    const target = join(directory, "outside-sentinel");
    await writeFile(target, "SENTINEL_OUTSIDE_STATE");
    await symlink(target, panePaths.statePath);

    await expect(loadPaneState(panePaths)).rejects.toMatchObject({ code: "state_corrupt" });
    expect(await readFile(target, "utf8")).toBe("SENTINEL_OUTSIDE_STATE");
  });

  it("cleans only old temporary files for the selected pane", async () => {
    const directory = await temporaryDirectory();
    const panePaths = paths(directory);
    await writePaneState(panePaths, state(), null, new Date("2026-08-01T01:00:00.000Z"));
    const stale = join(panePaths.temporaryDirectory, `state-${panePaths.paneKey}-12-test.tmp`);
    const unrelated = join(panePaths.temporaryDirectory, "unrelated.tmp");
    await writeFile(stale, "stale", { mode: 0o600 });
    await writeFile(unrelated, "keep", { mode: 0o600 });
    const old = new Date("2026-08-01T00:00:00.000Z");
    await utimes(stale, old, old);
    await utimes(unrelated, old, old);

    expect((await stat(stale)).mode & 0o777).toBe(0o600);
    expect(await cleanupPaneTemporaryFiles(panePaths, 1000, new Date("2026-08-01T00:00:02.000Z"))).toBe(1);
    expect(await readdir(panePaths.temporaryDirectory)).toEqual(["unrelated.tmp"]);
  });
});

describe("exclusive pane locks", () => {
  it("allows only one concurrent completion worker", async () => {
    const directory = await temporaryDirectory();
    const panePaths = paths(directory);
    const results = await Promise.allSettled(
      Array.from({ length: 8 }, () => acquirePaneLock(panePaths, { eventType: "done" }))
    );
    const acquired = results.filter((result) => result.status === "fulfilled");
    const rejected = results.filter((result) => result.status === "rejected");

    expect(acquired).toHaveLength(1);
    expect(rejected).toHaveLength(7);
    for (const result of rejected) {
      expect(result.reason).toMatchObject({ code: "state_locked", retryable: true });
    }
    await (acquired[0] as PromiseFulfilledResult<Awaited<ReturnType<typeof acquirePaneLock>>>).value.release();
  });

  it("protects a live owner and keeps the lock user-only", async () => {
    const directory = await temporaryDirectory();
    const panePaths = paths(directory);
    const lock = await acquirePaneLock(panePaths, { eventType: "done" });

    expect((await stat(panePaths.lockPath)).mode & 0o777).toBe(0o600);
    await expect(acquirePaneLock(panePaths, { eventType: "idle" })).rejects.toMatchObject({
      code: "state_locked",
      retryable: true
    });
    await lock.release();
    await expect(stat(panePaths.lockPath)).rejects.toMatchObject({ code: "ENOENT" });
  });

  it("recovers a stale dead lock without letting the old owner remove the replacement", async () => {
    const directory = await temporaryDirectory();
    const panePaths = paths(directory);
    const oldLock = await acquirePaneLock(panePaths, {
      eventType: "working",
      now: new Date("2026-08-01T00:00:00.000Z"),
      processId: 12345
    });
    const replacementTime = new Date(new Date("2026-08-01T00:00:00.000Z").getTime() + POLICY_LIMITS.staleLockAgeMs + 1);
    const replacement = await acquirePaneLock(panePaths, {
      eventType: "done",
      now: replacementTime,
      processId: 12346,
      isProcessAlive: () => false
    });

    await oldLock.release();
    expect(await stat(panePaths.lockPath)).toBeDefined();
    await replacement.release();
    await expect(stat(panePaths.lockPath)).rejects.toMatchObject({ code: "ENOENT" });
  });
});
