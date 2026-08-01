import { describe, expect, it } from "vitest";

import { POLICY_LIMITS } from "../../src/core/limits.js";
import { parseMatchingAnsiSnapshot, parseMatchingAnsiSuffixSnapshot } from "../../src/presentation/ansi-snapshot.js";

describe("matching ANSI terminal snapshots", () => {
  it("normalizes safe SGR styles and terminal row padding", () => {
    const plain = "Reasoning $x$.\nFinal $y$.\n";
    const ansi = [
      "\u001b[3;38;2;128;128;128mReasoning $x$.   \u001b[0m",
      "\u001b[1;38;5;7mFinal\u001b[22m $y$.  \u001b[0m",
      ""
    ].join("\r\n");

    const result = parseMatchingAnsiSnapshot(plain, ansi);

    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.value.text).toBe(plain);
    expect(result.value.lines).toHaveLength(2);
    expect(result.value.lines[0]).toMatchObject({
      text: "Reasoning $x$.",
      hasItalic: true,
      hasForeground: true,
      italicCharacters: 13,
      nonWhitespaceCharacters: 13
    });
    expect(result.value.lines[1]).toMatchObject({
      text: "Final $y$.",
      hasBold: true,
      hasForeground: true,
      hasItalic: false
    });
  });

  it("accepts bounded plain text as a style-free ANSI snapshot", () => {
    const result = parseMatchingAnsiSnapshot("Summary $x$.", "Summary $x$.");
    expect(result).toMatchObject({
      ok: true,
      value: {
        text: "Summary $x$.",
        lines: [{ text: "Summary $x$.", hasItalic: false }]
      }
    });
  });

  it("recovers only the exact line-aligned suffix after an earlier mismatch", () => {
    const plain = "History\nplain tool row\nReasoning $r$.\n\nFinal $x$.\n\n────────────────────────\nstatus";
    const ansi =
      "History\nANSI tool row\n\u001b[3mReasoning $r$.\u001b[0m\n\nFinal $x$.\n\n────────────────────────\nstatus";

    expect(parseMatchingAnsiSnapshot(plain, ansi)).toMatchObject({ ok: false });
    const result = parseMatchingAnsiSuffixSnapshot(plain, ansi);

    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.value.text).toBe(plain);
    expect(result.value.lines[0]).toMatchObject({
      text: "Reasoning $r$.",
      startOffset: "History\nplain tool row\n".length,
      hasItalic: true
    });
    expect(result.value.lines.at(-1)?.text).toBe("status");
  });

  it("rejects suffix recovery when ANSI line positions differ", () => {
    expect(parseMatchingAnsiSuffixSnapshot("History\nFinal $x$.", "History\nextra\nFinal $x$.")).toEqual({
      ok: false,
      error: { code: "conclusion_boundary_failed", retryable: false }
    });
  });

  it.each([
    ["mismatched text", "Final $x$.", "Final $y$."],
    ["unsupported cursor control", "Final $x$.", "\u001b[2JFinal $x$."],
    ["unsupported SGR", "Final $x$.", "\u001b[6mFinal $x$."],
    ["lone carriage return", "A\nB", "A\rB"]
  ])("fails closed for %s", (_name, plain, ansi) => {
    expect(parseMatchingAnsiSnapshot(plain, ansi)).toEqual({
      ok: false,
      error: { code: "conclusion_boundary_failed", retryable: false }
    });
  });

  it("rejects ANSI input above the pane-read byte policy without leaking it", () => {
    const sentinel = "PRIVATE_PRESENTATION_SENTINEL";
    const result = parseMatchingAnsiSnapshot("", `${sentinel}${"x".repeat(POLICY_LIMITS.paneReadBytes)}`);
    expect(result).toEqual({
      ok: false,
      error: { code: "conclusion_boundary_failed", retryable: false }
    });
    expect(JSON.stringify(result)).not.toContain(sentinel);
  });
});
