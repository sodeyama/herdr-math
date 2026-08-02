import {
  type FingerprintStateV1,
  type LifecycleAuthority,
  type SupportedAgent,
  isFingerprintDigest,
  isStateIdentifier
} from "../boundary/fingerprint-schema.js";
import type { ErrorCode } from "../core/errors.js";

export type AgentStatus = "working" | "blocked" | "done" | "idle" | "unknown";

export interface LifecycleEvent {
  status: AgentStatus;
  sessionKey: string;
  workspaceId: string;
  sourcePaneId: string;
  agent: SupportedAgent;
  lifecycleAuthority: LifecycleAuthority;
  occupantKey: string;
  paneRevision: number;
  eventSequence: number;
}

export interface CompletionAuthorization {
  status: "done" | "idle";
  generation: number;
  agent: SupportedAgent;
  lifecycleAuthority: LifecycleAuthority;
  occupantKey: string;
  paneRevision: number;
  eventSequence: number;
}

export type LifecycleDecision =
  | { kind: "store_baseline"; state: FingerprintStateV1 }
  | { kind: "process_completion"; authorization: CompletionAuthorization }
  | {
      kind: "preserve";
      reason: "blocked" | "unknown" | "duplicate_working" | "duplicate_completion" | "stale_event";
    }
  | { kind: "reject"; code: Extract<ErrorCode, "baseline_missing" | "event_invalid"> };

export interface CompletionCommit extends CompletionAuthorization {
  contentDigest: string;
  processedAt: Date;
  viewerPaneId?: string;
}

export type CompletionDecision =
  | { kind: "commit_completion"; state: FingerprintStateV1 }
  | { kind: "preserve"; reason: "duplicate_completion" | "stale_completion" }
  | { kind: "reject"; code: Extract<ErrorCode, "baseline_missing" | "event_invalid"> };

export function transitionLifecycle(
  current: FingerprintStateV1 | undefined,
  event: LifecycleEvent,
  workingCandidate?: FingerprintStateV1
): LifecycleDecision {
  if (!isValidEvent(event) || (current !== undefined && !sameNamespace(current, event))) {
    return { kind: "reject", code: "event_invalid" };
  }

  if (event.status === "working") return transitionWorking(current, event, workingCandidate);
  if (workingCandidate !== undefined) return { kind: "reject", code: "event_invalid" };
  if (event.status === "blocked" || event.status === "unknown") {
    return { kind: "preserve", reason: event.status };
  }
  if (current === undefined || !sameOccupant(current, event)) {
    return { kind: "reject", code: "baseline_missing" };
  }
  if (event.eventSequence < current.event_sequence || event.paneRevision < current.pane_revision) {
    return { kind: "preserve", reason: "stale_event" };
  }
  if (current.processed !== undefined && event.eventSequence <= current.event_sequence) {
    return { kind: "preserve", reason: "duplicate_completion" };
  }
  return {
    kind: "process_completion",
    authorization: {
      status: event.status,
      generation: current.generation,
      agent: event.agent,
      lifecycleAuthority: event.lifecycleAuthority,
      occupantKey: event.occupantKey,
      paneRevision: event.paneRevision,
      eventSequence: event.eventSequence
    }
  };
}

export function commitCompletion(
  current: FingerprintStateV1 | undefined,
  commit: CompletionCommit
): CompletionDecision {
  if (!isValidCommit(commit)) return { kind: "reject", code: "event_invalid" };
  if (current === undefined) return { kind: "reject", code: "baseline_missing" };
  if (
    current.generation !== commit.generation ||
    current.agent !== commit.agent ||
    current.lifecycle_authority !== commit.lifecycleAuthority ||
    current.occupant_key !== commit.occupantKey ||
    commit.eventSequence < current.event_sequence ||
    commit.paneRevision < current.pane_revision
  ) {
    return { kind: "preserve", reason: "stale_completion" };
  }
  if (current.processed?.content_digest === commit.contentDigest) {
    return { kind: "preserve", reason: "duplicate_completion" };
  }
  const processedAt = commit.processedAt.getTime();
  if (processedAt < Date.parse(current.created_at) || processedAt > Date.parse(current.expires_at)) {
    return { kind: "reject", code: "baseline_missing" };
  }

  const state: FingerprintStateV1 = {
    ...current,
    pane_revision: commit.paneRevision,
    event_sequence: commit.eventSequence,
    processed: {
      content_digest: commit.contentDigest,
      pane_revision: commit.paneRevision,
      processed_at: commit.processedAt.toISOString()
    }
  };
  if (commit.viewerPaneId !== undefined) state.viewer_pane_id = commit.viewerPaneId;
  return { kind: "commit_completion", state };
}

function transitionWorking(
  current: FingerprintStateV1 | undefined,
  event: LifecycleEvent,
  candidate: FingerprintStateV1 | undefined
): LifecycleDecision {
  if (candidate === undefined || !candidateMatchesEvent(candidate, event)) {
    return { kind: "reject", code: "event_invalid" };
  }
  if (current !== undefined && sameOccupant(current, event)) {
    if (event.eventSequence === current.event_sequence) {
      return { kind: "preserve", reason: "duplicate_working" };
    }
    if (event.eventSequence < current.event_sequence || event.paneRevision < current.pane_revision) {
      return { kind: "preserve", reason: "stale_event" };
    }
  }

  const state: FingerprintStateV1 = { ...candidate, generation: (current?.generation ?? 0) + 1 };
  delete state.processed;
  delete state.viewer_pane_id;
  if (current?.viewer_pane_id !== undefined) state.viewer_pane_id = current.viewer_pane_id;
  return { kind: "store_baseline", state };
}

function candidateMatchesEvent(candidate: FingerprintStateV1, event: LifecycleEvent): boolean {
  return (
    candidate.session_key === event.sessionKey &&
    candidate.workspace_id === event.workspaceId &&
    candidate.source_pane_id === event.sourcePaneId &&
    candidate.agent === event.agent &&
    candidate.lifecycle_authority === event.lifecycleAuthority &&
    candidate.occupant_key === event.occupantKey &&
    candidate.pane_revision === event.paneRevision &&
    candidate.event_sequence === event.eventSequence &&
    candidate.processed === undefined
  );
}

function sameNamespace(state: FingerprintStateV1, event: LifecycleEvent): boolean {
  return state.session_key === event.sessionKey && state.source_pane_id === event.sourcePaneId;
}

function sameOccupant(state: FingerprintStateV1, event: LifecycleEvent): boolean {
  return (
    state.workspace_id === event.workspaceId &&
    state.agent === event.agent &&
    state.lifecycle_authority === event.lifecycleAuthority &&
    state.occupant_key === event.occupantKey
  );
}

function isValidEvent(event: LifecycleEvent): boolean {
  return (
    ["working", "blocked", "done", "idle", "unknown"].includes(event.status) &&
    isFingerprintDigest(event.sessionKey) &&
    isStateIdentifier(event.workspaceId) &&
    isStateIdentifier(event.sourcePaneId) &&
    ["claude", "codex", "cursor", "pi", "opencode"].includes(event.agent) &&
    ["screen_detection", "integration_hook"].includes(event.lifecycleAuthority) &&
    isFingerprintDigest(event.occupantKey) &&
    isCount(event.paneRevision) &&
    isCount(event.eventSequence)
  );
}

function isValidCommit(commit: CompletionCommit): boolean {
  return (
    (commit.status === "done" || commit.status === "idle") &&
    isCount(commit.generation) &&
    ["claude", "codex", "cursor", "pi", "opencode"].includes(commit.agent) &&
    ["screen_detection", "integration_hook"].includes(commit.lifecycleAuthority) &&
    isFingerprintDigest(commit.occupantKey) &&
    isCount(commit.paneRevision) &&
    isCount(commit.eventSequence) &&
    isFingerprintDigest(commit.contentDigest) &&
    commit.processedAt instanceof Date &&
    !Number.isNaN(commit.processedAt.getTime()) &&
    (commit.viewerPaneId === undefined || isStateIdentifier(commit.viewerPaneId))
  );
}

function isCount(value: number): boolean {
  return Number.isSafeInteger(value) && value >= 0;
}
