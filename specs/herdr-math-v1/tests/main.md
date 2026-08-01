# Herdr Math V1 Acceptance Tests

## Status

- Specification state: Planned
- Target release: `0.1.0`
- Last updated: August 1, 2026
- Canonical plan: `../plans/main.md`
- Executable tasks: `../tasks/main.md`

## Purpose

This document defines the acceptance contract for the first public Herdr Math release. Implementation is complete only when every P0 case passes with the required evidence. A skipped, retried, flaky, partially implemented, or manually assumed case is not a pass.

## Priority

- **P0**: Required to publish `0.1.0`.
- **P1**: Required before claiming the associated optional platform or capability.
- **P2**: Post-release exploration; not part of the `0.1.0` gate.

## Evidence Types

- **Unit**: deterministic automated test of a pure module.
- **Contract**: automated validation against Herdr-compatible request, response, manifest, or event fixtures.
- **Integration**: automated test using a fake Herdr socket and real plugin modules.
- **Render**: real local renderer output with image assertions.
- **Runtime**: real Herdr session and graphics-capable terminal.
- **Install**: clean GitHub-managed plugin installation from a tag.
- **Static**: source, dependency, license, secret, or artifact inspection.

Runtime evidence must record the date, Herdr version, operating system, architecture, outer terminal and version, commands used, expected result, observed result, and screenshot or structured log where appropriate. Evidence must be redacted before commit.

## Test Environment Rules

1. Unit and integration tests run from a clean dependency installation.
2. Install tests use a temporary or dedicated test environment and do not reuse the author's linked development plugin.
3. Runtime restart tests use an isolated named Herdr session so active user agents are not stopped.
4. Terminal compatibility is tested in the actual terminal, not inferred from the terminal's generic Kitty graphics documentation.
5. The release tag, manifest version, package version, and changelog version must agree.
6. No acceptance fixture may contain credentials, private transcripts, real home-directory paths, or an environment dump.
7. Agent compatibility is verified separately for Claude Code, Codex, Pi, and OpenCode. A passing result from one agent must not be used as evidence for another.

## A. Repository, Manifest, and Installation

### AT-001 - Canonical public identity

- Priority: P0
- Evidence: Static
- Given the repository metadata and manifest
- When names and descriptions are inspected
- Then the display name is `Herdr Math`, the repository is `sodeyama/herdr-math`, the plugin id is `io.github.sodeyama.herdr-math`, and the public description is English.

### AT-002 - Manifest validates at the minimum Herdr version

- Priority: P0
- Evidence: Contract, Runtime
- Given the declared `min_herdr_version`
- When the built plugin is linked or installed with that exact Herdr version
- Then registration succeeds with no unknown-event, missing-platform, incompatible-version, or malformed-command warning.

### AT-003 - Newer-required plugin is rejected by an older Herdr binary

- Priority: P0
- Evidence: Contract
- Given a Herdr version older than the declared minimum
- When plugin installation or linking is attempted
- Then Herdr rejects it as incompatible rather than starting a partially supported plugin.

### AT-004 - Clean GitHub install

- Priority: P0
- Evidence: Install
- Given no linked or installed copy of the plugin and an empty managed checkout
- When `herdr plugin install sodeyama/herdr-math --ref v0.1.0` is run
- Then Herdr previews the expected commands, runs every declared build command successfully, registers the plugin, and lists its actions, events, and viewer entrypoint.

### AT-005 - Reinstall from the same tag

- Priority: P0
- Evidence: Install
- Given an installed plugin with existing state/config directories
- When the same tagged source is installed again
- Then the managed checkout is recreated successfully, runtime source is not read from the previous checkout, and plugin-owned state/config behavior matches the documented retention policy.

### AT-006 - Local development link

- Priority: P0
- Evidence: Install
- Given a clean checkout whose dependencies and build have been run explicitly
- When `herdr plugin link <checkout>` is used
- Then the plugin links without relying on install-time build execution and all entrypoints resolve from the checkout.

### AT-007 - Self-contained dependencies

- Priority: P0
- Evidence: Static, Install
- Given a clean clone on a machine without the prototype vault
- When dependencies are installed and the plugin is built
- Then no import, asset, script, fixture, or runtime path resolves outside the repository or Herdr-provided config/state directories.

### AT-008 - No user-specific absolute paths

- Priority: P0
- Evidence: Static
- When source, tests, manifest, scripts, generated JavaScript, documentation examples, and committed artifacts are scanned
- Then no local username, home-directory fallback, prototype path, or default socket path is present.

### AT-009 - Build output is reproducible

- Priority: P0
- Evidence: Static, Install
- Given two clean checkouts at the same commit with the supported runtime and lockfile
- When the declared build is run
- Then both builds produce functionally equivalent output and the build does not modify `herdr-plugin.toml`.

### AT-010 - Uninstall safety

- Priority: P0
- Evidence: Install
- Given an installed plugin, an unrelated Herdr plugin, and unrelated Herdr panes
- When Herdr Math is uninstalled
- Then only Herdr Math registration and its managed checkout are removed by Herdr, unrelated plugins and panes are unchanged, and retained config/state behavior is documented accurately.

### AT-011 - Version agreement

- Priority: P0
- Evidence: Static
- When a release tag is prepared
- Then the tag, `herdr-plugin.toml`, package metadata, changelog heading, and release notes use the same semantic version.

## B. Event Decoding and Lifecycle

### AT-100 - Valid supported-agent event

- Priority: P0
- Evidence: Unit, Contract
- Given a schema-compatible `pane.agent_status_changed` event containing workspace id, pane id, status, and an optional agent hint, `pane.get` reports matching pane values, and `agent.get` reports that pane as Claude Code, Codex, Pi, or OpenCode with a current state-change sequence
- When the event decoder and authoritative pane resolver run
- Then the decoder returns only the validated event name, workspace id, pane id, status, and optional agent hint supplied by the event, and the resolver returns the canonical agent id, current status, pane revision, and `state_change_seq` without treating the optional hint as sole authority.

### AT-101 - Malformed or oversized event

- Priority: P0
- Evidence: Unit, Integration
- Given missing required ids, invalid JSON, an unrecognized status such as `paused`, wrong types, path-like pane ids, or an event larger than the configured limit
- When the worker runs
- Then it returns `event_invalid`, performs no pane read or state mutation outside a bounded diagnostic, and exits safely.

### AT-102 - Non-agent or unsupported pane

- Priority: P0
- Evidence: Unit, Integration
- Given a valid status event whose authoritative `pane.get` result identifies no agent or an agent outside the v1 allowlist
- When the worker runs
- Then it returns a successful ignored outcome, performs no baseline, rendering, or viewer action, and does not report an event failure.

### AT-103 - Working creates one generation

- Priority: P0
- Evidence: Unit, Integration
- Given a supported source pane in `working`
- When the event is processed
- Then one fingerprint generation is atomically stored for that pane and no raw pane text is written.
- And the hook captures one bounded pane snapshot before fingerprint-secret or state I/O and before authoritative lookups, but uses it only after `pane.get` and `agent.get` still confirm the same working event.
- And lifecycle ordering uses the authoritative `agent.get` `state_change_seq`, not the pane metadata revision.
- And the socket request uses the protocol enum `recent_unwrapped`, not the CLI spelling `recent-unwrapped`.

### AT-104 - Duplicate working is idempotent

- Priority: P0
- Evidence: Unit, Integration
- Given the same working event and unchanged baseline content are delivered twice
- When both workers complete
- Then state remains one logical generation and no corrupt or competing state file remains.

### AT-105 - New working invalidates stale completion

- Priority: P0
- Evidence: Integration
- Given completion processing for generation N has started
- When a new working event creates generation N+1 before rendering commits
- Then generation N does not update the viewer or processed digest.

### AT-106 - Blocked preserves baseline

- Priority: P0
- Evidence: Unit, Integration
- Given an active working generation
- When the agent transitions to `blocked`
- Then the baseline remains active and the plugin does not render or create a viewer.

### AT-107 - Done processes one stable completion

- Priority: P0
- Evidence: Integration, Runtime
- Given an active baseline and a valid formula in the completed answer
- When a `done` event arrives
- Then the worker performs bounded debounce and stable reads, renders once, and records the final digest.

### AT-108 - Done and idle duplicate suppression

- Priority: P0
- Evidence: Integration, Runtime
- Given `done` and `idle` events for the same final pane content
- When both are delivered concurrently or sequentially
- Then exactly one graphics update occurs and exactly one viewer exists.

### AT-109 - Completion without baseline

- Priority: P0
- Evidence: Unit, Integration
- Given a completion event after plugin installation, state cleanup, or missed working event
- When no valid baseline exists
- Then the worker returns `baseline_missing`, does not scan historical pane content, and does not update a viewer.

### AT-110 - Unknown status is non-destructive

- Priority: P0
- Evidence: Unit
- Given an `unknown` status
- When it is processed
- Then no render occurs and a valid, unexpired baseline is not immediately destroyed.

### AT-111 - Source occupant replacement

- Priority: P0
- Evidence: Integration
- Given a pane id is reused for a different agent occupant or lifecycle authority
- When completion arrives
- Then state from the previous occupant cannot authorize rendering for the replacement.

### AT-112 - Supported coding-agent compatibility matrix

- Priority: P0
- Evidence: Contract, Integration, Runtime
- Given a Herdr pane detected as Claude Code (`claude`), Codex (`codex`), Pi (`pi`), or OpenCode (`opencode`), using the lifecycle authority supported by the minimum Herdr version
- When that coding agent completes a response containing valid `$...$` or `$$...$$` LaTeX
- Then Herdr Math accepts the authoritative lifecycle event, proves the current-answer boundary, renders the detected formulas locally, and creates or updates exactly one owned viewer without changing source focus.
- And release evidence records the detected agent id, lifecycle authority, integration version when installed, observed status sequence, and render result separately for all four agents.

### AT-113 - Stale or unresolved event pane

- Priority: P0
- Evidence: Unit, Contract, Integration
- Given a valid status event but `pane.get` or `agent.get` reports a missing pane, no current agent, a different workspace, an optional event agent that disagrees with the current agent, or a current status that no longer matches the event
- When the worker resolves the event source
- Then it returns a stable fail-closed result, creates no baseline, performs no render or viewer operation, and does not use a previous occupant's agent identity.

## C. Answer Boundary

### AT-200 - Exact prefix

- Priority: P0
- Evidence: Unit
- Given current content begins with the complete working baseline
- When the resolver uses stored fingerprints
- Then it returns only the appended answer with strategy `exact_prefix`.

### AT-201 - Stable prefix after a changing tail

- Priority: P0
- Evidence: Unit
- Given most of the baseline is stable but a spinner or status tail changed
- When a configured prefix checkpoint still matches
- Then the resolver starts after the longest safe checkpoint and excludes known history.

### AT-202 - Sliding read window

- Priority: P0
- Evidence: Unit, Runtime
- Given the completion read dropped the oldest portion of a baseline because the 1,000-line window moved
- When a stored suffix-window fingerprint is found
- Then the resolver returns only text after that verified window and marks the read as recovered truncation.

### AT-203 - Context-qualified repeated prompt

- Priority: P0
- Evidence: Unit
- Given the same shell prompt occurs before and after the new answer
- When tail-anchor candidates are compared
- Then the occurrence whose preceding context matches the baseline is chosen and the answer is retained.

### AT-204 - Prompt-only baseline

- Priority: P0
- Evidence: Unit
- Given the baseline contains only a prompt and the completion contains the prompt, an answer, and the repeated prompt
- When exact-prefix resolution is possible
- Then the entire appended region remains available for scanning.

### AT-205 - Unprovable boundary

- Priority: P0
- Evidence: Unit, Integration
- Given unrelated baseline and completion content with no verified checkpoint, window, or contextual anchor
- When resolution runs
- Then it returns `boundary_failed` and no renderer or viewer method is called.

### AT-206 - Truncation without recovery

- Priority: P0
- Evidence: Unit, Integration
- Given the pane read reaches its limit and no stored boundary fingerprint can be matched
- When completion is processed
- Then it returns `answer_truncated`, preserves the previous viewer, and does not guess.

### AT-207 - Fingerprint-only persistence

- Priority: P0
- Evidence: Static, Unit
- Given a baseline containing unique sentinel text and formulas
- When state is written
- Then the state bytes contain neither the sentinel, any substring above the allowed metadata set, nor reversible encoded transcript content.

### AT-208 - Boundary complexity is bounded

- Priority: P0
- Evidence: Unit
- Given maximum-size permitted pane input and repeated anchor candidates
- When resolution runs
- Then it stays within configured candidate, byte, and time bounds without quadratic growth.

### AT-209 - Alternate-screen middle insertion

- Priority: P0
- Evidence: Unit, Integration, Runtime
- Given a working baseline whose prompt is followed by a stable alternate-screen footer, and the completed answer is inserted between them
- When a context-qualified anchor before the answer and a distinct unique anchor after the answer retain their baseline order with a larger current offset gap, and the stored baseline gap digest matches the suffix of the current gap
- Then the resolver returns the inserted region with strategy `middle_insertion`.
- And it fails closed when either side is absent, ambiguous, reordered, the preserved gap differs, or the comparison does not prove a positive insertion.

### AT-210 - Eligible anchors beyond blank tail rows

- Priority: P0
- Evidence: Unit, Runtime
- Given a bounded alternate-screen baseline ending in more physical blank or short rows than the tail-anchor limit
- When fingerprint creation scans the baseline
- Then it searches up to the pane-read line limit and stores the nearest eligible anchors until the anchor-count limit is reached.
- And persisted line offsets remain bounded metadata without storing line content.

### AT-211 - Alternate-screen middle replacement

- Priority: P0
- Evidence: Unit, Integration, Runtime
- Given an alternate-screen agent replaces a working progress region with its completed answer instead of preserving the baseline gap
- When a unique context-qualified anchor before the region and a unique forward-context-qualified anchor after it retain their order
- Then the resolver returns only the bounded replacement region with strategy `middle_replacement`.
- And each formula already present in the baseline gap is excluded by a keyed formula digest before rendering.
- And absent, ambiguous, reordered, oversized, or unscannable evidence fails closed without rendering.
- And no formula source, delimiter contents, or reversible transcript data is persisted.

## D. LaTeX Scanner

### AT-300 - Inline formula

- Priority: P0
- Evidence: Unit
- Given `The relation is $E=mc^2$.`
- Then exactly one inline formula `E=mc^2` is returned with correct offsets.

### AT-301 - Multiline display formula

- Priority: P0
- Evidence: Unit
- Given a multiline `$$...$$` block
- Then one display formula is returned with internal newlines preserved.

### AT-302 - Ordered multiple formulas

- Priority: P0
- Evidence: Unit
- Given mixed inline and display formulas
- Then all formulas are returned in source order with correct display flags.

### AT-303 - Fenced and inline code

- Priority: P0
- Evidence: Unit
- Given dollar-delimited text in backtick spans and fenced code plus one real equation outside code
- Then only the outside equation is returned.

### AT-304 - Escaped and unclosed dollar signs

- Priority: P0
- Evidence: Unit
- Given escaped currency and unclosed math delimiters
- Then they are ignored without consuming later valid math.

### AT-305 - Prices and shell variables

- Priority: P0
- Evidence: Unit
- Given `$10 and $20`, `$HOME and $PATH`, and an unclosed `$VALUE`
- Then no formula is returned.

### AT-306 - Closed numeric formula

- Priority: P0
- Evidence: Unit
- Given `math $1$`
- Then the closed numeric expression remains a valid formula.

### AT-307 - Unicode surrounding text

- Priority: P0
- Evidence: Unit
- Given English, Japanese, and mixed Unicode prose around valid formulas
- Then offsets and extraction remain correct and no unrelated prose is included.

### AT-308 - Scanner input limits

- Priority: P0
- Evidence: Unit
- Given answer text above the scanner byte limit or excessive delimiter runs
- Then processing is rejected with a stable bounded error and does not allocate unbounded memory.

## E. Rendering and Image Safety

### AT-400 - Representative formula corpus

- Priority: P0
- Evidence: Render
- Given the fixed release corpus containing powers, fractions, roots, sums, integrals, aligned equations, matrices, Greek letters, and Unicode
- When rendered
- Then every valid case produces a non-empty PNG with expected visual content and bounded dimensions.

### AT-401 - Invalid LaTeX

- Priority: P0
- Evidence: Unit, Render, Integration
- Given an unsupported or malformed command
- When rendering runs
- Then it returns `invalid_latex`, does not emit input text in logs, and does not update the viewer.

### AT-402 - Formula-count limit

- Priority: P0
- Evidence: Unit, Render
- Given 21 formulas with a limit of 20
- Then rendering is rejected as `renderer_input_limit` before expensive work starts.

### AT-403 - Per-formula and aggregate length limits

- Priority: P0
- Evidence: Unit, Render
- Given a 2,001-character formula or more than 10,000 aggregate characters
- Then rendering is rejected as `renderer_input_limit`.

### AT-404 - Timeout and recovery

- Priority: P0
- Evidence: Render, Integration
- Given a forced render exceeding the 8-second limit
- When the timeout fires
- Then the worker returns `renderer_timeout`, cleans up its resources, preserves the existing image, and the next valid render succeeds.

### AT-405 - No remote resource access

- Priority: P0
- Evidence: Static, Integration
- Given LaTeX containing URL-like or link-capable input
- When rendering runs under a network-deny harness
- Then no DNS, HTTP, file-outside-plugin, or remote font request occurs.

### AT-406 - No executable input path

- Priority: P0
- Evidence: Static
- When production source and transitive runtime entrypoints owned by the project are inspected
- Then no user input reaches a shell, `child_process`, `eval`, dynamic executable import, or TeX binary.

### AT-407 - Raw and encoded image limits

- Priority: P0
- Evidence: Unit, Integration
- Given a PNG of 512 KiB plus one byte or a base64 payload above the protocol policy
- Then `image_too_large` is returned before `pane.graphics.set` and the previous image remains.

### AT-408 - Image dimension limit

- Priority: P0
- Evidence: Unit, Render
- Given a render whose width, height, or pixel count exceeds policy
- Then it is rejected before graphics placement even if compressed bytes are small.

### AT-409 - Deterministic renderer cleanup

- Priority: P0
- Evidence: Render
- Given repeated successful, invalid, and timed-out cases
- When the suite completes
- Then no browser, page, file handle, worker, or native renderer process remains owned by the test.

### AT-410 - Renderer selection gate

- Priority: P0
- Evidence: Render, Static
- Given the proven browser path and the candidate browser-free path
- When both are measured against the same corpus
- Then the selected backend has a recorded correctness, install-size, cold/warm latency, image-size, native-dependency, and security comparison.

## F. Viewer and Graphics Lifecycle

### AT-500 - First viewer creation

- Priority: P0
- Evidence: Integration, Runtime
- Given valid formulas and no owned viewer
- When completion is processed
- Then one right-side plugin split is opened with focus disabled and receives the image.
- And the split request uses `target_pane_id` without `workspace_id`, then validates the workspace returned by Herdr.
- And after metadata reporting returns `ok`, the plugin re-reads the viewer pane and validates its ownership metadata.

### AT-501 - Viewer reuse

- Priority: P0
- Evidence: Integration, Runtime
- Given an owned viewer already exists
- When another valid answer completes
- Then the same pane id is updated and the pane count does not increase.

### AT-502 - Source focus preservation

- Priority: P0
- Evidence: Integration, Runtime
- Given the source pane is focused
- When the viewer is created or updated
- Then the source pane remains focused.

### AT-503 - Replace without clear

- Priority: P0
- Evidence: Integration
- Given a previous valid image and a new valid image
- When replacement occurs
- Then one `pane.graphics.set` request is issued without a preceding clear, and the new layer replaces the old layer.

### AT-504 - Invalid update preserves previous image

- Priority: P0
- Evidence: Integration, Runtime
- Given a valid viewer image
- When the next answer has invalid LaTeX, exceeds a limit, times out, or fails graphics validation
- Then the existing image remains visible.

### AT-505 - Closed viewer recreation

- Priority: P0
- Evidence: Integration, Runtime
- Given the user closes the owned viewer
- When the next valid formula answer completes
- Then stale state is discarded and exactly one replacement viewer is created.
- And a `pane_not_found` response from `pane.get` is treated as authoritative absence, not as a protocol failure.

### AT-506 - Viewer ownership validation

- Priority: P0
- Evidence: Integration
- Given state points to an existing user pane that is not owned by Herdr Math
- When an update is attempted
- Then the pane is not modified or closed; ownership recovery creates or finds a valid plugin viewer.

### AT-507 - Graphics disabled

- Priority: P0
- Evidence: Integration, Runtime
- Given `[experimental].kitty_graphics` is false
- When rendering would otherwise occur
- Then the plugin returns `graphics_disabled`, provides the exact configuration action through diagnostics, does not loop, and does not create a useless viewer.

### AT-508 - Cell size unavailable

- Priority: P0
- Evidence: Integration, Runtime
- Given graphics are enabled but cell dimensions are zero or unavailable
- When placement is attempted
- Then the plugin returns `cell_size_unavailable`, suggests reattaching a compatible client, and preserves the previous viewer.

### AT-509 - Resize behavior

- Priority: P0
- Evidence: Runtime
- Given a visible image
- When the split ratio changes and another formula is rendered
- Then placement uses current cell and layout dimensions, remains within the viewer, and does not create a second pane.

### AT-510 - Source pane closure cleanup

- Priority: P0
- Evidence: Integration
- Given source state and an owned viewer mapping
- When the source pane closes
- Then source fingerprint state is removed and no later unrelated pane reuse can trigger an update.

### AT-511 - No-formula completion

- Priority: P0
- Evidence: Integration, Runtime
- Given a proven answer delta with no formulas
- When completion is processed
- Then no viewer is created, an existing viewer is unchanged, and the final digest is recorded to suppress duplicates.

## G. State, Concurrency, Privacy, and Recovery

### AT-600 - Session namespace isolation

- Priority: P0
- Evidence: Unit, Integration, Runtime
- Given the same pane id exists in default and named Herdr sessions
- When both process events
- Then locks, fingerprints, viewer mappings, and processed digests cannot collide.

### AT-601 - Atomic concurrent completion

- Priority: P0
- Evidence: Integration
- Given multiple completion workers start for one pane
- When they contend for state
- Then at most one worker renders, state remains valid JSON, and no temporary file is treated as canonical state.

### AT-602 - Stale-lock recovery

- Priority: P0
- Evidence: Unit, Integration, Runtime
- Given a lock from a terminated worker or stopped isolated server
- When a later event arrives after the stale threshold
- Then the lock is recovered using age and liveness checks and valid processing resumes.

### AT-603 - Live-lock protection

- Priority: P0
- Evidence: Integration
- Given a live worker owns the lock
- When another worker arrives
- Then the second worker does not remove the lock, does not render concurrently, and exits or retries within a bounded policy.

### AT-604 - Corrupt state

- Priority: P0
- Evidence: Unit, Integration
- Given malformed, oversized, unknown-version, or path-manipulating state
- When loaded
- Then it returns `state_corrupt`, quarantines or replaces only the affected plugin state, and never follows an attacker-controlled path.

### AT-605 - State permissions

- Priority: P0
- Evidence: Static, Runtime
- Given a fresh state secret, lock, temporary file, and pane-state file
- When filesystem permissions are inspected
- Then they are restricted to the current user according to the supported platform policy.

### AT-606 - Startup cleanup is one-shot

- Priority: P0
- Evidence: Integration, Runtime
- Given stale temporary files and expired state
- When the Herdr server runs the plugin startup hook
- Then cleanup completes and exits; no background controller remains.

### AT-607 - Server restart recovery

- Priority: P0
- Evidence: Runtime
- Given an isolated named session with existing viewer ownership state
- When the server is stopped and restarted
- Then startup cleanup exits, later event hooks run normally, stale locks do not block work, and the default user session is unaffected.

### AT-608 - Log privacy

- Priority: P0
- Evidence: Integration, Static
- Given sentinel answer text, formula text, local paths, and environment secrets in the test harness
- When success and every error path run
- Then Herdr plugin logs contain none of those sentinel values and only allowlisted fields.

### AT-609 - No environment dump

- Priority: P0
- Evidence: Static, Integration
- When diagnostics and unexpected errors run
- Then they never serialize `process.env`, arbitrary request objects, arbitrary exception objects, or full Herdr event JSON.

### AT-610 - State expiry

- Priority: P0
- Evidence: Unit, Integration
- Given an abandoned generation past its expiry
- When cleanup or a new event runs
- Then it is removed without affecting a newer generation or another session.

## H. Compatibility, Documentation, and Release

### AT-700 - Minimum supported macOS runtime

- Priority: P0 if macOS is declared
- Evidence: Install, Runtime
- Given each declared macOS architecture and minimum runtime
- When clean install, render, event, viewer, resize, and restart tests run
- Then all release-gate cases pass.

### AT-701 - Linux runtime

- Priority: P1 unless Linux is declared for `0.1.0`
- Evidence: Install, Runtime
- Given each declared Linux architecture and terminal
- When the release matrix runs
- Then clean native dependency installation and full graphics behavior pass before `linux` is added to the manifest.

### AT-702 - Windows runtime

- Priority: P2 unless explicitly promoted
- Evidence: Install, Runtime
- Windows is not claimed until named-pipe, build-command, native dependency, graphics, and lifecycle tests pass on the Herdr Windows beta.

### AT-703 - Ghostty compatibility

- Priority: P0 for the initial verified terminal
- Evidence: Runtime
- Given the release versions of Herdr, Herdr Math, and Ghostty
- When first display, replacement, resize, invalid preservation, viewer recreation, and restart cases run
- Then all pass without raw escape text or focus loss.

### AT-704 - Additional terminal compatibility

- Priority: P1
- Evidence: Runtime
- Kitty, WezTerm, or another terminal is listed as verified only after the same matrix as AT-703 passes through Herdr.

### AT-705 - Unsupported terminal messaging

- Priority: P0
- Evidence: Runtime, Documentation
- Given an attached client without working graphics
- When diagnostics run
- Then documentation and output distinguish disabled Herdr configuration, unavailable cell size, and unverified terminal support without claiming Ghostty installation is mandatory.

### AT-706 - English public surface

- Priority: P0
- Evidence: Static
- When README, docs, specs, manifest text, commands, logs, comments, release notes, issue templates, and contribution files are scanned
- Then user-facing content is English except fixtures explicitly testing multilingual behavior.

### AT-707 - Required public documentation

- Priority: P0
- Evidence: Static
- Before release, the repository contains accurate installation, configuration, usage, troubleshooting, privacy/security, compatibility, contribution, license, changelog, uninstall, and known-limit documentation.

### AT-708 - License and dependency notices

- Priority: P0
- Evidence: Static
- Given production dependencies and distributed assets
- When license metadata is audited
- Then the repository license, dependency licenses, font licenses, and required notices permit the planned distribution and are included where required.

### AT-709 - Secret and artifact scan

- Priority: P0
- Evidence: Static
- When the complete release tree and Git diff are scanned
- Then no credential pattern, local transcript, username, home path, unredacted screenshot, browser profile, state file, lock file, generated diagnostic log, or private fixture is present.

### AT-710 - Release install from immutable tag

- Priority: P0
- Evidence: Install, Runtime
- Given the proposed immutable release tag
- When a clean user installs that tag and runs the documented first-use flow
- Then installation and runtime pass without using the development checkout, unpublished files, global package state, or another repository's dependencies.

### AT-711 - Marketplace metadata

- Priority: P0
- Evidence: Static, Runtime
- Given the install test passes
- When repository metadata is prepared
- Then the description is accurate, topics include `herdr-plugin`, `latex`, `math`, and `terminal`, and no marketplace listing is requested before release readiness.

## Release Acceptance Rule

Release `0.1.0` only when:

1. Every P0 case applicable to the declared manifest platforms is passed.
2. Every result has current evidence from the public implementation.
3. P1 and P2 gaps are described as unsupported or planned, not implied as working.
4. The task checklist contains no incomplete release-gate task.
5. The final clean-tag install test passes after all release files are committed.
