# Automated Release Evidence

Date: 2026-08-01

## Scope

This evidence covers commit `fba459c` on macOS 26.5.2 arm64 with Node.js 22.21.1 and npm 10.9.4. It covers
automated P0 unit, contract, integration, render, static, dependency, license, artifact, and clean-build checks.
Real Herdr installation, runtime, terminal, restart, and uninstall cases remain Phase 8 release gates.

## Clean-checkout procedure

Two independent clean checkouts were created at the same commit. The first checkout ran this sequence once,
without a failed, skipped, or retried test:

```sh
npm ci
npm run install:browser
npm run audit:browser
npm run build
npm run check
npm run test:unit
npm run test:contract
npm run test:integration
npm run smoke:render
npm test
npm run build
npm pack --dry-run --json
```

The second checkout ran the four manifest build commands. A recursive comparison found all 114 generated files
byte-equivalent between checkouts. A byte comparison also confirmed that the build did not modify
`herdr-plugin.toml`. Both checkout worktrees remained clean.

## Results

| Gate | Result |
|---|---|
| `npm ci` | 152 packages added, 153 audited, 0 vulnerabilities |
| Browser install | Chromium headless shell 151.0.7922.34 and FFmpeg 1011 installed plugin-locally |
| Browser/runtime license audit | Passed with locked versions, native artifacts, fonts, and retained notices |
| TypeScript, ESLint, Prettier | Passed |
| Manifest validation | Passed against the recorded Herdr 0.7.5 contract |
| Security and artifact scan | 38 runtime files and 228 release files passed |
| Unit suite | 18 files, 149 tests passed |
| Contract suite | 2 files, 10 tests passed |
| Integration suite | 17 files, 104 tests passed |
| Renderer smoke | 1 file, 5 tests passed |
| Complete suite | 35 files, 253 tests passed |
| TypeScript build | Passed before and after the tests |
| Package dry run | 120 entries, 81,274 bytes packed, 402,093 bytes unpacked |
| Reproducible build | 114 generated files matched byte-for-byte across two clean checkouts |

The clean-checkout performance case stayed within every T-704 budget. Its first browser render was 1,682.6 ms,
the warm median was 160.6 ms, maximum boundary resolution was 63.0 ms, and Node RSS growth was 56.0 MiB.

`npm ci` emitted a package-manager notice that the optional `fsevents` install scripts were not allowlisted. One
copy belongs to the production Playwright tree and one to the development Vite tree. Installation, the native
artifact audit, rendering, and all tests succeeded; no script approval or retry was used.

## Automated acceptance boundary

- Static source and generated-output checks cover local paths, secrets, forbidden artifacts, external network
  APIs, dynamic execution, environment serialization, and Herdr-only local socket use.
- Contract and integration tests cover recorded Herdr schema fixtures, four supported coding-agent identities,
  lifecycle ordering, state isolation, rendering, viewer reuse, graphics placement, failures, and cleanup.
- Renderer tests cover real local KaTeX, Chromium, and Sharp output with network denial and bounded images.
- Dependency audits cover lockfile integrity, licenses, fonts, Chromium, FFmpeg, Sharp, and libvips artifacts.
- Install or runtime cases requiring a real Herdr session, terminal graphics, immutable tag, or uninstall operation
  are not represented as passed here.
