import { Buffer } from "node:buffer";
import { mkdtemp, readFile, readdir, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterEach, describe, expect, it } from "vitest";

import { deriveStateKey } from "../../src/boundary/fingerprint-builder.js";
import type { SupportedAgent } from "../../src/boundary/fingerprint-schema.js";
import { failure, success, type OperationResult, type RenderedImage } from "../../src/core/contracts.js";
import { HerdrMathError, serializeError } from "../../src/core/errors.js";
import {
  processAgentStatusEvent,
  type AgentStatusHerdrClient,
  type AgentStatusWorkerDependencies,
  type AgentStatusWorkerOutcome,
  type ImagePublishRequest
} from "../../src/events/agent-status-worker.js";
import { publishImage } from "../../src/graphics/publisher.js";
import { HerdrSocketClient } from "../../src/herdr/socket-client.js";
import { createPaneStatePaths } from "../../src/state/paths.js";
import { loadPaneState } from "../../src/state/store.js";
import { FakeHerdrServer } from "../support/fake-herdr-server.js";
import { createFakePane, type FakePaneState, type FakeStatusEvent } from "../support/fake-herdr-types.js";

const NOW = new Date("2026-08-01T00:00:00.000Z");
const SECRET = Buffer.alloc(32, 11);
const PNG = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 1]);
const activeServers = new Set<FakeHerdrServer>();
const temporaryDirectories: string[] = [];

interface TestRig {
  server: FakeHerdrServer;
  directory: string;
  renders: Array<readonly { latex: string; display: boolean }[]>;
  publications: ImagePublishRequest[];
  dependencies: AgentStatusWorkerDependencies;
}

afterEach(async () => {
  await Promise.all([...activeServers].map((server) => server.close()));
  activeServers.clear();
  await Promise.all(temporaryDirectories.splice(0).map((directory) => rm(directory, { recursive: true, force: true })));
});

describe("agent status worker", () => {
  it("processes valid formula lifecycles for Claude Code, Codex, Pi, and OpenCode", async () => {
    for (const agent of ["claude", "codex", "pi", "opencode"] satisfies SupportedAgent[]) {
      const baseline = `BASELINE_SENTINEL_${agent}_with_unique_history`;
      const rig = await createRig({
        agent,
        agent_session: { source: `herdr:${agent}`, agent, kind: "id", value: `session-${agent}` }
      });
      rig.server.setPaneOutput("w1:p1", baseline);
      const working = rig.server.transitionPane("w1:p1", "working");
      expect(await process(rig, working)).toEqual({
        ok: true,
        value: { kind: "baseline_stored", status: "working", agent, generation: 1 }
      });

      rig.server.setPaneOutput("w1:p1", `${baseline}\nThe relation is $E=mc^2$.`);
      const done = rig.server.transitionPane("w1:p1", "done");
      expect(await process(rig, done)).toEqual({
        ok: true,
        value: {
          kind: "image_published",
          status: "done",
          agent,
          generation: 1,
          formulaCount: 1,
          viewerPaneId: "w1:p9"
        }
      });
      expect(rig.renders).toEqual([[{ latex: "E=mc^2", display: false }]]);
      expect(rig.publications).toHaveLength(1);
      expect(rig.server.requests.filter(({ method }) => method === "pane.read")).toHaveLength(3);

      const state = await loadState(rig);
      expect(state?.agent).toBe(agent);
      expect(state?.lifecycle_authority).toBe(
        agent === "claude" || agent === "codex" ? "screen_detection" : "integration_hook"
      );
      expect(state?.processed?.pane_revision).toBe(3);
      expect(state?.viewer_pane_id).toBe("w1:p9");
      const stateBytes = await readFile(statePath(rig), "utf8");
      expect(stateBytes).not.toContain(baseline);
      expect(stateBytes).not.toContain("E=mc^2");
      expect(stateBytes).not.toContain(`session-${agent}`);
    }
  });

  it("publishes a completed formula through the owned graphics viewer", async () => {
    const rig = await createRig();
    const client = new HerdrSocketClient(rig.server.socketPath);
    rig.dependencies.client = client;
    rig.dependencies.publish = (request) => publishImage(request, { client, sessionIdentity: rig.server.socketPath });
    const baseline = "End-to-end baseline before the formula response.";
    rig.server.setPaneOutput("w1:p1", baseline);
    expect((await process(rig, rig.server.transitionPane("w1:p1", "working"))).ok).toBe(true);
    rig.server.setPaneOutput("w1:p1", `${baseline}\nResult $x^2$.`);

    expect(await process(rig, rig.server.transitionPane("w1:p1", "done"))).toMatchObject({
      ok: true,
      value: { kind: "image_published", viewerPaneId: "w1:p2", formulaCount: 1 }
    });
    expect(rig.server.getGraphics("w1:p2")).toMatchObject({
      image_width: 1,
      image_height: 1,
      placement: { grid_cols: 1, grid_rows: 1 }
    });
    expect(rig.server.getPane("w1:p1")?.focused).toBe(true);
    expect(await loadState(rig)).toMatchObject({ viewer_pane_id: "w1:p2", processed: { pane_revision: 3 } });
  });

  it("preserves blocked and unknown baselines, records no-formula content, and suppresses idle duplicates", async () => {
    const rig = await createRig();
    const baseline = "A stable baseline before the coding agent response.";
    rig.server.setPaneOutput("w1:p1", baseline);
    expect((await process(rig, rig.server.transitionPane("w1:p1", "working"))).ok).toBe(true);

    expect(await process(rig, rig.server.transitionPane("w1:p1", "blocked"))).toMatchObject({
      ok: true,
      value: { kind: "preserved", reason: "blocked", generation: 1 }
    });
    expect(await process(rig, rig.server.transitionPane("w1:p1", "unknown"))).toMatchObject({
      ok: true,
      value: { kind: "preserved", reason: "unknown", generation: 1 }
    });

    rig.server.setPaneOutput("w1:p1", `${baseline}\nNo equation was included.`);
    expect(await process(rig, rig.server.transitionPane("w1:p1", "done"))).toMatchObject({
      ok: true,
      value: { kind: "completion_recorded", formulaCount: 0, generation: 1 }
    });
    expect(rig.renders).toHaveLength(0);
    expect(rig.publications).toHaveLength(0);
    expect((await loadState(rig))?.processed).toBeDefined();

    expect(await process(rig, rig.server.transitionPane("w1:p1", "idle"))).toMatchObject({
      ok: true,
      value: { kind: "preserved", reason: "duplicate_completion", generation: 1 }
    });
    expect(rig.publications).toHaveLength(0);

    const missing = await createRig({ pane_id: "w2:p1", workspace_id: "w2" });
    missing.server.updatePane("w2:p1", { agent_status: "done" });
    expect(await process(missing, statusEvent("w2", "w2:p1", "done", "codex"))).toEqual({
      ok: false,
      error: { code: "baseline_missing", retryable: false }
    });
  });

  it("fails closed before state mutation for invalid, unresolved, stale, and unsupported events", async () => {
    const rig = await createRig();
    let calls = 0;
    const rejectingClient: AgentStatusHerdrClient = {
      paneGet: () => {
        calls += 1;
        return Promise.resolve(failure(serializeError(new HerdrMathError("herdr_protocol_error"))));
      },
      paneRead: () => {
        calls += 1;
        return Promise.resolve(failure(serializeError(new HerdrMathError("herdr_protocol_error"))));
      }
    };
    const invalid = await processAgentStatusEvent("not-json", { ...rig.dependencies, client: rejectingClient });
    expect(invalid).toEqual({ ok: false, error: { code: "event_invalid", retryable: false } });
    expect(calls).toBe(0);
    expect(await readdir(rig.directory)).toEqual([]);

    const missing = await process(rig, statusEvent("w1", "w1:p9", "working", "codex"));
    expect(missing).toEqual({ ok: false, error: { code: "herdr_protocol_error", retryable: false } });

    const cases: Array<{
      pane: Partial<Omit<FakePaneState, "pane_id">>;
      event: FakeStatusEvent;
      code: "event_invalid" | "agent_unsupported";
    }> = [
      {
        pane: { agent: null, agent_session: null, agent_status: "working" },
        event: statusEvent("w1", "w1:p1", "working"),
        code: "event_invalid"
      },
      {
        pane: { agent: "codex", agent_session: null, agent_status: "working" },
        event: statusEvent("w2", "w1:p1", "working", "codex"),
        code: "event_invalid"
      },
      {
        pane: { agent: "codex", agent_session: null, agent_status: "working" },
        event: statusEvent("w1", "w1:p1", "working", "claude"),
        code: "event_invalid"
      },
      {
        pane: { agent: "codex", agent_session: null, agent_status: "working" },
        event: statusEvent("w1", "w1:p1", "done", "codex"),
        code: "event_invalid"
      },
      {
        pane: { agent: "cursor", agent_session: null, agent_status: "working" },
        event: statusEvent("w1", "w1:p1", "working", "cursor"),
        code: "agent_unsupported"
      }
    ];
    for (const testCase of cases) {
      rig.server.updatePane("w1:p1", testCase.pane);
      const result = await process(rig, testCase.event);
      expect(result).toEqual({ ok: false, error: { code: testCase.code, retryable: false } });
    }
    expect(rig.renders).toHaveLength(0);
    expect(rig.publications).toHaveLength(0);
    expect(await readdir(rig.directory)).toEqual([]);
  });

  it("rejects unprovable and unrecovered truncated boundaries without rendering", async () => {
    const rig = await createRig();
    rig.server.setPaneOutput("w1:p1", "Original baseline with enough distinct content to fingerprint.");
    expect((await process(rig, rig.server.transitionPane("w1:p1", "working"))).ok).toBe(true);
    rig.server.setPaneOutput("w1:p1", "Completely unrelated final content with $x$.");
    expect(await process(rig, rig.server.transitionPane("w1:p1", "done"))).toEqual({
      ok: false,
      error: { code: "boundary_failed", retryable: false }
    });

    rig.server.setPaneOutput("w1:p1", "A replacement baseline with another unique sequence.");
    expect((await process(rig, rig.server.transitionPane("w1:p1", "working"))).ok).toBe(true);
    rig.server.setPaneOutput("w1:p1", "Truncated unrelated tail containing $y$.", true);
    expect(await process(rig, rig.server.transitionPane("w1:p1", "done"))).toEqual({
      ok: false,
      error: { code: "answer_truncated", retryable: false }
    });
    expect(rig.renders).toHaveLength(0);
    expect(rig.publications).toHaveLength(0);
  });

  it("allows only one graphics commit for concurrent duplicate completion hooks", async () => {
    const rig = await createRig();
    const baseline = "Concurrent baseline with deterministic content.";
    rig.server.setPaneOutput("w1:p1", baseline);
    expect((await process(rig, rig.server.transitionPane("w1:p1", "working"))).ok).toBe(true);
    rig.server.setPaneOutput("w1:p1", `${baseline}\nResult $x+y$.`);
    const done = rig.server.transitionPane("w1:p1", "done");

    const results = await Promise.all([process(rig, done), process(rig, done)]);
    expect(results.filter((result) => result.ok && result.value.kind === "image_published")).toHaveLength(1);
    expect(rig.publications).toHaveLength(1);
    expect((await loadState(rig))?.processed).toBeDefined();
  });

  it("invalidates generation N when a new working event arrives during rendering", async () => {
    const rig = await createRig();
    const baseline = "First generation baseline with unique content.";
    rig.server.setPaneOutput("w1:p1", baseline);
    expect((await process(rig, rig.server.transitionPane("w1:p1", "working"))).ok).toBe(true);
    rig.server.setPaneOutput("w1:p1", `${baseline}\nOld answer $x$.`);
    const done = rig.server.transitionPane("w1:p1", "done");

    let releaseRender: (() => void) | undefined;
    let signalStarted: (() => void) | undefined;
    const renderStarted = new Promise<void>((resolve) => (signalStarted = resolve));
    const renderRelease = new Promise<void>((resolve) => (releaseRender = resolve));
    rig.dependencies.render = async () => {
      signalStarted?.();
      await renderRelease;
      return success(image());
    };
    const staleCompletion = process(rig, done);
    await renderStarted;

    rig.server.setPaneOutput("w1:p1", "Second generation baseline after a new prompt.");
    const nextWorking = rig.server.transitionPane("w1:p1", "working");
    expect(await process(rig, nextWorking)).toMatchObject({
      ok: true,
      value: { kind: "baseline_stored", generation: 2 }
    });
    releaseRender?.();
    const staleResult = await staleCompletion;
    expect(staleResult.ok ? staleResult.value.kind : staleResult.error.code).not.toBe("image_published");
    expect(rig.publications).toHaveLength(0);
    expect((await loadState(rig))?.generation).toBe(2);
  });

  it("fails within three completion reads when pane output never stabilizes", async () => {
    const rig = await createRig();
    const baseline = "Unstable read baseline with unique content.";
    rig.server.setPaneOutput("w1:p1", baseline);
    expect((await process(rig, rig.server.transitionPane("w1:p1", "working"))).ok).toBe(true);
    const baseClient = rig.dependencies.client;
    let readNumber = 0;
    rig.dependencies.client = {
      paneGet: (paneId) => baseClient.paneGet(paneId),
      paneRead: (paneId) => {
        readNumber += 1;
        rig.server.setPaneOutput(paneId, `${baseline}\nChanging answer ${readNumber}: $x_${readNumber}$.`);
        return baseClient.paneRead(paneId);
      }
    };
    const done = rig.server.transitionPane("w1:p1", "done");

    expect(await process(rig, done)).toEqual({
      ok: false,
      error: { code: "herdr_timeout", retryable: true }
    });
    expect(readNumber).toBe(3);
    expect(rig.renders).toHaveLength(0);
    expect(rig.publications).toHaveLength(0);
  });

  it("keeps the previous viewer mapping when a later render fails", async () => {
    const rig = await createRig();
    const firstBaseline = "Initial baseline used to create the first viewer.";
    rig.server.setPaneOutput("w1:p1", firstBaseline);
    expect((await process(rig, rig.server.transitionPane("w1:p1", "working"))).ok).toBe(true);
    rig.server.setPaneOutput("w1:p1", `${firstBaseline}\nValid $x$.`);
    expect((await process(rig, rig.server.transitionPane("w1:p1", "done"))).ok).toBe(true);
    expect(rig.publications).toHaveLength(1);

    const secondBaseline = "New baseline for a response that cannot be rendered.";
    rig.server.setPaneOutput("w1:p1", secondBaseline);
    expect((await process(rig, rig.server.transitionPane("w1:p1", "working"))).ok).toBe(true);
    rig.server.setPaneOutput("w1:p1", `${secondBaseline}\nInvalid $\\href{https://example.test}{x}$.`);
    rig.dependencies.render = () => Promise.resolve(failure(serializeError(new HerdrMathError("invalid_latex"))));

    expect(await process(rig, rig.server.transitionPane("w1:p1", "done"))).toEqual({
      ok: false,
      error: { code: "invalid_latex", retryable: false }
    });
    expect(rig.publications).toHaveLength(1);
    expect(await loadState(rig)).toMatchObject({ generation: 2, viewer_pane_id: "w1:p9" });
    expect((await loadState(rig))?.processed).toBeUndefined();
  });
});

async function createRig(overrides: Partial<FakePaneState> = {}): Promise<TestRig> {
  const pane = createFakePane({ agent_status: "idle", agent_session: null, ...overrides });
  const server = await FakeHerdrServer.start({ panes: [pane] });
  activeServers.add(server);
  const directory = await mkdtemp(join(tmpdir(), "herdr-math-worker-"));
  temporaryDirectories.push(directory);
  const renders: TestRig["renders"] = [];
  const publications: ImagePublishRequest[] = [];
  const dependencies: AgentStatusWorkerDependencies = {
    client: new HerdrSocketClient(server.socketPath),
    stateDirectory: directory,
    sessionIdentity: server.socketPath,
    secret: SECRET,
    now: () => NOW,
    sleep: () => Promise.resolve(),
    timing: { completionDebounceMs: 0, stableReadIntervalMs: 0 },
    render: (formulas) => {
      renders.push(formulas.map(({ latex, display }) => ({ latex, display })));
      return Promise.resolve(success(image()));
    },
    publish: (request) => {
      publications.push(request);
      return Promise.resolve(success({ viewerPaneId: "w1:p9" }));
    }
  };
  return { server, directory, renders, publications, dependencies };
}

function process(rig: TestRig, event: FakeStatusEvent): Promise<OperationResult<AgentStatusWorkerOutcome>> {
  return processAgentStatusEvent(JSON.stringify(event), rig.dependencies);
}

function statusEvent(
  workspaceId: string,
  paneId: string,
  status: FakeStatusEvent["data"]["agent_status"],
  agent?: string
): FakeStatusEvent {
  const data: FakeStatusEvent["data"] = {
    type: "pane_agent_status_changed",
    workspace_id: workspaceId,
    pane_id: paneId,
    agent_status: status
  };
  if (agent !== undefined) data.agent = agent;
  return { event: "pane_agent_status_changed", data };
}

function image(): RenderedImage {
  return { buffer: Buffer.from(PNG), width: 1, height: 1, bytes: PNG.byteLength, renderer: "fake" };
}

function statePath(rig: TestRig): string {
  const sessionKey = deriveStateKey("session", rig.server.socketPath, SECRET);
  return createPaneStatePaths(rig.directory, sessionKey, "w1:p1", SECRET).statePath;
}

async function loadState(rig: TestRig) {
  const sessionKey = deriveStateKey("session", rig.server.socketPath, SECRET);
  const paths = createPaneStatePaths(rig.directory, sessionKey, "w1:p1", SECRET);
  return loadPaneState(paths, NOW);
}
