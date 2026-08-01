import { randomBytes, randomUUID } from "node:crypto";
import { chmod, link, lstat, mkdir, open, readFile, unlink } from "node:fs/promises";
import { join } from "node:path";

import { HerdrMathError } from "../core/errors.js";
import { FINGERPRINT_SECRET_BYTES } from "./fingerprint-schema.js";

const DIRECTORY_MODE = 0o700;
const SECRET_MODE = 0o600;

export async function loadOrCreateFingerprintSecret(stateDirectory: string): Promise<Buffer> {
  const versionDirectory = join(stateDirectory, "v1");
  const secretPath = join(versionDirectory, "secret");
  await mkdir(versionDirectory, { recursive: true, mode: DIRECTORY_MODE });
  await chmod(versionDirectory, DIRECTORY_MODE);

  const existing = await readSecretIfPresent(secretPath);
  if (existing !== undefined) {
    return existing;
  }

  const temporaryPath = join(versionDirectory, `.secret-${process.pid}-${randomUUID()}.tmp`);
  const candidate = randomBytes(FINGERPRINT_SECRET_BYTES);
  const handle = await open(temporaryPath, "wx", SECRET_MODE);
  try {
    await handle.writeFile(candidate);
    await handle.sync();
  } finally {
    await handle.close();
  }

  try {
    await link(temporaryPath, secretPath);
  } catch (error: unknown) {
    if (!isNodeError(error, "EEXIST")) {
      throw error;
    }
  } finally {
    await unlink(temporaryPath).catch((error: unknown) => {
      if (!isNodeError(error, "ENOENT")) {
        throw error;
      }
    });
  }

  const secret = await readSecretIfPresent(secretPath);
  if (secret === undefined) {
    throw new HerdrMathError("state_corrupt");
  }
  return secret;
}

async function readSecretIfPresent(secretPath: string): Promise<Buffer | undefined> {
  let metadata;
  try {
    metadata = await lstat(secretPath);
  } catch (error: unknown) {
    if (isNodeError(error, "ENOENT")) {
      return undefined;
    }
    throw error;
  }

  if (!metadata.isFile() || metadata.isSymbolicLink() || metadata.size !== FINGERPRINT_SECRET_BYTES) {
    throw new HerdrMathError("state_corrupt");
  }
  await chmod(secretPath, SECRET_MODE);
  const secret = await readFile(secretPath);
  if (secret.byteLength !== FINGERPRINT_SECRET_BYTES) {
    throw new HerdrMathError("state_corrupt");
  }
  return secret;
}

function isNodeError(error: unknown, code: string): error is NodeJS.ErrnoException {
  return error instanceof Error && "code" in error && error.code === code;
}
