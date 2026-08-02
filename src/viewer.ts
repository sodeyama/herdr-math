import { pathToFileURL } from "node:url";

import { startManagedViewer } from "./viewer/runtime.js";

const SCROLL_LINE_ROWS = 3;
const SCROLL_PAGE_ROWS = 20;

async function main(): Promise<void> {
  const result = await startManagedViewer({
    HERDR_SOCKET_PATH: process.env.HERDR_SOCKET_PATH,
    HERDR_PLUGIN_ID: process.env.HERDR_PLUGIN_ID,
    HERDR_PLUGIN_ENTRYPOINT_ID: process.env.HERDR_PLUGIN_ENTRYPOINT_ID,
    HERDR_PANE_ID: process.env.HERDR_PANE_ID,
    HERDR_WORKSPACE_ID: process.env.HERDR_WORKSPACE_ID,
    HERDR_PLUGIN_STATE_DIR: process.env.HERDR_PLUGIN_STATE_DIR,
    HERDR_MATH_SOURCE_TOKEN: process.env.HERDR_MATH_SOURCE_TOKEN
  });
  if (!result.ok) {
    process.stderr.write(`${JSON.stringify({ level: "error", code: result.error.code })}\n`);
    process.exitCode = 1;
    return;
  }

  const ready = result.value;
  startKeyboardScroll(ready.presenter, ready.paneId, ready.workspaceId);

  process.stdout.write("Herdr Math viewer ready.\n");
  try {
    await waitForTermination();
  } finally {
    ready.subscription?.close();
    ready.layoutSubscription?.close();
    await ready.transport.close();
  }
}

function startKeyboardScroll(
  presenter: { scrollBy(viewerPaneId: string, workspaceId: string, deltaRows: number): Promise<unknown> },
  paneId: string,
  workspaceId: string
): void {
  const stdin = process.stdin;
  if (!stdin.readable || stdin.destroyed) return;
  if (typeof stdin.setRawMode === "function") {
    try {
      stdin.setRawMode(true);
    } catch {
      return;
    }
  }
  stdin.resume();
  let pending = Buffer.alloc(0);
  stdin.on("data", (chunk: Buffer) => {
    pending = Buffer.concat([pending, chunk]);
    const { deltaRows, consumed } = parseScrollKeys(pending);
    pending = pending.subarray(consumed);
    if (deltaRows !== 0) {
      void presenter.scrollBy(paneId, workspaceId, deltaRows);
    }
  });
}

function parseScrollKeys(buffer: Buffer): { deltaRows: number; consumed: number } {
  const text = buffer.toString("utf8");
  // Up: ESC [ A, Down: ESC [ B, PgUp: ESC [ 5 ~, PgDn: ESC [ 6 ~
  if (text.startsWith("[A") || text === "k")
    return { deltaRows: SCROLL_LINE_ROWS, consumed: sequenceLength(buffer, text, "[A", "k") };
  if (text.startsWith("[B") || text === "j")
    return { deltaRows: -SCROLL_LINE_ROWS, consumed: sequenceLength(buffer, text, "[B", "j") };
  if (text.startsWith("[5~")) return { deltaRows: SCROLL_PAGE_ROWS, consumed: Buffer.byteLength("[5~") };
  if (text.startsWith("[6~")) return { deltaRows: -SCROLL_PAGE_ROWS, consumed: Buffer.byteLength("[6~") };
  if (text === "g") return { deltaRows: Number.MAX_SAFE_INTEGER, consumed: 1 };
  if (text === "G") return { deltaRows: -Number.MAX_SAFE_INTEGER, consumed: 1 };
  // Incomplete escape sequence: wait for more bytes.
  if (text.startsWith("") && text.length < 6) return { deltaRows: 0, consumed: 0 };
  return { deltaRows: 0, consumed: buffer.byteLength };
}

function sequenceLength(buffer: Buffer, text: string, escapeSequence: string, plain: string): number {
  if (text.startsWith(escapeSequence)) return Buffer.byteLength(escapeSequence);
  if (text === plain) return 1;
  return buffer.byteLength;
}

async function waitForTermination(): Promise<void> {
  if (!process.stdin.readable || process.stdin.destroyed) return;
  await new Promise<void>((resolve) => {
    let finished = false;
    const finish = (): void => {
      if (finished) return;
      finished = true;
      process.stdin.off("end", finish);
      process.stdin.off("close", finish);
      process.off("SIGHUP", finish);
      process.off("SIGINT", finish);
      process.off("SIGTERM", finish);
      resolve();
    };
    process.stdin.once("end", finish);
    process.stdin.once("close", finish);
    process.once("SIGHUP", finish);
    process.once("SIGINT", finish);
    process.once("SIGTERM", finish);
  });
}

if (process.argv[1] !== undefined && import.meta.url === pathToFileURL(process.argv[1]).href) {
  await main();
}
