import { Buffer } from "node:buffer";
import { readFileSync } from "node:fs";

import { describe, expect, it, vi } from "vitest";

import { POLICY_LIMITS } from "../../src/core/limits.js";
import { decodeAgentStatusEvent } from "../../src/herdr/event-decoder.js";

interface EventFixture {
  event: string;
  data: Record<string, unknown>;
}

const fixtures = JSON.parse(
  readFileSync(new URL("../fixtures/herdr/agent-status-events.json", import.meta.url), "utf8")
) as EventFixture[];

describe("Herdr agent-status event decoder", () => {
  it("extracts only bounded fields from every supported-agent fixture", () => {
    for (const fixture of fixtures) {
      const result = decodeAgentStatusEvent(JSON.stringify({ ...fixture, ignored: "not returned" }));
      expect(result.ok).toBe(true);
      if (!result.ok) continue;
      expect(result.value).toEqual({
        event: "pane_agent_status_changed",
        workspaceId: fixture.data.workspace_id,
        sourcePaneId: fixture.data.pane_id,
        status: fixture.data.agent_status,
        agentHint: fixture.data.agent
      });
      expect(Object.keys(result.value).sort()).toEqual(["agentHint", "event", "sourcePaneId", "status", "workspaceId"]);
      expect(Object.isFrozen(result.value)).toBe(true);
    }
  });

  it("accepts a null or absent optional agent hint without allowlisting it", () => {
    for (const agent of [null, undefined]) {
      const data: Record<string, unknown> = {
        type: "pane_agent_status_changed",
        workspace_id: "w1",
        pane_id: "w1:p5",
        agent_status: "working",
        state_labels: { safe: "ignored" }
      };
      if (agent !== undefined) data.agent = agent;
      const result = decodeAgentStatusEvent(JSON.stringify({ event: "pane_agent_status_changed", data }));
      expect(result).toEqual({
        ok: true,
        value: {
          event: "pane_agent_status_changed",
          workspaceId: "w1",
          sourcePaneId: "w1:p5",
          status: "working"
        }
      });
    }

    const unsupportedHint = decodeAgentStatusEvent(
      JSON.stringify({
        event: "pane_agent_status_changed",
        data: {
          type: "pane_agent_status_changed",
          workspace_id: "w1",
          pane_id: "w1:p5",
          agent_status: "working",
          agent: "future-agent"
        }
      })
    );
    expect(unsupportedHint.ok && unsupportedHint.value.agentHint).toBe("future-agent");
  });

  it.each([
    ["invalid JSON", "{"],
    ["array envelope", "[]"],
    ["wrong event", validSource({ event: "pane_closed" })],
    ["wrong data type", validSource({ data: { type: "pane_closed" } })],
    ["missing workspace", validSource({ data: { workspace_id: undefined } })],
    ["missing pane", validSource({ data: { pane_id: undefined } })],
    ["wrong workspace type", validSource({ data: { workspace_id: 1 } })],
    ["wrong pane type", validSource({ data: { pane_id: false } })],
    ["unknown status", validSource({ data: { agent_status: "paused" } })],
    ["wrong agent type", validSource({ data: { agent: { id: "codex" } } })],
    ["empty agent", validSource({ data: { agent: "" } })],
    ["path-like pane", validSource({ data: { pane_id: "../pane" } })],
    ["backslash workspace", validSource({ data: { workspace_id: "workspace\\child" } })],
    ["control character", validSource({ data: { pane_id: "pane\nchild" } })],
    ["oversized identifier", validSource({ data: { pane_id: "p".repeat(129) } })]
  ])("rejects %s with one safe error", (_name, source) => {
    expect(decodeAgentStatusEvent(source)).toEqual({
      ok: false,
      error: { code: "event_invalid", retryable: false }
    });
  });

  it("rejects an oversized event before returning or logging its contents", () => {
    const sentinel = "PRIVATE_EVENT_SENTINEL";
    const source = validSource({ data: { ignored: `${sentinel}${"x".repeat(POLICY_LIMITS.eventJsonBytes)}` } });
    const log = vi.spyOn(console, "log").mockImplementation(() => undefined);
    const error = vi.spyOn(console, "error").mockImplementation(() => undefined);

    const result = decodeAgentStatusEvent(source);
    const serialized = JSON.stringify(result);
    expect(Buffer.byteLength(source, "utf8")).toBeGreaterThan(POLICY_LIMITS.eventJsonBytes);
    expect(result).toEqual({ ok: false, error: { code: "event_invalid", retryable: false } });
    expect(serialized).not.toContain(sentinel);
    expect(log).not.toHaveBeenCalled();
    expect(error).not.toHaveBeenCalled();
    log.mockRestore();
    error.mockRestore();
  });
});

function validSource(overrides: { event?: unknown; data?: Record<string, unknown> } = {}): string {
  const data: Record<string, unknown> = {
    type: "pane_agent_status_changed",
    workspace_id: "w1",
    pane_id: "w1:p1",
    agent_status: "working",
    agent: "codex",
    ...overrides.data
  };
  for (const [key, value] of Object.entries(data)) {
    if (value === undefined) delete data[key];
  }
  return JSON.stringify({ event: overrides.event ?? "pane_agent_status_changed", data });
}
