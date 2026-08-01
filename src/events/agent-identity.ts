import type { LifecycleAuthority, SupportedAgent } from "../boundary/fingerprint-schema.js";
import type { HerdrPaneSnapshot } from "../herdr/socket-client.js";

export const AGENT_AUTHORITIES: Readonly<Record<SupportedAgent, LifecycleAuthority>> = Object.freeze({
  claude: "screen_detection",
  codex: "screen_detection",
  pi: "integration_hook",
  opencode: "integration_hook"
});

export function isSupportedAgent(value: string): value is SupportedAgent {
  return Object.hasOwn(AGENT_AUTHORITIES, value);
}

export function buildOccupantIdentity(
  pane: HerdrPaneSnapshot,
  agent: SupportedAgent,
  authority: LifecycleAuthority
): string | undefined {
  if (pane.agentSession === null) return `pane-agent\0${pane.paneId}\0${agent}\0${authority}`;
  if (pane.agentSession.agent !== agent) return undefined;
  return `agent-session\0${pane.agentSession.source}\0${pane.agentSession.agent}\0${pane.agentSession.kind}\0${pane.agentSession.value}`;
}
