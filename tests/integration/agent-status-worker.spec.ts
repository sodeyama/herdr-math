import { Buffer } from "node:buffer";
import { mkdtemp, readFile, readdir, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterEach, describe, expect, it } from "vitest";

import { deriveStateKey } from "../../src/boundary/fingerprint-builder.js";
import type { SupportedAgent } from "../../src/boundary/fingerprint-schema.js";
import { parsePluginConfig } from "../../src/config/plugin-config.js";
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
import { ViewerPresenter } from "../../src/viewer/presenter.js";
import { runAgentStatusHook } from "../../src/on-agent-status.js";
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
  renderedTexts: string[];
  publications: ImagePublishRequest[];
  dependencies: AgentStatusWorkerDependencies;
}

afterEach(async () => {
  await Promise.all([...activeServers].map((server) => server.close()));
  activeServers.clear();
  await Promise.all(temporaryDirectories.splice(0).map((directory) => rm(directory, { recursive: true, force: true })));
});

describe("agent status worker", () => {
  it("processes valid formula lifecycles for Claude Code, Codex, Cursor, Pi, and OpenCode", async () => {
    for (const agent of ["claude", "codex", "cursor", "pi", "opencode"] satisfies SupportedAgent[]) {
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
      expect(rig.renderedTexts).toEqual(["The relation is $E=mc^2$."]);
      expect(rig.publications).toHaveLength(1);
      expect(rig.server.requests.filter(({ method }) => method === "pane.read")).toHaveLength(5);

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

  it("does not publish a formula that appears only in Pi reasoning", async () => {
    const rig = await createRig({
      agent: "pi",
      agent_session: { source: "herdr:pi", agent: "pi", kind: "id", value: "session-pi" }
    });
    const baseline = "Synthetic Pi baseline before the response.";
    rig.server.setPaneOutput("w1:p1", baseline);
    expect((await process(rig, rig.server.transitionPane("w1:p1", "working"))).ok).toBe(true);

    const completion = `${baseline}\nReasoning checks $r=1$.\n\nFinal response has no equation.\n\n────────────────────────`;
    const ansiCompletion = `${baseline}\n\u001b[3mReasoning checks $r=1$.\u001b[0m\n\nFinal response has no equation.\n\n────────────────────────`;
    rig.server.setPaneOutput("w1:p1", completion, false, ansiCompletion);

    expect(await process(rig, rig.server.transitionPane("w1:p1", "done"))).toMatchObject({
      ok: true,
      value: { kind: "completion_recorded", formulaCount: 0 }
    });
    expect(rig.renders).toHaveLength(0);
    expect(rig.publications).toHaveLength(0);
  });

  it("renders Pi final prose and math without reasoning or tool output", async () => {
    const rig = await createRig({
      agent: "pi",
      agent_session: { source: "herdr:pi", agent: "pi", kind: "id", value: "session-pi-final" }
    });
    const baseline = "Synthetic Pi baseline before a styled response.";
    rig.server.setPaneOutput("w1:p1", baseline);
    expect((await process(rig, rig.server.transitionPane("w1:p1", "working"))).ok).toBe(true);

    const plain =
      "Reasoning uses $r=1$.\n\nread package.json\n\nFinal answer is $E=mc^2$.\n\n────────────────────────\nstatus";
    const ansi =
      "\u001b[3mReasoning uses $r=1$.\u001b[0m\n\n\u001b[1;48;2;40;50;40mread package.json\u001b[0m\n\nFinal answer is $E=mc^2$.\n\n────────────────────────\nstatus";
    rig.server.setPaneOutput("w1:p1", `${baseline}\n${plain}`, false, `${baseline}\n${ansi}`);

    expect(await process(rig, rig.server.transitionPane("w1:p1", "done"))).toMatchObject({
      ok: true,
      value: { kind: "image_published", formulaCount: 1 }
    });
    expect(rig.renderedTexts).toEqual(["Final answer is $E=mc^2$."]);
    expect(rig.renders).toEqual([[{ latex: "E=mc^2", display: false }]]);
  });

  it("recovers a complete Pi final response after an earlier ANSI unwrap mismatch", async () => {
    const rig = await createRig({
      agent: "pi",
      agent_session: { source: "herdr:pi", agent: "pi", kind: "id", value: "session-pi-ansi-suffix" }
    });
    const before = "Synthetic Pi anchor before an unwrap mismatch.";
    const stableFooter = "\n────────────────────────\nstatus with stable footer context";
    const baseline = `${before}\nworking row${stableFooter}`;
    rig.server.setPaneOutput("w1:p1", baseline);
    expect((await process(rig, rig.server.transitionPane("w1:p1", "working"))).ok).toBe(true);

    const plain = `${before}\nplain historical tool row\nReasoning checks $r=1$.\n\nFinal answer is $E=mc^2$.${stableFooter}`;
    const ansi = `${before}\ndifferent ANSI tool row\n\u001b[3mReasoning checks $r=1$.\u001b[0m\n\nFinal answer is $E=mc^2$.${stableFooter}`;
    rig.server.setPaneOutput("w1:p1", plain, false, ansi);

    expect(await process(rig, rig.server.transitionPane("w1:p1", "done"))).toMatchObject({
      ok: true,
      value: { kind: "image_published", formulaCount: 1 }
    });
    expect(rig.renderedTexts).toEqual(["Final answer is $E=mc^2$."]);
  });

  it("rejects Pi ANSI suffix recovery without a preceding styled boundary", async () => {
    const rig = await createRig({
      agent: "pi",
      agent_session: { source: "herdr:pi", agent: "pi", kind: "id", value: "session-pi-partial-suffix" }
    });
    const baseline = "Synthetic Pi baseline before a partial suffix.";
    rig.server.setPaneOutput("w1:p1", baseline);
    expect((await process(rig, rig.server.transitionPane("w1:p1", "working"))).ok).toBe(true);

    const plain = "plain historical row\nFinal answer tail is $E=mc^2$.\n\n────────────────────────\nstatus";
    const ansi = "different ANSI row\nFinal answer tail is $E=mc^2$.\n\n────────────────────────\nstatus";
    rig.server.setPaneOutput("w1:p1", `${baseline}\n${plain}`, false, `${baseline}\n${ansi}`);

    expect(await process(rig, rig.server.transitionPane("w1:p1", "done"))).toMatchObject({
      ok: false,
      error: { code: "conclusion_boundary_failed" }
    });
    expect(rig.renders).toHaveLength(0);
    expect(rig.publications).toHaveLength(0);
  });

  it("recovers OpenCode final output from plain text only between textual chrome boundaries", async () => {
    const rig = await createRig({
      agent: "opencode",
      agent_session: { source: "herdr:opencode", agent: "opencode", kind: "id", value: "session-opencode-plain" }
    });
    const anchor = "Synthetic fixed OpenCode footer anchor with unique value 1234567890";
    const suffix = `\n\n${anchor}\n\nok`;
    const baseline = `Short prompt\n→ Read source\n\nWorking $u$.\n\n▣ Working${suffix}`;
    rig.server.setPaneOutput("w1:p1", baseline);
    expect((await process(rig, rig.server.transitionPane("w1:p1", "working"))).ok).toBe(true);

    const plain = `A longer submitted prompt\n→ Read package.json\n\nFinal response keeps $u$ and adds $x=1$.\n\n▣ Done${suffix}`;
    const ansi = `A longer submitted prompt\n→ Read changed package.json\n\nFinal response keeps $u$ and adds $x=1$.\n\n▣ Done${suffix}`;
    rig.server.setPaneOutput("w1:p1", plain, false, ansi);

    expect(await process(rig, rig.server.transitionPane("w1:p1", "done"))).toMatchObject({
      ok: true,
      value: { kind: "image_published", formulaCount: 1 }
    });
    expect(rig.renderedTexts).toEqual(["Final response keeps $u$ and adds $x=1$."]);
    expect(rig.renders).toEqual([[{ latex: "x=1", display: false }]]);
  });

  it("rejects OpenCode plain snapshot recovery without both textual chrome boundaries", async () => {
    for (const testCase of [
      { plain: "Final response is $x=1$.\n\n▣ Done", ansi: "Changed final response is $x=1$.\n\n▣ Done" },
      { plain: "→ Read source\n\nFinal response is $x=1$.", ansi: "→ Read changed source\n\nFinal response is $x=1$." }
    ]) {
      const rig = await createRig({
        agent: "opencode",
        agent_session: {
          source: "herdr:opencode",
          agent: "opencode",
          kind: "id",
          value: "session-opencode-incomplete"
        }
      });
      const baseline = "Synthetic OpenCode baseline before an incomplete plain snapshot.";
      rig.server.setPaneOutput("w1:p1", baseline);
      expect((await process(rig, rig.server.transitionPane("w1:p1", "working"))).ok).toBe(true);
      rig.server.setPaneOutput("w1:p1", `${baseline}\n${testCase.plain}`, false, `${baseline}\n${testCase.ansi}`);

      expect(await process(rig, rig.server.transitionPane("w1:p1", "done"))).toMatchObject({
        ok: false,
        error: { code: "conclusion_boundary_failed" }
      });
      expect(rig.renders).toHaveLength(0);
      expect(rig.publications).toHaveLength(0);
    }
  });

  it("recovers OpenCode final output after a fixed same-row header", async () => {
    const rig = await createRig({
      agent: "opencode",
      agent_session: { source: "herdr:opencode", agent: "opencode", kind: "id", value: "session-opencode-suffix" }
    });
    const anchor = "Synthetic fixed OpenCode header anchor with unique value 1234567890";
    const prefix = `Stable header context with unique value abcdefghij\n${anchor}`;
    const baseline = `${prefix}\nstable bridge one\nstable bridge two\n→ Read source\n\nWorking $u$.\n\n▣ Working`;
    rig.server.setPaneOutput("w1:p1", baseline);
    expect((await process(rig, rig.server.transitionPane("w1:p1", "working"))).ok).toBe(true);

    const plain = `${prefix}\nstable bridge one\nstable bridge two\n→ Read package.json\n\nFinal response keeps $u$ and adds $x=1$.\n\n▣ Done`;
    const ansi = `${prefix}\nstable bridge one\nstable bridge two\n→ Read changed package.json\n\nFinal response keeps $u$ and adds $x=1$.\n\n▣ Done`;
    rig.server.setPaneOutput("w1:p1", plain, false, ansi);

    expect(await process(rig, rig.server.transitionPane("w1:p1", "done"))).toMatchObject({
      ok: true,
      value: { kind: "image_published", formulaCount: 1 }
    });
    expect(rig.renderedTexts).toEqual(["Final response keeps $u$ and adds $x=1$."]);
    expect(rig.renders).toEqual([[{ latex: "x=1", display: false }]]);
  });

  it("publishes a completed formula through the owned graphics viewer", async () => {
    const rig = await createRig();
    const client = new HerdrSocketClient(rig.server.socketPath);
    const presenter = new ViewerPresenter(client);
    rig.dependencies.client = client;
    rig.dependencies.publish = (request) =>
      publishImage(request, {
        client,
        sessionIdentity: rig.server.socketPath,
        present: (presentation) => presenter.present(presentation)
      });
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

  it("publishes a formula inserted before a stable alternate-screen gap", async () => {
    const rig = await createRig({
      agent: "pi",
      agent_session: { source: "herdr:pi", agent: "pi", kind: "id", value: "alternate-screen-session" }
    });
    const before = "Synthetic submitted prompt anchor with unique value 1234567890";
    const gap = "\nstable alternate-screen status\n";
    const after = "Synthetic footer anchor with unique value abcdefghijklmnop";
    rig.server.setPaneOutput("w1:p1", `${before}${gap}${after}`);
    expect((await process(rig, rig.server.transitionPane("w1:p1", "working"))).ok).toBe(true);

    rig.server.setPaneOutput("w1:p1", `${before}\nanswer $$x^2+y^2=z^2$$${gap}${after}`);
    expect(await process(rig, rig.server.transitionPane("w1:p1", "idle"))).toMatchObject({
      ok: true,
      value: { kind: "image_published", formulaCount: 1 }
    });
    expect(rig.renders).toEqual([[{ latex: "x^2+y^2=z^2", display: true }]]);
  });

  it("publishes only new formulas from an alternate-screen replacement", async () => {
    const rig = await createRig({
      agent: "opencode",
      agent_session: { source: "herdr:opencode", agent: "opencode", kind: "id", value: "replacement-session" }
    });
    const before = "Synthetic working anchor with unique value 1234567890";
    const baselineGap = "\nworking $u$\n";
    const after = "Synthetic footer anchor with unique value abcdefghij";
    const following = "\n\nSynthetic stable footer context with unique value 9876543210";
    rig.server.setPaneOutput("w1:p1", `${before}${baselineGap}${after}${following}`);
    expect((await process(rig, rig.server.transitionPane("w1:p1", "working"))).ok).toBe(true);

    const replacement = "\ncompleted $u$ and $$x^2+y^2=z^2$$\n";
    rig.server.setPaneOutput("w1:p1", `${before}${replacement}${after}${following}`);
    expect(await process(rig, rig.server.transitionPane("w1:p1", "idle"))).toMatchObject({
      ok: true,
      value: { kind: "image_published", formulaCount: 1 }
    });
    expect(rig.renders).toEqual([[{ latex: "x^2+y^2=z^2", display: true }]]);

    const nextBefore = "Synthetic next working anchor with unique value 9876543210";
    const nextGap = "\nworking without a formula\n";
    const nextAfter = "Synthetic next footer anchor with unique value zyxwvutsrq";
    rig.server.setPaneOutput("w1:p1", `${nextBefore}${nextGap}${nextAfter}${following}`);
    expect((await process(rig, rig.server.transitionPane("w1:p1", "working"))).ok).toBe(true);
    rig.server.setPaneOutput("w1:p1", `${nextBefore}\nno equation here\n${nextAfter}${following}`);
    expect(await process(rig, rig.server.transitionPane("w1:p1", "idle"))).toMatchObject({
      ok: true,
      value: { kind: "completion_recorded", formulaCount: 0 }
    });
    expect(rig.renders).toHaveLength(1);
    expect(rig.publications).toHaveLength(1);
  });

  it("processes a later turn when the agent sequence advances without a pane revision change", async () => {
    const rig = await createRig({ revision: 4, state_change_seq: 10 });
    const firstBaseline = "First baseline for a stable pane metadata revision.";
    rig.server.setPaneOutput("w1:p1", firstBaseline);
    rig.server.updatePane("w1:p1", { agent_status: "working", revision: 4, state_change_seq: 11 });
    expect(await process(rig, statusEvent("w1", "w1:p1", "working", "codex"))).toMatchObject({
      ok: true,
      value: { kind: "baseline_stored", generation: 1 }
    });
    rig.server.setPaneOutput("w1:p1", `${firstBaseline}\nFirst $x$.`);
    rig.server.updatePane("w1:p1", { agent_status: "done", revision: 4, state_change_seq: 12 });
    expect((await process(rig, statusEvent("w1", "w1:p1", "done", "codex"))).ok).toBe(true);

    const secondBaseline = "Second baseline while pane revision remains unchanged.";
    rig.server.setPaneOutput("w1:p1", secondBaseline);
    rig.server.updatePane("w1:p1", { agent_status: "working", revision: 4, state_change_seq: 13 });
    expect(await process(rig, statusEvent("w1", "w1:p1", "working", "codex"))).toMatchObject({
      ok: true,
      value: { kind: "baseline_stored", generation: 2 }
    });
    rig.server.setPaneOutput("w1:p1", `${secondBaseline}\nSecond $y$.`);
    rig.server.updatePane("w1:p1", { agent_status: "done", revision: 4, state_change_seq: 14 });
    expect(await process(rig, statusEvent("w1", "w1:p1", "done", "codex"))).toMatchObject({
      ok: true,
      value: { kind: "image_published", generation: 2 }
    });

    expect(rig.renders).toEqual([[{ latex: "x", display: false }], [{ latex: "y", display: false }]]);
    expect(rig.publications).toHaveLength(2);
    expect(await loadState(rig)).toMatchObject({ generation: 2, pane_revision: 4, event_sequence: 14 });
  });

  it("ignores panes outside configured allowed directories", async () => {
    const rig = await createRig({ cwd: "/tmp/other-project" });
    rig.dependencies.pluginConfig = parsePluginConfig({
      allowed_directories: ["/Users/example/obsidian"]
    });

    expect(await process(rig, rig.server.transitionPane("w1:p1", "working"))).toEqual({
      ok: true,
      value: { kind: "ignored", status: "working", reason: "directory_out_of_scope" }
    });
    expect(rig.publications).toHaveLength(0);
  });

  it("processes panes inside configured allowed directories", async () => {
    const allowed = "/Users/example/obsidian";
    const rig = await createRig({ cwd: `${allowed}/notes` });
    rig.dependencies.pluginConfig = parsePluginConfig({ allowed_directories: [allowed] });

    rig.server.setPaneOutput("w1:p1", "BASELINE");
    expect(await process(rig, rig.server.transitionPane("w1:p1", "working"))).toMatchObject({
      ok: true,
      value: { kind: "baseline_stored" }
    });
  });

  it("captures the working snapshot before setup and authoritative lookups", async () => {
    const rig = await createRig();
    rig.server.setPaneOutput("w1:p1", "Immediate working baseline before a fast response.");
    const event = rig.server.transitionPane("w1:p1", "working");

    expect(
      await runAgentStatusHook({
        HERDR_PLUGIN_EVENT_JSON: JSON.stringify(event),
        HERDR_PLUGIN_CONFIG_DIR: rig.directory,
        HERDR_PLUGIN_STATE_DIR: rig.directory,
        HERDR_SOCKET_PATH: rig.server.socketPath
      })
    ).toMatchObject({ ok: true, value: { kind: "baseline_stored", generation: 1 } });
    expect(rig.server.requests.map(({ method }) => method)).toEqual([
      "pane.read",
      "pane.get",
      "agent.get",
      "pane.get",
      "agent.get"
    ]);
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

  it("ignores panes without a supported agent before reading or mutating state", async () => {
    const rig = await createRig({ agent: null, agent_session: null, agent_status: "working" });
    expect(await process(rig, statusEvent("w1", "w1:p1", "working"))).toEqual({
      ok: true,
      value: { kind: "ignored", status: "working", reason: "no_agent" }
    });

    rig.server.updatePane("w1:p1", { agent: "hermes", agent_session: null, agent_status: "working" });
    expect(await process(rig, statusEvent("w1", "w1:p1", "working", "hermes"))).toEqual({
      ok: true,
      value: { kind: "ignored", status: "working", reason: "agent_unsupported" }
    });

    expect(rig.server.requests.map(({ method }) => method)).toEqual(["pane.get", "pane.get"]);
    expect(rig.renders).toHaveLength(0);
    expect(rig.publications).toHaveLength(0);
    expect(await readdir(rig.directory)).toEqual([]);
  });

  it("fails closed before state mutation for invalid, unresolved, and stale events", async () => {
    const rig = await createRig();
    let calls = 0;
    const rejectingClient: AgentStatusHerdrClient = {
      paneGet: () => {
        calls += 1;
        return Promise.resolve(failure(serializeError(new HerdrMathError("herdr_protocol_error"))));
      },
      paneGetIfPresent: () => {
        calls += 1;
        return Promise.resolve(failure(serializeError(new HerdrMathError("herdr_protocol_error"))));
      },
      agentGet: () => {
        calls += 1;
        return Promise.resolve(failure(serializeError(new HerdrMathError("herdr_protocol_error"))));
      },
      paneRead: () => {
        calls += 1;
        return Promise.resolve(failure(serializeError(new HerdrMathError("herdr_protocol_error"))));
      },
      paneReadAnsi: () => {
        calls += 1;
        return Promise.resolve(failure(serializeError(new HerdrMathError("herdr_protocol_error"))));
      },
      paneLayout: () => {
        calls += 1;
        return Promise.resolve(failure(serializeError(new HerdrMathError("herdr_protocol_error"))));
      },
      paneGraphicsInfo: () => {
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
      code: "event_invalid";
    }> = [
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
      }
    ];
    for (const testCase of cases) {
      rig.server.updatePane("w1:p1", testCase.pane);
      const result = await process(rig, testCase.event);
      expect(result).toEqual({ ok: false, error: { code: testCase.code, retryable: false } });
    }

    rig.server.updatePane("w1:p1", { agent: "codex", agent_status: "working" });
    const baseClient = rig.dependencies.client;
    expect(
      await processAgentStatusEvent(JSON.stringify(statusEvent("w1", "w1:p1", "working", "codex")), {
        ...rig.dependencies,
        client: {
          paneGet: (paneId) => baseClient.paneGet(paneId),
          paneGetIfPresent: (paneId) => baseClient.paneGetIfPresent(paneId),
          paneRead: (paneId) => baseClient.paneRead(paneId),
          paneReadAnsi: (paneId) => baseClient.paneReadAnsi(paneId),
          paneLayout: (paneId) => baseClient.paneLayout(paneId),
          paneGraphicsInfo: (paneId) => baseClient.paneGraphicsInfo(paneId),
          agentGet: async (paneId) => {
            const resolved = await baseClient.agentGet(paneId);
            return resolved.ok ? success({ ...resolved.value, status: "done" }) : resolved;
          }
        }
      })
    ).toEqual({ ok: false, error: { code: "event_invalid", retryable: false } });
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
      paneGetIfPresent: (paneId) => baseClient.paneGetIfPresent(paneId),
      agentGet: (paneId) => baseClient.agentGet(paneId),
      paneReadAnsi: (paneId) => baseClient.paneReadAnsi(paneId),
      paneLayout: (paneId) => baseClient.paneLayout(paneId),
      paneGraphicsInfo: (paneId) => baseClient.paneGraphicsInfo(paneId),
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
  const renderedTexts: string[] = [];
  const publications: ImagePublishRequest[] = [];
  const dependencies: AgentStatusWorkerDependencies = {
    client: new HerdrSocketClient(server.socketPath),
    stateDirectory: directory,
    sessionIdentity: server.socketPath,
    secret: SECRET,
    pluginConfig: { allowedDirectories: [] },
    now: () => NOW,
    sleep: () => Promise.resolve(),
    timing: { completionDebounceMs: 0, stableReadIntervalMs: 0 },
    render: ({ text, formulas }) => {
      renderedTexts.push(text);
      renders.push(formulas.map(({ latex, display }) => ({ latex, display })));
      return Promise.resolve(success(image()));
    },
    publish: (request) => {
      publications.push(request);
      return Promise.resolve(success({ viewerPaneId: "w1:p9" }));
    }
  };
  return { server, directory, renders, renderedTexts, publications, dependencies };
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
