import { describe, expect, it } from "vitest";

import { POLICY_LIMITS } from "../../src/core/limits.js";
import { FullLifecycleRig } from "../support/full-lifecycle-rig.js";

describe("adversarial event and protocol boundaries", () => {
  it.each([
    ["malformed JSON", "not-json"],
    [
      "path traversal identifier",
      JSON.stringify({
        event: "pane_agent_status_changed",
        data: {
          type: "pane_agent_status_changed",
          workspace_id: "w1",
          pane_id: "../outside",
          agent_status: "working"
        }
      })
    ],
    ["oversized JSON", JSON.stringify({ ignored: "x".repeat(POLICY_LIMITS.eventJsonBytes) })]
  ])("rejects %s before Herdr or state I/O", async (_name, source) => {
    const rig = await FullLifecycleRig.start("codex");
    try {
      expect(await rig.processRaw(source)).toEqual({
        ok: false,
        error: { code: "event_invalid", retryable: false }
      });
      expect(rig.server.requests).toHaveLength(0);
      expect(await rig.state()).toBeUndefined();
    } finally {
      await rig.close();
    }
  });

  it("maps slow and disconnected Herdr responses, then recovers without fallback", async () => {
    const rig = await FullLifecycleRig.start("codex", { paneGetTimeoutMs: 25, paneReadTimeoutMs: 25 });
    try {
      const baseline = "Protocol recovery baseline with unique content.\n";
      rig.server.setPaneOutput("w1:p1", baseline);
      rig.server.queueResponse("pane.get", { delayMs: 50 });
      const firstWorking = rig.server.transitionPane("w1:p1", "working");
      expect(await rig.process(firstWorking)).toEqual({
        ok: false,
        error: { code: "herdr_timeout", retryable: true }
      });
      expect(await rig.state()).toBeUndefined();

      const working = rig.server.transitionPane("w1:p1", "working");
      expect(await rig.process(working)).toMatchObject({ ok: true, value: { kind: "baseline_stored" } });
      rig.server.setPaneOutput("w1:p1", `${baseline}Recovered $x=1$.`);
      const done = rig.server.transitionPane("w1:p1", "done");
      rig.server.queueResponse("pane.read", { disconnect: true });
      expect(await rig.process(done)).toEqual({
        ok: false,
        error: { code: "herdr_protocol_error", retryable: true }
      });
      expect((await rig.state())?.processed).toBeUndefined();

      expect(await rig.process(done)).toMatchObject({
        ok: true,
        value: { kind: "image_published", formulaCount: 1 }
      });
      expect(rig.server.graphicsUpdates).toHaveLength(1);
      expect(rig.server.getPane("w1:p1")?.focused).toBe(true);
    } finally {
      await rig.close();
    }
  });

  it("does not serialize malformed remote frames or their sentinel content", async () => {
    const rig = await FullLifecycleRig.start("pi");
    const sentinel = "SECRET_REMOTE_FRAME_AND_PATH";
    try {
      rig.server.setPaneOutput("w1:p1", "Malformed remote frame baseline.");
      rig.server.queueResponse("pane.get", { raw: `${sentinel}\n` });
      const result = await rig.process(rig.server.transitionPane("w1:p1", "working"));
      expect(result).toEqual({ ok: false, error: { code: "herdr_protocol_error", retryable: false } });
      expect(JSON.stringify(result)).not.toContain(sentinel);
      expect(await rig.state()).toBeUndefined();
    } finally {
      await rig.close();
    }
  });

  it("rejects an old completion after a newer working generation", async () => {
    const rig = await FullLifecycleRig.start("opencode");
    try {
      const firstBaseline = "First out-of-order baseline.\n";
      rig.server.setPaneOutput("w1:p1", firstBaseline);
      expect(await rig.process(rig.server.transitionPane("w1:p1", "working"))).toMatchObject({
        ok: true,
        value: { generation: 1 }
      });
      rig.server.setPaneOutput("w1:p1", `${firstBaseline}Old result $x=1$.`);
      const oldDone = rig.server.transitionPane("w1:p1", "done");

      const secondBaseline = "Second out-of-order baseline.\n";
      rig.server.setPaneOutput("w1:p1", secondBaseline);
      expect(await rig.process(rig.server.transitionPane("w1:p1", "working"))).toMatchObject({
        ok: true,
        value: { generation: 2 }
      });
      expect(await rig.process(oldDone)).toEqual({
        ok: false,
        error: { code: "event_invalid", retryable: false }
      });
      expect(rig.server.graphicsUpdates).toHaveLength(0);

      rig.server.setPaneOutput("w1:p1", `${secondBaseline}Current result $x=2$.`);
      expect(await rig.process(rig.server.transitionPane("w1:p1", "done"))).toMatchObject({
        ok: true,
        value: { kind: "image_published", generation: 2 }
      });
      expect(rig.server.graphicsUpdates).toHaveLength(1);
    } finally {
      await rig.close();
    }
  });
});
