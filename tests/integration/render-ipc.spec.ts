import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

import type { RenderIpcResponse } from "../../src/renderer/ipc-contract.js";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
const subprocessPath = resolve(root, "dist", "renderer", "subprocess.js");

function renderSubprocess(requestJson: string): Promise<RenderIpcResponse> {
  return new Promise((resolvePromise, reject) => {
    const child = spawn(process.execPath, [subprocessPath], { stdio: ["pipe", "pipe", "pipe"] });
    const stdout: Buffer[] = [];
    const stderr: Buffer[] = [];
    child.stdout.on("data", (chunk: Buffer) => stdout.push(chunk));
    child.stderr.on("data", (chunk: Buffer) => stderr.push(chunk));
    child.on("error", reject);
    child.on("exit", (code) => {
      if (code !== 0) {
        reject(new Error(`subprocess exited ${code}: ${Buffer.concat(stderr).toString("utf8")}`));
        return;
      }
      const payload = Buffer.concat(stdout).toString("utf8");
      const [line] = payload.split("\n");
      resolvePromise(JSON.parse(line ?? "null") as RenderIpcResponse);
    });
    child.stdin.write(requestJson);
    child.stdin.end();
  });
}

describe("one-shot render subprocess transport", () => {
  it("is available because the renderer has been built", () => {
    expect(existsSync(subprocessPath)).toBe(true);
  });

  it("reads one request and writes exactly one response for a document", async () => {
    const response = await renderSubprocess(
      JSON.stringify({
        protocol: "tmath-render/1",
        kind: "document",
        text: "The relation is $E=mc^2$."
      })
    );
    expect(response.protocol).toBe("tmath-render/1");
    expect(response.ok).toBe(true);
    if (!response.ok) return;
    expect(response.width).toBeGreaterThan(0);
    expect(response.height).toBeGreaterThan(0);
    expect(response.bytes).toBeGreaterThan(0);
    expect(response.base64.startsWith("iVBORw0KGgo")).toBe(true);
  }, 30_000);

  it("handles a formula request", async () => {
    const response = await renderSubprocess(
      JSON.stringify({
        protocol: "tmath-render/1",
        kind: "formulas",
        formulas: [{ latex: "e^{i\\pi}+1=0", display: true }]
      })
    );
    expect(response.ok).toBe(true);
  }, 30_000);

  it("returns a stable error for invalid LaTeX and never emits source", async () => {
    const source = "bad $\\href{https://example.com}{x}$";
    const response = await renderSubprocess(
      JSON.stringify({ protocol: "tmath-render/1", kind: "document", text: source })
    );
    expect(response.ok).toBe(false);
    if (response.ok) return;
    expect(response.error).toEqual({ code: "invalid_latex", retryable: false });
    expect(JSON.stringify(response)).not.toContain("example.com");
    expect(JSON.stringify(response)).not.toContain("href");
  }, 30_000);

  it("fails closed for an empty source", async () => {
    const response = await renderSubprocess(
      JSON.stringify({ protocol: "tmath-render/1", kind: "document", text: "   " })
    );
    expect(response.ok).toBe(false);
    if (response.ok) return;
    expect(response.error.code).toBe("formula_not_found");
  });

  it("rejects an unsupported protocol", async () => {
    const response = await renderSubprocess(
      JSON.stringify({ protocol: "other/9", kind: "document", text: "ok $a$" })
    );
    expect(response.ok).toBe(false);
    if (response.ok) return;
    expect(response.error.code).toBe("renderer_input_limit");
  });
});
