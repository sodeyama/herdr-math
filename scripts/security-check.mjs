import { readFile, readdir } from "node:fs/promises";
import { extname, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(fileURLToPath(new URL("..", import.meta.url)));
// `.cursor` holds editor-local debug artifacts that are gitignored and can
// never be committed; scanning them only makes the gate fail on developer
// machines while CI's clean checkout never sees them.
const ignoredDirectories = new Set([".git", "node_modules", "coverage", "target", ".cursor"]);
// Same reasoning for per-developer local settings: gitignored by design and
// expected to contain absolute paths on the developer's machine.
const localOnlyFiles = new Set([".claude/settings.local.json"]);
const textExtensions = new Set([".css", ".html", ".js", ".json", ".md", ".mjs", ".rs", ".swift", ".toml", ".ts", ".txt"]);
const textNames = new Set([".editorconfig", ".gitignore", "LICENSE"]);
const allowedEnvironmentKeys = new Set();

const runtimeRules = [
  ["child_process", /(?:node:)?child_process/u],
  ["network_module", /(?:from|import\s*\()\s*["'](?:node:)?(?:dgram|dns|http|http2|https|tls)["']/u],
  ["network_api", /\b(?:EventSource|WebSocket|XMLHttpRequest|fetch)\s*\(/u],
  ["dynamic_import", /\bimport\s*\((?!\s*["'][^"']+["']\s*\))/u],
  ["eval", /\beval\s*\(|\bnew\s+Function\s*\(/u],
  ["shell_execution", /\b(?:execFile|execFileSync|execSync|fork|spawn|spawnSync)\s*\(/u],
  ["environment_spread", /\.\.\.\s*process\.env\b/u],
  ["environment_iteration", /Object\.(?:entries|keys|values)\s*\(\s*process\.env\s*\)/u],
  ["environment_bracket_access", /process\.env\s*\[/u],
  ["environment_serialization", /JSON\.stringify\s*\(\s*process\.env\b/u],
  ["remote_url", /["'`]https?:\/\//u],
  ["local_home_path", /["'`]\/(?:Users|home)\//u]
];

const repositoryRules = [
  ["macos_home_path", new RegExp(String.raw`\/Users\/[A-Za-z0-9._-]+\/`, "u")],
  ["linux_home_path", new RegExp(String.raw`\/home\/[A-Za-z0-9._-]+\/`, "u")],
  ["windows_home_path", new RegExp(String.raw`[A-Za-z]:\\Users\\[^\\\s]+\\`, "u")],
  ["aws_access_key", new RegExp(`\\b${"AK" + "IA"}[0-9A-Z]{16}\\b`, "u")],
  [
    "github_token",
    new RegExp(`\\b(?:${"github" + "_pat_"}[A-Za-z0-9_]{30,}|${"gh" + "[pousr]_"}[A-Za-z0-9]{30,})\\b`, "u")
  ],
  ["google_api_key", new RegExp(`\\b${"AI" + "za"}[A-Za-z0-9_-]{35}\\b`, "u")],
  ["slack_token", new RegExp(`\\b${"xo" + "x[baprs]-"}[A-Za-z0-9-]{20,}\\b`, "u")],
  ["stripe_live_key", new RegExp(`\\b${"sk" + "_live_"}[A-Za-z0-9]{20,}\\b`, "u")],
  ["private_key", new RegExp(`-----${"BEGIN"} (?:RSA |OPENSSH |EC )?PRIVATE KEY-----`, "u")]
];

const forbiddenArtifactNames = [/^\.env(?:\..+)?$/u, /\.(?:lock|log|tmp|tgz)$/u, /^\.DS_Store$/u];
const committedLockfiles = new Set(["Cargo.lock"]);
const violations = [];
const files = await collectFiles(root);
const runtimeFiles = files.filter((path) => path.startsWith("src/") && path.endsWith(".ts"));

for (const path of files) {
  if (localOnlyFiles.has(path)) continue;
  const name = path.split("/").at(-1) ?? path;
  if (name !== ".env.example" && !committedLockfiles.has(name) && forbiddenArtifactNames.some((pattern) => pattern.test(name))) {
    violations.push(`${path}: forbidden generated or local artifact`);
  }
  if (!textExtensions.has(extname(path)) && !textNames.has(name)) continue;
  const source = await readFile(resolve(root, path), "utf8");
  for (const [code, pattern] of repositoryRules) {
    if (pattern.test(source)) violations.push(`${path}: ${code}`);
  }
}

for (const path of runtimeFiles) {
  const source = await readFile(resolve(root, path), "utf8");
  for (const [code, pattern] of runtimeRules) {
    if (pattern.test(source)) violations.push(`${path}: ${code}`);
  }
  for (const match of source.matchAll(/process\.env\.([A-Z0-9_]+)/gu)) {
    const key = match[1];
    if (key === undefined || !allowedEnvironmentKeys.has(key)) violations.push(`${path}: environment_key`);
  }
  for (const match of source.matchAll(/executablePath\s*:\s*([A-Za-z0-9_.$]+)/gu)) {
    if (match[1] !== "BROWSER_EXECUTABLE_PATH") violations.push(`${path}: executable_path_input`);
  }
  if (source.includes('from "node:net"')) {
    violations.push(`${path}: network_socket_import`);
  }
}

if (violations.length > 0) {
  for (const violation of violations.sort()) process.stderr.write(`security gate: ${violation}\n`);
  process.exitCode = 1;
} else {
  process.stdout.write(`Security gates passed: ${runtimeFiles.length} runtime files and ${files.length} release files.\n`);
}

async function collectFiles(directory) {
  const collected = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    if (entry.isDirectory() && ignoredDirectories.has(entry.name)) continue;
    const absolute = resolve(directory, entry.name);
    const path = relative(root, absolute).split("\\").join("/");
    if (entry.isDirectory() && entry.name === ".herdr-math") {
      violations.push(`${path}: local_runtime_directory`);
      continue;
    }
    if (entry.isSymbolicLink()) {
      violations.push(`${path}: symbolic_link_artifact`);
      continue;
    }
    if (entry.isDirectory()) collected.push(...(await collectFiles(absolute)));
    else if (entry.isFile()) collected.push(path);
  }
  return collected;
}
