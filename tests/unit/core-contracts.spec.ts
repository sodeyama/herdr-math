import { Buffer } from "node:buffer";

import { describe, expect, it } from "vitest";

import { failure, success, type BoundaryResult, type Formula, type RenderedImage } from "../../src/core/contracts.js";
import { ERROR_CODES, HerdrMathError, serializeError } from "../../src/core/errors.js";
import { POLICY_LIMITS } from "../../src/core/limits.js";
import { ScannerLimitError } from "../../src/scanner/scan-latex.js";

describe("shared core contracts", () => {
  it("defines formula, boundary, and rendered image shapes", () => {
    const formula: Formula = { latex: "x", display: false, start: 0, end: 3 };
    const boundary: BoundaryResult = {
      answer: "$x$",
      startOffset: 10,
      strategy: "exact_prefix",
      recoveredTruncation: false,
      currentDigest: "a".repeat(64),
      proof: { kind: "exact_prefix", baselineCharacters: 10 }
    };
    const image: RenderedImage = {
      buffer: Buffer.from([1, 2, 3]),
      width: 1,
      height: 1,
      bytes: 3,
      renderer: "test"
    };

    expect(success(formula)).toEqual({ ok: true, value: formula });
    expect(boundary.strategy).toBe("exact_prefix");
    expect(boundary.proof.kind).toBe(boundary.strategy);
    expect(image.bytes).toBe(image.buffer.byteLength);
  });

  it("keeps policy limits positive and internally coherent", () => {
    for (const value of Object.values(POLICY_LIMITS)) {
      expect(Number.isSafeInteger(value)).toBe(true);
      expect(value).toBeGreaterThan(0);
    }

    const encodedRawPngBytes = 4 * Math.ceil(POLICY_LIMITS.rawPngBytes / 3);
    expect(POLICY_LIMITS.base64PayloadBytes).toBeGreaterThanOrEqual(encodedRawPngBytes);
    expect(POLICY_LIMITS.scannerInputBytes).toBeLessThanOrEqual(POLICY_LIMITS.paneReadBytes);
    expect(POLICY_LIMITS.formulasPerAnswer * POLICY_LIMITS.charactersPerFormula).toBeGreaterThanOrEqual(
      POLICY_LIMITS.aggregateFormulaCharacters
    );
  });

  it("publishes unique stable error codes", () => {
    expect(new Set(ERROR_CODES).size).toBe(ERROR_CODES.length);
    expect(ERROR_CODES).toEqual(
      expect.arrayContaining([
        "event_invalid",
        "scanner_input_limit",
        "invalid_latex",
        "renderer_input_limit",
        "renderer_timeout",
        "image_too_large",
        "internal_error"
      ])
    );
  });

  it("serializes only allowlisted error details", () => {
    const error = new HerdrMathError("image_too_large", {
      limit_kind: "raw_png_bytes",
      limit: POLICY_LIMITS.rawPngBytes,
      actual: POLICY_LIMITS.rawPngBytes + 1,
      bytes: POLICY_LIMITS.rawPngBytes + 1
    });
    const record = serializeError(error);

    expect(failure(record)).toEqual({ ok: false, error: record });
    expect(record).toEqual({
      code: "image_too_large",
      retryable: false,
      details: {
        limit_kind: "raw_png_bytes",
        limit: POLICY_LIMITS.rawPngBytes,
        actual: POLICY_LIMITS.rawPngBytes + 1,
        bytes: POLICY_LIMITS.rawPngBytes + 1
      }
    });
  });

  it("maps scanner limits into the shared safe record", () => {
    expect(serializeError(new ScannerLimitError("delimiter_runs", 4, 5))).toEqual({
      code: "scanner_input_limit",
      retryable: false,
      details: { limit_kind: "delimiter_runs", limit: 4, actual: 5 }
    });
  });

  it("does not serialize arbitrary exceptions, messages, stacks, or input", () => {
    const sentinel = "PRIVATE_FORMULA_SENTINEL";
    const record = serializeError(new Error(sentinel));
    const serialized = JSON.stringify(record);

    expect(record).toEqual({ code: "internal_error", retryable: false });
    expect(serialized).not.toContain(sentinel);
    expect(serialized).not.toContain("stack");
    expect(serialized).not.toContain("message");
  });

  it("drops invalid numeric details at runtime", () => {
    const unsafeDetails = { limit: -1, actual: Number.POSITIVE_INFINITY };
    const error = new HerdrMathError("renderer_input_limit", unsafeDetails);
    expect(serializeError(error)).toEqual({ code: "renderer_input_limit", retryable: false });
  });
});
