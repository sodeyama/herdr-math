import { describe, expect, it } from "vitest";

import { createViewerMetadata, deriveViewerSourceToken, VIEWER_IDENTITY } from "../../src/viewer/ownership.js";

describe("viewer ownership metadata", () => {
  it("derives a stable, session-bound token without exposing the source pane id", () => {
    const session = "/runtime/herdr.sock";
    const sourcePaneId = "w1:private-pane";
    const token = deriveViewerSourceToken(session, sourcePaneId);

    expect(token).toMatch(/^[a-f0-9]{64}$/);
    expect(token).toBe(deriveViewerSourceToken(session, sourcePaneId));
    expect(token).not.toContain(sourcePaneId);
    expect(token).not.toContain(session);
    expect(deriveViewerSourceToken("/runtime/other.sock", sourcePaneId)).not.toBe(token);
    expect(deriveViewerSourceToken(session, "w1:other-pane")).not.toBe(token);
  });

  it("builds an immutable, bounded English metadata report", () => {
    const token = "a".repeat(64);
    const report = createViewerMetadata(token);

    expect(report).toEqual({
      source: "plugin:io.github.sodeyama.herdr-math.viewer",
      title: "Herdr Math",
      tokens: {
        herdr_math_owner: VIEWER_IDENTITY.ownerToken,
        herdr_math_source: token
      }
    });
    expect(Object.isFrozen(report)).toBe(true);
    expect(Object.isFrozen(report.tokens)).toBe(true);
  });

  it("rejects malformed source identities and tokens", () => {
    expect(() => deriveViewerSourceToken("", "w1:p1")).toThrow("viewer_ownership_failed");
    expect(() => deriveViewerSourceToken("/runtime/herdr.sock", "../pane")).toThrow("viewer_ownership_failed");
    expect(() => createViewerMetadata("not-a-token")).toThrow("viewer_ownership_failed");
  });
});
