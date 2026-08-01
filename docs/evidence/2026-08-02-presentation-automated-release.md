# Presentation Automated Release Evidence

Date: August 2, 2026

## Scope

This evidence covers commit `2e80d1a` on macOS 26.5.2 arm64 with Node.js 22.21.1 and npm 10.9.4. It refreshes the automated release suite after final-response extraction, transparent response-document rendering, private viewer transport, and bounded automatic scrolling were added.

The final evidence checkout ran every command below once. No test failed, was skipped, or was retried. A separate preliminary checkout used to identify the required workspace permission for npm's read-only package dry run is not part of this evidence run.

## Clean-checkout procedure

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

The checkout remained clean, and the second build did not modify `herdr-plugin.toml`.

## Results

| Gate | Result |
|---|---|
| `npm ci` | 152 packages added, 153 audited, 0 vulnerabilities |
| Browser install and audit | Chromium headless shell 151.0.7922.34 and FFmpeg 1011 installed plugin-locally; audit passed |
| TypeScript, ESLint, Prettier | Passed |
| Manifest validation | Passed |
| Runtime dependency and license audit | Passed |
| Security and artifact scan | 45 runtime files and 283 release files passed |
| Unit suite | 23 files, 192 tests passed |
| Contract suite | 2 files, 10 tests passed |
| Integration suite | 20 files, 128 tests passed |
| Renderer smoke | 1 file, 6 tests passed |
| Complete suite | 43 files, 320 tests passed |
| TypeScript build | Passed before and after the tests |
| Package dry run | 141 entries, 107,245 bytes packed, 550,475 bytes unpacked |

The clean integration performance case remained within every recorded budget. Cold rendering was 3,520.6 ms, warm rendering median was 269.6 ms, maximum boundary resolution was 83.2 ms, and Node RSS growth was 15.5 MiB. The complete-suite performance pass measured a 360.7 ms cold render, 294.8 ms warm median, and 37.2 MiB RSS growth.

## Presentation and privacy coverage

- Claude Code, Codex, Pi, and OpenCode terminal structures pass conclusion-only extraction with reasoning, tool, progress, prompt, and footer exclusion.
- Escaped prose, inline math, and display math pass through the real KaTeX, Chromium, and Sharp backend in source order.
- Renderer tests verify transparent and opaque alpha values plus one inherited prose-and-KaTeX base size.
- Long responses use bounded overlapping crop frames, stop at the bottom, and issue no graphics clear.
- A failed later frame restores the previous final frame when it exists in managed-viewer memory.
- The completion worker and managed viewer exchange only bounded PNG pixels over a source-token-authenticated `0600` local socket.
- Oversized and unauthorized transport requests are rejected, and the socket is the only filesystem entry created by the transport test.
- Durable-state and observable-output sentinel tests continue to exclude response text, LaTeX source, paths, and environment values.

`npm ci` retained the existing optional `fsevents` allow-scripts warning. Installation, native artifact audit, rendering, and all tests succeeded without approving or executing those optional scripts.

## Remaining runtime boundary

This automated evidence does not replace real Herdr and terminal verification. The changed presentation behavior still requires a real Ghostty session for all four coding agents, including transparent background, text sizing, automatic scrolling, final-frame retention, and sanitized screenshots.
