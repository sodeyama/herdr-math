# LaTeX Viewer Prototype Experiment Report

## Report Metadata

- Experiment date: August 1, 2026
- Reported environment: macOS
- Herdr version: 0.7.5
- Outer terminal: Ghostty 1.3.1
- Prototype name: `local.herdr-latex`
- Public successor: Herdr Math
- Overall result: **GO for a split-pane product; prototype code requires release redesign**

## Purpose

The experiment tested whether a Herdr plugin could automatically detect LaTeX equations in a completed Claude Code or Codex answer, render them locally, and display them in a reusable side pane.

The primary unknowns were not LaTeX parsing alone. The experiment had to prove the full terminal path:

1. Image transport through the outer terminal and Herdr
2. Image placement in a plugin-managed pane
3. Replacement of an existing image without duplicate panes
4. Reliable detection of the current agent answer
5. Safe handling of shell variables, prices, malformed math, limits, and restarts

## Initial Alternatives

Three product shapes were considered:

- Emit a Kitty graphics image directly over the agent pane
- Open a Herdr plugin pane as a split or overlay
- Use a Ghostty Quick Terminal or Ghostty-specific automation

The Herdr plugin pane was selected because it keeps the source transcript untouched, gives the image an explicit owner, supports pane lifecycle operations, and avoids a direct Ghostty application dependency.

The chosen prototype placement was a right split. Overlay and popup evaluation was deliberately moved outside the MVP gate.

## Prototype Architecture

The experiment used this runtime flow:

```text
long-running local controller
  -> subscribe to pane.agent_status_changed
  -> capture a working-state pane snapshot in memory
  -> read the pane again at done or idle
  -> compute the answer delta
  -> scan $...$ and $$...$$
  -> render with KaTeX + Playwright + Sharp
  -> open or reuse a plugin viewer split
  -> update it with pane.graphics.set
```

This architecture was appropriate for proving the mechanism. It is not the target public architecture because current Herdr documentation defines startup hooks as one-shot initialization commands rather than supervised daemons. The release plan replaces the controller with manifest event hooks and short-lived workers.

## Experiment Plan

The work was divided into gated phases:

| Phase | Purpose | MVP gate |
|---|---|---|
| 0 | Verify Herdr graphics configuration and client cell size | Yes |
| 1 | Isolate image transport across three paths | Yes |
| 2 | Prove LaTeX rendering and same-pane image replacement | Yes |
| 3 | Prove lifecycle events and current-answer boundaries | Yes |
| 4 | Join the pieces into an automatic split-pane MVP | Yes |
| 5 | Exercise invalid input, limits, concurrency, and restart behavior | Hardening gate |
| 6 | Compare popup and overlay placement | No |

Phase 6 was not performed.

## Phase 0: Graphics Capability

### Setup

Herdr's experimental graphics flag was enabled:

```toml
[experimental]
kitty_graphics = true
```

The test called `pane.graphics.info` for a specific target pane rather than relying on a global capability assumption.

### Observed sequence

| State | Result |
|---|---|
| Before enabling the flag | `feature_disabled` |
| After config reload with the existing client | `cell_size_unavailable` |
| After attaching a new Herdr client | `cell_width_px=7`, `cell_height_px=15` |

### Finding

A server configuration reload was not sufficient for the already attached client to provide usable pixel cell dimensions. Reattaching the Herdr client produced valid dimensions.

This established two separate diagnostics for the public plugin:

- `graphics_disabled`: the feature flag is off
- `cell_size_unavailable`: graphics are enabled, but the attached client cannot currently provide usable pixel dimensions

### Result

**Pass.** The required graphics API was available after the client was reattached.

## Phase 1: Image Transport Isolation

### Method

One known PNG fixture was used for every path. LaTeX generation was intentionally excluded so a failure could be attributed to image transport rather than rendering.

Fixture properties:

- Dimensions: `480 x 220`
- PNG bytes: `10,966`
- Base64 bytes: `14,624`

### Paths tested

| Path | Result | Observation |
|---|---|---|
| Raw Kitty sequence in bare Ghostty | Pass | Image displayed; escape text did not leak |
| Raw Kitty sequence in a normal Herdr shell pane | Pass | Image displayed in a temporary split |
| Raw Kitty sequence in a Herdr plugin split | Pass | Image displayed from a fixture entrypoint |
| Herdr `pane.graphics.set` | Pass | The same fixture was placed in the requested pane |

### Finding

The complete path from Herdr to the outer terminal could carry an image. The raw Kitty sequence tests were diagnostic probes; they did not need to remain in the production runtime.

The higher-level `pane.graphics.set` API was selected for the application path because it gives Herdr ownership of the layer and allows a controller or event worker to replace it by pane id.

### Result

**Pass.** Image transport was not a blocker.

## Phase 2: Rendering and Viewer Replacement

### Renderer used

The prototype rendered formulas through:

```text
KaTeX HTML
  -> headless Playwright page
  -> element screenshot
  -> Sharp PNG optimization
```

The renderer used local assets and KaTeX `trust: false`.

### Cases

1. Render `E=mc^2` and display it in the viewer.
2. Render `a^2+b^2=c^2` and update the same viewer pane.
3. Resize the pane layout.
4. Verify source focus.
5. Close the viewer and trigger another valid formula.

### Observations

- `pane.graphics.set` alone replaced the existing graphics layer.
- A preceding `pane.graphics.clear` was unnecessary.
- The old image did not remain behind the new image.
- Opening the viewer with focus disabled preserved the source pane focus.
- Resizing from an approximately 50:50 split to 58:42 retained the image.
- Closing the viewer caused the next valid answer to create exactly one replacement viewer.

Recorded examples:

| Case | PNG bytes | Base64 bytes |
|---|---:|---:|
| Renderer smoke | 4,666 | 6,224 |
| Automatic multi-formula update | 4,244 | 5,660 |
| Update after viewer recreation | 1,575 | 2,100 |

### Decision

Use controller-owned or worker-owned `pane.graphics.set` replacement. Do not make the viewer poll an image file, and do not clear the old image before a validated replacement is ready.

### Result

**Pass.** One reusable split could display and replace rendered formulas without focus loss.

## Phase 3: Lifecycle and Answer Boundary

### Event model

The controller subscribed to `pane.agent_status_changed` for detected Claude Code and Codex panes.

Observed status values were handled distinctly:

- `working`: capture baseline
- `blocked`: preserve the active lifecycle
- `done`: schedule completion processing
- `idle`: schedule completion processing
- `unknown`: do not render

`done` and `idle` were not assumed to be identical events. A 500 ms debounce and a processed-content key prevented duplicate rendering.

### Five core answers

| Input class | Observed result |
|---|---|
| `$E=mc^2$` | Viewer opened and rendered the formula |
| No formula | `answer_without_formula`; no update |
| `$HOME` inside a fenced code block | No formula |
| Inline plus display equations | Same viewer updated with two equations |
| Unclosed `$HOME` text | No formula |

### Herdr read behavior

The prototype found two important Herdr 0.7.5 behaviors:

- `pane.read` returned a fixed `revision=0` and `truncated=false` in the tested path, so a content hash was required for identity.
- `recent_unwrapped` omitted visible content in a tested scenario, while `recent` with a maximum of 1,000 lines provided the necessary data.

Reaching the 1,000-line limit was therefore treated conservatively as possible truncation even when the API flag was false.

### Boundary strategies

The prototype implemented four strategies:

1. Exact baseline prefix
2. Stable common prefix
3. Suffix-to-prefix sliding-window overlap
4. Tail anchor with preceding-context comparison

A 1,005-line fixture forced a sliding read window. The controller recovered the boundary and rendered only the new formula, recording `truncated_boundary_recovered`.

When no relationship could be proven, the prototype returned `boundary_failed` or `answer_truncated` and did not render.

### Result

**Pass with a required redesign.** The boundary algorithms worked, but the prototype kept the raw baseline in controller memory. The public event-hook architecture must persist only fingerprints across one-shot workers.

## Phase 4: Automatic End-to-End MVP

The components were joined into an automatic flow:

- Detect Claude Code and Codex panes
- Subscribe to their lifecycle events
- Capture a working baseline
- Process stable done or idle content once
- Scan only the current answer
- Render formulas locally
- Open a right split with focus disabled
- Reuse that viewer for later valid answers
- Recreate the viewer after manual closure

The scanner recognized `$...$` and `$$...$$` and ignored fenced code, inline code, escaped dollar signs, and unclosed delimiters.

The prototype also used a process lock to prevent multiple controllers from running in the same Herdr session.

### Result

**Pass.** The split-pane MVP worked automatically in a real Herdr session.

## Phase 5: Hardening

### Safety limits

The prototype enforced:

| Boundary | Value |
|---|---:|
| Formulas per answer | 20 |
| Characters per formula | 2,000 |
| Aggregate formula characters | 10,000 |
| Renderer timeout | 8 seconds |
| Raw PNG payload | 512 KiB |
| Recent pane read | 1,000 lines |

It did not execute shell commands or a TeX binary. It logged hashes, counts, pane ids, dimensions, and payload sizes rather than answer or formula text.

### Hardening matrix

| Case | Expected behavior | Observed result |
|---|---|---|
| Invalid LaTeX | Preserve previous image and continue | `invalid_latex`; previous image remained |
| 21 formulas | Reject and do not create a split | `renderer_input_limit`; pane count unchanged |
| 2,001-character formula | Reject and do not create a split | `renderer_input_limit`; pane count unchanged |
| Multiline display math | Render normally | Rendered after preceding error cases |
| `$10 and $20` | Do not treat as math | No formula |
| `$HOME and $PATH` | Do not treat as math | No formula |
| Closed `$1$` | Preserve as valid math | Scanner unit test passed |
| Forced renderer timeout | Classify timeout and recover | `renderer_timeout`; next valid render passed |
| PNG of 512 KiB plus one byte | Reject before replacement | `image_too_large`; previous image remained |
| Pane resize | Keep image | Image remained after layout change |
| Viewer close | Recreate one viewer on demand | Exactly one new viewer created |
| 1,005-line output | Recover only with a proven boundary | Current formula rendered once |
| Server restart | Recover automatic processing | Controller restarted in isolated session |

The oversized-image test used:

- Raw bytes: `524,289`
- Base64 bytes: `699,052`

The result reinforced that raw PNG and encoded payload limits must be treated separately.

### Repeated-prompt bug

The first tail-anchor implementation selected the last occurrence of an anchor. Shell prompts often repeat after a command, so the algorithm could select the post-answer prompt and skip the new formula.

The fix evaluated every candidate anchor occurrence and compared its preceding context with the baseline context. The best contextual match was selected, with the earlier occurrence winning a tie.

Regression tests covered:

- A repeated prompt after the answer
- A baseline that contains only the prompt
- A changed dynamic line after a stable anchor

This bug is the reason the public design forbids prompt-only heuristics and requires context-qualified fingerprints.

### Restart isolation

Restart testing was performed in a named Herdr session so the active default agent session would not be stopped.

The sequence verified:

1. A startup command created a controller in the isolated session.
2. Stopping the isolated server terminated its child process.
3. A lock file could remain after server termination.
4. Restarting the session detected the stale lock and created a new controller.
5. Removing the isolated session did not affect the default session controller.

This validated session-specific state and stale-lock recovery. It did not validate the current official recommendation that startup hooks be one-shot; that lifecycle is corrected in the target architecture.

### Result

**Pass for prototype hardening.** All required error cases preserved controller availability and viewer ownership.

## Automated Validation

Final prototype checks reported:

```text
Unit tests:        15 / 15 passed
Renderer smoke:   444 x 412, 4,666 bytes
Hardening smoke:  7 / 7 cases passed
Syntax checks:    all source, script, and test modules passed
```

The hardening smoke covered successful rendering, invalid LaTeX, formula-count limit, formula-length limit, multiline and multiple equations, timeout classification, and recovery after timeout.

## Security Observation

An early diagnostic command printed a process environment into local tool output, which can expose credentials unrelated to the plugin. The temporary diagnostic log was immediately truncated, and credential rotation was advised.

No credential values belong in this repository or report.

The public implementation must never dump full environments or arbitrary error objects. Diagnostics must use an allowlist of non-sensitive Herdr variables and redact paths where they are not necessary.

## What the Experiment Verified

- Herdr can display PNG data in a plugin-managed split through `pane.graphics.set`.
- A Ghostty-hosted Herdr client carried the tested graphics correctly.
- One viewer can be reused and updated without focus loss.
- The viewer can be recreated after manual closure.
- Current-answer detection can survive normal repaint changes and a sliding read window.
- A stateful scanner can reject the tested code, shell-variable, price, escape, and delimiter ambiguities.
- Renderer errors and size limits can preserve the previous valid image.
- Session-specific locks can recover after an isolated server restart.

## What the Experiment Did Not Verify

- Public GitHub installation through `herdr plugin install`
- Manifest build commands in a clean managed checkout
- A self-contained dependency graph
- Current official startup-hook lifecycle compliance
- One-shot event-hook concurrency
- Baseline fingerprinting without persisted raw text
- Linux, Windows, Intel macOS, or multiple CPU architectures
- Kitty, WezTerm, iTerm2, Alacritty, or other outer terminals
- Remote Herdr attach or direct agent attach
- Popup or overlay placement
- Accessibility behavior
- Theme adaptation
- Long-running production resource usage

## Prototype Gaps That Block Direct Release

The prototype cannot be copied directly into the public repository because it contained release-specific gaps:

1. It imported KaTeX from another project's `node_modules` directory.
2. Its package manifest did not declare all runtime dependencies.
3. Some development paths contained a local username and absolute home-directory paths.
4. The manifest used a local plugin id and included diagnostic fixture panes.
5. It used a long-running controller from a startup hook.
6. It had no clean GitHub install or build-command test.
7. It declared macOS and documented Ghostty as a prerequisite without separating direct dependency from verified terminal evidence.
8. It kept its working baseline as raw text in controller memory, which does not transfer directly to one-shot workers.
9. It had no public license, contribution guide, security policy, changelog, or release automation.

## Decisions Carried Into Herdr Math

- Keep the split-pane product shape.
- Use `pane.graphics.set` replacement without clearing first.
- Keep source focus unchanged.
- Keep one viewer per source pane.
- Preserve the existing image on all new-render failures.
- Use a stateful conservative scanner.
- Treat 1,000-line reads as potentially truncated.
- Keep exact-prefix, stable-prefix, sliding-window, and contextual-anchor strategies.
- Keep the initial count, length, timeout, and image-size limits until a release test justifies a change.
- Keep logs free of answer and formula text.
- Test restarts in an isolated named session.

## Decisions Replaced for the Public Release

- Replace the startup daemon with one-shot event workers.
- Replace raw baseline persistence with boundary fingerprints.
- Replace external dependency imports with repository-owned locked dependencies.
- Replace hard-coded paths with authoritative `HERDR_*` environment variables.
- Remove raw Kitty fixture entrypoints from the production manifest.
- Replace the local plugin id with `io.github.sodeyama.herdr-math`.
- Replace "Ghostty required" with an evidence-based terminal compatibility matrix.
- Add clean-install, package, license, security, and release gates.

## Evidence Retention

The private prototype produced screenshots for phases 1, 2, 4, and 5 and retained known PNG fixtures. Before public release, only redacted, product-relevant images should be copied into this repository. Diagnostic images containing local pane labels, usernames, paths, or unrelated terminal output must not be published.

The acceptance tests and task list define which evidence must be regenerated from the public implementation rather than inherited from the prototype.

## Conclusion

The experiment proved the central product hypothesis: a Herdr plugin can turn equations in agent answers into a stable, automatically updated side-pane image.

The result justifies building Herdr Math. It does not justify releasing the prototype unchanged. The formal implementation must preserve the proven graphics, scanner, boundary, and error behavior while adopting Herdr-native event hooks, privacy-preserving state, self-contained dependencies, and reproducible installation.

## Primary References

- [Herdr plugins](https://herdr.dev/docs/plugins/)
- [Herdr Socket API](https://herdr.dev/docs/socket-api/)
- [Herdr marketplace](https://herdr.dev/docs/marketplace/)
- [Ghostty Kitty graphics feature](https://ghostty.org/docs/features)
