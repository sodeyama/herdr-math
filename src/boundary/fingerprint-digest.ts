import { Buffer } from "node:buffer";
import { createHmac, timingSafeEqual } from "node:crypto";

import { HerdrMathError } from "../core/errors.js";
import { FINGERPRINT_SECRET_BYTES, type FingerprintDigest } from "./fingerprint-schema.js";

export function fingerprintDigest(domain: string, value: string, secret: Uint8Array): FingerprintDigest {
  assertFingerprintSecret(secret);
  return createHmac("sha256", secret).update("herdr-math:v1\0").update(domain).update("\0").update(value).digest("hex");
}

export function fingerprintDigestsEqual(left: string, right: string): boolean {
  if (!/^[a-f0-9]{64}$/.test(left) || !/^[a-f0-9]{64}$/.test(right)) {
    return false;
  }
  return timingSafeEqual(Buffer.from(left, "hex"), Buffer.from(right, "hex"));
}

export function assertFingerprintSecret(secret: Uint8Array): void {
  if (secret.byteLength !== FINGERPRINT_SECRET_BYTES) {
    throw new HerdrMathError("state_corrupt");
  }
}
