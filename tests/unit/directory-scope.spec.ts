import { describe, expect, it } from "vitest";

import { isDirectoryAllowed, resolvePaneWorkingDirectory } from "../../src/config/directory-scope.js";

describe("directory scope", () => {
  it("allows every pane when no directories are configured", () => {
    expect(isDirectoryAllowed("/tmp/other", [])).toBe(true);
    expect(isDirectoryAllowed(null, [])).toBe(true);
  });

  it("prefers pane cwd over foreground cwd", () => {
    expect(
      resolvePaneWorkingDirectory({
        cwd: "/tmp/root",
        foregroundCwd: "/tmp/root/project"
      })
    ).toBe("/tmp/root");
  });

  it("matches exact roots and nested paths only", () => {
    const allowed = ["/Users/example/docs"];
    expect(isDirectoryAllowed("/Users/example/docs", allowed)).toBe(true);
    expect(isDirectoryAllowed("/Users/example/docs/notes", allowed)).toBe(true);
    expect(isDirectoryAllowed("/Users/example/docs-extra", allowed)).toBe(false);
    expect(isDirectoryAllowed("/Users/example/doc", allowed)).toBe(false);
    expect(isDirectoryAllowed(null, allowed)).toBe(false);
  });
});
