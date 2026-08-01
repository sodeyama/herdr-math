import { describe, expect, it } from "vitest";

import { POLICY_LIMITS } from "../../src/core/limits.js";
import { composeRendererDocument } from "../../src/renderer/document.js";
import { scanLatex } from "../../src/scanner/scan-latex.js";

describe("response renderer document", () => {
  it("composes prose and math once in source order", () => {
    const text = "Before $E=mc^2$.\n\nThen:\n$$a^2+b^2=c^2$$\nAfter.";
    const result = composeRendererDocument(text, scanLatex(text));

    expect(result).toEqual({
      ok: true,
      value: {
        segments: [
          { kind: "text", text: "Before " },
          { kind: "math", latex: "E=mc^2", display: false },
          { kind: "text", text: ".\n\nThen:\n" },
          { kind: "math", latex: "a^2+b^2=c^2", display: true },
          { kind: "text", text: "\nAfter." }
        ],
        textBytes: Buffer.byteLength(text, "utf8"),
        lineCount: 5,
        blockCount: 2,
        formulaCount: 2,
        formulaCharacters: 17
      }
    });
  });

  it("leaves unselected delimiter text as escaped prose", () => {
    const text = "Prompt-like $p=0$. Final $x=1$.";
    const formulas = scanLatex(text);
    const result = composeRendererDocument(text, formulas.slice(1));
    expect(result).toMatchObject({
      ok: true,
      value: {
        segments: [
          { kind: "text", text: "Prompt-like $p=0$. Final " },
          { kind: "math", latex: "x=1", display: false },
          { kind: "text", text: "." }
        ]
      }
    });
  });

  it.each([
    ["response_document_bytes", "界".repeat(4), { responseDocumentBytes: 10 }],
    ["response_document_lines", "a\nb\n$x$", { responseDocumentLines: 2 }],
    ["response_document_blocks", "a\n\nb\n\n$x$", { responseDocumentBlocks: 2 }]
  ])("rejects the %s limit before rendering", (kind, text, limits) => {
    const result = composeRendererDocument(text, scanLatex(text), limits);
    expect(result).toMatchObject({
      ok: false,
      error: { code: "renderer_input_limit", details: { limit_kind: kind } }
    });
  });

  it("rejects missing, overlapping, and source-mismatched formulas", () => {
    const text = "Value $x$.";
    const formula = scanLatex(text)[0];
    expect(composeRendererDocument(text, [])).toEqual({
      ok: false,
      error: { code: "formula_not_found", retryable: false }
    });
    expect(composeRendererDocument(text, formula === undefined ? [] : [{ ...formula, start: 0 }])).toEqual({
      ok: false,
      error: { code: "invalid_latex", retryable: false }
    });
  });

  it("keeps the default document limits within pane-read policy", () => {
    expect(POLICY_LIMITS.responseDocumentBytes).toBeLessThanOrEqual(POLICY_LIMITS.paneReadBytes);
    expect(POLICY_LIMITS.responseDocumentLines).toBeGreaterThan(0);
    expect(POLICY_LIMITS.responseDocumentBlocks).toBeGreaterThan(0);
  });
});
