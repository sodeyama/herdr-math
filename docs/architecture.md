# Herdr Math V1 Architecture

## Status

This document defines the **target** architecture for the first public release. It is informed by the August 1, 2026 prototype, but it intentionally changes the prototype lifecycle and state model where they conflict with current Herdr plugin semantics or public-release requirements.

The acceptance tests in `specs/herdr-math-v1/tests/main.md` are the executable contract.

## Architectural Decisions

V1 adopts the following decisions:

1. The plugin is installed and launched through `herdr-plugin.toml`.
2. Agent lifecycle work is handled by manifest event hooks and bounded one-shot workers.
3. A startup hook may prune stale state, but it must exit; it is not a daemon launcher.
4. Raw pane text and LaTeX source are never written to durable state or logs.
5. The working-state baseline is represented by cryptographic boundary fingerprints.
6. Rendering occurs fully on the local machine with no executable TeX engine.
7. One Herdr-managed viewer pane is owned per source pane.
8. Images are replaced with `pane.graphics.set` only after complete validation.
9. Ambiguous answer boundaries, unsupported graphics, and invalid formulas fail closed.
10. V1 compatibility claims are restricted to the release matrix that has real runtime evidence.

## System Context

```text
AI agent process
    |
    | terminal output and Herdr lifecycle state
    v
Herdr server
    |
    | manifest event hooks + HERDR_PLUGIN_EVENT_JSON
    v
Herdr Math event worker
    |-- reads current pane through the Herdr socket API
    |-- reads/writes fingerprint state in HERDR_PLUGIN_STATE_DIR
    |-- scans only a proven answer delta
    |-- invokes the local renderer
    |-- opens or finds the owned viewer pane
    `-- sends a validated PNG through pane.graphics.set
            |
            v
      Herdr viewer pane
            |
            v
  attached graphics-capable terminal
```

Herdr owns plugin installation, event dispatch, process launch, pane layout, socket access, command logs, and the graphics layer. Herdr Math owns parsing, boundary proof, rendering, state files, viewer ownership metadata, limits, and error classification.

## Manifest Contract

The planned manifest shape is:

```toml
id = "io.github.sodeyama.herdr-math"
name = "Herdr Math"
version = "0.1.0"
min_herdr_version = "<validated minimum>"
description = "Render LaTeX from AI agent responses in a side pane"
platforms = ["<release-gated platforms>"]

[[build]]
command = ["npm", "ci"]

[[build]]
command = ["npm", "run", "build"]

[[startup]]
command = ["node", "dist/startup.js"]

[[events]]
on = "pane.agent_status_changed"
command = ["node", "dist/on-agent-status.js"]

[[events]]
on = "pane.closed"
command = ["node", "dist/on-pane-closed.js"]

[[actions]]
id = "diagnose"
title = "Diagnose Herdr Math"
contexts = ["pane"]
command = ["node", "dist/diagnose.js"]

[[panes]]
id = "viewer"
title = "Math"
placement = "split"
command = ["node", "dist/viewer.js"]
```

The exact minimum Herdr version and platform list remain release-gated values. They must not be copied from the prototype without validation.

Build commands run for GitHub installation, while local `plugin link` development requires an explicit local build. The README must state both flows.

## Runtime Environment

Runtime commands use only Herdr-provided context:

- `HERDR_SOCKET_PATH`
- `HERDR_BIN_PATH`
- `HERDR_PLUGIN_ID`
- `HERDR_PLUGIN_ROOT`
- `HERDR_PLUGIN_CONFIG_DIR`
- `HERDR_PLUGIN_STATE_DIR`
- `HERDR_PLUGIN_EVENT`
- `HERDR_PLUGIN_EVENT_JSON`
- `HERDR_PLUGIN_CONTEXT_JSON`
- Available pane, tab, and workspace ids

No runtime code may contain a fallback to a specific user's home directory or default Herdr socket path.

The raw socket API is used where the CLI does not provide an equivalent graphics or subscription operation. The client accepts both Unix-domain socket paths and Windows named-pipe paths as opaque values even if Windows is not declared for v1.

## Component Boundaries

### 1. Event decoder

Responsibilities:

- Parse `HERDR_PLUGIN_EVENT_JSON` with a strict schema
- Extract event name, source pane id, agent label, status, session identity, and sequence fields when present
- Reject malformed, oversized, or unrelated events
- Route events to the state machine

The decoder does not read panes or perform rendering.

### 2. State machine

Responsibilities:

- Interpret `working`, `blocked`, `done`, `idle`, and `unknown`
- Create a new generation for each accepted `working` event
- Ignore unsupported agents
- Ensure `done` and `idle` for the same final pane content are idempotent
- Prevent a stale completion worker from rendering after a newer `working` generation
- Apply expiry and stale-lock rules

The state machine exposes deterministic pure functions wherever possible.

### 3. Boundary fingerprint builder

At `working`, the worker reads a bounded recent pane snapshot and derives a fingerprint without retaining the text.

A v1 fingerprint contains:

- Schema version
- Pane and session namespace
- Baseline character and line counts
- HMAC or cryptographic digest of the full baseline
- Prefix checkpoint offsets and digests
- Several bounded suffix-window lengths and digests
- Hashes and lengths for eligible tail-anchor lines
- Context digests for each tail anchor
- Creation time, event sequence, and expiry time

The implementation secret used for keyed fingerprints is created locally with restrictive file permissions. Fingerprints are not logged.

The builder discards the pane text before the process exits.

### 4. Boundary resolver

At completion, the resolver receives the stored fingerprint and a current bounded pane read. It attempts these strategies in order:

1. **Exact prefix**: hash the current prefix at the stored baseline length and compare it with the full baseline digest.
2. **Stable prefix checkpoint**: find the longest stored prefix checkpoint that still matches and meets the configured stability threshold.
3. **Sliding window**: locate a matching stored suffix-window digest inside the current read, then begin after the verified window.
4. **Contextual tail anchor**: hash candidate lines in the current read and select only an occurrence whose preceding-context digest matches the stored context.

Each strategy returns:

- Proven start offset
- Strategy code
- Confidence facts used by policy
- Current content digest

It returns no result if a boundary cannot be proven. It never guesses based only on the last prompt string or a generic agent marker.

### 5. LaTeX scanner

The scanner consumes only the proven answer delta and emits ordered records:

```ts
type Formula = {
  latex: string;
  display: boolean;
  start: number;
  end: number;
};
```

It recognizes `$...$` and `$$...$$` while tracking:

- Fenced code blocks
- Inline code spans
- Escaped dollar signs
- Delimiter length
- Newlines in inline math
- Unclosed delimiters
- Shell-variable and price-like ambiguity

The scanner is deterministic, linear-time for bounded input, and independent of the renderer.

### 6. Renderer

The renderer accepts a bounded array of formulas and returns:

```ts
type RenderedImage = {
  buffer: Buffer;
  width: number;
  height: number;
  bytes: number;
  renderer: string;
};
```

Required properties:

- Fully local after plugin installation
- No executable TeX engine
- No remote resources
- Explicit untrusted-input mode
- Deterministic layout for a fixed version, font set, and input
- Hard timeout and image-size limits
- Transparent or theme-controlled background decided before release
- Stable error mapping

The prototype used KaTeX, Playwright, and Sharp. The release plan includes a renderer gate that compares that proven path with an SVG-first path that may avoid a browser download. The selected backend must pass the same contract suite; the rest of the architecture must not depend on backend details.

### 7. Herdr client

Responsibilities:

- Newline-delimited JSON request and response handling
- Unique request ids
- Bounded response size
- Method-specific timeouts
- Structured Herdr error mapping
- Safe socket close and reconnect behavior

Required methods include:

- `pane.read`
- `pane.get` or `pane.list`
- `pane.layout`
- `pane.graphics.info`
- `pane.graphics.set`
- `pane.report_metadata`
- `plugin.pane.open`

The client does not silently fall back to a user-specific socket.

### 8. Viewer manager

The viewer manager owns the one-to-one mapping between a source pane and a viewer pane.

Discovery order:

1. Validate the viewer id stored in source state.
2. If missing or stale, inspect plugin-owned pane metadata for the same source pane.
3. If no valid viewer exists, open the manifest `viewer` entrypoint as a right split with focus disabled.
4. Report plugin metadata from the viewer process so it can be recovered after worker or server restart.

The manager verifies ownership before updating or closing a pane. It never treats an arbitrary pane id from state as trusted.

### 9. Graphics placer

Before updating the viewer, the placer:

1. Calls `pane.graphics.info` for pixel cell dimensions.
2. Rejects disabled graphics, missing client dimensions, zero values, or unsupported responses.
3. Reads viewer layout dimensions.
4. Computes bounded grid columns and rows.
5. Verifies raw image bytes and base64 expansion against limits.
6. Calls `pane.graphics.set` once with the complete image.

The existing graphics layer is not cleared first. This preserves the last valid image if the new render or API call fails.

### 10. Diagnostics

The `diagnose` action checks:

- Plugin and Herdr versions
- Required environment variables
- State/config directory access
- Renderer dependency availability
- Graphics feature state
- Attached client cell dimensions
- Current pane and viewer ownership facts

Diagnostics report no pane text or formula text. Human-readable output is paired with stable machine codes.

## Event Lifecycle

### Working event

```text
decode event
  -> confirm supported agent and source pane
  -> acquire per-pane lock
  -> read bounded baseline
  -> build fingerprint generation N
  -> atomically replace source state
  -> release lock and exit
```

Repeated `working` events with the same sequence and content digest are idempotent. A later working event creates a new generation and invalidates older completion work.

### Blocked event

`blocked` preserves the active baseline. It does not render and does not create a new generation. A later `working`, `done`, or `idle` event continues the same lifecycle unless Herdr identifies a replacement agent occupant.

### Done or idle event

```text
decode event
  -> acquire per-pane lock
  -> load unexpired generation
  -> wait bounded debounce interval
  -> read pane until two bounded reads are stable
  -> confirm generation is still current
  -> prove answer boundary
  -> scan formulas
  -> if none: record processed digest and exit
  -> validate limits
  -> render complete image
  -> find or create owned viewer
  -> validate graphics capability and placement
  -> set new image
  -> atomically record processed digest and viewer id
  -> release lock and exit
```

If `done` and `idle` arrive for the same final content, the second worker sees the processed digest and exits without rendering.

### Unknown event

`unknown` does not render. It may expire an old generation only after the configured age limit.

### Pane closed event

- If a source pane closes, remove its fingerprint and viewer mapping state.
- If a viewer closes, clear only the viewer id while retaining the source generation when still relevant.
- Do not automatically close another pane unless ownership is verified.

## Concurrency and Atomicity

Event hooks may overlap. Every mutation for one source pane is serialized with an atomic lock file created using exclusive creation.

Lock records contain only:

- Schema version
- Process id
- Start time
- Event type
- Pane id

Stale-lock recovery verifies both age and process liveness where the platform supports it. A PID match alone is insufficient because ids can be reused.

State updates use write-to-temporary-file, `fsync` where practical, and atomic rename. Temporary files are restricted to `HERDR_PLUGIN_STATE_DIR` and removed by the startup cleanup hook after an age threshold.

## State Layout

Planned layout:

```text
$HERDR_PLUGIN_STATE_DIR/
  v1/
    secret
    sessions/
      <session-key>/
        panes/
          <encoded-source-pane-id>.json
        locks/
          <encoded-source-pane-id>.lock
        tmp/
```

`session-key` is derived from authoritative Herdr runtime context. File names use a restricted encoding and never accept path separators from environment or event input.

Example state shape:

```json
{
  "schema_version": 1,
  "source_pane_id": "w1:p1",
  "agent": "codex",
  "generation": 42,
  "baseline": {
    "character_count": 8120,
    "line_count": 240,
    "digest": "<redacted>",
    "checkpoints": [],
    "suffix_windows": [],
    "tail_anchors": []
  },
  "viewer_pane_id": "w1:p2",
  "processed_digest": "<redacted>",
  "created_at": "2026-08-01T00:00:00Z",
  "expires_at": "2026-08-02T00:00:00Z"
}
```

No answer text, formula text, or rendered image is stored in this record.

## Limits

Prototype values provide the initial defaults, subject to release validation:

| Limit | Initial value | Enforcement point |
|---|---:|---|
| Pane read | 1,000 recent lines | Herdr reader |
| Formulas per answer | 20 | Scanner/renderer boundary |
| Characters per formula | 2,000 | Scanner/renderer boundary |
| Aggregate formula characters | 10,000 | Renderer boundary |
| Render duration | 8 seconds | Renderer |
| Raw PNG bytes | 512 KiB | Graphics placer |
| Anchor occurrences examined | 256 | Boundary resolver |

The public implementation must also set explicit limits for event JSON size, pane-read bytes, image width and height, state-file bytes, socket response bytes, and lock age.

## Error Model

Stable v1 error codes include:

- `event_invalid`
- `agent_unsupported`
- `baseline_missing`
- `boundary_failed`
- `answer_truncated`
- `formula_not_found`
- `invalid_latex`
- `renderer_input_limit`
- `renderer_timeout`
- `renderer_failed`
- `image_too_large`
- `graphics_disabled`
- `cell_size_unavailable`
- `viewer_open_failed`
- `viewer_ownership_failed`
- `herdr_timeout`
- `herdr_protocol_error`
- `state_locked`
- `state_corrupt`

Expected input rejection is not logged as an unhandled exception. Unexpected failures produce a bounded error record and a non-zero worker exit without affecting the agent process.

## Logging

Workers write structured JSON Lines to stdout and stderr so Herdr's plugin command log remains the primary operational log.

Allowed fields include:

- Timestamp
- Plugin version
- Event and status
- Pane, tab, and workspace ids
- Agent label
- Generation and strategy
- Formula count
- Image dimensions and byte counts
- Duration
- Error code

Forbidden fields include:

- Pane text
- Answer delta
- Formula source
- Rendered HTML or SVG containing formula source
- Environment dumps
- Home-directory paths
- Tokens, credentials, or arbitrary exception objects

## Renderer Selection Gate

Before committing to a release renderer, compare at least:

1. The proven KaTeX + browser screenshot + PNG optimization path.
2. A browser-free SVG math renderer + local SVG-to-PNG path.

Use a fixed corpus containing inline, display, aligned, matrix, integral, Unicode, invalid, large, and multiline cases. Measure correctness, install size, install time, cold render time, warm render time, image size, font consistency, macOS/Linux native dependency behavior, and security surface.

Prefer the browser-free path only if it reaches behavior parity. Otherwise ship the proven renderer with explicit installation cost and revisit optimization after v0.1.

## Packaging and Release

The repository is an ordinary public GitHub plugin repository.

Release flow:

1. Clean checkout at the release commit.
2. Run the manifest build commands exactly as Herdr will run them.
3. Validate manifest and plugin warnings with the minimum Herdr version.
4. Run unit, integration, rendering, install, and security checks.
5. Link the checkout for local runtime smoke tests.
6. Install the tagged repository through `herdr plugin install` in a clean user-level test environment.
7. Run the terminal compatibility matrix.
8. Confirm documentation, version, tag, and lockfile agreement.
9. Publish the GitHub release.
10. Add or retain the `herdr-plugin` topic only after the install test passes.

GitHub installation replaces the managed checkout on reinstall. Durable user state must therefore remain entirely under Herdr's state and config directories.

## Compatibility Matrix

The release matrix records each dimension independently:

| Dimension | Required evidence |
|---|---|
| Herdr version | Manifest link/install plus required socket methods |
| Operating system | Clean dependency build and runtime smoke |
| CPU architecture | Native dependency installation and render smoke |
| Outer terminal | Real image display, update, resize, and focus behavior |
| Local session | Full automatic lifecycle test |
| Named session | State namespace and restart test |
| Remote attach | Explicitly tested or documented as unsupported |

The prototype provides initial evidence only for Herdr 0.7.5, macOS, and Ghostty 1.3.1 in a local session.

## Test Architecture

- Unit tests cover pure event decoding, state transitions, fingerprint building, boundary resolution, scanning, limit policy, placement math, and error mapping.
- Contract tests validate event and response fixtures against the Herdr schema generated by the minimum supported binary.
- Integration tests use a fake Herdr socket server to exercise concurrency, retries, pane lifecycle, and graphics requests.
- Renderer tests use a fixed formula corpus and image assertions.
- Runtime smoke tests use real Herdr panes and a supported terminal.
- Install tests use a clean managed install from a tag, not only `plugin link`.

Every acceptance-test id in the specification maps to at least one automated or recorded manual test.

## Open Decisions

The following decisions must be closed during the implementation plan, not silently assumed:

- Final renderer backend
- Exact Node.js support range or whether to ship a standalone binary later
- Minimum Herdr version after manifest event-hook validation
- First-release platform list
- First-release outer-terminal matrix
- Theme behavior and default image background
- Whether a manual `render-current` action belongs in v0.1
- Whether remote attach can display the graphics layer reliably

Changing an open decision requires updating the plan, tests, user documentation, and release checklist together.

## Primary References

- [Herdr plugins](https://herdr.dev/docs/plugins/)
- [Herdr Socket API](https://herdr.dev/docs/socket-api/)
- [Herdr CLI reference](https://herdr.dev/docs/cli-reference/)
- [Herdr marketplace](https://herdr.dev/docs/marketplace/)
