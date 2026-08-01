import { Buffer } from "node:buffer";
import { mkdir, mkdtemp, readFile, rm, stat, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterEach, describe, expect, it } from "vitest";

import { buildBaselineFingerprint, deriveStateKey } from "../../src/boundary/fingerprint-builder.js";
import { fingerprintDigest } from "../../src/boundary/fingerprint-digest.js";
import { isFingerprintDigest } from "../../src/boundary/fingerprint-schema.js";
import { loadOrCreateFingerprintSecret } from "../../src/boundary/fingerprint-secret.js";
import { HerdrMathError, serializeError } from "../../src/core/errors.js";
import { POLICY_LIMITS } from "../../src/core/limits.js";

const temporaryDirectories: string[] = [];

async function temporaryDirectory(): Promise<string> {
  const directory = await mkdtemp(join(tmpdir(), "herdr-math-fingerprint-"));
  temporaryDirectories.push(directory);
  return directory;
}

afterEach(async () => {
  await Promise.all(temporaryDirectories.splice(0).map((directory) => rm(directory, { recursive: true, force: true })));
});

function metadata() {
  return {
    sessionIdentity: "synthetic-session-identity",
    occupantIdentity: "synthetic-agent-session",
    workspaceId: "w1",
    sourcePaneId: "w1:p1",
    agent: "codex" as const,
    lifecycleAuthority: "screen_detection" as const,
    paneRevision: 12,
    eventSequence: 5,
    generation: 2,
    createdAt: new Date("2026-08-01T00:00:00.000Z")
  };
}

describe("fingerprint secret", () => {
  it("creates and reloads a user-only secret", async () => {
    const directory = await temporaryDirectory();
    const first = await loadOrCreateFingerprintSecret(directory);
    const second = await loadOrCreateFingerprintSecret(directory);
    const secretPath = join(directory, "v1", "secret");

    expect(first.byteLength).toBe(32);
    expect(second.equals(first)).toBe(true);
    expect((await stat(join(directory, "v1"))).mode & 0o777).toBe(0o700);
    expect((await stat(secretPath)).mode & 0o777).toBe(0o600);
    expect((await readFile(secretPath)).equals(first)).toBe(true);
  });

  it("converges on one secret under concurrent creation", async () => {
    const directory = await temporaryDirectory();
    const secrets = await Promise.all(Array.from({ length: 8 }, () => loadOrCreateFingerprintSecret(directory)));
    expect(new Set(secrets.map((secret) => secret.toString("hex"))).size).toBe(1);
  });

  it("rejects malformed and symbolic-link secret files", async () => {
    const malformedDirectory = await temporaryDirectory();
    await loadOrCreateFingerprintSecret(malformedDirectory);
    await writeFile(join(malformedDirectory, "v1", "secret"), "short");
    await expect(loadOrCreateFingerprintSecret(malformedDirectory)).rejects.toMatchObject({ code: "state_corrupt" });

    const linkedDirectory = await temporaryDirectory();
    const target = join(linkedDirectory, "target");
    await writeFile(target, Buffer.alloc(32));
    await mkdir(join(linkedDirectory, "v1"), { mode: 0o700 });
    await symlink(target, join(linkedDirectory, "v1", "secret"));
    await expect(loadOrCreateFingerprintSecret(linkedDirectory)).rejects.toMatchObject({ code: "state_corrupt" });
  });
});

describe("baseline fingerprint builder", () => {
  it("creates deterministic bounded fingerprints without raw input", () => {
    const secret = Buffer.alloc(32, 7);
    const sentinel = "SYNTHETIC_PRIVATE_ANSWER";
    const input = `History line with enough unique characters 1234567890\nPrompt line with enough unique characters abcdefghijklmnop\n${sentinel} $E=mc^2$`;
    const first = buildBaselineFingerprint(input, metadata(), secret);
    const second = buildBaselineFingerprint(input, metadata(), secret);
    const serialized = JSON.stringify(first);

    expect(second).toEqual(first);
    expect(serialized).not.toContain(sentinel);
    expect(serialized).not.toContain("E=mc^2");
    expect(serialized).not.toContain(metadata().sessionIdentity);
    expect(serialized).not.toContain(metadata().occupantIdentity);
    expect(isFingerprintDigest(first.baseline.digest)).toBe(true);
    expect(first.baseline.prefix_checkpoints.length).toBeLessThanOrEqual(16);
    expect(first.baseline.suffix_windows.length).toBeLessThanOrEqual(4);
    expect(first.baseline.tail_anchors.length).toBeLessThanOrEqual(20);
    expect(first.expires_at).toBe("2026-08-02T00:00:00.000Z");
  });

  it("changes content and identity digests independently", () => {
    const secret = Buffer.alloc(32, 9);
    const first = buildBaselineFingerprint("A".repeat(600), metadata(), secret);
    const contentChanged = buildBaselineFingerprint(`${"A".repeat(599)}B`, metadata(), secret);
    const identityChanged = buildBaselineFingerprint(
      "A".repeat(600),
      { ...metadata(), occupantIdentity: "different-agent-session" },
      secret
    );

    expect(contentChanged.baseline.digest).not.toBe(first.baseline.digest);
    expect(identityChanged.baseline.digest).toBe(first.baseline.digest);
    expect(identityChanged.occupant_key).not.toBe(first.occupant_key);
  });

  it("binds a tail anchor to its immediately preceding line", () => {
    const secret = Buffer.alloc(32, 4);
    const anchor = "Synthetic repeated prompt with unique value 1234567890";
    const first = buildBaselineFingerprint(`old history A\nmatching context\n${anchor}`, metadata(), secret);
    const olderHistoryChanged = buildBaselineFingerprint(
      `old history B\nmatching context\n${anchor}`,
      metadata(),
      secret
    );
    const contextChanged = buildBaselineFingerprint(`old history A\ndifferent context\n${anchor}`, metadata(), secret);

    expect(olderHistoryChanged.baseline.tail_anchors[0]?.context_digest).toBe(
      first.baseline.tail_anchors[0]?.context_digest
    );
    expect(contextChanged.baseline.tail_anchors[0]?.context_digest).not.toBe(
      first.baseline.tail_anchors[0]?.context_digest
    );
    expect(first.baseline.tail_anchors[0]?.end_offset).toBe(first.baseline.character_count);
  });

  it("binds eligible adjacent anchors to their intervening baseline gap", () => {
    const secret = Buffer.alloc(32, 6);
    const before = "Synthetic prompt anchor with unique value 1234567890";
    const gap = "\nstable alternate-screen status\n";
    const after = "Synthetic footer anchor with unique value abcdefghij";
    const state = buildBaselineFingerprint(`${before}${gap}${after}`, metadata(), secret);
    const beforeAnchor = state.baseline.tail_anchors.find((anchor) => anchor.end_offset === before.length);
    const afterAnchor = state.baseline.tail_anchors.find(
      (anchor) => anchor.end_offset === before.length + gap.length + after.length
    );

    expect(beforeAnchor?.next_anchor_gap_digest).toBe(fingerprintDigest("anchor-gap", gap, secret));
    expect(afterAnchor?.next_anchor_gap_digest).toBeUndefined();
    expect(JSON.stringify(state)).not.toContain(gap);
  });

  it("collects eligible anchors beyond blank alternate-screen tail rows", () => {
    const secret = Buffer.alloc(32, 8);
    const first = "Synthetic older eligible anchor with unique value 1234567890";
    const second = "Synthetic nearer eligible anchor with unique value abcdefghij";
    const blankTail = Array.from({ length: 40 }, () => "").join("\n");
    const state = buildBaselineFingerprint(`${first}\n${second}\n${blankTail}`, metadata(), secret);

    expect(state.baseline.tail_anchors).toHaveLength(2);
    expect(state.baseline.tail_anchors.map(({ line_index_from_end }) => line_index_from_end)).toEqual([40, 41]);
    expect(state.baseline.tail_anchors.every(({ end_offset }) => end_offset !== undefined)).toBe(true);
  });

  it("derives path-safe non-reversible state keys", () => {
    const secret = Buffer.alloc(32, 1);
    const key = deriveStateKey("pane", "session/path\\pane", secret);
    expect(key).toMatch(/^[a-f0-9]{64}$/);
    expect(key).not.toContain("/");
    expect(key).not.toContain("\\");
    expect(key).not.toContain("session");
  });

  it("rejects oversized input and unsafe metadata without exposing it", () => {
    const secret = Buffer.alloc(32, 1);
    const oversized = "x".repeat(POLICY_LIMITS.paneReadBytes + 1);
    try {
      buildBaselineFingerprint(oversized, metadata(), secret);
      throw new Error("Expected oversized input rejection.");
    } catch (error: unknown) {
      expect(serializeError(error)).toEqual({
        code: "scanner_input_limit",
        retryable: false,
        details: {
          limit_kind: "pane_read_bytes",
          limit: POLICY_LIMITS.paneReadBytes,
          actual: POLICY_LIMITS.paneReadBytes + 1
        }
      });
    }

    let unsafeRecord: ReturnType<typeof serializeError> | undefined;
    try {
      buildBaselineFingerprint("safe", { ...metadata(), sourcePaneId: "../unsafe-pane" }, secret);
    } catch (error: unknown) {
      unsafeRecord = serializeError(error);
    }
    expect(unsafeRecord).toEqual({ code: "event_invalid", retryable: false });
    expect(JSON.stringify(unsafeRecord)).not.toContain("unsafe-pane");
  });

  it("rejects invalid secret and expiry inputs", () => {
    expect(() => buildBaselineFingerprint("safe", metadata(), Buffer.alloc(31))).toThrow(HerdrMathError);
    expect(() => buildBaselineFingerprint("safe", metadata(), Buffer.alloc(32), 0)).toThrow(RangeError);
  });
});
