# Herdr Math V1 Task List

## Status

- Target release: `0.1.0`
- Last updated: August 1, 2026
- Acceptance contract: `../tests/main.md`
- Implementation plan: `../plans/main.md`

## Progress Rules

- One task represents one reviewable logical change and normally one implementation commit.
- Mark a task complete only after every listed acceptance test passes with the stated evidence.
- Commit a task's implementation first. Commit this checklist and related documentation progress separately immediately afterward.
- If code reality invalidates the plan, update all three specification documents before continuing.
- Do not skip a task because a prototype contains similar code. Port behavior selectively and re-verify it in this repository.
- P1 tasks may remain open for `0.1.0` only when the associated platform or capability is explicitly excluded from release claims.

## Phase 0 - Repository Design Baseline

- [x] **T-000: Create the English planning baseline**
  - Scope: Add repository instructions, Claude reference file, README, concept, target architecture, experiment report, acceptance tests, implementation plan, and this task list.
  - Dependencies: None
  - Acceptance: Documentation links resolve; no document claims the plugin is released; startup hooks are documented as one-shot; prototype and target behavior are distinguished.
  - Evidence: Static review
  - Commit: `docs(repo): define Herdr Math concept and v1 plan`

## Phase 1 - Public Package and Manifest

- [x] **T-101: Select and add the public license**
  - Scope: Evaluate the intended distribution, prototype ownership, runtime dependency licenses, font licenses, and Herdr naming boundary; add `LICENSE` and the initial notice policy.
  - Dependencies: T-000
  - Acceptance tests: AT-708
  - Commit: `docs(legal): add project license and notice policy`

- [x] **T-102: Add package metadata and locked dependencies**
  - Scope: Add `package.json`, lockfile, supported Node.js range, module/type strategy, scripts, package metadata, and repository URLs without adding the renderer backend prematurely.
  - Dependencies: T-101
  - Acceptance tests: AT-001, AT-007, AT-009, AT-706
  - Commit: `chore(repo): add package metadata and lockfile`

- [x] **T-103: Add repository quality tooling**
  - Scope: Add `.gitignore`, `.editorconfig`, formatter, lint, type checking, test runner configuration, source/test directories, and initial `check`/`build` commands.
  - Dependencies: T-102
  - Acceptance tests: AT-008, AT-009, AT-706, AT-709
  - Commit: `chore(repo): add quality and build tooling`

- [x] **T-104: Capture and validate the Herdr manifest contract**
  - Scope: Export the schema from the proposed minimum Herdr version; confirm that `pane.agent_status_changed` carries required workspace id/pane id/status and an optional agent hint; confirm manifest fields, canonical `claude`/`codex`/`pi`/`opencode` ids, lifecycle authorities, and minimum integration versions; add schema-compatible public fixtures without machine paths.
  - Dependencies: T-103
  - Acceptance tests: AT-002, AT-003, AT-100, AT-112, AT-113
  - Commit: `test(herdr): add manifest and event contract fixtures`

- [x] **T-105: Add the public plugin manifest skeleton**
  - Scope: Add `herdr-plugin.toml` with public identity, build commands, one-shot startup cleanup, status and pane-close events, diagnostics action, viewer entrypoint, and conservative platform declaration.
  - Dependencies: T-104
  - Acceptance tests: AT-001, AT-002, AT-006, AT-011
  - Commit: `chore(plugin): add public manifest skeleton`

- [x] **T-106: Add manifest and release metadata validation**
  - Scope: Check version agreement, command targets, paths, platform declarations, unknown events, public text, and forbidden prototype fixture entrypoints.
  - Dependencies: T-105
  - Acceptance tests: AT-002, AT-008, AT-011, AT-706
  - Commit: `test(plugin): validate manifest and release metadata`

- [x] **T-107: Record Pi and OpenCode lifecycle evidence**
  - Scope: In an isolated real Herdr session, run Pi and OpenCode with the integrations required by the selected minimum Herdr version; record redacted canonical agent ids, lifecycle authorities, integration versions, observed status transitions, completion behavior, and alternate-screen pane-read behavior. Do not enable either agent in the plugin allowlist yet.
  - Dependencies: T-104
  - Acceptance tests: Evidence prerequisite for AT-100 and AT-112
  - Commit: `docs(test): record Pi and OpenCode lifecycle evidence`

## Phase 2 - Scanner and Reference Boundary Logic

- [x] **T-201: Create a synthetic public answer corpus**
  - Scope: Add synthetic Claude Code, Codex, Pi, and OpenCode answer fixtures covering valid math, code, prices, shell variables, escapes, repeated prompts, repaint changes, alternate-screen patterns, and truncated windows. Do not copy private transcripts.
  - Dependencies: T-103, T-107
  - Acceptance tests: AT-112, AT-203, AT-204, AT-202, AT-300 through AT-307, AT-709
  - Commit: `test(fixtures): add synthetic agent answer corpus`

- [x] **T-202: Implement the conservative LaTeX scanner**
  - Scope: Port and type the stateful `$...$`/`$$...$$` scanner with code, escape, ambiguity, offset, byte, delimiter, and complexity handling.
  - Dependencies: T-201
  - Acceptance tests: AT-300 through AT-308
  - Commit: `feat(scanner): parse supported math delimiters`

- [x] **T-203: Add the prototype boundary reference implementation**
  - Scope: Port exact-prefix, stable-prefix, sliding-window, and contextual-anchor behavior as a test oracle, including the repeated-prompt fix.
  - Dependencies: T-201
  - Acceptance tests: AT-200 through AT-206, AT-208
  - Commit: `test(boundary): preserve prototype boundary behavior`

- [x] **T-204: Define shared result, limit, and error contracts**
  - Scope: Add typed formulas, boundary results, rendered-image shape, policy limits, error codes, and safe error serialization shared by later modules.
  - Dependencies: T-202, T-203
  - Acceptance tests: AT-101, AT-308, AT-401 through AT-408
  - Commit: `feat(core): define bounded result and error contracts`

## Phase 3 - Fingerprint Boundary and Atomic State

- [x] **T-301: Define the boundary fingerprint schema**
  - Scope: Version the full digest, prefix checkpoints, suffix windows, contextual tail anchors, session/pane metadata, expiry, and processed-digest record.
  - Dependencies: T-203, T-204
  - Acceptance tests: AT-200 through AT-208, AT-207 in particular
  - Commit: `feat(boundary): define fingerprint baseline schema`

- [x] **T-302: Implement keyed fingerprint creation**
  - Scope: Create the local secret, restrictive permissions, bounded fingerprint builder, safe pane/session encoding, and immediate raw-text discard.
  - Dependencies: T-301
  - Acceptance tests: AT-103, AT-207, AT-208, AT-605, AT-608
  - Commit: `feat(boundary): create privacy-preserving baselines`

- [x] **T-303: Implement fingerprint answer resolution**
  - Scope: Resolve exact prefix, stable checkpoints, sliding windows, and contextual anchors against current text and compare parity with the reference implementation.
  - Dependencies: T-302
  - Acceptance tests: AT-200 through AT-208
  - Commit: `feat(boundary): resolve answers from fingerprints`

- [x] **T-304: Implement atomic session-scoped state**
  - Scope: Add state paths, exclusive locks, generation guards, atomic writes, size/version validation, corruption handling, expiry, and temporary-file cleanup.
  - Dependencies: T-302
  - Acceptance tests: AT-600 through AT-605, AT-610
  - Commit: `feat(state): add atomic session-scoped storage`

- [x] **T-305: Implement pure lifecycle transitions**
  - Scope: Model working, blocked, done, idle, unknown, missing baseline, duplicate final content, new generation, and occupant replacement without Herdr I/O.
  - Dependencies: T-303, T-304
  - Acceptance tests: AT-103 through AT-111, AT-601, AT-603
  - Commit: `feat(events): model idempotent agent lifecycle`

- [x] **T-306: Add fingerprint privacy and complexity gates**
  - Scope: Run sentinel, reversible-encoding, dictionary-like short-anchor, maximum-size, repeated-anchor, and timing tests; adjust schema thresholds without weakening boundary proof.
  - Dependencies: T-303, T-304
  - Acceptance tests: AT-207, AT-208, AT-608, AT-609
  - Commit: `test(boundary): enforce privacy and complexity limits`

## Phase 4 - Renderer Decision and Implementation

- [x] **T-401: Freeze the release formula corpus**
  - Scope: Add powers, fractions, roots, sums, integrals, aligned equations, matrices, Greek letters, Unicode, multiline, invalid, oversized, and link-capable inputs.
  - Dependencies: T-204
  - Acceptance tests: AT-400 through AT-405
  - Commit: `test(renderer): add release formula corpus`

- [x] **T-402: Prototype and measure renderer candidates**
  - Scope: Compare the proven browser path and a browser-free SVG path for correctness, visual output, clean install size/time, cold/warm latency, memory, PNG size, native modules, cleanup, and offline behavior.
  - Dependencies: T-401
  - Acceptance tests: AT-400, AT-405, AT-409, AT-410
  - Commit: `experiment(renderer): compare local rendering backends`

- [x] **T-403: Record the renderer decision**
  - Scope: Add an architecture decision record with measurements, selected backend, rejected alternative, install cost, security analysis, and supported architectures.
  - Dependencies: T-402
  - Acceptance tests: AT-410, AT-708
  - Commit: `docs(renderer): select the v1 rendering backend`

- [x] **T-404: Implement the backend-neutral renderer contract**
  - Scope: Add local assets, strict trust policy, remote-resource denial, count/length/timeout/dimension/byte limits, error mapping, and deterministic resource cleanup.
  - Dependencies: T-403
  - Acceptance tests: AT-400 through AT-409
  - Commit: `feat(renderer): render bounded local PNG output`

- [x] **T-405: Add renderer dependency and license audit**
  - Scope: Lock production dependencies, verify supported native artifacts, record licenses and fonts, remove unused packages, and prove the build has no external repository dependency.
  - Dependencies: T-404
  - Acceptance tests: AT-007, AT-405, AT-406, AT-708
  - Commit: `chore(renderer): lock and audit runtime dependencies`

## Phase 5 - Herdr Protocol and One-Shot Workers

- [x] **T-501: Implement strict Herdr event decoding**
  - Scope: Parse bounded `HERDR_PLUGIN_EVENT_JSON`, accept only the schema-compatible event name, workspace id, pane id, status, and optional agent hint, reject malformed ids and oversized payloads, and avoid logging full events. Agent allowlisting occurs only after an authoritative pane lookup.
  - Dependencies: T-104, T-107, T-204
  - Acceptance tests: AT-100 through AT-102, AT-112, AT-609
  - Commit: `feat(herdr): decode bounded plugin events`

- [x] **T-502: Implement the bounded Herdr socket client**
  - Scope: Add opaque socket-path handling, unique ids, response-size limits, method timeouts, JSON framing, disconnect behavior, `pane.get` support, and stable error mapping.
  - Dependencies: T-104, T-204
  - Acceptance tests: AT-002, AT-100, AT-101, AT-113, AT-608, AT-609
  - Commit: `feat(herdr): add bounded socket client`

- [x] **T-503: Build the fake Herdr server harness**
  - Scope: Simulate pane reads, pane lifecycle, layout, metadata, plugin pane opening, graphics capability, graphics updates, errors, delays, disconnects, and request recording.
  - Dependencies: T-502
  - Acceptance tests: Supports AT-100 through AT-113 and AT-500 through AT-511
  - Commit: `test(herdr): add fake socket integration server`

- [x] **T-504: Implement the agent-status event worker**
  - Scope: Connect event decoding, authoritative `pane.get` agent/status/revision resolution, supported-agent allowlisting, lifecycle state, baseline capture, stable completion reads, boundary resolution, scanning, rendering, generation checks, and processed digests in a bounded process.
  - Dependencies: T-305, T-404, T-501, T-503
  - Acceptance tests: AT-103 through AT-113, AT-200 through AT-208, AT-511, AT-601
  - Commit: `feat(events): process agent completion hooks`

- [x] **T-505: Implement one-shot startup cleanup**
  - Scope: Remove expired state, stale temporary files, and recoverable locks without launching a daemon or modifying live state.
  - Dependencies: T-304, T-501
  - Acceptance tests: AT-602, AT-603, AT-606, AT-610
  - Commit: `feat(state): clean expired plugin state at startup`

- [x] **T-506: Implement pane-close cleanup**
  - Scope: Distinguish source and viewer closure, remove only owned mappings, and prevent pane-id reuse from inheriting stale state.
  - Dependencies: T-304, T-501, T-503
  - Acceptance tests: AT-111, AT-505, AT-510
  - Commit: `feat(events): clean state when panes close`

## Phase 6 - Viewer and Graphics

- [x] **T-601: Implement the viewer entrypoint and metadata**
  - Scope: Add the bounded Herdr-managed viewer process, English title, ownership metadata, source-pane token, and safe exit behavior.
  - Dependencies: T-105, T-502
  - Acceptance tests: AT-500, AT-506, AT-510, AT-706
  - Commit: `feat(viewer): report plugin pane ownership`

- [x] **T-602: Implement viewer discovery and reuse**
  - Scope: Validate stored ids, recover by metadata, open one right split without focus, reuse it, and recreate after closure.
  - Dependencies: T-503, T-601
  - Acceptance tests: AT-500 through AT-502, AT-505, AT-506, AT-511
  - Commit: `feat(viewer): reuse one pane per source agent`

- [x] **T-603: Implement graphics capability and placement**
  - Scope: Read cell/layout dimensions, validate capability, compute bounded placement, enforce raw/encoded/dimension limits, and update through one `pane.graphics.set` call.
  - Dependencies: T-404, T-503, T-602
  - Acceptance tests: AT-407, AT-408, AT-503, AT-504, AT-507 through AT-509
  - Commit: `feat(graphics): place validated images in viewers`

- [x] **T-604: Implement privacy-safe diagnostics**
  - Scope: Check versions, authoritative environment presence, directories, renderer, graphics flag, cell size, and ownership using allowlisted output and stable error codes.
  - Dependencies: T-502, T-603
  - Acceptance tests: AT-507, AT-508, AT-608, AT-609, AT-705
  - Commit: `feat(diagnostics): explain local capability failures`

## Phase 7 - Integrated Hardening

- [ ] **T-701: Add the full lifecycle integration matrix**
  - Scope: Run Claude Code, Codex, Pi, and OpenCode through valid-math and completion cases, then run no-formula, code, price, variable, multiple, invalid, limits, timeout, recovery, repeated prompt, truncation, duplicate status, viewer close, and resize sequences through the fake server.
  - Dependencies: T-504, T-506, T-603
  - Acceptance tests: All applicable P0 Integration cases
  - Commit: `test(integration): cover the full formula lifecycle`

- [ ] **T-702: Add adversarial state and protocol tests**
  - Scope: Test malformed events, oversized JSON, slow responses, disconnects, corrupt state, path traversal attempts, viewer spoofing, lock contention, PID reuse assumptions, and out-of-order generations.
  - Dependencies: T-701
  - Acceptance tests: AT-101, AT-105, AT-111, AT-506, AT-601 through AT-604
  - Commit: `test(security): harden state and Herdr protocol boundaries`

- [ ] **T-703: Add privacy, network, and executable-path gates**
  - Scope: Run sentinel scans across state/log/output, deny network, scan source for environment dumps and executable input paths, and audit release fixtures and artifacts.
  - Dependencies: T-404, T-701
  - Acceptance tests: AT-207, AT-405, AT-406, AT-608, AT-609, AT-709
  - Commit: `test(security): enforce local-only privacy invariants`

- [ ] **T-704: Measure resource and latency behavior**
  - Scope: Record idle state, worker startup, boundary resolution, cold/warm render, memory, image size, and cleanup across repeated success/error runs; set regression budgets.
  - Dependencies: T-701
  - Acceptance tests: AT-208, AT-404, AT-409, AT-410
  - Commit: `test(perf): establish worker and renderer budgets`

- [ ] **T-705: Run the complete automated release suite without retry**
  - Scope: Execute clean install, checks, unit, contract, integration, render, build, static security, license, and artifact scans; save bounded evidence.
  - Dependencies: T-702, T-703, T-704
  - Acceptance tests: Every automated P0 case
  - Commit: `docs(test): record automated release evidence`

## Phase 8 - Real Herdr and Terminal Verification

- [ ] **T-801: Verify clean local-link development flow**
  - Scope: Build from a clean clone, link the checkout, inspect warnings/actions/events/panes, invoke diagnostics, and unlink without affecting unrelated plugins.
  - Dependencies: T-705
  - Acceptance tests: AT-002, AT-006, AT-010
  - Commit: `docs(test): record clean local-link verification`

- [ ] **T-802: Run the Ghostty runtime matrix**
  - Scope: In real Herdr and Ghostty, run Claude Code, Codex, Pi, and OpenCode separately; record each canonical agent id, lifecycle authority, installed integration version, status sequence, and valid-math render; then test first render, same-viewer update, no formula, focus, resize, invalid preservation, limit rejection, timeout recovery, viewer closure, and recreation.
  - Dependencies: T-801
  - Acceptance tests: AT-100, AT-107, AT-108, AT-112, AT-500 through AT-509, AT-511, AT-703
  - Commit: `docs(test): record Ghostty runtime evidence`

- [ ] **T-803: Verify default and named session isolation**
  - Scope: Use an isolated named session for stale lock and server restart tests while confirming the default session is unaffected.
  - Dependencies: T-802
  - Acceptance tests: AT-600, AT-602, AT-606, AT-607
  - Commit: `docs(test): record Herdr restart evidence`

- [ ] **T-804: Test an additional outer terminal**
  - Scope: Run the same graphics matrix in Kitty, WezTerm, or another candidate through Herdr; add support only if all required cases pass.
  - Dependencies: T-802
  - Priority: P1
  - Acceptance tests: AT-704
  - Commit: `docs(compat): verify an additional terminal`

- [ ] **T-805: Validate candidate release platforms and architectures**
  - Scope: Run clean install, native dependency, render, and runtime tests for each platform/architecture proposed in the manifest.
  - Dependencies: T-705
  - Acceptance tests: AT-700, and AT-701 if Linux is proposed
  - Commit: `docs(compat): record platform release matrix`

- [ ] **T-806: Finalize minimum Herdr version and manifest platforms**
  - Scope: Set values only from T-801 through T-805 evidence and document verified, expected, and unsupported combinations.
  - Dependencies: T-801, T-802, T-803, T-805
  - Acceptance tests: AT-002, AT-003, AT-700 through AT-705
  - Commit: `chore(plugin): finalize v1 compatibility metadata`

## Phase 9 - Public Documentation and Release

- [ ] **T-901: Write tested installation and usage documentation**
  - Scope: Replace planning status with clean install, local link, configuration, first use, supported coding-agent matrix, required Herdr integrations, diagnose, update/reinstall, uninstall, and limitations based on actual commands.
  - Dependencies: T-806
  - Acceptance tests: AT-004 through AT-006, AT-010, AT-705 through AT-707
  - Commit: `docs(readme): add tested install and usage guide`

- [ ] **T-902: Add contribution, security, changelog, and support files**
  - Scope: Add `CONTRIBUTING.md`, `SECURITY.md`, `CHANGELOG.md`, support policy, issue templates if needed, privacy statement, and disclosure instructions.
  - Dependencies: T-101, T-705
  - Acceptance tests: AT-706 through AT-709
  - Commit: `docs(project): add public maintenance policies`

- [ ] **T-903: Add sanitized public screenshots and evidence index**
  - Scope: Generate new screenshots from the public implementation, redact local labels/paths, compress assets, and link them without importing private prototype screens by default.
  - Dependencies: T-802
  - Acceptance tests: AT-709
  - Commit: `docs(media): add sanitized runtime screenshots`

- [ ] **T-904: Add continuous integration and release checks**
  - Scope: Run clean dependency install, checks, unit/contract/integration/render tests, build, version agreement, secret scan, license audit, and release-tree validation on supported CI platforms.
  - Dependencies: T-705, T-806
  - Acceptance tests: AT-009, AT-011, AT-700, AT-708, AT-709
  - Commit: `ci: enforce the v1 release gates`

- [ ] **T-905: Prepare version `0.1.0`**
  - Scope: Set version in manifest/package/changelog, write release notes, verify repository description and planned topics, and run the full suite from the release commit.
  - Dependencies: T-901, T-902, T-903, T-904
  - Acceptance tests: AT-001, AT-011, AT-706 through AT-711
  - Commit: `chore(release): prepare v0.1.0`

- [ ] **T-906: Run immutable-tag clean installation**
  - Scope: Create a release-candidate tag, install it through Herdr in a clean environment, run first-use and runtime smoke, and create a new tag rather than moving it if a fix is needed.
  - Dependencies: T-905
  - Acceptance tests: AT-004, AT-005, AT-710
  - Commit: Evidence-only follow-up if a new commit is required; never mutate a published tag.

- [ ] **T-907: Publish the first public release**
  - Scope: Publish the passing immutable tag and release notes, set accurate repository description/topics including `herdr-plugin`, verify marketplace discovery after refresh, and record the release URL and final matrix.
  - Dependencies: T-906 and every applicable P0 task
  - Acceptance tests: AT-711 and the Release Acceptance Rule
  - Commit: No code change required unless final metadata is version-controlled.

## Post-V1 Tasks

- [ ] **T-P01: Evaluate popup and overlay placement**
  - Priority: P2
  - Scope: Repeat the known-fixture and viewer lifecycle matrix for overlay and popup. Account for popup's lack of pane id and pane API participation.

- [ ] **T-P02: Evaluate remote attach graphics**
  - Priority: P2
  - Scope: Test local-client rendering against a remote Herdr server without inferring support from local sessions.

- [ ] **T-P03: Expand Linux support**
  - Priority: P1
  - Scope: Complete AT-701 and the full terminal matrix before adding Linux to the manifest.

- [ ] **T-P04: Evaluate Windows beta support**
  - Priority: P2
  - Scope: Test named pipes, build commands, renderer native dependencies, event hooks, and Windows Terminal graphics before any claim.

- [ ] **T-P05: Evaluate accessibility and alternate output**
  - Priority: P2
  - Scope: Explore accessible equation text, copy actions, MathML, and non-image fallbacks without changing v1 behavior silently.

## Current Next Task

Start with **T-101**. License selection must precede dependency and public distribution decisions.
