import { Buffer } from "node:buffer";
import { mkdtemp, rm, stat } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterEach, describe, expect, it } from "vitest";

import { buildBaselineFingerprint, deriveStateKey } from "../../src/boundary/fingerprint-builder.js";
import type { FingerprintStateV1 } from "../../src/boundary/fingerprint-schema.js";
import {
  processPaneClosedEvent,
  type PaneCloseWorkerDependencies,
  type PaneCloseWorkerOutcome
} from "../../src/events/pane-close-worker.js";
import { HerdrSocketClient } from "../../src/herdr/socket-client.js";
import { acquirePaneLock } from "../../src/state/pane-lock.js";
import { createPaneStatePaths, type PaneStatePaths } from "../../src/state/paths.js";
import { loadPaneState, writePaneState } from "../../src/state/store.js";
import type { OperationResult } from "../../src/core/contracts.js";
import { FakeHerdrServer } from "../support/fake-herdr-server.js";
import { createFakePane, type FakePaneState } from "../support/fake-herdr-types.js";

const SECRET = Buffer.alloc(32, 17);
const NOW = new Date("2026-08-01T00:00:00.000Z");
const servers = new Set<FakeHerdrServer>();
const directories: string[] = [];

interface CloseRig {
  server: FakeHerdrServer;
  directory: string;
  dependencies: PaneCloseWorkerDependencies;
}

afterEach(async () => {
  await Promise.all([...servers].map((server) => server.close()));
  servers.clear();
  await Promise.all(directories.splice(0).map((directory) => rm(directory, { recursive: true, force: true })));
});

describe("pane close worker", () => {
  it("removes source state only after the source pane is authoritatively absent", async () => {
    const rig = await createRig([createFakePane()]);
    const panePaths = await storeState(rig, { viewerPaneId: "w1:p2" });
    rig.server.closePane("w1:p1");

    expect(await closeEvent(rig, "w1", "w1:p1")).toEqual({
      ok: true,
      value: { kind: "cleaned", sourceStatesRemoved: 1, viewerMappingsCleared: 0 }
    });
    await expect(stat(panePaths.statePath)).rejects.toMatchObject({ code: "ENOENT" });
    expect(await closeEvent(rig, "w1", "w1:p1")).toEqual({
      ok: true,
      value: { kind: "preserved", reason: "not_tracked" }
    });
  });

  it("clears a closed viewer mapping while preserving source fingerprints and processed state", async () => {
    const source = createFakePane();
    const viewer = createFakePane({ pane_id: "w1:p2", terminal_id: "term-2", agent: null, focused: false });
    const rig = await createRig([source, viewer]);
    const panePaths = await storeState(rig, { viewerPaneId: "w1:p2", processed: true });
    rig.server.closePane("w1:p2");

    expect(await closeEvent(rig, "w1", "w1:p2")).toEqual({
      ok: true,
      value: { kind: "cleaned", sourceStatesRemoved: 0, viewerMappingsCleared: 1 }
    });
    const state = await loadPaneState(panePaths, NOW);
    expect(state?.viewer_pane_id).toBeUndefined();
    expect(state?.processed).toBeDefined();
    expect(state?.baseline).toBeDefined();
  });

  it("does not change state for unrelated panes or a mismatched workspace", async () => {
    const source = createFakePane();
    const viewer = createFakePane({ pane_id: "w1:p2", terminal_id: "term-2", agent: null, focused: false });
    const unrelated = createFakePane({ pane_id: "w1:p3", terminal_id: "term-3", agent: null, focused: false });
    const rig = await createRig([source, viewer, unrelated]);
    const panePaths = await storeState(rig, { viewerPaneId: "w1:p2" });
    rig.server.closePane("w1:p3");
    expect(await closeEvent(rig, "w1", "w1:p3")).toMatchObject({
      ok: true,
      value: { kind: "preserved", reason: "not_tracked" }
    });
    rig.server.closePane("w1:p2");
    expect(await closeEvent(rig, "w2", "w1:p2")).toMatchObject({
      ok: true,
      value: { kind: "preserved", reason: "not_tracked" }
    });
    expect((await loadPaneState(panePaths, NOW))?.viewer_pane_id).toBe("w1:p2");
  });

  it("removes an old occupant state after pane-id reuse but preserves a matching new occupant", async () => {
    const oldPane = createFakePane({
      agent_session: { source: "herdr:codex", agent: "codex", kind: "id", value: "old-session" }
    });
    const rig = await createRig([oldPane]);
    const oldPaths = await storeState(rig, { agentSessionId: "old-session" });
    rig.server.closePane("w1:p1");
    rig.server.addPane(
      createFakePane({
        agent_status: "working",
        agent_session: { source: "herdr:codex", agent: "codex", kind: "id", value: "new-session" }
      })
    );

    expect(await closeEvent(rig, "w1", "w1:p1")).toMatchObject({
      ok: true,
      value: { kind: "cleaned", sourceStatesRemoved: 1 }
    });
    await expect(stat(oldPaths.statePath)).rejects.toMatchObject({ code: "ENOENT" });

    const newPaths = await storeState(rig, { agentSessionId: "new-session" });
    expect(await closeEvent(rig, "w1", "w1:p1")).toEqual({
      ok: true,
      value: { kind: "preserved", reason: "pane_reused" }
    });
    expect(await loadPaneState(newPaths, NOW)).toBeDefined();
  });

  it("removes fallback identity state when the pane id is reused by the same agent", async () => {
    const rig = await createRig([createFakePane()]);
    const oldPaths = await storeState(rig, {});
    rig.server.closePane("w1:p1");
    rig.server.addPane(createFakePane({ agent_status: "working", revision: 2 }));

    expect(await closeEvent(rig, "w1", "w1:p1")).toMatchObject({
      ok: true,
      value: { kind: "cleaned", sourceStatesRemoved: 1 }
    });
    await expect(stat(oldPaths.statePath)).rejects.toMatchObject({ code: "ENOENT" });
  });

  it("preserves state on transport errors and live lock contention", async () => {
    const rig = await createRig([createFakePane()]);
    const panePaths = await storeState(rig, {});
    rig.server.closePane("w1:p1");
    rig.server.queueResponse("pane.get", { error: { code: "busy", message: "retry" } });
    expect(await closeEvent(rig, "w1", "w1:p1")).toEqual({
      ok: false,
      error: { code: "herdr_protocol_error", retryable: false }
    });
    expect(await loadPaneState(panePaths, NOW)).toBeDefined();

    const lock = await acquirePaneLock(panePaths, { eventType: "working", now: NOW });
    expect(await closeEvent(rig, "w1", "w1:p1")).toEqual({
      ok: false,
      error: { code: "state_locked", retryable: true }
    });
    expect(await loadPaneState(panePaths, NOW)).toBeDefined();
    await lock.release();
  });
});

async function createRig(panes: FakePaneState[]): Promise<CloseRig> {
  const server = await FakeHerdrServer.start({ panes });
  servers.add(server);
  const directory = await mkdtemp(join(tmpdir(), "herdr-math-close-"));
  directories.push(directory);
  return {
    server,
    directory,
    dependencies: {
      client: new HerdrSocketClient(server.socketPath),
      stateDirectory: directory,
      sessionIdentity: server.socketPath,
      secret: SECRET,
      now: () => NOW
    }
  };
}

async function storeState(
  rig: CloseRig,
  options: { viewerPaneId?: string; processed?: boolean; agentSessionId?: string }
): Promise<PaneStatePaths> {
  const occupantIdentity =
    options.agentSessionId === undefined
      ? "pane-agent\0w1:p1\0codex\0screen_detection"
      : `agent-session\0herdr:codex\0codex\0id\0${options.agentSessionId}`;
  const initial = buildBaselineFingerprint(
    "Fingerprint-only state for pane close integration testing.",
    {
      sessionIdentity: rig.server.socketPath,
      occupantIdentity,
      workspaceId: "w1",
      sourcePaneId: "w1:p1",
      agent: "codex",
      lifecycleAuthority: "screen_detection",
      paneRevision: 1,
      eventSequence: 1,
      generation: 1,
      createdAt: NOW
    },
    SECRET
  );
  const state: FingerprintStateV1 = { ...initial };
  if (options.viewerPaneId !== undefined) state.viewer_pane_id = options.viewerPaneId;
  if (options.processed === true) {
    state.processed = {
      content_digest: "a".repeat(64),
      pane_revision: 1,
      processed_at: NOW.toISOString()
    };
  }
  const panePaths = paths(rig);
  await writePaneState(panePaths, state, null, NOW);
  return panePaths;
}

function paths(rig: CloseRig): PaneStatePaths {
  const sessionKey = deriveStateKey("session", rig.server.socketPath, SECRET);
  return createPaneStatePaths(rig.directory, sessionKey, "w1:p1", SECRET);
}

function closeEvent(
  rig: CloseRig,
  workspaceId: string,
  paneId: string
): Promise<OperationResult<PaneCloseWorkerOutcome>> {
  return processPaneClosedEvent(
    JSON.stringify({ event: "pane_closed", data: { type: "pane_closed", workspace_id: workspaceId, pane_id: paneId } }),
    rig.dependencies
  );
}
