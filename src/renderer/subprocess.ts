import { Buffer } from "node:buffer";
import { resolve } from "node:path";
import { stdin, stdout } from "node:process";
import { pathToFileURL } from "node:url";

import type { OperationResult, RenderedImage } from "../core/contracts.js";
import { HerdrMathError, serializeError, type SafeErrorRecord } from "../core/errors.js";
import { scanLatex } from "../scanner/scan-latex.js";
import {
  IPC_MAX_REQUEST_BYTES,
  IPC_PROTOCOL,
  decodeRequest,
  encodeResponse,
  validateRequest,
  type RenderIpcRequest,
  type RenderIpcResponse
} from "./ipc-contract.js";
import { renderFormulas, renderResponse } from "./index.js";

/**
 * Entrypoint for the one-shot renderer subprocess.
 *
 * Reads exactly one bounded JSON request on stdin, renders it with the
 * existing local pipeline, writes exactly one bounded JSON response on stdout,
 * and exits. It never stays alive waiting for a second request.
 */
export async function main(): Promise<number> {
  let request: RenderIpcRequest;
  try {
    const payload = await readBoundedStdin();
    request = decodeRequest(payload);
    const invalid = validateRequest(request);
    if (invalid !== undefined) {
      writeError(makeProtocolError(invalid));
      return 0;
    }
  } catch (error) {
    writeError(
      error instanceof RangeError
        ? new HerdrMathError("renderer_input_limit", {}, true)
        : new HerdrMathError("renderer_input_limit", {})
    );
    return 0;
  }
  const options = request.options ?? {};
  if (request.kind === "formulas") {
    const outcome = await renderFormulas(
      (request.formulas ?? []).map(({ latex, display }) => ({ latex, display })),
      options
    );
    return finish(request.protocol, outcome);
  }

  const text = request.text ?? "";
  try {
    const outcome = await renderResponse(text, scanLatex(text), options);
    return finish(request.protocol, outcome);
  } catch (error) {
    // The scanner throws a HerdrMathError (e.g. scanner_input_limit) that the
    // renderer never sees; it must become a stable, safe JSON error record
    // rather than crashing the subprocess.
    writeError(error instanceof HerdrMathError ? error : new HerdrMathError("renderer_failed", {}, true));
    return 0;
  }
}

function finish(protocol: string, outcome: OperationResult<RenderedImage>): number {
  if (outcome.ok) {
    const image = outcome.value;
    const response: RenderIpcResponse = {
      protocol,
      ok: true,
      width: image.width,
      height: image.height,
      bytes: image.bytes,
      renderer: image.renderer,
      base64: image.buffer.toString("base64")
    };
    return writeResponse(response);
  }
  writeError(outcome.error);
  return 0;
}

function makeProtocolError(message: string): HerdrMathError {
  return message === "No formulas" || message === "No source text"
    ? new HerdrMathError("formula_not_found", {})
    : new HerdrMathError("renderer_input_limit", {});
}

function writeResponse(response: RenderIpcResponse): number {
  try {
    stdout.write(encodeResponse(response));
    return 0;
  } catch {
    writeError(new HerdrMathError("renderer_failed", {}, true));
    return 1;
  }
}

function writeError(error: SafeErrorRecord | HerdrMathError): void {
  const record = error instanceof HerdrMathError ? serializeError(error) : error;
  const response: RenderIpcResponse = { protocol: IPC_PROTOCOL, ok: false, error: record };
  try {
    stdout.write(encodeResponse(response));
  } catch {
    // The response is too large to write safely; nothing else to do.
  }
}

function readBoundedStdin(): Promise<Buffer> {
  return new Promise((resolvePromise, reject) => {
    const chunks: Buffer[] = [];
    let total = 0;
    stdin.on("data", (chunk: Buffer) => {
      total += chunk.byteLength;
      if (total > IPC_MAX_REQUEST_BYTES) {
        reject(new RangeError("Request exceeds limit"));
        stdin.destroy();
        return;
      }
      chunks.push(chunk);
    });
    stdin.on("end", () => resolvePromise(Buffer.concat(chunks)));
    stdin.on("error", reject);
  });
}

if (import.meta.url === pathToFileURL(resolve(process.argv[1] ?? "")).href) {
  const code = await main();
  process.exit(code);
}
