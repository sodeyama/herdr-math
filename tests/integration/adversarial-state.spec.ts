import { readFile, rm, stat, symlink, writeFile } from "node:fs/promises";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

import { POLICY_LIMITS } from "../../src/core/limits.js";
import { acquirePaneLock } from "../../src/state/pane-lock.js";
import { deriveViewerSourceToken, VIEWER_IDENTITY } from "../../src/viewer/ownership.js";
import { createFakePane } from "../support/fake-herdr-types.js";
import { FullLifecycleRig } from "../support/full-lifecycle-rig.js";

const NOW = new Date("2026-08-01T00:00:00.000Z");

describe("adversarial state and ownership boundaries", () => {
  it.each(["malformed", "unknown_version", "path_field", "oversized"])(
    "rejects %s canonical state inside the real worker",
    async (kind) => {
      const rig = await FullLifecycleRig.start("codex");
      try {
        const baseline = `Corrupt state baseline for ${kind}.\n`;
        rig.server.setPaneOutput("w1:p1", baseline);
        expect((await rig.process(rig.server.transitionPane("w1:p1", "working"))).ok).toBe(true);
        const panePaths = rig.paths();
        const current = JSON.parse(await readFile(panePaths.statePath, "utf8")) as Record<string, unknown>;
        const content =
          kind === "malformed"
            ? "not-json"
            : kind === "unknown_version"
              ? JSON.stringify({ ...current, schema_version: 2 })
              : kind === "path_field"
                ? JSON.stringify({ ...current, source_pane_id: "../outside" })
                : "x".repeat(POLICY_LIMITS.stateFileBytes + 1);
        await writeFile(panePaths.statePath, content);
        const outside = join(rig.stateDirectory, "outside-sentinel");
        await writeFile(outside, "KEEP_OUTSIDE_STATE");

        rig.server.setPaneOutput("w1:p1", `${baseline}Unsafe result $x=1$.`);
        const result = await rig.process(rig.server.transitionPane("w1:p1", "done"));
        expect(result).toEqual({ ok: false, error: { code: "state_corrupt", retryable: false } });
        await expect(stat(panePaths.statePath)).rejects.toMatchObject({ code: "ENOENT" });
        expect(await readFile(outside, "utf8")).toBe("KEEP_OUTSIDE_STATE");
        expect(rig.server.graphicsUpdates).toHaveLength(0);
      } finally {
        await rig.close();
      }
    }
  );

  it("rejects a symlinked canonical state without reading or changing its target", async () => {
    const rig = await FullLifecycleRig.start("claude");
    try {
      const baseline = "Symlink state baseline with unique content.\n";
      rig.server.setPaneOutput("w1:p1", baseline);
      expect((await rig.process(rig.server.transitionPane("w1:p1", "working"))).ok).toBe(true);
      const panePaths = rig.paths();
      const outside = join(rig.stateDirectory, "outside-symlink-target");
      await writeFile(outside, "SECRET_OUTSIDE_TARGET");
      await rm(panePaths.statePath);
      await symlink(outside, panePaths.statePath);

      rig.server.setPaneOutput("w1:p1", `${baseline}Unsafe result $x=1$.`);
      expect(await rig.process(rig.server.transitionPane("w1:p1", "done"))).toEqual({
        ok: false,
        error: { code: "state_corrupt", retryable: false }
      });
      expect(await readFile(outside, "utf8")).toBe("SECRET_OUTSIDE_TARGET");
      expect(rig.server.graphicsUpdates).toHaveLength(0);
    } finally {
      await rig.close();
    }
  });

  it("protects live locks and treats an old live PID as unsafe to reclaim", async () => {
    const rig = await FullLifecycleRig.start("pi");
    try {
      const panePaths = rig.paths();
      const old = new Date(NOW.getTime() - POLICY_LIMITS.staleLockAgeMs - 1);
      const lock = await acquirePaneLock(panePaths, { eventType: "working", now: old, processId: process.pid });
      await expect(
        acquirePaneLock(panePaths, {
          eventType: "done",
          now: NOW,
          processId: process.pid + 1,
          isProcessAlive: (processId) => processId === process.pid
        })
      ).rejects.toMatchObject({ code: "state_locked", retryable: true });

      rig.server.setPaneOutput("w1:p1", "Live lock baseline.");
      const event = rig.server.transitionPane("w1:p1", "working");
      expect(await rig.process(event)).toEqual({
        ok: false,
        error: { code: "state_locked", retryable: true }
      });
      await lock.release();
      expect(await rig.process(event)).toMatchObject({ ok: true, value: { kind: "baseline_stored" } });
    } finally {
      await rig.close();
    }
  });

  it("never updates or closes a user pane that spoofs a stored viewer id", async () => {
    const rig = await FullLifecycleRig.start("codex");
    try {
      const first = await rig.runCycle("Owned viewer baseline.\n", "First $x=1$.");
      expect(first.completion).toMatchObject({ ok: true, value: { viewerPaneId: "w1:p2" } });
      expect(rig.server.closePane("w1:p2")).toBe(true);
      rig.server.addPane(
        createFakePane({
          pane_id: "w1:p2",
          terminal_id: "user-term",
          agent: null,
          focused: false,
          title: "User notes",
          tokens: {
            [VIEWER_IDENTITY.ownerTokenKey]: "spoofed-owner",
            [VIEWER_IDENTITY.sourceTokenKey]: deriveViewerSourceToken(rig.server.socketPath, "w1:p1")
          }
        })
      );

      const second = await rig.runCycle("Spoofed viewer baseline.\n", "Second $x=2$.");
      expect(second.completion).toMatchObject({ ok: true, value: { kind: "image_published" } });
      if (!second.completion.ok || second.completion.value.kind !== "image_published") {
        throw new Error("Expected a replacement viewer");
      }
      expect(second.completion.value.viewerPaneId).not.toBe("w1:p2");
      expect(rig.server.getPane("w1:p2")).toMatchObject({ title: "User notes" });
      expect(rig.server.getGraphics("w1:p2")).toBeUndefined();
      expect(rig.server.requests.some(({ method }) => method === "plugin.pane.close")).toBe(false);
      expect(rig.server.getGraphics(second.completion.value.viewerPaneId)).toBeDefined();
      expect(rig.server.getPane("w1:p1")?.focused).toBe(true);
    } finally {
      await rig.close();
    }
  });
});
