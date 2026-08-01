import { Buffer } from "node:buffer";
import { constants } from "node:fs";
import { lstat, open, readdir } from "node:fs/promises";
import { isAbsolute, join } from "node:path";

import { deriveStateKey } from "../boundary/fingerprint-builder.js";
import { assertFingerprintSecret } from "../boundary/fingerprint-digest.js";
import type { FingerprintStateV1 } from "../boundary/fingerprint-schema.js";
import { failure, success, type OperationResult } from "../core/contracts.js";
import { HerdrMathError, serializeError } from "../core/errors.js";
import { POLICY_LIMITS } from "../core/limits.js";
import { decodePaneClosedEvent, type DecodedPaneClosedEvent } from "../herdr/event-decoder.js";
import type { HerdrPaneSnapshot } from "../herdr/socket-client.js";
import { acquirePaneLock } from "../state/pane-lock.js";
import { createPaneStatePaths } from "../state/paths.js";
import { loadPaneState, removePaneState, writePaneState } from "../state/store.js";
import { parseFingerprintState } from "../state/validate.js";
import { AGENT_AUTHORITIES, buildOccupantIdentity, isSupportedAgent } from "./agent-identity.js";

const MAX_PANE_STATES = 4096;
const STATE_FILE = /^([a-f0-9]{64})\.json$/;

export interface PaneCloseHerdrClient {
  paneGetIfPresent(paneId: string): Promise<OperationResult<HerdrPaneSnapshot | null>>;
}

export interface PaneCloseWorkerDependencies {
  client: PaneCloseHerdrClient;
  stateDirectory: string;
  sessionIdentity: string;
  secret: Uint8Array;
  now?: () => Date;
}

export type PaneCloseWorkerOutcome =
  | { kind: "cleaned"; sourceStatesRemoved: number; viewerMappingsCleared: number }
  | { kind: "preserved"; reason: "pane_reused" | "not_tracked" };

interface StateCandidate {
  sourcePaneId: string;
}

export async function processPaneClosedEvent(
  source: string,
  dependencies: PaneCloseWorkerDependencies
): Promise<OperationResult<PaneCloseWorkerOutcome>> {
  const decoded = decodePaneClosedEvent(source);
  if (!decoded.ok) return failure(decoded.error);
  return processDecodedPaneClosedEvent(decoded.value, dependencies);
}

export async function processDecodedPaneClosedEvent(
  decoded: DecodedPaneClosedEvent,
  dependencies: PaneCloseWorkerDependencies
): Promise<OperationResult<PaneCloseWorkerOutcome>> {
  try {
    assertFingerprintSecret(dependencies.secret);
    if (!isAbsolute(dependencies.stateDirectory) || dependencies.stateDirectory.includes("\0")) {
      return safeFailure("event_invalid");
    }
    const current = await dependencies.client.paneGetIfPresent(decoded.paneId);
    if (!current.ok) return failure(current.error);

    const now = currentTime(dependencies);
    const sessionKey = deriveStateKey("session", dependencies.sessionIdentity, dependencies.secret);
    const candidates = await findSessionStates(dependencies.stateDirectory, sessionKey, dependencies.secret);
    let sourceStatesRemoved = 0;
    let viewerMappingsCleared = 0;
    for (const candidate of candidates) {
      const paths = createPaneStatePaths(
        dependencies.stateDirectory,
        sessionKey,
        candidate.sourcePaneId,
        dependencies.secret
      );
      const lock = await acquirePaneLock(paths, { eventType: "pane_closed", now });
      try {
        const state = await loadPaneState(paths, now);
        if (state === undefined || state.workspace_id !== decoded.workspaceId) continue;
        if (state.source_pane_id === decoded.paneId) {
          if (current.value !== null && stateMatchesCurrentPane(state, current.value, dependencies.secret)) continue;
          if (await removePaneState(paths, state.generation, now)) sourceStatesRemoved += 1;
          continue;
        }
        if (state.viewer_pane_id !== decoded.paneId) continue;
        if (current.value !== null) continue;
        const next = { ...state };
        delete next.viewer_pane_id;
        if (await writePaneState(paths, next, state.generation, now)) viewerMappingsCleared += 1;
      } finally {
        await lock.release();
      }
    }

    if (sourceStatesRemoved === 0 && viewerMappingsCleared === 0) {
      return success({ kind: "preserved", reason: current.value === null ? "not_tracked" : "pane_reused" });
    }
    return success({ kind: "cleaned", sourceStatesRemoved, viewerMappingsCleared });
  } catch (error) {
    return failure(serializeError(error));
  }
}

function stateMatchesCurrentPane(state: FingerprintStateV1, pane: HerdrPaneSnapshot, secret: Uint8Array): boolean {
  if (
    pane.paneId !== state.source_pane_id ||
    pane.workspaceId !== state.workspace_id ||
    pane.agent === null ||
    pane.agentSession === null ||
    !isSupportedAgent(pane.agent)
  ) {
    return false;
  }
  const authority = AGENT_AUTHORITIES[pane.agent];
  const identity = buildOccupantIdentity(pane, pane.agent, authority);
  return (
    identity !== undefined &&
    state.agent === pane.agent &&
    state.lifecycle_authority === authority &&
    state.occupant_key === deriveStateKey("occupant", identity, secret) &&
    state.pane_revision <= pane.revision
  );
}

async function findSessionStates(
  stateDirectory: string,
  sessionKey: string,
  secret: Uint8Array
): Promise<StateCandidate[]> {
  const panesDirectory = join(stateDirectory, "v1", "sessions", sessionKey, "panes");
  const metadata = await lstat(panesDirectory).catch((error: unknown) => {
    if (isNodeError(error, "ENOENT")) return undefined;
    throw error;
  });
  if (metadata === undefined) return [];
  if (!metadata.isDirectory() || metadata.isSymbolicLink()) throw new HerdrMathError("state_corrupt");
  const entries = await readdir(panesDirectory, { withFileTypes: true });
  if (entries.length > MAX_PANE_STATES) throw new HerdrMathError("state_corrupt");

  const candidates: StateCandidate[] = [];
  for (const entry of entries) {
    const match = STATE_FILE.exec(entry.name);
    if (match === null || !entry.isFile()) continue;
    const paneKey = match[1];
    if (paneKey === undefined) continue;
    const state = await readCandidate(join(panesDirectory, entry.name));
    if (
      state === undefined ||
      state.session_key !== sessionKey ||
      deriveStateKey("pane", state.source_pane_id, secret) !== paneKey
    ) {
      continue;
    }
    candidates.push({ sourcePaneId: state.source_pane_id });
  }
  return candidates;
}

async function readCandidate(path: string): Promise<FingerprintStateV1 | undefined> {
  let handle;
  try {
    handle = await open(path, constants.O_RDONLY | constants.O_NOFOLLOW);
    const metadata = await handle.stat();
    if (!metadata.isFile() || metadata.size > POLICY_LIMITS.stateFileBytes) return undefined;
    const source = await handle.readFile("utf8");
    if (Buffer.byteLength(source, "utf8") > POLICY_LIMITS.stateFileBytes) return undefined;
    return parseFingerprintState(JSON.parse(source) as unknown);
  } catch {
    return undefined;
  } finally {
    await handle?.close();
  }
}

function currentTime(dependencies: PaneCloseWorkerDependencies): Date {
  const value = dependencies.now?.() ?? new Date();
  if (!(value instanceof Date) || Number.isNaN(value.getTime())) throw new HerdrMathError("event_invalid");
  return new Date(value.getTime());
}

function safeFailure<T>(code: HerdrMathError["code"]): OperationResult<T> {
  return failure(serializeError(new HerdrMathError(code)));
}

function isNodeError(error: unknown, code: string): error is NodeJS.ErrnoException {
  return error instanceof Error && "code" in error && error.code === code;
}
