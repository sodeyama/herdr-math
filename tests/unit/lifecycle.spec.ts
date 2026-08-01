import { Buffer } from "node:buffer";

import { describe, expect, it } from "vitest";

import { buildBaselineFingerprint } from "../../src/boundary/fingerprint-builder.js";
import type { FingerprintStateV1 } from "../../src/boundary/fingerprint-schema.js";
import {
  commitCompletion,
  transitionLifecycle,
  type CompletionAuthorization,
  type LifecycleEvent
} from "../../src/events/lifecycle.js";

const secret = Buffer.alloc(32, 8);
const now = new Date("2026-08-01T00:00:00.000Z");

function candidate(overrides: Partial<LifecycleEvent> = {}): FingerprintStateV1 {
  const event = lifecycleEvent(overrides);
  const built = buildBaselineFingerprint(
    "Synthetic baseline with enough unique content for lifecycle tests.",
    {
      sessionIdentity: "session",
      occupantIdentity: "occupant",
      workspaceId: event.workspaceId,
      sourcePaneId: event.sourcePaneId,
      agent: event.agent,
      lifecycleAuthority: event.lifecycleAuthority,
      paneRevision: event.paneRevision,
      eventSequence: event.eventSequence,
      generation: 0,
      createdAt: now
    },
    secret
  );
  return { ...built, session_key: event.sessionKey, occupant_key: event.occupantKey };
}

function lifecycleEvent(overrides: Partial<LifecycleEvent> = {}): LifecycleEvent {
  return {
    status: "working",
    sessionKey: candidateSessionKey,
    workspaceId: "w1",
    sourcePaneId: "w1:p1",
    agent: "codex",
    lifecycleAuthority: "screen_detection",
    occupantKey: "b".repeat(64),
    paneRevision: 10,
    eventSequence: 5,
    ...overrides
  };
}

const candidateSessionKey = buildBaselineFingerprint(
  "seed",
  {
    sessionIdentity: "session",
    occupantIdentity: "seed",
    workspaceId: "w1",
    sourcePaneId: "w1:p1",
    agent: "codex",
    lifecycleAuthority: "screen_detection",
    paneRevision: 0,
    eventSequence: 0,
    generation: 0,
    createdAt: now
  },
  secret
).session_key;

function initialState(): FingerprintStateV1 {
  const event = lifecycleEvent();
  const decision = transitionLifecycle(undefined, event, candidate());
  if (decision.kind !== "store_baseline") throw new Error("Expected a stored baseline.");
  return decision.state;
}

function completionEvent(overrides: Partial<LifecycleEvent> = {}): LifecycleEvent {
  return lifecycleEvent({ status: "done", paneRevision: 11, eventSequence: 6, ...overrides });
}

function authorization(state = initialState(), event = completionEvent()): CompletionAuthorization {
  const decision = transitionLifecycle(state, event);
  if (decision.kind !== "process_completion") throw new Error("Expected completion authorization.");
  return decision.authorization;
}

describe("pure lifecycle transitions", () => {
  it("creates generation one for the first working event without raw text", () => {
    const decision = transitionLifecycle(undefined, lifecycleEvent(), candidate());

    expect(decision.kind).toBe("store_baseline");
    if (decision.kind !== "store_baseline") return;
    expect(decision.state.generation).toBe(1);
    expect(JSON.stringify(decision)).not.toContain("Synthetic baseline");
  });

  it("preserves one generation for duplicate working and rejects stale working", () => {
    const current = initialState();
    expect(transitionLifecycle(current, lifecycleEvent(), candidate())).toEqual({
      kind: "preserve",
      reason: "duplicate_working"
    });
    const staleEvent = lifecycleEvent({ eventSequence: 4, paneRevision: 9 });
    expect(transitionLifecycle(current, staleEvent, candidate(staleEvent))).toEqual({
      kind: "preserve",
      reason: "stale_event"
    });
    const staleRevision = lifecycleEvent({ eventSequence: 6, paneRevision: 9 });
    expect(transitionLifecycle(current, staleRevision, candidate(staleRevision))).toEqual({
      kind: "preserve",
      reason: "stale_event"
    });
  });

  it("creates a new generation and clears the previous processed digest", () => {
    const current = {
      ...initialState(),
      viewer_pane_id: "w1:p2",
      processed: {
        content_digest: "c".repeat(64),
        pane_revision: 11,
        processed_at: "2026-08-01T00:01:00.000Z"
      }
    };
    const nextEvent = lifecycleEvent({ eventSequence: 7, paneRevision: 12 });
    const decision = transitionLifecycle(current, nextEvent, candidate(nextEvent));

    expect(decision.kind).toBe("store_baseline");
    if (decision.kind !== "store_baseline") return;
    expect(decision.state.generation).toBe(2);
    expect(decision.state.processed).toBeUndefined();
    expect(decision.state.viewer_pane_id).toBe("w1:p2");
  });

  it("preserves the baseline for blocked and unknown events", () => {
    const current = initialState();
    expect(transitionLifecycle(current, completionEvent({ status: "blocked" }))).toEqual({
      kind: "preserve",
      reason: "blocked"
    });
    expect(transitionLifecycle(current, completionEvent({ status: "unknown" }))).toEqual({
      kind: "preserve",
      reason: "unknown"
    });
    expect(current).toEqual(initialState());
  });

  it("authorizes done and idle only with a matching active baseline", () => {
    const current = initialState();
    expect(transitionLifecycle(current, completionEvent()).kind).toBe("process_completion");
    expect(transitionLifecycle(current, completionEvent({ status: "idle" })).kind).toBe("process_completion");
    expect(transitionLifecycle(undefined, completionEvent())).toEqual({ kind: "reject", code: "baseline_missing" });
    expect(transitionLifecycle(current, completionEvent({ occupantKey: "d".repeat(64) }))).toEqual({
      kind: "reject",
      code: "baseline_missing"
    });
    expect(transitionLifecycle(current, completionEvent({ lifecycleAuthority: "integration_hook" }))).toEqual({
      kind: "reject",
      code: "baseline_missing"
    });
  });

  it("replaces the occupant only on a new working baseline", () => {
    const current = initialState();
    const replacementEvent = lifecycleEvent({
      occupantKey: "d".repeat(64),
      eventSequence: 1,
      paneRevision: 1
    });
    const replacement = transitionLifecycle(current, replacementEvent, candidate(replacementEvent));
    if (replacement.kind !== "store_baseline") throw new Error("Expected an occupant replacement.");

    expect(replacement.state.generation).toBe(2);
    expect(replacement.state.occupant_key).toBe("d".repeat(64));
    expect(transitionLifecycle(replacement.state, completionEvent())).toEqual({
      kind: "reject",
      code: "baseline_missing"
    });
  });

  it("suppresses done and idle after the same pane revision was processed", () => {
    const current = commitCompletion(initialState(), {
      ...authorization(),
      contentDigest: "d".repeat(64),
      processedAt: new Date("2026-08-01T00:01:00.000Z"),
      viewerPaneId: "w1:p2"
    });
    if (current.kind !== "commit_completion") throw new Error("Expected a committed completion.");

    expect(transitionLifecycle(current.state, completionEvent({ status: "idle" }))).toEqual({
      kind: "preserve",
      reason: "duplicate_completion"
    });
  });
});

describe("completion commit guards", () => {
  it("records a rendered completion and viewer", () => {
    const decision = commitCompletion(initialState(), {
      ...authorization(),
      contentDigest: "d".repeat(64),
      processedAt: new Date("2026-08-01T00:01:00.000Z"),
      viewerPaneId: "w1:p2"
    });

    expect(decision.kind).toBe("commit_completion");
    if (decision.kind !== "commit_completion") return;
    expect(decision.state.processed).toEqual({
      content_digest: "d".repeat(64),
      pane_revision: 11,
      processed_at: "2026-08-01T00:01:00.000Z"
    });
    expect(decision.state.viewer_pane_id).toBe("w1:p2");
  });

  it("records a no-formula completion without changing the viewer", () => {
    const current = { ...initialState(), viewer_pane_id: "w1:p2" };
    const decision = commitCompletion(current, {
      ...authorization(),
      contentDigest: "e".repeat(64),
      processedAt: new Date("2026-08-01T00:01:00.000Z")
    });

    expect(decision.kind).toBe("commit_completion");
    if (decision.kind !== "commit_completion") return;
    expect(decision.state.processed?.content_digest).toBe("e".repeat(64));
    expect(decision.state.viewer_pane_id).toBe("w1:p2");
  });

  it("suppresses duplicate final content", () => {
    const first = commitCompletion(initialState(), {
      ...authorization(),
      contentDigest: "f".repeat(64),
      processedAt: new Date("2026-08-01T00:01:00.000Z")
    });
    if (first.kind !== "commit_completion") throw new Error("Expected a committed completion.");
    expect(
      commitCompletion(first.state, {
        ...authorization(first.state, completionEvent({ status: "idle", paneRevision: 12, eventSequence: 7 })),
        contentDigest: "f".repeat(64),
        processedAt: new Date("2026-08-01T00:02:00.000Z")
      })
    ).toEqual({ kind: "preserve", reason: "duplicate_completion" });
  });

  it("prevents generation N from committing after generation N plus one", () => {
    const current = initialState();
    const staleAuthorization = authorization(current);
    const nextEvent = lifecycleEvent({ eventSequence: 7, paneRevision: 12 });
    const next = transitionLifecycle(current, nextEvent, candidate(nextEvent));
    if (next.kind !== "store_baseline") throw new Error("Expected a replacement generation.");

    expect(
      commitCompletion(next.state, {
        ...staleAuthorization,
        contentDigest: "a".repeat(64),
        processedAt: new Date("2026-08-01T00:01:00.000Z"),
        viewerPaneId: "w1:p2"
      })
    ).toEqual({ kind: "preserve", reason: "stale_completion" });
    expect(next.state.processed).toBeUndefined();
    expect(next.state.viewer_pane_id).toBeUndefined();
  });

  it("rejects invalid commit metadata without exposing it", () => {
    expect(
      commitCompletion(initialState(), {
        ...authorization(),
        contentDigest: "not-a-digest",
        processedAt: new Date("2026-08-01T00:01:00.000Z"),
        viewerPaneId: "../unsafe-viewer"
      })
    ).toEqual({ kind: "reject", code: "event_invalid" });
  });
});
