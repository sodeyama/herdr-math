import { Buffer } from "node:buffer";

import { describe, expect, it } from "vitest";

import type { RenderedImage } from "../../src/core/contracts.js";
import {
  decodeViewerTransportRequest,
  encodeViewerTransportRequest,
  parseViewerTransportResponse
} from "../../src/viewer/transport-protocol.js";

const token = "a".repeat(64);
const image = pngImage();

describe("viewer transport protocol", () => {
  it("round-trips only authenticated pixels and bounded identifiers", () => {
    const encoded = encodeViewerTransportRequest({
      stateDirectory: "/tmp/herdr-math-state",
      sourceToken: token,
      viewerPaneId: "w1:p2",
      workspaceId: "w1",
      generation: 2,
      image
    });
    expect(encoded.ok).toBe(true);
    if (!encoded.ok) return;
    const decoded = decodeViewerTransportRequest(encoded.value.payload.trimEnd(), {
      sourceToken: token,
      viewerPaneId: "w1:p2",
      workspaceId: "w1"
    });
    expect(decoded).toMatchObject({ ok: true, value: { image: { width: 1, height: 1, bytes: image.bytes } } });
  });

  it("rejects a source-token mismatch without exposing request data", () => {
    const encoded = encodeViewerTransportRequest({
      stateDirectory: "/tmp/herdr-math-state",
      sourceToken: token,
      viewerPaneId: "w1:p2",
      workspaceId: "w1",
      generation: 2,
      image
    });
    expect(encoded.ok).toBe(true);
    if (!encoded.ok) return;
    expect(
      decodeViewerTransportRequest(encoded.value.payload.trimEnd(), {
        sourceToken: "b".repeat(64),
        viewerPaneId: "w1:p2",
        workspaceId: "w1"
      })
    ).toEqual({ ok: false, error: { code: "viewer_ownership_failed", retryable: false } });
  });

  it("fails closed for malformed server responses", () => {
    expect(parseViewerTransportResponse('{"ok":true,"viewerPaneId":"../pane"}')).toEqual({
      ok: false,
      error: { code: "viewer_ownership_failed", retryable: false }
    });
  });
});

function pngImage(): RenderedImage {
  const buffer = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 1]);
  return { buffer, width: 1, height: 1, bytes: buffer.byteLength, renderer: "test" };
}
