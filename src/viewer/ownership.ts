import { Buffer } from "node:buffer";
import { createHash } from "node:crypto";

import { HerdrMathError } from "../core/errors.js";
import { HERDR_CLIENT_LIMITS, type HerdrPaneMetadataReport } from "../herdr/socket-client.js";

const HERDR_IDENTIFIER = /^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$/;
const SOURCE_TOKEN = /^[a-f0-9]{64}$/;

export const VIEWER_IDENTITY = Object.freeze({
  pluginId: "io.github.sodeyama.herdr-math",
  entrypointId: "viewer",
  title: "Herdr Math",
  metadataSource: "plugin:io.github.sodeyama.herdr-math.viewer",
  ownerToken: "herdr-math-viewer-v1",
  ownerTokenKey: "herdr_math_owner",
  sourceTokenKey: "herdr_math_source"
});

export function deriveViewerSourceToken(sessionIdentity: string, sourcePaneId: string): string {
  if (
    typeof sessionIdentity !== "string" ||
    sessionIdentity.length === 0 ||
    sessionIdentity.includes("\0") ||
    Buffer.byteLength(sessionIdentity, "utf8") > HERDR_CLIENT_LIMITS.socketPathBytes ||
    !HERDR_IDENTIFIER.test(sourcePaneId)
  ) {
    throw new HerdrMathError("viewer_ownership_failed");
  }
  return createHash("sha256")
    .update("herdr-math-viewer-source-v1\0", "utf8")
    .update(sessionIdentity, "utf8")
    .update("\0", "utf8")
    .update(sourcePaneId, "utf8")
    .digest("hex");
}

export function isViewerSourceToken(value: unknown): value is string {
  return typeof value === "string" && SOURCE_TOKEN.test(value);
}

export function createViewerMetadata(sourceToken: string): HerdrPaneMetadataReport {
  if (!isViewerSourceToken(sourceToken)) throw new HerdrMathError("viewer_ownership_failed");
  return Object.freeze({
    source: VIEWER_IDENTITY.metadataSource,
    title: VIEWER_IDENTITY.title,
    tokens: Object.freeze({
      [VIEWER_IDENTITY.ownerTokenKey]: VIEWER_IDENTITY.ownerToken,
      [VIEWER_IDENTITY.sourceTokenKey]: sourceToken
    })
  });
}
