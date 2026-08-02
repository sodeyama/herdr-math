import { resolve, sep } from "node:path";

import type { HerdrPaneSnapshot } from "../herdr/socket-client.js";

export function resolvePaneWorkingDirectory(pane: Pick<HerdrPaneSnapshot, "cwd" | "foregroundCwd">): string | null {
  return pane.cwd ?? pane.foregroundCwd ?? null;
}

export function isDirectoryAllowed(paneDirectory: string | null, allowedDirectories: readonly string[]): boolean {
  if (allowedDirectories.length === 0) return true;
  if (paneDirectory === null) return false;
  const resolvedPaneDirectory = resolve(paneDirectory);
  return allowedDirectories.some((allowed) => isWithinDirectory(resolvedPaneDirectory, allowed));
}

function isWithinDirectory(candidate: string, allowed: string): boolean {
  const root = resolve(allowed);
  if (candidate === root) return true;
  return candidate.startsWith(`${root}${sep}`);
}
