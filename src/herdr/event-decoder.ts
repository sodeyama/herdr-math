import { Buffer } from "node:buffer";

import { failure, success, type OperationResult } from "../core/contracts.js";
import { HerdrMathError, serializeError } from "../core/errors.js";
import { POLICY_LIMITS } from "../core/limits.js";
import type { AgentStatus } from "../events/lifecycle.js";

const EVENT_NAME = "pane_agent_status_changed" as const;
const EVENT_IDENTIFIER = /^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$/;
const AGENT_STATUSES = new Set<AgentStatus>(["working", "blocked", "done", "idle", "unknown"]);

export interface DecodedAgentStatusEvent {
  event: typeof EVENT_NAME;
  workspaceId: string;
  sourcePaneId: string;
  status: AgentStatus;
  agentHint?: string;
}

export function decodeAgentStatusEvent(source: string): OperationResult<DecodedAgentStatusEvent> {
  try {
    if (typeof source !== "string" || Buffer.byteLength(source, "utf8") > POLICY_LIMITS.eventJsonBytes) {
      throw new HerdrMathError("event_invalid");
    }
    const envelope: unknown = JSON.parse(source);
    if (!isRecord(envelope) || envelope.event !== EVENT_NAME || !isRecord(envelope.data)) {
      throw new HerdrMathError("event_invalid");
    }

    const data = envelope.data;
    if (
      data.type !== EVENT_NAME ||
      !isEventIdentifier(data.workspace_id) ||
      !isEventIdentifier(data.pane_id) ||
      !isAgentStatus(data.agent_status) ||
      (data.agent !== undefined && data.agent !== null && !isEventIdentifier(data.agent))
    ) {
      throw new HerdrMathError("event_invalid");
    }

    const decoded: DecodedAgentStatusEvent = {
      event: EVENT_NAME,
      workspaceId: data.workspace_id,
      sourcePaneId: data.pane_id,
      status: data.agent_status
    };
    if (typeof data.agent === "string") decoded.agentHint = data.agent;
    return success(Object.freeze(decoded));
  } catch {
    return failure(serializeError(new HerdrMathError("event_invalid")));
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isEventIdentifier(value: unknown): value is string {
  return typeof value === "string" && EVENT_IDENTIFIER.test(value);
}

function isAgentStatus(value: unknown): value is AgentStatus {
  return typeof value === "string" && AGENT_STATUSES.has(value as AgentStatus);
}
