import { createReadStream, constants, existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import { access } from "node:fs/promises";
import { createHash } from "node:crypto";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const packageJson = readJson("package.json");
const lock = readJson("package-lock.json");
const expectedDirect = { katex: "0.18.1", playwright: "1.62.1", sharp: "0.35.3", "markdown-it": "14.3.0", "highlight.js": "11.11.1" };
const allowedLicenses = new Set([
  "0BSD",
  "Apache-2.0",
  "Apache-2.0 AND LGPL-3.0-or-later",
  "Apache-2.0 AND LGPL-3.0-or-later AND MIT",
  "BSD-2-Clause",
  "BSD-3-Clause",
  "ISC",
  "LGPL-3.0-or-later",
  "MIT",
  "Python-2.0"
]);
const expectedLockEntries = [
  ["node_modules/katex", "0.18.1", "MIT"],
  ["node_modules/commander", "8.3.0", "MIT"],
  ["node_modules/playwright", "1.62.1", "Apache-2.0"],
  ["node_modules/playwright-core", "1.62.1", "Apache-2.0"],
  ["node_modules/playwright/node_modules/fsevents", "2.3.2", "MIT"],
  ["node_modules/sharp", "0.35.3", "Apache-2.0"],
  ["node_modules/@img/colour", "1.1.0", "MIT"],
  ["node_modules/detect-libc", "2.1.2", "Apache-2.0"],
  ["node_modules/semver", "7.8.5", "ISC"],
  ["node_modules/@img/sharp-darwin-arm64", "0.35.3", "Apache-2.0"],
  ["node_modules/@img/sharp-darwin-x64", "0.35.3", "Apache-2.0"],
  ["node_modules/@img/sharp-libvips-darwin-arm64", "1.3.2", "LGPL-3.0-or-later"],
  ["node_modules/@img/sharp-libvips-darwin-x64", "1.3.2", "LGPL-3.0-or-later"],
  ["node_modules/markdown-it", "14.3.0", "MIT"],
  ["node_modules/highlight.js", "11.11.1", "BSD-3-Clause"],
  ["node_modules/argparse", "2.0.1", "Python-2.0"],
  ["node_modules/entities", "4.5.0", "BSD-2-Clause"],
  ["node_modules/linkify-it", "5.0.2", "MIT"],
  ["node_modules/mdurl", "2.1.0", "MIT"],
  ["node_modules/punycode.js", "2.3.1", "MIT"],
  ["node_modules/uc.micro", "2.1.0", "MIT"]
];
const requiredInstalledNotices = [
  "node_modules/katex/LICENSE",
  "node_modules/commander/LICENSE",
  "node_modules/playwright/LICENSE",
  "node_modules/playwright/NOTICE",
  "node_modules/playwright-core/LICENSE",
  "node_modules/playwright-core/NOTICE",
  "node_modules/sharp/LICENSE",
  "node_modules/@img/colour/LICENSE.md",
  "node_modules/detect-libc/LICENSE",
  "node_modules/semver/LICENSE",
  "node_modules/playwright/node_modules/fsevents/LICENSE",
  "node_modules/markdown-it/LICENSE",
  "node_modules/highlight.js/LICENSE"
];
try {
  auditLockfile();
  auditInstalledPackages();
  auditFontsAndNotices();
  if (process.argv.includes("--browser")) await auditCurrentRuntime();
  process.stdout.write("Runtime dependency audit passed.\n");
} catch (error) {
  const message = error instanceof AuditError ? error.message : "unexpected audit failure";
  process.stderr.write(`Runtime dependency audit failed: ${message}.\n`);
  process.exitCode = 1;
}

function auditLockfile() {
  assert(equalRecords(packageJson.dependencies, expectedDirect), "direct dependency versions changed");
  assert(equalRecords(lock.packages?.[""]?.dependencies, expectedDirect), "lockfile root dependencies changed");

  const source = readText("src/renderer/browser-backend.ts");
  const markdownSource = readText("src/renderer/markdown.ts");
  for (const name of Object.keys(expectedDirect)) {
    assert(
      source.includes(`from "${name}"`) || markdownSource.includes(`from "${name}"`),
      `direct dependency is unused: ${name}`
    );
  }

  for (const [path, entry] of Object.entries(lock.packages ?? {})) {
    if (path === "") continue;
    if (entry.resolved !== undefined) {
      assert(entry.resolved.startsWith("https://registry.npmjs.org/"), "lockfile uses a non-registry artifact");
      assert(
        typeof entry.integrity === "string" && entry.integrity.startsWith("sha512-"),
        "lockfile integrity is missing"
      );
    }
    for (const group of [entry.dependencies, entry.optionalDependencies]) {
      for (const value of Object.values(group ?? {})) {
        assert(!/^(?:file:|git|git\+|https?:|github:)/.test(value), "lockfile uses an external dependency specifier");
      }
    }
    if (entry.dev !== true) assert(allowedLicenses.has(entry.license), "production license metadata changed");
  }

  for (const [path, version, license] of expectedLockEntries) {
    const entry = lock.packages?.[path];
    assert(entry?.version === version && entry.license === license, `locked package metadata changed: ${path}`);
  }
}

function auditInstalledPackages() {
  for (const [name, version] of Object.entries(expectedDirect)) {
    const metadata = readJson(`node_modules/${name}/package.json`);
    assert(metadata.version === version, `installed package version changed: ${name}`);
  }
  for (const path of requiredInstalledNotices) assert(nonEmpty(path), "an installed license or notice is missing");

  const playwrightNotice = readText("node_modules/playwright/NOTICE");
  assert(playwrightNotice.includes("Puppeteer project"), "Playwright notice content changed");
}

function auditFontsAndNotices() {
  const css = readText("node_modules/katex/dist/katex.min.css");
  const referenced = new Set([...css.matchAll(/url\(fonts\/([^)'\"]+)/g)].map((match) => match[1]));
  const installed = new Set(readdirSync(join(root, "node_modules/katex/dist/fonts")));
  assert(referenced.size === 60 && installed.size === 60, "KaTeX font inventory changed");
  for (const font of referenced) assert(installed.has(font), "a referenced KaTeX font is missing");

  const notices = readText("THIRD_PARTY_NOTICES.md");
  for (const name of ["KaTeX", "Playwright", "Chromium", "FFmpeg", "Sharp", "libvips", "markdown-it", "highlight.js"]) {
    assert(notices.includes(name), `third-party notice is incomplete: ${name}`);
  }
}

async function auditCurrentRuntime() {
  assert(process.platform === "darwin", "browser artifact audit is currently release-gated to macOS");
  assert(process.arch === "arm64" || process.arch === "x64", "unsupported macOS architecture");

  const sharp = await import("sharp");
  assert(sharp.default.versions.sharp === "0.35.3", "Sharp native runtime version changed");
  assert(sharp.default.versions.vips === "8.18.3", "libvips native runtime version changed");
  assert(nonEmpty(`node_modules/@img/sharp-darwin-${process.arch}/LICENSE`), "Sharp native license is missing");
  assert(
    readText(`node_modules/@img/sharp-libvips-darwin-${process.arch}/README.md`).includes("## Licensing"),
    "libvips license inventory is missing"
  );
  const sharpAddon = join(
    root,
    `node_modules/@img/sharp-darwin-${process.arch}/lib/sharp-darwin-${process.arch}-0.35.3.node`
  );
  const libvips = join(root, `node_modules/@img/sharp-libvips-darwin-${process.arch}/lib/libvips-cpp.8.18.3.dylib`);
  assert(existsSync(sharpAddon) && existsSync(libvips), "Sharp native artifact is missing");

  const browsers = readJson("node_modules/playwright-core/browsers.json");
  const descriptor = browsers.browsers?.find((entry) => entry.name === "chromium-headless-shell");
  assert(descriptor?.revision === "1234" && descriptor.browserVersion === "151.0.7922.34", "browser revision changed");
  const ffmpegDescriptor = browsers.browsers?.find((entry) => entry.name === "ffmpeg");
  assert(ffmpegDescriptor?.revision === "1011", "FFmpeg revision changed");
  const artifact = join(browserRoot(), `chromium_headless_shell-${descriptor.revision}`);
  const executable = findFile(artifact, new Set(["chrome-headless-shell"]));
  const license = findFile(artifact, new Set(["LICENSE.headless_shell"]));
  assert(executable !== undefined && license !== undefined, "browser executable or license is missing");
  await access(executable, constants.X_OK);
  assert(statSync(license).size > 500_000, "Chromium bundled license inventory is incomplete");
  assert(readFileSync(license, "utf8").includes("The Chromium Project"), "Chromium license content changed");

  const ffmpegArtifact = join(browserRoot(), `ffmpeg-${ffmpegDescriptor.revision}`);
  const ffmpeg = findFile(ffmpegArtifact, new Set(["ffmpeg-mac"]));
  const ffmpegLicense = findFile(ffmpegArtifact, new Set(["COPYING.LGPLv2.1"]));
  assert(ffmpeg !== undefined && ffmpegLicense !== undefined, "FFmpeg executable or license is missing");
  await access(ffmpeg, constants.X_OK);
  assert(readFileSync(ffmpegLicense, "utf8").includes("GNU LESSER GENERAL PUBLIC LICENSE"), "FFmpeg license changed");

  if (process.arch === "arm64") {
    const expectedHashes = new Map([
      [executable, "7687bff7cb2db075f250e6d5848bbc8838cac3802ac3952a899c574f8eccab45"],
      [license, "8e19f44970a51cb101004127a567f78f8efada271dcb2a16b4b5c4d0cc76a4cd"],
      [ffmpeg, "662398c99429f5493ff0ff40cdcd97ea25132addcbdedafd0741389964147825"],
      [ffmpegLicense, "b634ab5640e258563c536e658cad87080553df6f34f62269a21d554844e58bfe"],
      [sharpAddon, "5efbf349396808af22ae4f6b67c327767297a97b54a1ecf04dedec63aeff4d46"],
      [libvips, "50090a3a7c8f455de3c6cf2b274d231ddcc92901f4b1957c3b3391b6b9877989"]
    ]);
    for (const [path, expected] of expectedHashes) {
      assert((await sha256(path)) === expected, "macOS arm64 runtime artifact hash changed");
    }
  }
}

function browserRoot() {
  return join(root, "node_modules/playwright-core/.local-browsers");
}

function findFile(directory, names, depth = 3) {
  if (depth < 0 || !existsSync(directory)) return undefined;
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.isFile() && names.has(entry.name)) return path;
    if (entry.isDirectory()) {
      const nested = findFile(path, names, depth - 1);
      if (nested !== undefined) return nested;
    }
  }
  return undefined;
}

async function sha256(path) {
  const hash = createHash("sha256");
  for await (const chunk of createReadStream(path)) hash.update(chunk);
  return hash.digest("hex");
}

function readJson(path) {
  try {
    return JSON.parse(readText(path));
  } catch {
    throw new AuditError(`invalid JSON: ${path}`);
  }
}

function readText(path) {
  const target = join(root, path);
  if (!existsSync(target)) throw new AuditError(`required file is missing: ${path}`);
  return readFileSync(target, "utf8");
}

function nonEmpty(path) {
  const target = join(root, path);
  return existsSync(target) && statSync(target).isFile() && statSync(target).size > 0;
}

function equalRecords(actual, expected) {
  return JSON.stringify(Object.entries(actual ?? {}).sort()) === JSON.stringify(Object.entries(expected).sort());
}

function assert(condition, message) {
  if (!condition) throw new AuditError(message);
}

class AuditError extends Error {}
