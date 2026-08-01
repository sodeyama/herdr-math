# Ghostty Runtime Evidence

Date: 2026-08-01

## Scope

This evidence covers the Herdr Math implementation through commit `2f909eb`. The plugin was built from a clean
checkout, linked into Herdr, and exercised with four installed coding agents. It records bounded status and pane
metadata only. No prompt, answer, LaTeX source, agent session value, local checkout path, or screenshot is included.

## Environment

- Herdr: 0.7.5, protocol 17
- Herdr Math: 0.1.0 development build
- Ghostty: 1.3.1 stable, Metal renderer
- Platform: macOS 26.5.2 on arm64
- Claude Code: 2.1.220 with Herdr integration v7
- Codex CLI: 0.146.0 with Herdr integration v6
- Pi: 0.83.0 with Herdr integration v6
- OpenCode: 1.18.10 with Herdr integration v9

The clean checkout used the manifest build commands and retained a clean worktree. The linked runtime had one
unrelated local plugin enabled. That plugin and its pane were excluded from Herdr Math ownership counts and were
not modified.

## Coding-agent matrix

| Agent | Canonical id | Lifecycle authority | Observed completion | Boundary result | Render result |
|---|---|---|---|---|---|
| Claude Code | `claude` | Screen detection | `working -> done` | Proven alternate-screen replacement | One owned viewer updated |
| Codex CLI | `codex` | Screen detection | `working -> done` | Proven current-answer boundary | One owned viewer updated |
| Pi | `pi` | Integration hook | `working -> done` | Proven alternate-screen replacement | One owned viewer updated |
| OpenCode | `opencode` | Integration hook | `working -> done` | Proven alternate-screen replacement | One owned viewer updated |

Each real agent produced `baseline_stored` followed by `image_published`. Claude Code, Pi, and OpenCode exposed
alternate-screen replacement behavior that was not represented by the original prototype. The implementation was
updated to fingerprint bounded adjacent gaps, resolve conservative replacement regions, and exclude formulas that
already existed in the baseline gap before this matrix was accepted.

## Viewer and failure matrix

| Case | Runtime result |
|---|---|
| First valid render | Passed for all four real agents |
| Same-viewer replacement | Pi reused the same owned viewer; pane count did not increase |
| Source focus | Source focus was unchanged while creating and updating the viewer |
| Resize | A right split changed from 0.50 to 0.65; the next update used current geometry and reused the viewer |
| No formula | A proven replacement with no formula returned `completion_recorded` and preserved the viewer |
| Invalid LaTeX | Real Pi returned `invalid_latex`; the existing viewer remained present |
| Formula-count limit | A deterministic 21-formula lifecycle in real Herdr returned `scanner_input_limit` and created no viewer |
| Render timeout and recovery | A temporary 1 ms test limit returned `renderer_timeout`; the normal limit was rebuilt and the next render passed |
| Viewer close and recreation | Closing the owned viewer cleared its mapping; the next valid completion created one new owned viewer |
| Graphics disabled | With `kitty_graphics = false`, the worker returned `graphics_disabled` and created no viewer |
| Cell size unavailable | A client-free named server returned `cell_size_unavailable` and created no viewer |

The graphics-disabled setting changed only the existing `kitty_graphics` line. The configuration passed
`herdr config check`, was reloaded for the test, and was restored to `true` immediately afterward.

The first headless cell-size run exposed a client mapping defect: Herdr 0.7.5 returned the stable remote error
`cell_size_unavailable`, while the plugin converted it to `herdr_protocol_error`. Commit `2f909eb` preserves the
Herdr error and adds a socket contract test. The repeated real-server case then returned the expected code.

The formula-count case used a synthetic lifecycle because long real Pi and Codex alternate-screen answers failed
closed at boundary resolution before reaching the scanner. This still exercised the built plugin, real Herdr
socket, event hook, state store, scanner, and no-viewer result. The timeout test changed only generated output in
the temporary clean checkout; `npm run build` restored the production 8,000 ms limit before the recovery case.

## Graphics transport checks

Every successful case completed a real `pane.graphics.set` request. The four owned viewer buffers were then read
as ANSI data and reduced to counts without printing their contents. Each buffer was 24 bytes and contained neither
a Kitty graphics control prefix nor a PNG base64 prefix. This confirms that graphics payloads did not appear as
terminal text in the Herdr pane buffer.

The available Computer Use safety policy did not permit direct inspection of the Ghostty application. Therefore
this evidence does not include or claim a public screenshot. Sanitized screenshot production remains T-903.

## Final validation

```sh
npm run check
npm test
npm run smoke:render
npm run build
```

- Complete suite: 35 files, 273 tests passed
- Clean-checkout renderer smoke: 1 file, 5 tests passed
- Manifest, type, lint, format, runtime dependency, and security checks passed
- The clean linked checkout remained synchronized with `origin/main` and clean

## Acceptance result

- AT-100, AT-107, AT-108, and AT-112 passed for all four real coding agents.
- AT-500 through AT-502, AT-505, AT-507 through AT-509, and AT-511 passed with real Herdr runtime evidence.
- AT-503, AT-504 failure variants, and AT-506 retain their integration evidence; the real invalid-LaTeX case also
  confirmed previous-viewer preservation.
- AT-703 passed through real Ghostty-hosted Herdr graphics requests, focus and resize checks, viewer recreation,
  invalid preservation, restart recovery recorded separately, and pane-buffer leakage checks.

