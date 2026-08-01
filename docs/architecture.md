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

The validated v0.1 manifest shape is:

```toml
id = "io.github.sodeyama.herdr-math"
name = "Herdr Math"
version = "0.1.0"
min_herdr_version = "0.7.5"
description = "Render LaTeX from AI agent responses in a side pane"
platforms = ["macos"]

[[build]]
command = ["npm", "ci"]

[[build]]
command = ["npm", "run", "install:browser"]

[[build]]
command = ["npm", "run", "audit:browser"]

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

Build commands run for GitHub installation. `npm ci` installs the exact Playwright browser revision through its postinstall step, the explicit browser command makes that contract visible and idempotent, and the audit rejects a missing executable, native addon, or license inventory before compilation. Browser assets remain under the plugin's `node_modules`, not a user-global cache. Local `plugin link` development requires the same dependency installation and an explicit local build. The README must state both flows before release.

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
- `HERDR_MATH_SOURCE_TOKEN`, supplied only by the viewer manager when it opens the manifest pane

No runtime code may contain a fallback to a specific user's home directory or default Herdr socket path.

The raw socket API is used where the CLI does not provide an equivalent graphics or subscription operation. The client accepts both Unix-domain socket paths and Windows named-pipe paths as opaque values even if Windows is not declared for v1.

## Component Boundaries

### 1. Event decoder

Responsibilities:

- Parse `HERDR_PLUGIN_EVENT_JSON` with a strict schema
- Extract only the event name, workspace id, source pane id, status, and optional agent hint supplied by the current Herdr event schema
- Reject malformed, oversized, or unrelated events
- Pass the validated pane id to the Herdr client for authoritative source resolution

The decoder does not infer agent identity, read panes, or perform rendering. The event's optional agent field is a hint, not sole authority. The Herdr client calls `pane.get` to resolve the current pane and `agent.get` to resolve the canonical agent id, workspace id, status, pane revision, and lifecycle `state_change_seq`. The worker cross-checks both responses. A missing pane, missing agent, disagreement, workspace mismatch, optional-agent mismatch, or status mismatch is stale input and fails closed before state mutation.

### 2. State machine

Responsibilities:

- Interpret `working`, `blocked`, `done`, `idle`, and `unknown`
- Create a new generation for each accepted `working` event
- Treat panes with no agent or an unsupported agent as successful no-ops after authoritative `pane.get` verification
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
- Baseline end offsets for eligible tail-anchor lines
- Preceding and following context digests for each eligible tail anchor
- HMAC digests and bounded formula HMAC sets for eligible adjacent-anchor gaps
- Creation time, event sequence, and expiry time

The implementation secret used for keyed fingerprints is created locally with restrictive file permissions. Fingerprints are not logged.

The builder discards the pane text before the process exits.

### 4. Boundary resolver

At completion, the resolver receives the stored fingerprint and a current bounded pane read. It attempts these strategies in order:

1. **Exact prefix**: hash the current prefix at the stored baseline length and compare it with the full baseline digest.
2. **Middle insertion**: prove a context-qualified anchor before an alternate-screen insertion, a distinct unique anchor after it, and an unchanged baseline gap suffix, then return only the bounded inserted prefix.
3. **Middle replacement**: prove a unique preceding-context anchor and a unique after anchor around a changed alternate-screen region, use following context when the after-anchor line repeats, then return the bounded replacement and exclude formulas already fingerprinted in the baseline gap.
4. **Stable prefix checkpoint**: find the longest stored prefix checkpoint that still matches and meets the configured stability threshold.
5. **Sliding window**: locate a matching stored suffix-window digest inside the current read, then begin after the verified window.
6. **Contextual tail anchor**: hash candidate lines in the current read and select only an occurrence whose preceding-context digest matches the stored context.

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
- Opaque white background for deterministic v0.1 output
- Stable error mapping

V0.1 uses KaTeX, Playwright Chromium headless shell, and Sharp. The backend remains behind this interface so event, state, and viewer modules do not depend on browser details. [ADR 0001](decisions/0001-v1-renderer.md) records the comparison, security boundary, packaging cost, and compatibility limit.

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

The socket request for an unwrapped recent pane read uses the protocol enum `recent_unwrapped`. The hyphenated
`recent-unwrapped` spelling belongs to the Herdr CLI and is not sent over the socket.

For optional pane lookup, the runtime `pane_not_found` error means that the pane is authoritatively absent. Other
remote errors remain protocol failures and do not authorize state deletion or viewer replacement.

### 8. Viewer manager

The viewer manager owns the one-to-one mapping between a source pane and a viewer pane.

The source token is a domain-separated SHA-256 digest of the Herdr session identity and source pane id. The raw source pane id is not copied into presentation metadata. The managed viewer validates `HERDR_PLUGIN_ID`, `HERDR_PLUGIN_ENTRYPOINT_ID`, its pane and workspace ids, and the 64-character source token before reporting the English `Herdr Math` title and the `herdr_math_owner` and `herdr_math_source` tokens. Herdr acknowledges `pane.report_metadata` with `ok`; the client then reads the pane and validates the reported ownership metadata and workspace. Metadata registration and verification each have a two-second timeout. After registration, the viewer does not poll; it remains attached to its Herdr pane until stdin closes or Herdr sends `SIGHUP`, `SIGINT`, or `SIGTERM`.

Discovery order:

1. Validate the viewer id stored in source state.
2. If missing or stale, inspect plugin-owned pane metadata for the same source pane.
3. If no valid viewer exists, open the manifest `viewer` entrypoint as a right split with focus disabled.
4. Report plugin metadata from the viewer process so it can be recovered after worker or server restart.

Validation requires the current workspace plus exact `herdr_math_owner` and session-bound `herdr_math_source` token values. More than one matching recovery candidate is an ownership failure; the manager does not choose or close a candidate. Before creation, it confirms the source pane again and sends one bounded `plugin.pane.open` request with `target_pane_id`, `placement: split`, `direction: right`, and `focus: false`. The split request omits `workspace_id`; Herdr derives the destination from the target pane. The returned plugin id, entrypoint, workspace, tab, pane id, and focus state must match the validated source.

The manager verifies ownership before updating or closing a pane. It never treats an arbitrary pane id from state as trusted, and it does not modify or close a user pane referenced by stale state.

### 9. Graphics placer

Before updating the viewer, the placer:

1. Revalidates PNG signature, declared dimensions, pixel count, raw bytes, and base64 expansion independently of the renderer.
2. Calls `pane.graphics.info` on the source before viewer discovery, so disabled graphics or unavailable client dimensions cannot create a useless viewer.
3. Resolves the owned viewer, calls `pane.graphics.info` again for that pane, and reads its current `pane.layout` rectangle.
4. Converts image pixels to natural cell columns and rows, then scales both dimensions by one bounded factor so the placement remains within the current viewer rectangle.
5. Calls `pane.graphics.set` once with the complete PNG and placement. Graphics and layout calls have two-second timeouts.

The existing graphics layer is not cleared first. Invalid or oversized images, missing cell dimensions, stale ownership, and graphics API failure leave the last valid image intact. A successful completion records the viewer id only after `pane.graphics.set` succeeds.

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

The implemented action reads only explicitly named Herdr environment variables and extracts only the focused pane and
workspace ids from the bounded action context. Its JSON output is restricted to plugin and Herdr versions, protocol
numbers, fixed check ids, fixed status values, stable codes, and fixed English messages and actions. It never reports
directory paths, pane ids, selected text, environment secrets, remote error messages, or exception objects.

`graphics_disabled` instructs the user to set `[experimental].kitty_graphics = true` and run
`herdr server reload-config`. `cell_size_unavailable` separately instructs the user to reconnect Herdr from a compatible
graphics-capable terminal. Available cell dimensions still produce `terminal_unverified`; they do not by themselves
create a terminal compatibility claim or require Ghostty.

## Event Lifecycle

### Working event

```text
decode event
  -> read one bounded baseline snapshot immediately
  -> resolve and cross-check pane.get with agent.get state_change_seq
  -> confirm the same working event, supported agent, and source pane
  -> acquire per-pane lock
  -> build fingerprint generation N
  -> atomically replace source state
  -> release lock and exit
```

Repeated `working` events with the same `state_change_seq` and content digest are idempotent. A later working event has a newer sequence, creates a new generation, and invalidates older completion work even when the pane metadata revision is unchanged.

### Blocked event

`blocked` preserves the active baseline. It does not render and does not create a new generation. A later `working`, `done`, or `idle` event continues the same lifecycle unless Herdr identifies a replacement agent occupant.

### Done or idle event

```text
decode event
  -> resolve and cross-check pane.get with agent.get state_change_seq
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

V0.1 waits 500 ms before completion reads, then performs at most three `recent-unwrapped` reads 100 ms apart. Two consecutive reads must have the same keyed content digest and truncation flag. `pane.get` and `pane.read` each have a two-second client timeout. Test-only overrides may lower these values but cannot raise them.

### Unknown event

`unknown` does not render. It may expire an old generation only after the configured age limit.

### Pane closed event

- Resolve the closed pane id through `pane.get`; mutate state only when Herdr returns `not_found`, except that a reused source id may remove an old occupant's state.
- Scan only the current session namespace, with a hard limit of 4096 pane-state entries, and take each source-pane lock before changing its state.
- If a source pane closes, remove its fingerprint and viewer mapping state. A matching native agent-session identity protects state from a delayed close event; fallback pane identities do not.
- If a viewer closes, clear only the viewer id while retaining the source fingerprint, processed digest, and generation.
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

Startup cleanup inspects locks before state. A young, live, malformed, or liveness-uncertain lock protects the matching pane state and temporary files. Cleanup removes only identity-checked dead stale locks, expired fingerprint state, and allowlisted stale temporary files; corrupt, symbolic-link, and unknown artifacts remain untouched for fail-closed handling.

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
  "session_key": "<digest>",
  "workspace_id": "w1",
  "source_pane_id": "w1:p1",
  "agent": "codex",
  "lifecycle_authority": "screen_detection",
  "occupant_key": "<digest>",
  "pane_revision": 120,
  "event_sequence": 18,
  "generation": 42,
  "baseline": {
    "character_count": 8120,
    "utf8_bytes": 8200,
    "line_count": 240,
    "digest": "<redacted>",
    "prefix_checkpoints": [],
    "suffix_windows": [],
    "tail_anchors": []
  },
  "viewer_pane_id": "w1:p2",
  "processed": {
    "content_digest": "<redacted>",
    "pane_revision": 121,
    "processed_at": "2026-08-01T00:01:00.000Z"
  },
  "created_at": "2026-08-01T00:00:00.000Z",
  "expires_at": "2026-08-02T00:00:00.000Z"
}
```

No answer text, formula text, or rendered image is stored in this record.

## Limits

Prototype values provide the initial defaults, subject to release validation:

| Limit | Initial value | Enforcement point |
|---|---:|---|
| Pane read | 1,000 recent lines | Herdr reader |
| Event JSON | 64 KiB UTF-8 | Event decoder |
| Pane read bytes | 1 MiB UTF-8 | Herdr reader |
| Scanner input | 1 MiB UTF-8 | Scanner |
| Delimiter runs per answer | 4,096 | Scanner |
| Characters per delimiter run | 8 | Scanner |
| Formulas per answer | 20 | Scanner/renderer boundary |
| Characters per formula | 2,000 | Scanner/renderer boundary |
| Aggregate formula characters | 10,000 | Renderer boundary |
| Render duration | 8 seconds | Renderer |
| Raw PNG bytes | 512 KiB | Graphics placer |
| Base64 graphics payload | 700 KiB | Graphics placer |
| Image width | 4,096 px | Renderer/graphics placer |
| Image height | 16,384 px | Renderer/graphics placer |
| Image pixels | 33,554,432 | Renderer/graphics placer |
| Tail anchor minimum | 32 characters | Fingerprint builder/state validator |
| Anchor occurrences examined | 256 | Boundary resolver |
| Boundary candidates examined | 2,048 | Boundary resolver |
| State file | 64 KiB | State store |
| Socket response | 2 MiB | Herdr client |
| Herdr method timeout | 2 seconds | `pane.get` and `pane.read` |
| Completion debounce | 500 ms | Completion worker |
| Stable completion reads | 3 attempts, 100 ms apart | Completion worker |
| Startup sessions examined | 256 | Startup cleanup |
| Startup directory entries | 4,096 per directory | Startup cleanup |
| Stale lock age | 120 seconds | State store |
| Fingerprint expiry | 24 hours | State store |

These initial values remain release-gated. Lower them if renderer or protocol evidence requires a stricter bound; do not raise them without updating the threat model and acceptance tests.

## Error Model

Stable v1 error codes include:

- `event_invalid`
- `agent_unsupported`
- `baseline_missing`
- `boundary_failed`
- `answer_truncated`
- `formula_not_found`
- `scanner_input_limit`
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
- `internal_error`

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

## Renderer Selection

The completed selection compared:

1. The proven KaTeX + browser screenshot + PNG optimization path.
2. A browser-free SVG math renderer + local SVG-to-PNG path.

Both candidates used a fixed corpus containing inline, display, aligned, matrix, integral, Unicode, invalid, large, and multiline cases. The experiment measured correctness, install size, install time, cold and warm latency, image size, native artifacts, cleanup, offline behavior, and security surface.

The browser-free candidate did not reach behavior parity. V0.1 therefore selects the browser path with an explicit measured installation cost. See [ADR 0001](decisions/0001-v1-renderer.md) and the [candidate measurements](evidence/2026-08-01-renderer-candidates.md).

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

The current implementation is verified with Herdr 0.7.5 on macOS arm64 and Ghostty 1.3.1. Named-session restart
and default-session isolation are also verified. See [Compatibility](compatibility.md) for exact release-candidate
and unverified combinations.

## Test Architecture

- Unit tests cover pure event decoding, state transitions, fingerprint building, boundary resolution, scanning, limit policy, placement math, and error mapping.
- Contract tests validate event and response fixtures against the Herdr schema generated by the minimum supported binary.
- Integration tests use a fake Herdr socket server to exercise concurrency, retries, pane lifecycle, and graphics requests.
- Renderer tests use a fixed formula corpus and image assertions.
- `npm run security:check`, included in `npm run check`, scans runtime source and release files for environment dumps,
  external network APIs, dynamic execution, input-controlled executable paths, credential formats, private home paths,
  symbolic links, and local runtime artifacts. The only runtime socket connection must remain the path-based Herdr client.
- Runtime smoke tests use real Herdr panes and a supported terminal.
- Install tests use a clean managed install from a tag, not only `plugin link`.

Every acceptance-test id in the specification maps to at least one automated or recorded manual test.

## V0.1 Compatibility Decisions

- Node.js 22 or later is required; v0.1 does not ship a standalone binary.
- Herdr 0.7.5 is the verified minimum and protocol 17 is the recorded contract.
- The manifest declares macOS; arm64 is the only verified architecture.
- Ghostty 1.3.1 is the only verified outer terminal.
- V0.1 has no manual `render-current` action.
- Remote attach graphics are unverified and unsupported as a v0.1 claim.

Changing one of these decisions requires updating the plan, tests, [compatibility matrix](compatibility.md), user
documentation, and release checklist together.

## Primary References

- [Herdr plugins](https://herdr.dev/docs/plugins/)
- [Herdr Socket API](https://herdr.dev/docs/socket-api/)
- [Herdr CLI reference](https://herdr.dev/docs/cli-reference/)
- [Herdr marketplace](https://herdr.dev/docs/marketplace/)
