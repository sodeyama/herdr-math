# Herdr Math V1 Implementation Plan

## Status

- Plan state: Ready for implementation
- Target release: `0.1.0`
- Last updated: August 1, 2026
- Acceptance contract: `../tests/main.md`
- Task checklist: `../tasks/main.md`

## Objective

Build a self-contained, publicly installable Herdr plugin that renders LaTeX equations from the current completed response of Claude Code, Codex, Pi, or OpenCode in a reusable side pane.

The release must preserve the successful behavior of the August 1 prototype while replacing its local-only packaging, hard-coded paths, external dependency imports, long-running startup controller, and in-memory-only cross-event state assumptions.

## Release Outcome

A user with a supported Herdr, operating system, architecture, Node.js runtime, and outer terminal can run:

```sh
herdr plugin install sodeyama/herdr-math --ref v0.1.0
```

After enabling Herdr's experimental Kitty graphics setting, valid math in a supported agent's completed response appears automatically in one right-side viewer. Later answers reuse that viewer. No pane or equation content leaves the machine or appears in logs.

## Input Evidence

The prototype established:

- `pane.graphics.set` can display and replace PNG images in a plugin split.
- Ghostty 1.3.1 successfully carried the tested Herdr graphics path.
- A split can open without stealing source focus.
- One viewer can be reused and recreated after closure.
- Four answer-boundary strategies handled repaint and sliding-window cases.
- A stateful scanner handled the tested code, escape, shell-variable, price, and delimiter cases.
- Renderer and payload failures preserved the previous valid image.
- Session-specific stale locks recovered after an isolated Herdr restart.

The prototype did not establish clean installation, self-contained dependencies, official one-shot hook lifecycle, fingerprint-only state, cross-platform support, or public release hygiene.

## Non-Negotiable Decisions

1. Product name: `Herdr Math`.
2. Plugin id: `io.github.sodeyama.herdr-math`.
3. V1 placement: right split.
4. V1 agents: Claude Code, Codex, Pi, and OpenCode as identified by Herdr.
5. V1 syntax: `$...$` and `$$...$$` only.
6. Event model: manifest event hooks with bounded one-shot workers.
7. Startup model: cleanup/restore work that exits; no daemon launcher.
8. State model: cryptographic fingerprints and metadata only; no durable raw transcript.
9. Update model: validate completely, then call `pane.graphics.set` without clearing first.
10. Error model: fail closed and preserve the previous valid image.
11. Network model: no runtime network access or telemetry.
12. Compatibility model: declare only tested platforms and terminals.
13. Public language: English.

## Supported Coding Agents

V1 supports equations emitted by these coding agents. The implementation uses Herdr's canonical agent id and authoritative lifecycle event; it does not parse process names itself.

| Coding agent | Canonical Herdr id | Lifecycle authority to verify |
|---|---|---|
| Claude Code | `claude` | Herdr screen manifest; integration supplies session identity |
| Codex | `codex` | Herdr screen manifest; integration supplies session identity |
| Pi (pi.dev) | `pi` | Herdr lifecycle hooks when installed; otherwise screen manifest |
| OpenCode | `opencode` | Herdr lifecycle plugin when installed; otherwise screen manifest |

The table reflects the current official Herdr [Agents](https://herdr.dev/docs/agents/) and [Integrations](https://herdr.dev/docs/integrations/) documentation. Phase 1 must confirm the ids, event shape, status values, and minimum integration versions against the selected minimum Herdr release. Pi and OpenCode must not be enabled in the plugin allowlist until their recorded lifecycle evidence and derived tests exist. Phase 8 must record real lifecycle and render evidence for every row before `0.1.0`; support must not be inferred from another agent.

## Scope

### Included in `0.1.0`

- Repository-owned dependency graph and lockfile
- Production manifest with install build commands
- One-shot startup cleanup and event workers
- Strict Herdr event and socket protocol handling
- Claude Code, Codex, Pi, and OpenCode lifecycle compatibility
- Fingerprint-based answer boundary detection
- Conservative LaTeX scanner
- Locally rendered PNG output
- Viewer ownership, recovery, and focus preservation
- Diagnostics action
- Unit, contract, integration, rendering, install, and runtime tests
- Public documentation, security policy, license, changelog, and release notes
- At least one fully verified platform/architecture/terminal matrix

### Deferred

- Popup or overlay default placement
- Full Markdown rendering
- MathML input
- User themes and custom CSS
- Remote attach support claims
- Windows support
- Telemetry
- Automatic updates beyond Herdr's reinstall workflow
- Standalone binary packaging unless Node.js becomes a release blocker

## Target Architecture

The target flow is:

```text
pane.agent_status_changed event
  -> strict event decoder
  -> authoritative pane.get agent/status resolution
  -> per-pane atomic state machine
  -> working: store boundary fingerprint
  -> done/idle: prove answer delta
  -> conservative LaTeX scanner
  -> bounded local renderer
  -> validate viewer ownership and graphics capability
  -> pane.graphics.set
  -> record processed digest
```

`docs/architecture.md` defines module contracts, state shape, failure codes, and lifecycle details.

## Implementation Strategy

The work is ordered so that pure deterministic logic and privacy invariants are stable before Herdr runtime integration. Runtime claims are made only after clean-install and real-terminal evidence.

## Phase 0 - Planning and Repository Baseline

### Goals

- Establish English repository instructions and public design documents.
- Record the prototype evidence and production gaps.
- Create the acceptance, plan, and task specification triad.

### Deliverables

- `AGENTS.md` and referencing `CLAUDE.md`
- English README and documentation index
- Concept, architecture, and experiment report
- This specification triad

### Exit gate

- Internal links resolve.
- No document presents the plugin as released.
- The production architecture explicitly rejects an unsupervised startup daemon.
- The task list maps all P0 acceptance sections.

## Phase 1 - Public Package and Manifest Skeleton

### Goals

- Make the repository buildable without the prototype vault.
- Establish public legal and maintenance metadata.
- Create a manifest that can be validated before runtime implementation is complete.

### Work

1. Select and add a repository license. Do not assume a license from Herdr or prototype dependencies.
2. Add `package.json`, lockfile, supported Node.js range, module type, scripts, and package metadata.
3. Add `.gitignore`, `.editorconfig`, formatter/linter/type-check configuration, contribution guide, security policy, and changelog.
4. Add the planned source, test, fixture, script, and evidence directories.
5. Add a minimal `herdr-plugin.toml` with the public identity, build commands, startup cleanup, lifecycle events, diagnostics action, and viewer pane.
6. Confirm current manifest fields and event names against `herdr api schema --json` for the proposed minimum version.
7. Add a manifest validation script that checks identity, paths, version agreement, platform declarations, command targets, and forbidden diagnostic fixture entrypoints.
8. In an isolated real Herdr session, record redacted Pi and OpenCode detection, lifecycle authority, status transitions, integration versions, and pane-read behavior before adding them to the plugin allowlist.

### Tests

- AT-001 through AT-003
- AT-006 through AT-009
- AT-011
- AT-100 and the evidence prerequisite for AT-112
- AT-706

### Exit gate

- `npm ci`, static checks, and a placeholder build work from a clean clone.
- `herdr plugin link` validates the built skeleton without warnings.
- No runtime command points outside the repository.
- The chosen license and dependency policy are documented.
- Pi and OpenCode have recorded lifecycle evidence suitable for synthetic contract and integration fixtures.

## Phase 2 - Pure Scanner and Prototype Boundary Parity

### Goals

- Port only the proven pure logic first.
- Preserve the scanner and boundary regression behavior before redesigning persistence.

### Work

1. Port the LaTeX scanner into a typed module.
2. Port the prototype boundary algorithm into a reference implementation used only for parity tests.
3. Convert prototype tests to English names and public fixtures.
4. Add missing Unicode, delimiter-run, offset, byte-limit, and complexity tests.
5. Build a fixed answer corpus covering Claude Code, Codex, Pi, and OpenCode terminal patterns without real transcripts.
6. Add stable error and result types.

### Tests

- AT-200 through AT-206 using the reference baseline implementation
- AT-208
- AT-300 through AT-308

### Exit gate

- All ported prototype cases pass.
- The scanner has no renderer, filesystem, Herdr, or process dependency.
- All fixtures are synthetic and safe to publish.

## Phase 3 - Fingerprint State and Boundary Redesign

### Goals

- Replace raw cross-event baselines with non-reversible boundary fingerprints.
- Make lifecycle state atomic, session-scoped, idempotent, and concurrency-safe.

### Work

1. Define and version the fingerprint schema.
2. Generate a local keyed-fingerprint secret with restrictive permissions.
3. Implement full-baseline digest, prefix checkpoints, suffix-window digests, and contextual tail-anchor digests with baseline offsets, bidirectional context, and adjacent-gap formula HMACs.
4. Implement resolution against current pane text without loading a persisted baseline.
5. Run a parity suite comparing the fingerprint resolver with the prototype reference algorithm.
6. Implement safe session and pane key encoding.
7. Implement atomic state writes, exclusive per-pane locks, generation guards, expiry, stale-lock recovery, and corruption handling.
8. Prove with sentinel tests that state and logs contain no raw or reversibly encoded pane text.
9. Define behavior for missing baseline, occupant replacement, blocked/unknown status, and out-of-order events.

### Tests

- AT-103 through AT-111
- AT-200 through AT-211
- AT-600 through AT-605
- AT-608 through AT-610

### Exit gate

- Fingerprint resolution reaches parity on the fixed corpus.
- The repeated-prompt, sliding-window, alternate-screen middle-insertion, and middle-replacement regressions pass.
- No state file contains answer or formula text.
- Concurrent workers cannot corrupt state or render twice.

## Phase 4 - Renderer Selection and Implementation

### Goals

- Select a self-contained local renderer based on evidence rather than prototype convenience.
- Preserve formula coverage while minimizing installation and security cost.

### Candidate A: proven browser path

- KaTeX for parsing and HTML
- A headless browser for layout and screenshot
- Sharp or equivalent for PNG optimization

Advantages:

- Already passed the prototype corpus
- High CSS/KaTeX fidelity

Costs:

- Browser installation size and time
- More cleanup and timeout surface
- Platform-specific browser packaging

### Candidate B: browser-free SVG path

- A local math parser that produces SVG
- A local SVG-to-PNG renderer

Advantages:

- Smaller runtime surface if parity is achieved
- No browser process lifecycle

Risks:

- Formula or font behavior may differ from the prototype
- Native modules may still complicate architecture support

### Decision experiment

1. Freeze a formula corpus before comparing candidates.
2. Run valid, invalid, large, Unicode, aligned, matrix, and multiline cases.
3. Measure clean install bytes/time, cold and warm render latency, peak memory, PNG bytes, output dimensions, native artifacts, cleanup, and offline behavior.
4. Perform visual review on representative output.
5. Record the decision in an architecture decision record.
6. Prefer the browser-free path only if it meets behavior parity and materially improves packaging.

### Implementation work

- Implement a backend-neutral renderer interface.
- Add explicit trust and remote-resource denial.
- Enforce count, length, aggregate, timeout, dimension, raw-byte, and encoded-byte limits.
- Normalize backend errors into stable codes.
- Ensure every success, invalid-input, and timeout path releases resources.

### Tests

- AT-400 through AT-410
- AT-608 and AT-609 on renderer failures

### Exit gate

- One renderer is selected with recorded evidence.
- The selected renderer installs from the repository lockfile on every proposed release platform.
- The fixed corpus and all failure-recovery cases pass offline.

## Phase 5 - Herdr Protocol and Event Workers

### Goals

- Integrate the pure modules with current Herdr plugin lifecycle and socket contracts.
- Keep workers short-lived and idempotent.

### Work

1. Capture `herdr api schema --output <temporary-path>` for contract-test generation without committing machine-specific paths.
2. Confirm the canonical `claude`, `codex`, `pi`, and `opencode` ids and their lifecycle authorities against the selected minimum Herdr version.
3. Implement strict event decoding for the event-provided name, workspace id, pane id, status, and optional agent hint in `HERDR_PLUGIN_EVENT_JSON`; do not treat the optional agent hint as sole authority.
4. Implement a bounded newline-delimited JSON socket client.
5. Resolve the current pane with `pane.get`, then resolve the canonical agent id, workspace id, status, revision, and lifecycle `state_change_seq` with `agent.get`; fail closed when the methods disagree or the pane is missing, has moved workspaces, disagrees with an optional event agent hint, or has already changed status.
6. Map Herdr timeouts and errors into stable plugin errors.
7. Implement the `working`, `blocked`, `done`, `idle`, and `unknown` state machine.
8. Capture a bounded working snapshot before secret/state I/O or authoritative lookups, discard it unless later authority checks still match, and implement stable completion reads with a bounded debounce.
9. Add generation checks immediately before renderer and graphics commit points.
10. Implement one-shot startup cleanup.
11. Implement pane-closed cleanup.
12. Build a fake Herdr socket server for deterministic integration tests.

The v1 allowlist starts from the already evidenced Claude Code and Codex paths. Add Pi and OpenCode only after the Phase 1 lifecycle evidence has been converted into passing public fixtures; do not infer their behavior from another agent.

### Tests

- AT-002
- AT-100 through AT-113
- AT-601 through AT-604
- AT-606
- AT-608 and AT-609

### Exit gate

- Event hooks exit after bounded work.
- No startup controller remains running.
- Duplicate and reordered event suites produce at most one render.
- Malformed protocol input cannot cause unbounded reads or state writes.

## Phase 6 - Viewer Ownership and Graphics Placement

### Goals

- Open, recover, update, and recreate exactly one viewer per source pane.
- Preserve focus and the previous valid image.

### Work

1. Implement the viewer process and ownership metadata.
2. Validate stored pane ids against current plugin ownership before use.
3. Recover viewer ownership from metadata when state is missing or stale.
4. Open the viewer through the manifest entrypoint as a right split with focus disabled.
5. Implement graphics capability diagnostics.
6. Compute placement from current cell and viewer layout dimensions.
7. Validate raw bytes, encoded bytes, width, height, pixel count, columns, and rows.
8. Call `pane.graphics.set` only after complete validation.
9. Implement no-formula, invalid-render, closed-viewer, source-close, and resize behavior.
10. Add the `diagnose` action with allowlisted output.

### Tests

- AT-500 through AT-511
- AT-600
- AT-607

### Exit gate

- Fake-socket integration tests prove request order and ownership checks.
- A real Herdr smoke proves first render, same-pane update, focus preservation, resize, invalid preservation, closure, and recreation.

## Phase 7 - Integrated Hardening and Security

### Goals

- Re-run the complete prototype hardening matrix against the public architecture.
- Add release-specific privacy, packaging, and adversarial tests.

### Work

1. Join event, fingerprint, scanner, renderer, viewer, and state modules in a full integration harness.
2. Add synthetic Claude Code, Codex, Pi, and OpenCode lifecycle fixtures.
3. Test invalid LaTeX, formula count, length, timeout, image size, dimensions, no formula, price, shell variable, code, repeated prompt, truncation, and viewer closure in sequence.
4. Add malformed socket responses, slow socket, disconnect, viewer ownership spoof, state corruption, lock contention, and out-of-order events.
5. Run network-deny, filesystem-boundary, secret, environment-dump, dependency, license, and static executable-path checks.
6. Measure idle resource use and per-completion latency.
7. Confirm logs through `herdr plugin log list` contain only allowlisted fields.

### Tests

- All P0 Unit, Contract, Integration, Render, and Static cases

### Exit gate

- The full automated suite passes without retry.
- No raw sentinel content appears in state, logs, screenshots, or test output.
- No worker, browser, or file handle remains after the suite.
- Known limitations are documented rather than hidden by fallback behavior.

## Phase 8 - Runtime and Compatibility Matrix

### Goals

- Convert expected compatibility into measured support claims.
- Validate the exact installation path users will run.

### Work

1. Choose the first-release OS and architectures based on available clean runtime evidence.
2. Run a clean dependency build and local-link smoke on each candidate platform.
3. Run the full graphics matrix in Ghostty for the initial verified path.
4. Test at least one additional terminal when practical, but do not block a truthful macOS/Ghostty-only `0.1.0` on P1 expansion.
5. Test graphics disabled, unavailable cell size, fresh client attach, resize, and server restart.
6. Test default and named sessions.
7. Run the full formula lifecycle for Claude Code, Codex, Pi, and OpenCode, recording the canonical agent id, lifecycle authority, integration version when installed, status sequence, and render result for each.
8. Run remote attach only as an explicit experiment; document unsupported status if it is not proven.
9. Set manifest platforms and minimum Herdr version from the results.

### Tests

- AT-002 through AT-006
- AT-507 through AT-509
- AT-600, AT-602, AT-607
- AT-112
- AT-700 through AT-705

### Exit gate

- Every declared platform has complete install and runtime evidence.
- Every v1 coding agent has separate lifecycle and formula-render evidence.
- Ghostty wording is `verified`, not `required`.
- Unverified platforms and terminals are clearly labeled.

## Phase 9 - Public Documentation and Release

### Goals

- Make the plugin understandable, auditable, installable, and removable by a new user.
- Publish only after the immutable-tag install path succeeds.

### Work

1. Replace README planning language with tested install, configuration, usage, diagnose, troubleshooting, update/reinstall, and uninstall instructions.
2. Add `LICENSE`, `CONTRIBUTING.md`, `SECURITY.md`, `CHANGELOG.md`, compatibility table, privacy statement, and known limitations.
3. Add sanitized screenshots generated from the public implementation.
4. Audit dependency and font licenses and generate notices if required.
5. Add CI for clean install, static checks, unit/integration/render tests, build, and release artifact checks.
6. Prepare release notes with exact supported versions and terminals.
7. Commit the final version bump and changelog entry.
8. Create an immutable release candidate tag and run `herdr plugin install` from it in a clean environment.
9. Fix any issue in a new commit and tag; do not move a published tag.
10. Publish the GitHub release only after the final tag passes.
11. Add GitHub repository topics including `herdr-plugin` after install readiness.

### Tests

- AT-004, AT-005, AT-010, AT-011
- AT-706 through AT-711
- Full applicable P0 suite from the final tag

### Exit gate

- The final immutable tag passes clean install and real runtime smoke.
- Documentation contains no planned-as-released statements.
- The task list has no incomplete P0 release item.
- Repository metadata makes the plugin discoverable in the Herdr marketplace.

## Dependency Order

```text
Phase 0 documentation/specification
  -> Phase 1 package and manifest skeleton
  -> Phase 2 pure scanner/reference boundary
  -> Phase 3 fingerprint state
  -> Phase 4 renderer selection
  -> Phase 5 Herdr event/protocol integration
  -> Phase 6 viewer/graphics lifecycle
  -> Phase 7 integrated hardening
  -> Phase 8 runtime compatibility
  -> Phase 9 release
```

Phases 3 and 4 may proceed in parallel only after Phase 2 interfaces are stable. Phases 5 and 6 must not claim runtime completion until both state and renderer contracts are fixed.

## Intended Commit Sequence

Each task remains one logical commit, with progress/spec updates as separate documentation commits. A likely sequence is:

1. `docs(repo): define Herdr Math concept and v1 plan`
2. `chore(repo): add package and quality tooling`
3. `chore(plugin): add public manifest skeleton`
4. `test(scanner): add public formula corpus`
5. `feat(scanner): parse supported math delimiters`
6. `test(boundary): preserve prototype regressions`
7. `feat(boundary): add fingerprint baseline model`
8. `feat(state): add atomic session-scoped state`
9. `docs(renderer): record renderer decision`
10. `feat(renderer): render bounded local PNG output`
11. `feat(herdr): add bounded socket client`
12. `feat(events): process agent lifecycle hooks`
13. `feat(viewer): manage one viewer per source pane`
14. `feat(graphics): place validated images`
15. `feat(diagnostics): report local capability failures`
16. `test(integration): cover lifecycle hardening matrix`
17. `docs(release): add public setup and support policy`
18. `chore(release): prepare v0.1.0`

Do not combine these into one large implementation commit.

## Risk Register

| Risk | Impact | Mitigation | Release decision |
|---|---|---|---|
| Manifest event semantics differ from prototype subscription | Automatic lifecycle fails | Validate against minimum Herdr schema before porting controller logic | P0 blocker |
| Fingerprint resolver loses parity | Missed or wrong answer boundaries | Reference parity corpus and fail-closed policy | P0 blocker |
| Browser renderer makes installation too heavy | Poor adoption and build failures | Renderer selection gate with browser-free candidate | Documented decision required |
| Native image dependency lacks an architecture build | Install failure | Clean matrix before platform declaration | Remove platform or change backend |
| Done/idle hooks race | Duplicate pane or render | Atomic per-pane lock and processed digest | P0 blocker |
| Graphics flag/client state is unavailable | No visible result | Diagnose action and no useless viewer | Supported error path |
| User closes or reuses a pane id | Wrong-pane update | Metadata ownership validation | P0 blocker |
| Pane read behavior changes across Herdr versions | Boundary regression | Contract fixtures plus minimum-version runtime tests | Pin minimum or adapt |
| Coding agents use different lifecycle authorities or alternate-screen behavior | Missed completion or incomplete answer boundary | Per-agent contract fixtures and real runtime matrix for Claude Code, Codex, Pi, and OpenCode | P0 blocker for the affected agent |
| Logs expose terminal content | Privacy failure | Allowlisted structured logs and sentinel tests | P0 blocker |
| Outer terminal supports Kitty generally but not this Herdr path | False compatibility claim | Real matrix through Herdr | Do not claim support |
| No repository license | Cannot safely reuse or distribute | Explicit license selection in Phase 1 | P0 blocker |

## Rollback and Failure Policy

- Before release, revert the smallest failing task commit; do not weaken an acceptance test to make a failure green.
- After release, publish a patch release for code fixes. Do not move or replace an existing tag.
- If a Herdr update breaks an experimental graphics API, update compatibility documentation immediately and pin or raise `min_herdr_version` only after evidence.
- If a security or transcript-exposure issue is found, disable the affected automatic path, publish an advisory, and require credential rotation when exposure cannot be ruled out.
- If the release renderer cannot install reliably, do not ship a partially working plugin; return to the renderer gate.

## Definition of Done

V1 is done only when:

- Every applicable P0 acceptance test passes from the final tag.
- The plugin installs from GitHub without the development checkout.
- All dependencies and assets are repository-owned and licensed.
- Event hooks are bounded and startup cleanup exits.
- State and logs contain no raw transcript or formula text.
- Current-answer boundaries fail closed.
- One viewer is reused without focus loss or duplicate panes.
- Invalid input, timeouts, and payload failures preserve the last valid image.
- Every declared platform and terminal has real evidence.
- Claude Code, Codex, Pi, and OpenCode each have real lifecycle and formula-render evidence.
- English installation, security, compatibility, contribution, troubleshooting, and uninstall documentation is complete.
- The release version is consistent across tag, manifest, package, changelog, and release notes.
- The repository is ready for the `herdr-plugin` marketplace topic.
