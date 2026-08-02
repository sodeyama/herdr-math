import { Buffer } from "node:buffer";
import { access, readFile } from "node:fs/promises";
import { constants } from "node:fs";
import { isAbsolute, resolve } from "node:path";

import { HerdrMathError } from "../core/errors.js";

export const PLUGIN_CONFIG_FILE_NAME = "config.json";
export const MAX_PLUGIN_CONFIG_BYTES = 16 * 1024;
export const MAX_ALLOWED_DIRECTORIES = 32;
export const MAX_ALLOWED_DIRECTORY_BYTES = 4096;

export interface PluginConfig {
  allowedDirectories: readonly string[];
}

const EMPTY_CONFIG: Readonly<PluginConfig> = Object.freeze({ allowedDirectories: Object.freeze([]) });

export async function loadPluginConfig(configDirectory: string): Promise<Readonly<PluginConfig>> {
  const path = resolve(configDirectory, PLUGIN_CONFIG_FILE_NAME);
  try {
    await access(path, constants.R_OK);
  } catch {
    return EMPTY_CONFIG;
  }
  const source = await readFile(path, "utf8");
  if (Buffer.byteLength(source, "utf8") > MAX_PLUGIN_CONFIG_BYTES) {
    throw new HerdrMathError("plugin_config_invalid");
  }
  return parsePluginConfig(JSON.parse(source) as unknown);
}

export function parsePluginConfig(source: unknown): Readonly<PluginConfig> {
  if (!isRecord(source)) throw new HerdrMathError("plugin_config_invalid");
  if (source.allowed_directories === undefined) return EMPTY_CONFIG;
  if (!Array.isArray(source.allowed_directories)) throw new HerdrMathError("plugin_config_invalid");
  if (source.allowed_directories.length > MAX_ALLOWED_DIRECTORIES) {
    throw new HerdrMathError("plugin_config_invalid");
  }
  const allowedDirectories = source.allowed_directories.map(parseAllowedDirectory);
  return Object.freeze({ allowedDirectories: Object.freeze(allowedDirectories) });
}

function parseAllowedDirectory(value: unknown): string {
  if (typeof value !== "string" || value.length === 0 || value.includes("\0")) {
    throw new HerdrMathError("plugin_config_invalid");
  }
  if (Buffer.byteLength(value, "utf8") > MAX_ALLOWED_DIRECTORY_BYTES) {
    throw new HerdrMathError("plugin_config_invalid");
  }
  if (!isAbsolute(value)) throw new HerdrMathError("plugin_config_invalid");
  return resolve(value);
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
