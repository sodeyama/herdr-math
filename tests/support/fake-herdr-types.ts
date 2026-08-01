import type { Buffer } from "node:buffer";

export type FakeAgentStatus = "idle" | "working" | "blocked" | "done" | "unknown";

export interface FakePaneState {
  pane_id: string;
  terminal_id: string;
  workspace_id: string;
  tab_id: string;
  focused: boolean;
  agent_status: FakeAgentStatus;
  revision: number;
  state_change_seq?: number;
  agent?: string | null;
  agent_session?: {
    source: string;
    agent: string;
    kind: "id" | "path";
    value: string;
  } | null;
  title?: string | null;
  display_agent?: string | null;
  state_labels?: Record<string, string>;
  tokens?: Record<string, string>;
}

export interface FakePaneOutput {
  text: string;
  truncated: boolean;
}

export interface FakeLayoutRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface FakeLayoutSnapshot {
  workspace_id: string;
  tab_id: string;
  zoomed: boolean;
  area: FakeLayoutRect;
  focused_pane_id: string;
  panes: Array<{ pane_id: string; focused: boolean; rect: FakeLayoutRect }>;
  splits: Array<{ id: string; direction: "right" | "down"; ratio: number; rect: FakeLayoutRect }>;
}

export interface RecordedHerdrRequest {
  id: string;
  method: string;
  params: Record<string, unknown>;
}

export interface FakeHerdrError {
  code: string;
  message: string;
}

export interface FakeResponsePlan {
  delayMs?: number;
  disconnect?: boolean;
  error?: FakeHerdrError;
  raw?: string | Buffer;
}

export interface FakeGraphicsCapability {
  enabled: boolean;
  cellWidthPx: number;
  cellHeightPx: number;
}

export interface FakeGraphicsUpdate {
  pane_id: string;
  format: "png" | "rgb" | "rgba";
  image_width: number;
  image_height: number;
  data_base64: string;
  placement: {
    viewport_col: number;
    viewport_row: number;
    grid_cols: number;
    grid_rows: number;
  };
}

export interface FakeStatusEvent {
  event: "pane_agent_status_changed";
  data: {
    type: "pane_agent_status_changed";
    workspace_id: string;
    pane_id: string;
    agent_status: FakeAgentStatus;
    agent?: string;
  };
}

export interface FakeHerdrServerOptions {
  panes?: FakePaneState[];
  graphics?: Partial<FakeGraphicsCapability>;
}

export function createFakePane(overrides: Partial<FakePaneState> = {}): FakePaneState {
  return {
    pane_id: "w1:p1",
    terminal_id: "term-1",
    workspace_id: "w1",
    tab_id: "w1:t1",
    focused: true,
    agent_status: "idle",
    revision: 1,
    agent: "codex",
    ...overrides
  };
}
