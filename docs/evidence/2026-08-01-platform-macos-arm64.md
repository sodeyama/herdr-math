# macOS arm64 Release-Candidate Evidence

Date: 2026-08-01

## Declared scope

`herdr-plugin.toml` declares only `macos`. This release-candidate check covers macOS 26.5.2 on arm64. It does not
verify or claim macOS x64, Linux, or Windows support.

## Fresh installation

A clean checkout synchronized with `origin/main` ran the manifest installation path:

```sh
npm ci
npm run audit:browser
npm run build
npm run smoke:render
```

`npm ci` installed 152 packages, audited 153 packages, and reported zero vulnerabilities. The postinstall step
downloaded the locked Chromium headless shell 151.0.7922.34 and FFmpeg 1011 macOS arm64 artifacts. The browser and
runtime license audit passed. The clean worktree remained unchanged.

The package manager reported two unapproved optional `fsevents` install scripts. No script approval was granted.
Fresh installation, native loading, audit, build, and rendering all passed without them.

## Native architecture

The installed runtime artifacts were inspected directly:

| Artifact | Result |
|---|---|
| Sharp 0.35.3 native module | Mach-O 64-bit bundle, arm64 |
| libvips 8.18.3 shared library | Mach-O 64-bit shared library, arm64 |
| Chromium headless shell | Mach-O 64-bit executable, arm64 |

Locked top-level runtime packages were KaTeX 0.18.1, Playwright 1.62.1, and Sharp 0.35.3.

## Render and runtime result

The renderer smoke suite passed all five cases after the fresh install, including the release corpus, malformed
input, policy limits, timeout recovery, and image limits. The first cold render completed in 1,997 ms.

The same macOS arm64 environment also passed the real Herdr and Ghostty coding-agent, viewer, resize, error,
graphics-disabled, session-isolation, stale-lock, and restart matrices recorded in the adjacent runtime evidence.

## Acceptance result

- AT-700 passed for the declared macOS arm64 release candidate.
- macOS x64 remains unverified and is not described as supported.
- AT-701 and AT-702 are not applicable because Linux and Windows are absent from the manifest.

