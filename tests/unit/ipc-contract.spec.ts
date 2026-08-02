import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

import {
  IPC_MAX_REQUEST_BYTES,
  IPC_MAX_RESPONSE_BYTES,
  IPC_PROTOCOL,
  decodeRequest,
  encodeRequest,
  encodeResponse,
  validateRequest
} from "../../src/renderer/ipc-contract.js";

describe("render IPC contract", () => {
  it("encodes and decodes a document request", () => {
    const request = {
      protocol: IPC_PROTOCOL,
      kind: "document",
      text: "The relation is $E=mc^2$."
    } as const;
    const decoded = decodeRequest(encodeRequest(request));
    expect(decoded.protocol).toBe(IPC_PROTOCOL);
    expect(decoded.kind).toBe("document");
    expect(decoded.text).toBe("The relation is $E=mc^2$.");
  });

  it("encodes and decodes a formula request", () => {
    const request = {
      protocol: IPC_PROTOCOL,
      kind: "formulas",
      formulas: [{ latex: "E=mc^2", display: false }]
    } as const;
    const decoded = decodeRequest(encodeRequest(request));
    expect(decoded.kind).toBe("formulas");
    expect(decoded.formulas?.[0]).toEqual({ latex: "E=mc^2", display: false });
  });

  it("rejects an oversized request before JSON parsing", () => {
    const oversized = Buffer.alloc(IPC_MAX_REQUEST_BYTES + 1, 0x61);
    expect(() => decodeRequest(oversized)).toThrow(RangeError);
    expect(() => encodeRequest(oversized as never)).toThrow(RangeError);
  });

  it("rejects malformed request JSON and missing fields", () => {
    expect(() => decodeRequest(Buffer.from("not json"))).toThrow();
    const invalid = { protocol: IPC_PROTOCOL, kind: "nope" };
    expect(() => decodeRequest(encodeRequest(invalid as never))).toThrow(TypeError);
  });

  it("validates protocol, kind, and required content", () => {
    expect(
      validateRequest({ protocol: IPC_PROTOCOL, kind: "document", text: "hi" })
    ).toBeUndefined();
    expect(
      validateRequest({ protocol: "other/9", kind: "document", text: "hi" })
    ).toMatch(/Unsupported protocol/);
    expect(validateRequest({ protocol: IPC_PROTOCOL, kind: "document", text: "  " })).toBe(
      "No source text"
    );
    expect(validateRequest({ protocol: IPC_PROTOCOL, kind: "formulas", formulas: [] })).toBe(
      "No formulas"
    );
  });

  it("rejects an oversized response", () => {
    const response = {
      protocol: IPC_PROTOCOL,
      ok: true,
      width: 1,
      height: 1,
      bytes: 1,
      renderer: "test",
      base64: "x".repeat(IPC_MAX_RESPONSE_BYTES + 1)
    } as const;
    expect(() => encodeResponse(response)).toThrow(RangeError);
  });

  it("accepts every fixture request the renderer should accept", () => {
    const fixture = JSON.parse(
      readFileSync(new URL("../fixtures/render-ipc/requests.json", import.meta.url), "utf8")
    ) as {
      protocol: string;
      cases: Array<{ id: string; request: unknown; expect: string; errorCode?: string }>;
    };
    expect(fixture.protocol).toBe(IPC_PROTOCOL);
    for (const testCase of fixture.cases) {
      const request = decodeRequest(Buffer.from(JSON.stringify(testCase.request)));
      const accepts = testCase.expect === "ok" || testCase.errorCode === "invalid_latex";
      if (accepts) {
        expect(request.protocol).toBe(IPC_PROTOCOL);
        expect(validateRequest(request)).toBeUndefined();
      } else {
        expect(validateRequest(request)).not.toBeUndefined();
      }
    }
  });
});
