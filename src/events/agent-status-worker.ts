import { buildBaselineFingerprint, deriveStateKey } from "../boundary/fingerprint-builder.js";
import {
  fingerprintDigest,
  fingerprintDigestsEqual,
  formulaFingerprintDigest
} from "../boundary/fingerprint-digest.js";
import type { FingerprintStateV1, LifecycleAuthority, SupportedAgent } from "../boundary/fingerprint-schema.js";
import { resolveAnswerFromFingerprint } from "../boundary/fingerprint-resolver.js";
import { failure, success, type Formula, type OperationResult, type RenderedImage } from "../core/contracts.js";
import { HerdrMathError, serializeError } from "../core/errors.js";
import {
  commitCompletion,
  transitionLifecycle,
  type CompletionAuthorization,
  type LifecycleEvent
} from "./lifecycle.js";
import { decodeAgentStatusEvent, type DecodedAgentStatusEvent } from "../herdr/event-decoder.js";
import type { HerdrAgentSnapshot, HerdrPaneReadSnapshot, HerdrPaneSnapshot } from "../herdr/socket-client.js";
import {
  parseMatchingAnsiSnapshot,
  parseMatchingAnsiSuffixSnapshot,
  type StyledTerminalSnapshot
} from "../presentation/ansi-snapshot.js";
import { extractFinalResponse } from "../presentation/final-response.js";
import { scanLatex } from "../scanner/scan-latex.js";
import { acquirePaneLock } from "../state/pane-lock.js";
import { createPaneStatePaths, type PaneStatePaths } from "../state/paths.js";
import { isCurrentGeneration, loadPaneState, writePaneState } from "../state/store.js";
import { AGENT_AUTHORITIES, buildOccupantIdentity, isSupportedAgent } from "./agent-identity.js";

export const AGENT_STATUS_TIMING = Object.freeze({
  completionDebounceMs: 500,
  stableReadIntervalMs: 100,
  stableReadAttempts: 3
});

export interface AgentStatusHerdrClient {
  paneGet(paneId: string): Promise<OperationResult<HerdrPaneSnapshot>>;
  agentGet(paneId: string): Promise<OperationResult<HerdrAgentSnapshot>>;
  paneRead(paneId: string): Promise<OperationResult<HerdrPaneReadSnapshot>>;
  paneReadAnsi(paneId: string): Promise<OperationResult<HerdrPaneReadSnapshot>>;
}

export interface ImagePublishRequest {
  sourcePaneId: string;
  workspaceId: string;
  generation: number;
  existingViewerPaneId?: string;
  image: RenderedImage;
}

export interface ImagePublishResult {
  viewerPaneId: string;
}

export interface AgentStatusWorkerTiming {
  completionDebounceMs?: number;
  stableReadIntervalMs?: number;
}

export interface ResponseRenderRequest {
  text: string;
  formulas: readonly Formula[];
}

export interface AgentStatusWorkerDependencies {
  client: AgentStatusHerdrClient;
  stateDirectory: string;
  sessionIdentity: string;
  secret: Uint8Array;
  render(request: ResponseRenderRequest): Promise<OperationResult<RenderedImage>>;
  publish(request: ImagePublishRequest): Promise<OperationResult<ImagePublishResult>>;
  workingSnapshot?: HerdrPaneReadSnapshot;
  now?: () => Date;
  sleep?: (milliseconds: number) => Promise<void>;
  timing?: AgentStatusWorkerTiming;
}

export type AgentStatusWorkerOutcome =
  | {
      kind: "ignored";
      status: DecodedAgentStatusEvent["status"];
      reason: "no_agent" | "agent_unsupported";
    }
  | { kind: "baseline_stored"; status: "working"; agent: SupportedAgent; generation: number }
  | {
      kind: "preserved";
      status: DecodedAgentStatusEvent["status"];
      agent: SupportedAgent;
      reason: string;
      generation?: number;
    }
  | {
      kind: "completion_recorded";
      status: "done" | "idle";
      agent: SupportedAgent;
      generation: number;
      formulaCount: 0;
    }
  | {
      kind: "image_published";
      status: "done" | "idle";
      agent: SupportedAgent;
      generation: number;
      formulaCount: number;
      viewerPaneId: string;
    };

type IgnoredAgentStatusOutcome = Extract<AgentStatusWorkerOutcome, { kind: "ignored" }>;

interface ResolvedEvent {
  decoded: DecodedAgentStatusEvent;
  pane: HerdrPaneSnapshot;
  agent: SupportedAgent;
  authority: LifecycleAuthority;
  occupantIdentity: string;
  stateChangeSequence: number;
  lifecycle: LifecycleEvent;
}

interface CompletionPhase {
  authorization: CompletionAuthorization;
  state: FingerprintStateV1;
}

interface StableRead {
  snapshot: HerdrPaneReadSnapshot;
  styledSnapshot: StyledTerminalSnapshot;
  snapshotMode: "strict" | "pi_suffix" | "opencode_plain";
  digest: string;
  styledDigest: string;
}

export async function processAgentStatusEvent(
  source: string,
  dependencies: AgentStatusWorkerDependencies
): Promise<OperationResult<AgentStatusWorkerOutcome>> {
  const decoded = decodeAgentStatusEvent(source);
  if (!decoded.ok) return failure(decoded.error);
  return processDecodedAgentStatusEvent(decoded.value, dependencies);
}

export async function processDecodedAgentStatusEvent(
  decoded: DecodedAgentStatusEvent,
  dependencies: AgentStatusWorkerDependencies
): Promise<OperationResult<AgentStatusWorkerOutcome>> {
  try {
    const timing = resolveTiming(dependencies.timing);
    const resolved = await resolveEvent(decoded, dependencies);
    if (!resolved.ok) return failure(resolved.error);
    if (isIgnoredOutcome(resolved.value)) return success(resolved.value);
    const sessionKey = deriveStateKey("session", dependencies.sessionIdentity, dependencies.secret);
    const paths = createPaneStatePaths(
      dependencies.stateDirectory,
      sessionKey,
      decoded.sourcePaneId,
      dependencies.secret
    );
    if (decoded.status === "working") return await processWorking(resolved.value, paths, dependencies);

    const phase = await authorizeNonWorking(resolved.value, paths, dependencies);
    if (!phase.ok) return failure(phase.error);
    if ("kind" in phase.value) return success(phase.value);
    return await processCompletion(resolved.value, phase.value, paths, dependencies, timing);
  } catch (error) {
    return failure(serializeError(error));
  }
}

async function resolveEvent(
  decoded: DecodedAgentStatusEvent,
  dependencies: AgentStatusWorkerDependencies
): Promise<OperationResult<ResolvedEvent | IgnoredAgentStatusOutcome>> {
  const result = await dependencies.client.paneGet(decoded.sourcePaneId);
  if (!result.ok) return failure(result.error);
  const pane = result.value;
  if (pane.paneId !== decoded.sourcePaneId || pane.workspaceId !== decoded.workspaceId) {
    return safeFailure("event_invalid");
  }
  if (pane.agent === null) return success({ kind: "ignored", status: decoded.status, reason: "no_agent" });
  if (!isSupportedAgent(pane.agent)) {
    return success({ kind: "ignored", status: decoded.status, reason: "agent_unsupported" });
  }
  if (pane.status !== decoded.status || (decoded.agentHint !== undefined && decoded.agentHint !== pane.agent)) {
    return safeFailure("event_invalid");
  }
  const agentResult = await dependencies.client.agentGet(decoded.sourcePaneId);
  if (!agentResult.ok) return failure(agentResult.error);
  const agent = agentResult.value;
  if (!sameAgentPane(pane, agent) || (decoded.agentHint !== undefined && decoded.agentHint !== agent.agent)) {
    return safeFailure("event_invalid");
  }
  if (agent.agent === null || !isSupportedAgent(agent.agent)) return safeFailure("event_invalid");
  const authority = AGENT_AUTHORITIES[agent.agent];
  const occupantIdentity = buildOccupantIdentity(agent, agent.agent, authority);
  if (occupantIdentity === undefined) return safeFailure("event_invalid");
  const occupantKey = deriveStateKey("occupant", occupantIdentity, dependencies.secret);
  const sessionKey = deriveStateKey("session", dependencies.sessionIdentity, dependencies.secret);
  return success({
    decoded,
    pane,
    agent: agent.agent,
    authority,
    occupantIdentity,
    stateChangeSequence: agent.stateChangeSequence,
    lifecycle: {
      status: decoded.status,
      sessionKey,
      workspaceId: decoded.workspaceId,
      sourcePaneId: decoded.sourcePaneId,
      agent: agent.agent,
      lifecycleAuthority: authority,
      occupantKey,
      paneRevision: pane.revision,
      eventSequence: agent.stateChangeSequence
    }
  });
}

async function processWorking(
  resolved: ResolvedEvent,
  paths: PaneStatePaths,
  dependencies: AgentStatusWorkerDependencies
): Promise<OperationResult<AgentStatusWorkerOutcome>> {
  const lock = await acquirePaneLock(paths, { eventType: "working", now: currentTime(dependencies) });
  try {
    const now = currentTime(dependencies);
    const current = await loadPaneState(paths, now);
    const read =
      dependencies.workingSnapshot === undefined
        ? await dependencies.client.paneRead(resolved.decoded.sourcePaneId)
        : success(dependencies.workingSnapshot);
    if (!read.ok) return failure(read.error);
    if (!readMatchesEvent(read.value, resolved)) return safeFailure("event_invalid");
    const confirmed = await resolveEvent(resolved.decoded, dependencies);
    if (!confirmed.ok) return failure(confirmed.error);
    if (isIgnoredOutcome(confirmed.value) || !sameResolvedEvent(resolved, confirmed.value)) {
      return safeFailure("event_invalid");
    }

    const candidate = buildBaselineFingerprint(
      read.value.text,
      {
        sessionIdentity: dependencies.sessionIdentity,
        occupantIdentity: resolved.occupantIdentity,
        workspaceId: resolved.decoded.workspaceId,
        sourcePaneId: resolved.decoded.sourcePaneId,
        agent: resolved.agent,
        lifecycleAuthority: resolved.authority,
        paneRevision: resolved.pane.revision,
        eventSequence: resolved.stateChangeSequence,
        generation: current?.generation ?? 0,
        createdAt: now
      },
      dependencies.secret
    );
    const decision = transitionLifecycle(current, resolved.lifecycle, candidate);
    if (decision.kind === "reject") return safeFailure(decision.code);
    if (decision.kind === "preserve") {
      return success({
        kind: "preserved",
        status: "working",
        agent: resolved.agent,
        reason: decision.reason,
        ...(current === undefined ? {} : { generation: current.generation })
      });
    }
    if (decision.kind !== "store_baseline") return safeFailure("event_invalid");
    const stored = await writePaneState(paths, decision.state, current?.generation ?? null, now);
    if (!stored) return safeFailure("state_locked", true);
    return success({
      kind: "baseline_stored",
      status: "working",
      agent: resolved.agent,
      generation: decision.state.generation
    });
  } finally {
    await lock.release();
  }
}

async function authorizeNonWorking(
  resolved: ResolvedEvent,
  paths: PaneStatePaths,
  dependencies: AgentStatusWorkerDependencies
): Promise<OperationResult<CompletionPhase | AgentStatusWorkerOutcome>> {
  const lock = await acquirePaneLock(paths, { eventType: resolved.decoded.status, now: currentTime(dependencies) });
  try {
    const state = await loadPaneState(paths, currentTime(dependencies));
    const decision = transitionLifecycle(state, resolved.lifecycle);
    if (decision.kind === "reject") return safeFailure(decision.code);
    if (decision.kind === "preserve") {
      return success({
        kind: "preserved",
        status: resolved.decoded.status,
        agent: resolved.agent,
        reason: decision.reason,
        ...(state === undefined ? {} : { generation: state.generation })
      });
    }
    if (decision.kind !== "process_completion" || state === undefined) return safeFailure("event_invalid");
    return success({ authorization: decision.authorization, state });
  } finally {
    await lock.release();
  }
}

async function processCompletion(
  resolved: ResolvedEvent,
  phase: CompletionPhase,
  paths: PaneStatePaths,
  dependencies: AgentStatusWorkerDependencies,
  timing: Required<AgentStatusWorkerTiming>
): Promise<OperationResult<AgentStatusWorkerOutcome>> {
  const sleep = dependencies.sleep ?? defaultSleep;
  await sleep(timing.completionDebounceMs);
  const stable = await readStableCompletion(resolved, dependencies, sleep, timing.stableReadIntervalMs);
  if (!stable.ok) return failure(stable.error);
  const confirmed = await resolveEvent(resolved.decoded, dependencies);
  if (!confirmed.ok) return failure(confirmed.error);
  if (isIgnoredOutcome(confirmed.value) || !sameResolvedEvent(resolved, confirmed.value)) {
    return safeFailure("event_invalid");
  }

  const boundary = resolveAnswerFromFingerprint(phase.state, stable.value.snapshot.text, dependencies.secret, {
    readTruncated: stable.value.snapshot.truncated
  });
  if (!boundary.ok) return failure(boundary.error);

  const answerEndOffset = boundary.value.startOffset + boundary.value.answer.length;
  const styledStartOffset = stable.value.styledSnapshot.lines[0]?.startOffset;
  const answerStartOffset =
    stable.value.snapshotMode === "pi_suffix" && styledStartOffset !== undefined
      ? Math.max(boundary.value.startOffset, styledStartOffset)
      : boundary.value.startOffset;
  const presentationEndOffset =
    stable.value.snapshotMode === "pi_suffix" ? stable.value.snapshot.text.length : answerEndOffset;
  if (styledStartOffset === undefined || answerStartOffset >= answerEndOffset) {
    return safeFailure("conclusion_boundary_failed");
  }
  const finalResponse = extractFinalResponse({
    agent: resolved.agent,
    answer: stable.value.snapshot.text.slice(answerStartOffset, presentationEndOffset),
    answerStartOffset,
    snapshot: stable.value.styledSnapshot,
    ...(stable.value.snapshotMode === "pi_suffix" ? { requirePiFooter: true } : {}),
    ...(stable.value.snapshotMode === "opencode_plain" || boundary.value.proof.kind === "anchored_prefix_replacement"
      ? { requireOpenCodeChrome: true }
      : {})
  });
  if (!finalResponse.ok) return failure(finalResponse.error);
  if (
    finalResponse.value.sourceStartOffset < boundary.value.startOffset ||
    finalResponse.value.sourceEndOffset > answerEndOffset
  ) {
    return safeFailure("conclusion_boundary_failed");
  }

  let formulas: ReturnType<typeof scanLatex>;
  try {
    formulas = scanLatex(finalResponse.value.text);
    if (
      boundary.value.proof.kind === "middle_replacement" ||
      boundary.value.proof.kind === "anchored_prefix_replacement"
    ) {
      const baselineFormulaDigests = boundary.value.proof.baselineFormulaDigests;
      formulas = formulas.filter((formula) => {
        const digest = formulaFingerprintDigest(formula, dependencies.secret);
        return !baselineFormulaDigests.some((baselineDigest) => fingerprintDigestsEqual(digest, baselineDigest));
      });
    }
  } catch (error) {
    return failure(serializeError(error));
  }

  const current = await loadPaneState(paths, currentTime(dependencies));
  if (current?.processed?.content_digest === boundary.value.currentDigest) {
    return preservedCompletion(resolved, phase.authorization.generation, "duplicate_completion");
  }
  if (
    !(await isCurrentGeneration(
      paths,
      phase.authorization.generation,
      phase.authorization.occupantKey,
      currentTime(dependencies)
    ))
  ) {
    return preservedCompletion(resolved, phase.authorization.generation, "stale_completion");
  }

  if (formulas.length === 0) {
    return commitFinal(resolved, phase.authorization, boundary.value.currentDigest, paths, dependencies);
  }

  if (
    !(await isCurrentGeneration(
      paths,
      phase.authorization.generation,
      phase.authorization.occupantKey,
      currentTime(dependencies)
    ))
  ) {
    return preservedCompletion(resolved, phase.authorization.generation, "stale_completion");
  }
  const rendered = await dependencies.render({ text: finalResponse.value.text, formulas });
  if (!rendered.ok) return failure(rendered.error);
  return commitFinal(
    resolved,
    phase.authorization,
    boundary.value.currentDigest,
    paths,
    dependencies,
    rendered.value,
    formulas.length
  );
}

async function readStableCompletion(
  resolved: ResolvedEvent,
  dependencies: AgentStatusWorkerDependencies,
  sleep: (milliseconds: number) => Promise<void>,
  intervalMs: number
): Promise<OperationResult<StableRead>> {
  let previous: StableRead | undefined;
  let snapshotMismatch = false;
  for (let attempt = 0; attempt < AGENT_STATUS_TIMING.stableReadAttempts; attempt += 1) {
    const read = await dependencies.client.paneRead(resolved.decoded.sourcePaneId);
    if (!read.ok) return failure(read.error);
    if (!readMatchesEvent(read.value, resolved)) return safeFailure("event_invalid");
    const ansiRead = await dependencies.client.paneReadAnsi(resolved.decoded.sourcePaneId);
    if (!ansiRead.ok) return failure(ansiRead.error);
    if (!readMatchesEvent(ansiRead.value, resolved)) return safeFailure("event_invalid");
    if (ansiRead.value.revision !== read.value.revision || ansiRead.value.truncated !== read.value.truncated) {
      snapshotMismatch = true;
      previous = undefined;
      if (attempt + 1 < AGENT_STATUS_TIMING.stableReadAttempts) await sleep(intervalMs);
      continue;
    }
    let styledSnapshot = parseMatchingAnsiSnapshot(read.value.text, ansiRead.value.text);
    let snapshotMode: StableRead["snapshotMode"] = "strict";
    if (!styledSnapshot.ok && resolved.agent === "pi") {
      styledSnapshot = parseMatchingAnsiSuffixSnapshot(read.value.text, ansiRead.value.text);
      if (styledSnapshot.ok) snapshotMode = "pi_suffix";
    }
    if (!styledSnapshot.ok && resolved.agent === "opencode") {
      styledSnapshot = parseMatchingAnsiSnapshot(read.value.text, read.value.text);
      if (styledSnapshot.ok) snapshotMode = "opencode_plain";
    }
    if (!styledSnapshot.ok) {
      snapshotMismatch = true;
      previous = undefined;
      if (attempt + 1 < AGENT_STATUS_TIMING.stableReadAttempts) await sleep(intervalMs);
      continue;
    }
    const candidate: StableRead = {
      snapshot: read.value,
      styledSnapshot: styledSnapshot.value,
      snapshotMode,
      digest: fingerprintDigest("stable-pane-read", read.value.text, dependencies.secret),
      styledDigest: fingerprintDigest("stable-pane-ansi", ansiRead.value.text, dependencies.secret)
    };
    if (
      previous !== undefined &&
      previous.digest === candidate.digest &&
      previous.styledDigest === candidate.styledDigest &&
      previous.snapshot.truncated === candidate.snapshot.truncated
    ) {
      return success(candidate);
    }
    previous = candidate;
    if (attempt + 1 < AGENT_STATUS_TIMING.stableReadAttempts) await sleep(intervalMs);
  }
  if (previous === undefined && snapshotMismatch) return safeFailure("conclusion_boundary_failed");
  return safeFailure("herdr_timeout", true);
}

async function commitFinal(
  resolved: ResolvedEvent,
  authorization: CompletionAuthorization,
  contentDigest: string,
  paths: PaneStatePaths,
  dependencies: AgentStatusWorkerDependencies,
  image?: RenderedImage,
  formulaCount = 0
): Promise<OperationResult<AgentStatusWorkerOutcome>> {
  const lock = await acquirePaneLock(paths, { eventType: authorization.status, now: currentTime(dependencies) });
  try {
    const now = currentTime(dependencies);
    const current = await loadPaneState(paths, now);
    const confirmed = await resolveEvent(resolved.decoded, dependencies);
    if (!confirmed.ok) return failure(confirmed.error);
    if (isIgnoredOutcome(confirmed.value) || !sameResolvedEvent(resolved, confirmed.value)) {
      return safeFailure("event_invalid");
    }
    const commit = { ...authorization, contentDigest, processedAt: now };
    const preliminary = commitCompletion(current, commit);
    if (preliminary.kind === "reject") return safeFailure(preliminary.code);
    if (preliminary.kind === "preserve") {
      return preservedCompletion(resolved, authorization.generation, preliminary.reason);
    }

    let viewerPaneId: string | undefined;
    if (image !== undefined) {
      const published = await dependencies.publish({
        sourcePaneId: resolved.decoded.sourcePaneId,
        workspaceId: resolved.decoded.workspaceId,
        generation: authorization.generation,
        ...(current?.viewer_pane_id === undefined ? {} : { existingViewerPaneId: current.viewer_pane_id }),
        image
      });
      if (!published.ok) return failure(published.error);
      viewerPaneId = published.value.viewerPaneId;
    }

    const final = viewerPaneId === undefined ? preliminary : commitCompletion(current, { ...commit, viewerPaneId });
    if (final.kind !== "commit_completion") return safeFailure("event_invalid");
    const stored = await writePaneState(paths, final.state, current?.generation ?? null, now);
    if (!stored) return safeFailure("state_locked", true);
    if (viewerPaneId === undefined) {
      return success({
        kind: "completion_recorded",
        status: authorization.status,
        agent: resolved.agent,
        generation: authorization.generation,
        formulaCount: 0
      });
    }
    return success({
      kind: "image_published",
      status: authorization.status,
      agent: resolved.agent,
      generation: authorization.generation,
      formulaCount,
      viewerPaneId
    });
  } finally {
    await lock.release();
  }
}

function preservedCompletion(
  resolved: ResolvedEvent,
  generation: number,
  reason: string
): OperationResult<AgentStatusWorkerOutcome> {
  return success({
    kind: "preserved",
    status: resolved.decoded.status,
    agent: resolved.agent,
    reason,
    generation
  });
}

function readMatchesEvent(read: HerdrPaneReadSnapshot, resolved: ResolvedEvent): boolean {
  return read.paneId === resolved.decoded.sourcePaneId && read.workspaceId === resolved.decoded.workspaceId;
}

function sameResolvedEvent(left: ResolvedEvent, right: ResolvedEvent): boolean {
  return (
    left.agent === right.agent &&
    left.authority === right.authority &&
    left.occupantIdentity === right.occupantIdentity &&
    left.stateChangeSequence === right.stateChangeSequence &&
    left.pane.revision === right.pane.revision &&
    left.pane.status === right.pane.status
  );
}

function sameAgentPane(pane: HerdrPaneSnapshot, agent: HerdrAgentSnapshot): boolean {
  return (
    pane.paneId === agent.paneId &&
    pane.workspaceId === agent.workspaceId &&
    pane.tabId === agent.tabId &&
    pane.agent === agent.agent &&
    pane.status === agent.status &&
    pane.revision === agent.revision &&
    sameAgentSession(pane.agentSession, agent.agentSession)
  );
}

function sameAgentSession(left: HerdrPaneSnapshot["agentSession"], right: HerdrPaneSnapshot["agentSession"]): boolean {
  if (left === null || right === null) return left === right;
  return (
    left.source === right.source && left.agent === right.agent && left.kind === right.kind && left.value === right.value
  );
}

function isIgnoredOutcome(value: ResolvedEvent | IgnoredAgentStatusOutcome): value is IgnoredAgentStatusOutcome {
  return "kind" in value && value.kind === "ignored";
}

function resolveTiming(overrides: AgentStatusWorkerTiming | undefined): Required<AgentStatusWorkerTiming> {
  const timing = {
    completionDebounceMs: overrides?.completionDebounceMs ?? AGENT_STATUS_TIMING.completionDebounceMs,
    stableReadIntervalMs: overrides?.stableReadIntervalMs ?? AGENT_STATUS_TIMING.stableReadIntervalMs
  };
  if (
    !isBoundedDuration(timing.completionDebounceMs, AGENT_STATUS_TIMING.completionDebounceMs) ||
    !isBoundedDuration(timing.stableReadIntervalMs, AGENT_STATUS_TIMING.stableReadIntervalMs)
  ) {
    throw new TypeError("Worker timing overrides must not exceed production policy");
  }
  return timing;
}

function isBoundedDuration(value: number, maximum: number): boolean {
  return Number.isSafeInteger(value) && value >= 0 && value <= maximum;
}

function currentTime(dependencies: AgentStatusWorkerDependencies): Date {
  const value = dependencies.now?.() ?? new Date();
  if (!(value instanceof Date) || Number.isNaN(value.getTime())) throw new HerdrMathError("event_invalid");
  return new Date(value.getTime());
}

function safeFailure<T>(code: HerdrMathError["code"], retryable = false): OperationResult<T> {
  return failure(serializeError(new HerdrMathError(code, {}, retryable)));
}

function defaultSleep(milliseconds: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}
