import { Buffer } from "node:buffer";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { deriveStateKey } from "../../src/boundary/fingerprint-builder.js";
import type { SupportedAgent } from "../../src/boundary/fingerprint-schema.js";
import type { OperationResult, RenderedImage } from "../../src/core/contracts.js";
import {
  processAgentStatusEvent,
  type AgentStatusWorkerDependencies,
  type AgentStatusWorkerOutcome,
  type ResponseRenderRequest
} from "../../src/events/agent-status-worker.js";
import { publishImage } from "../../src/graphics/publisher.js";
import { HerdrSocketClient, type HerdrSocketClientOptions } from "../../src/herdr/socket-client.js";
import type { RendererDocument } from "../../src/renderer/document.js";
import {
  renderResponseWithBackend,
  type RendererBackend,
  type RendererBackendContext
} from "../../src/renderer/render.js";
import { createPaneStatePaths, type PaneStatePaths } from "../../src/state/paths.js";
import { loadPaneState } from "../../src/state/store.js";
import { deriveViewerSourceToken, VIEWER_IDENTITY } from "../../src/viewer/ownership.js";
import { ViewerPresenter } from "../../src/viewer/presenter.js";
import { registerViewer } from "../../src/viewer/runtime.js";
import { FakeHerdrServer } from "./fake-herdr-server.js";
import { createFakePane, type FakeStatusEvent } from "./fake-herdr-types.js";

const NOW = new Date("2026-08-01T00:00:00.000Z");
const SECRET = Buffer.alloc(32, 29);
const PNG_SIGNATURE = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);

export type LifecycleRenderer = (request: ResponseRenderRequest) => Promise<OperationResult<RenderedImage>>;

export class FullLifecycleRig {
  readonly client: HerdrSocketClient;
  readonly renderedFormulas: Array<Array<{ latex: string; display: boolean }>> = [];
  readonly renderedResponses: string[] = [];
  renderer: LifecycleRenderer = renderStatic;
  readonly #dependencies: AgentStatusWorkerDependencies;
  #closed = false;

  private constructor(
    readonly server: FakeHerdrServer,
    readonly stateDirectory: string,
    clientOptions: HerdrSocketClientOptions
  ) {
    this.client = new HerdrSocketClient(server.socketPath, clientOptions);
    const presenter = new ViewerPresenter(this.client, () => Promise.resolve());
    this.#dependencies = {
      client: this.client,
      stateDirectory,
      sessionIdentity: server.socketPath,
      secret: SECRET,
      now: () => NOW,
      sleep: () => Promise.resolve(),
      timing: { completionDebounceMs: 0, stableReadIntervalMs: 0 },
      render: (request) => {
        this.renderedResponses.push(request.text);
        this.renderedFormulas.push(request.formulas.map(({ latex, display }) => ({ latex, display })));
        return this.renderer(request);
      },
      publish: (request) =>
        publishImage(request, {
          client: this.client,
          sessionIdentity: server.socketPath,
          present: (presentation) => presenter.present(presentation)
        })
    };
  }

  static async start(agent: SupportedAgent, clientOptions: HerdrSocketClientOptions = {}): Promise<FullLifecycleRig> {
    const server = await FakeHerdrServer.start({
      panes: [
        createFakePane({
          agent,
          agent_status: "idle",
          agent_session: { source: `herdr:${agent}`, agent, kind: "id", value: `session-${agent}` }
        })
      ]
    });
    try {
      const stateDirectory = await mkdtemp(join(tmpdir(), "herdr-math-full-lifecycle-"));
      return new FullLifecycleRig(server, stateDirectory, clientOptions);
    } catch (error) {
      await server.close();
      throw error;
    }
  }

  async runCycle(
    baseline: string,
    answer: string,
    truncated = false
  ): Promise<{
    working: OperationResult<AgentStatusWorkerOutcome>;
    completion: OperationResult<AgentStatusWorkerOutcome>;
  }> {
    return this.runOutputs(baseline, `${baseline}${answer}`, truncated);
  }

  async runOutputs(
    baseline: string,
    completionOutput: string,
    truncated = false
  ): Promise<{
    working: OperationResult<AgentStatusWorkerOutcome>;
    completion: OperationResult<AgentStatusWorkerOutcome>;
  }> {
    this.server.setPaneOutput("w1:p1", baseline);
    const working = await this.process(this.server.transitionPane("w1:p1", "working"));
    this.server.setPaneOutput("w1:p1", completionOutput, truncated);
    const completion = await this.process(this.server.transitionPane("w1:p1", "done"));
    return { working, completion };
  }

  process(event: FakeStatusEvent): Promise<OperationResult<AgentStatusWorkerOutcome>> {
    return this.processRaw(JSON.stringify(event));
  }

  processRaw(source: string): Promise<OperationResult<AgentStatusWorkerOutcome>> {
    return processAgentStatusEvent(source, this.#dependencies);
  }

  async registerViewer(viewerPaneId: string): Promise<void> {
    const result = await registerViewer(
      {
        HERDR_SOCKET_PATH: this.server.socketPath,
        HERDR_PLUGIN_ID: VIEWER_IDENTITY.pluginId,
        HERDR_PLUGIN_ENTRYPOINT_ID: VIEWER_IDENTITY.entrypointId,
        HERDR_PANE_ID: viewerPaneId,
        HERDR_WORKSPACE_ID: "w1",
        HERDR_MATH_SOURCE_TOKEN: deriveViewerSourceToken(this.server.socketPath, "w1:p1")
      },
      this.client
    );
    if (!result.ok) throw new Error(`Viewer registration failed: ${result.error.code}`);
  }

  async state() {
    return loadPaneState(this.paths(), NOW);
  }

  paths(): PaneStatePaths {
    const sessionKey = deriveStateKey("session", this.server.socketPath, SECRET);
    return createPaneStatePaths(this.stateDirectory, sessionKey, "w1:p1", SECRET);
  }

  async close(): Promise<void> {
    if (this.#closed) return;
    this.#closed = true;
    try {
      await this.server.close();
    } finally {
      await rm(this.stateDirectory, { recursive: true, force: true });
    }
  }
}

export function timeoutRenderer(durationMs = 10): LifecycleRenderer {
  return ({ text, formulas }) =>
    renderResponseWithBackend(text, formulas, new BlockingBackend(), { limits: { renderDurationMs: durationMs } });
}

export function renderStatic({ text, formulas }: ResponseRenderRequest): Promise<OperationResult<RenderedImage>> {
  return renderResponseWithBackend(text, formulas, new StaticBackend(image()));
}

function image(): RenderedImage {
  const buffer = Buffer.alloc(16);
  PNG_SIGNATURE.copy(buffer);
  return { buffer, width: 640, height: 320, bytes: buffer.byteLength, renderer: "lifecycle-test" };
}

class StaticBackend implements RendererBackend {
  constructor(private readonly renderedImage: RenderedImage) {}

  render(): Promise<RenderedImage> {
    return Promise.resolve(this.renderedImage);
  }

  close(): Promise<void> {
    return Promise.resolve();
  }
}

class BlockingBackend implements RendererBackend {
  render(_document: RendererDocument, context: RendererBackendContext): Promise<RenderedImage> {
    return new Promise((_resolve, reject) => {
      context.signal.addEventListener("abort", () => reject(new Error("aborted")), { once: true });
    });
  }

  close(): Promise<void> {
    return Promise.resolve();
  }
}
