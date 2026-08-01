import { pathToFileURL } from "node:url";

import { registerViewer } from "./viewer/runtime.js";

async function main(): Promise<void> {
  const result = await registerViewer({
    HERDR_SOCKET_PATH: process.env.HERDR_SOCKET_PATH,
    HERDR_PLUGIN_ID: process.env.HERDR_PLUGIN_ID,
    HERDR_PLUGIN_ENTRYPOINT_ID: process.env.HERDR_PLUGIN_ENTRYPOINT_ID,
    HERDR_PANE_ID: process.env.HERDR_PANE_ID,
    HERDR_WORKSPACE_ID: process.env.HERDR_WORKSPACE_ID,
    HERDR_MATH_SOURCE_TOKEN: process.env.HERDR_MATH_SOURCE_TOKEN
  });
  if (!result.ok) {
    process.stderr.write(`${JSON.stringify({ level: "error", code: result.error.code })}\n`);
    process.exitCode = 1;
    return;
  }

  process.stdout.write("Herdr Math viewer ready.\n");
  await waitForTermination();
}

async function waitForTermination(): Promise<void> {
  if (!process.stdin.readable || process.stdin.destroyed) return;
  process.stdin.resume();
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
