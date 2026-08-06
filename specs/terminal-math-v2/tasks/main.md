> **SUPERSEDED**: V2 task tracking is complete for the shipped `0.2.0` architecture.
> Active work lives in `specs/terminal-math-v3/tasks/main.md`. This checklist is
> kept as historical reference only.

# Terminal Math V2 Task List

## Status

- Target release: `0.2.0` (first standalone release without a Herdr runtime)
- Last updated: August 2, 2026
- Acceptance contract: `../tests/main.md`
- Implementation plan: `../plans/main.md`
- Predecessor spec: `../../../specs/herdr-math-v1/tasks/main.md`

## Progress Rules

- One task represents one reviewable logical change and normally one implementation commit.
- Mark a task complete only after every listed acceptance test passes with the stated evidence.
- Commit a task's implementation first. Commit this checklist and related documentation progress
  separately immediately afterward.
- If code reality invalidates the plan, update all three specification documents before continuing.
- Do not skip a task because a prototype contains similar code. Port behavior selectively and
  re-verify it in this repository.
- P1 tasks may remain open for `0.2.0` only when the associated platform, terminal, or capability
  is explicitly excluded from release claims.
- No task may claim a "pass" from CI mocks alone where the acceptance test requires runtime,
  render, or install evidence.

## Current Next Task

Phases 0-7 (terminal surface, render transport, placement, input loop, CLI and composition,
hardening, Herdr removal and docs, release-gate prep) are complete; the **0.2.0 release gate**
remains: real Ghostty runtime evidence (T-703) and clean-tag install validation (T-801/T-802)
must be recorded before publishing.

---

## Phase 0 - Rust Terminal Surface

The goal of Phase 0 is a self-contained Rust workspace that ports the terminal-facing pieces of
`terminal-browser`'s `pixel-core` crate: Kitty escape construction, terminal init/reset, mouse
parsing, and the scroll state machine, plus a native macOS Swift scroll helper. All logic must be
unit-tested before any real terminal is involved, using a fake termios / virtual-terminal harness.
The TypeScript renderer is untouched in this phase.

### T-100: Create the v2 documentation baseline

- Scope: Add `specs/terminal-math-v2/tests/main.md` and this task list, update `plans/main.md` to
  mark Phase 0 in progress, and keep V1 specs intact and superseded-not-deleted.
- Dependencies: None
- Acceptance: `AT-2-001`, links resolve; docs distinguish planned vs implemented.
- Evidence: Static
- Commit: `docs(spec): define terminal-math v2 acceptance and task baseline`

### T-101: Scaffold the Rust workspace and quality tooling

- Scope: Add a Cargo workspace carrying a `tmath-core` crate, `.gitignore` entries for the
  `target/` directory, rustfmt/clippy configuration, and a reproducible `Cargo.lock`. Keep the
  crate free of Herdr coupling and free of user-specific paths. Do not add a full CLI binary yet.
- Dependencies: T-100
- Acceptance: `AT-2-001`, `AT-2-004`, `AT-2-005`; `cargo test`, `cargo clippy`, and `cargo fmt
  --check` pass from a clean checkout.
- Evidence: Contract, Static
- Commit: `chore(rust): scaffold terminal-math workspace`

### T-102: Port the Kitty escape construction module

- Scope: Port `pixel-core/src/kitty.rs` `kitty_transmit_placed` chunking
  (`a=T,f=32,o=z,s,v,t=d,i,q=2,m`), virtual placement `U=1,c,r`, cursor placement `p=1,C=1`,
  `placeholder_grid` per-cell combining-character encoding, `kitty_delete`, and the `a=q`
  media/format probe. All emits target stdout; no tmux passthrough in V2. Add deterministic unit
  tests for chunk boundaries, compression round-trips, placement keys, placeholder encoding, and
  delete sequences.
- Dependencies: T-101
- Acceptance: `AT-2-100` through `AT-2-105`
- Evidence: Unit
- Commit: `feat(kitty): transmit placed scrollback-anchored images`

### T-103: Port terminal init/reset with a fake-tty harness

- Scope: Port termios raw mode, saved/restore attributes, mode-enable strings (all-motion `?1003h`,
  SGR mouse `?1006h`, pixel mouse `?1016h`, bracketed paste `?2004h`) and their reset strings,
  cell-size probe (`ESC[16t`, winsize fallback), and pixel-mouse probe (`?1016$p` DECRQM) with
  `read_report`-style timeout handling. The harness must be injectable so escape bytes are
  asserted with no real terminal and must **not** enter the alternate screen (`?1049h`).
- Dependencies: T-101, T-102
- Acceptance: `AT-2-106` through `AT-2-109`
- Evidence: Unit, Integration (fake tty)
- Commit: `feat(terminal): initialize and reset the main-buffer terminal`

### T-104: Port the SGR/pixel mouse parser

- Scope: Port `parse_sgr_mouse` (`<b;x;y` decoding for wheel/button/motion/release, modifier bits),
  the SGR/CSI event framing, and cell-to-pixel coordinate conversion with the measured cell size.
  Add unit tests for every kind, modifier combos, zero-coordinate rejection, and framing.
- Dependencies: T-103
- Acceptance: `AT-2-110`, `AT-2-111`
- Evidence: Unit
- Commit: `feat(terminal): decode SGR and pixel mouse input`

### T-105: Port the scroll state machine

- Scope: Port `ScrollState` (`tick`, `step`, `settled`, `chase`) and the `Smooth` profile
  (exponential easing, brake once the stream goes quiet), adapted to a single-document context.
- Dependencies: T-101
- Acceptance: `AT-2-112`
- Evidence: Unit
- Commit: `feat(scroll): animate document scrolling with smoothing`

### T-106: Port the native macOS scroll helper

- Scope: Port `native-scroll-helper.swift` and `build.rs` (compile with `swiftc` on macOS, set
  `NATIVE_SCROLL_HELPER`, skip cleanly off-macOS), and the `native.rs` line protocol (`s`, `z`,
  `m`, `w`, `scale`) with a subscriber/waker model.
- Dependencies: T-101
- Acceptance: `AT-2-113`, `AT-2-114`
- Evidence: Unit, Static
- Commit: `feat(native): add macOS trackpad scroll helper`

### T-107: Phase 0 integration gate

- Scope: Run the full Rust suite (`cargo test`), clippy, and fmt from a clean checkout; confirm no
  Herdr imports, no absolute paths; record the Phase 0 outcome in docs. No release claim is made.
- Dependencies: T-102 through T-106
- Acceptance: `AT-2-004`, `AT-2-005`; every Phase 0 case passes.
- Evidence: Contract, Static
- Commit: `docs(test): record phase 0 terminal-surface evidence`

Progress: T-100 through T-107 complete via commits `83111fd` (docs),
`958b9f3` (workspace), `102d014` (kitty), `764d8c6` (terminal), `aad464d` (mouse),
`d21b7c7` (scroll), `9a8a60f` (native). Evidence: `docs/evidence/2026-08-02-tmath-v2-phase0.md`.

---

## Phase 1 - Render Transport

The goal of Phase 1 is the versioned JSON IPC between the Rust `tmath` binary and the
one-shot TypeScript renderer subprocess, reusing the existing `renderer/*` pipeline. The
subprocess must read exactly one bounded request on stdin, write exactly one bounded response
(a transparent PNG plus dimensions, or a stable error) on stdout, and exit. The IPC boundary
enforces size, timeout, and trust limits.

### T-201: Define the versioned render IPC contract

- Scope: Define the `tmath-render/1` protocol: a request carrying the source document, optional
  pre-scanned formulas, and render options; a success response carrying width, height, byte size,
  renderer name, and base64 PNG; and an error response carrying a safe error record. Encode the
  protocol version and the bounded request/response size limits on both sides (TypeScript types and
  Rust serde structs) and add contract fixtures.
- Dependencies: T-107
- Acceptance: `AT-2-200`, `AT-2-202`
- Evidence: Unit, Contract
- Commit: `feat(ipc): define versioned render contract`

### T-202: Add the one-shot renderer subprocess

- Scope: Add a TS entrypoint that reads one bounded JSON request on stdin, renders the document
  with the existing pipeline (run the scanner when formulas are absent), applies the render timeout
  and trust policy, writes one bounded JSON response on stdout, and exits. It must never stay
  running after the response.
- Dependencies: T-201
- Acceptance: `AT-2-201`, `AT-2-203`
- Evidence: Contract, Integration
- Commit: `feat(renderer): add one-shot render subprocess`

### T-203: Add render IPC fixtures and integration tests

- Scope: Add request/response fixtures covering protocol version, formula and prose-plus-math
  documents, invalid LaTeX, over-limit input, and errors. Add integration tests running the
  subprocess end to end for one-request/one-response ordering, size limits, render timeout, trust
  rejection, and one-shot exit behavior.
- Dependencies: T-202
- Acceptance: `AT-2-200`, `AT-2-201`, `AT-2-202`
- Evidence: Contract, Integration
- Commit: `test(ipc): cover the render transport contract`

### T-204: Add a `tmath render` placeholder driving the subprocess

- Scope: Add the Rust `tmath` binary with a `render` placeholder that reads a whole request (file
  or `-` for stdin) and spawns the TS subprocess over pipes, forwarding the request and the bounded
  response with clean exit codes. No placement or input loop yet; the transport is the deliverable.
- Dependencies: T-202
- Acceptance: `AT-2-200`, `AT-2-201`
- Evidence: Contract, Integration
- Commit: `feat(cli): add tmath render transport placeholder`

Progress: T-201 through T-204 complete via commits `d554d17`, `c44874b`, `f8f65e3`, `3f667f7`
(plus `131f761` docs, and fix/style commits). Evidence: `docs/evidence/2026-08-02-tmath-v2-phase1.md`.

---

## Phase 2 - Placement and Scrollback Anchoring

The goal of Phase 2 is to transmit one Kitty placement per document block into the main screen
buffer, glued to real cells with a virtual placement (`U=1,c,r`) and a placeholder grid so the
images scroll with the shell scrollback. A placement tracker owns image ids, replacement/delete,
and the concurrent-pixel and total-pixel limits. The TypeScript renderer is unchanged beyond
Phase 1.

### T-301: Place one block per rendered document into the main buffer

- Scope: Add a `tmath-core` placement module that computes a cell grid from measured cell size
  (`cols = ceil(width/cell_w)`, `rows = ceil(height/cell_h)`, clamped to the addressable
  placeholder limit), emits the placed transmit (`a=T,f=32,o=z,s,v,t=d,i,q=2,U=1,c,r`) followed by
  a cursor-relative placeholder grid so the cells scroll with scrollback, and tracks the placed
  image id. Wire `tmath render` to initialize the terminal (main buffer only, no alternate
  screen), measure cell size, render, decode the PNG to RGBA, and place each block at its source
  row in order.
- Dependencies: T-204, T-103, T-102
- Acceptance: `AT-2-300`
- Evidence: Unit, Integration
- Commit: `feat(placement): place scrollback-anchored image blocks`

### T-302: Verify images scroll with the shell scrollback

- Scope: In a real Kitty-graphics terminal (Ghostty primary), render a multi-block document, then
  scroll the terminal back and forth and record that the images move with the real cells rather
  than staying pinned to the viewport. Record redacted runtime evidence.
- Dependencies: T-301
- Acceptance: `AT-2-301`
- Evidence: Runtime
- Commit: `docs(test): record scrollback placement runtime evidence`

### T-303: Replace and delete a pixel block

- Scope: Extend the placement tracker so re-rendering a block deletes its stale image
  (`a=d`) before or as part of the replacement, and removing a block deletes its image without
  leaving orphans. Use scoped `d=I,i=<id>` deletes so other blocks are untouched, with a stable
  error when a stale id is unknown.
- Dependencies: T-301
- Acceptance: `AT-2-302`
- Evidence: Unit, Integration
- Commit: `feat(placement): replace and delete stale image blocks`

### T-304: Enforce placement concurrency and pixel limits

- Scope: Add strict limits for the number of concurrent placements and the total placed pixels,
  rejected before emission with a stable error and no partial output. Invalid input, missing
  Kitty support, render timeouts, and payload rejection leave earlier valid placements intact and
  fail closed.
- Dependencies: T-301, T-303
- Acceptance: `AT-2-303`, `AT-2-304`
- Evidence: Unit, Contract
- Commit: `feat(placement): cap concurrent placements and pixels`

Progress: T-301 through T-304 implemented via commits `65a4825`, `8948775`, `f22b841` (plus
`d25cba8` docs). T-302 runtime observation remains a manual Ghostty step.
Evidence: `docs/evidence/2026-08-02-tmath-v2-phase2.md`.

---

## Phase 3 - Input Loop

The goal of Phase 3 is the interactive input loop: a bounded incremental decoder that turns raw
stdin bytes into mouse and keyboard events, a scroll driver that maps wheel deltas and fallback
keys through the existing `ScrollState`, and a clean `q`/`Ctrl-C` reset that always restores the
terminal. Bracketed paste and focus events are handled without leaking raw escape text.

### T-401: Bind mouse wheel input to the scroll state machine

- Scope: Add an incremental input decoder (`input.rs`) that buffers bounded bytes and emits one
  event at a time, decoding SGR pixel/cell mouse reports, arrows, `PgUp`/`PgDn`, `Home`/`End`,
  `j`/`k`/`g`/`G`, `q`, and `Ctrl-C`. Feed wheel deltas into `ScrollState` with the `Smooth`
  profile and bound per-frame work.
- Dependencies: T-112 (scroll), T-110 (mouse), T-103 (terminal)
- Acceptance: `AT-2-400`, `AT-2-403`
- Evidence: Unit
- Commit: `feat(input): decode bounded terminal input events`

### T-402: Keyboard fallback scrolling and clean reset

- Scope: Map fallback keys (arrows, `PgUp`/`PgDn`, `j`/`k`, `g`/`G`) to scroll offsets so the
  document can be scrolled without a mouse. `q` and `Ctrl-C` must reset the terminal on any exit
  path.
- Dependencies: T-401
- Acceptance: `AT-2-401`, `AT-2-404`
- Evidence: Unit, Integration
- Commit: `feat(input): scroll with keyboard fallback and reset cleanly`

### T-403: Bracketed paste and focus hygiene

- Scope: Decode bracketed-paste spans as single paste events without echoing or forwarding
  `ESC [ 200~` / `ESC [ 201~` markers, and decode focus in/out (`CSI I`/`CSI O`) into bounded
  focus events. Caps the recording buffer so an unclosed paste cannot grow unbounded.
- Dependencies: T-401
- Acceptance: `AT-2-403`
- Evidence: Unit
- Commit: `feat(input): handle bracketed paste and focus events`

### T-404: Fuzz and bounds coverage for the input parsers

- Scope: Run adversarial byte sequences (truncated CSIs, unclosed pastes, oversized parameter
  runs, garbage escapes) through the decoder; assert bounded allocation, stable event recovery at
  the next valid boundary, and no panics. Wire the decoder into `tmath render`'s real-terminal
  input loop with capped buffering.
- Dependencies: T-401, T-403
- Acceptance: `AT-2-403` (bounds), `AT-2-404` (reset)
- Evidence: Unit, Contract
- Commit: `test(input): fuzz and bound the input decoders`

Progress: T-401 through T-404 complete via commits `d5bcca3`, `3627579`, `dbba4a9`, `ba8ff71`
(plus `5133ddf` docs). Real-terminal mouse-wheel verification is part of the manual Ghostty step.
Evidence: `docs/evidence/2026-08-02-tmath-v2-phase3.md`.

---

## Phase 4 - CLI and Document Composition

The goal of Phase 4 is the user-facing CLI: `tmath render <path | ->` with bounded reads and
composition options (content width, font size), `tmath diagnose` for capability checks, and
accurate `--help`/`--version`. Document composition runs the scanner and the allowlisted Markdown
renderer through the Phase 1 transport.

### T-501: Harden `tmath render` file/stdin reads and composition options

- Scope: Keep `tmath render <path | ->` bounded for both file and stdin (cap at the request
  byte limit), and add `--content-width <px>` and `--font-size <px>` options that are forwarded
  through the IPC request to the renderer layout. Add CLI parser unit tests and an oversized-file
  rejection test.
- Dependencies: T-204, T-201
- Acceptance: `AT-2-501`
- Evidence: Unit, Integration
- Commit: `feat(cli): add bounded render reads and composition options`

### T-502: Compose prose and math documents through the standalone path

- Scope: Prove a Markdown document containing headings, prose, `$...$` and `$$...$$` math renders
  in source order through `tmath render` (scanner + allowlisted Markdown renderer via the
  Phase 1 transport), including a custom `--content-width` composition. Add a CLI integration test
  rendering such a document and asserting a transparent PNG result.
- Dependencies: T-501, T-202
- Acceptance: `AT-2-502`
- Evidence: Render, Integration
- Commit: `test(cli): compose prose and math documents end to end`

### T-503: Add `tmath diagnose` and accurate help/version text

- Scope: Add `tmath diagnose` reporting, in this order: renderer subprocess availability, node,
  a terminal for stdout, Kitty graphics support (probe), and cell size — each with a stable status
  and exit code. Expand `--help` to list commands and options and `--version` to print the crate
  version.
- Dependencies: T-501, T-103
- Acceptance: `AT-2-304`, `AT-2-706`
- Evidence: Contract, Integration
- Commit: `feat(cli): add capability diagnostics and help text`

Progress: T-501 through T-503 complete via commit `b3dc302` (the CLI refactor bundles bounded
reads, composition options, `diagnose`, and help/version; parse and composition are covered by
unit and renderer-integration tests). Evidence: `docs/evidence/2026-08-02-tmath-v2-phase4.md`.

## Phase 5 - Hardening and Security

The goal of Phase 5 is to close the security and privacy gaps in the new Rust/TS split: every
user input path (scanner, renderer, and CLI boundary) must fail closed with a stable error, all
aggregate limits must be enforced before emission, no content may leak into logs or state, and
the input/mouse/escape parsers and the scanner must withstand adversarial input.

### T-601: Fail closed on scanner and renderer limit errors

- Scope: Wrap the scanner and renderer calls in the render subprocess so scanner limit errors
  (`scanner_input_limit`) and renderer limit errors return a stable JSON error record instead of
  crashing the subprocess. Add an over-limit document test proving the subprocess still returns
  a bounded JSON error and exits cleanly.
- Dependencies: T-202
- Acceptance: `AT-2-501`, `AT-2-603`
- Evidence: Integration
- Commit: `fix(renderer): fail closed on scanner and renderer limits`

### T-602: Privacy audit for the new code paths

- Scope: Scan the new Rust + TS code paths for content leakage (no document text, formula source,
  or importable content in logs, diagnostics, or durable state). Extend the repo security gate to
  cover the Rust workspace output and CLI diagnostics, and record a bounded audit in evidence.
- Dependencies: T-601
- Acceptance: `AT-2-600`, `AT-2-601`, `AT-2-602`
- Evidence: Static, Integration
- Commit: `test(security): enforce local-only privacy invariants`

### T-603: Fuzz the input, mouse, escape, and scanner parsers

- Scope: Extend adversarial coverage across the decoder, mouse/escape decoders, and the TypeScript
  scanner with deterministic fuzz harnesses that assert bounded allocation, no panics, and event
  recovery at valid boundaries. Include over-limit and truncated scanner inputs.
- Dependencies: T-404, T-602
- Acceptance: `AT-2-403`, `AT-2-500`
- Evidence: Unit, Contract
- Commit: `test(security): fuzz input, mouse, and scanner parsers`

Progress: T-601 through T-603 complete via commits `a70a5d1`, `a6f104c`, `b7d6741` (plus
`1e9b118` docs). Evidence: `docs/evidence/2026-08-02-tmath-v2-phase5.md`.

## Phase 6 - Compatibility and Documentation

The goal of Phase 6 is to make the repository a self-consistent standalone product: remove every
Herdr-plugin artifact and `HERDR_*` read, update all source-of-truth and public docs to the
standalone identity, and record real-terminal compatibility evidence. V1 stays tagged for
rollback and its spec is marked superseded, never deleted.

### T-701: Remove the Herdr plugin contract

- Scope: Delete `herdr-plugin.toml`, `src/herdr`, `src/viewer`, `src/viewer.ts`, `src/graphics`,
  `src/manifest`, `src/on-agent-status.ts`, `src/on-pane-closed.ts`, `src/startup.ts`,
  `src/diagnose.ts`, `src/index.ts`, and the Herdr-coupled `src/boundary`, `src/state`,
  `src/config`, `src/events`, `src/presentation`, `src/diagnostics` trees, plus their tests,
  support rigs, reference oracles, and fixtures. Keep `answer-corpus.json` (scanner spec depends on
  it). Rewrite `package.json` scripts/files/keywords and the security gate so no `HERDR_*` key or
  deleted path remains, leaving no dangling imports.
- Dependencies: T-103 (check/build tooling)
- Acceptance: `AT-2-705`
- Evidence: Static, Contract
- Commit: `chore(v2): remove the herdr plugin contract`

### T-702: Document the standalone product and mark V1 superseded

- Scope: Update `AGENTS.md`, `docs/concept.md`, `docs/architecture.md`, `README.md`,
  `SECURITY.md`, `PRIVACY.md`, `getting-started`, CHANGELOG, and the V1 spec status to the
  standalone `tmath` identity. No public doc may claim Herdr plugin behavior; V1 is explicitly
  superseded, its tag remains for rollback.
- Dependencies: T-701
- Acceptance: `AT-2-702`, `AT-2-703`
- Evidence: Static
- Commit: `docs(v2): publish the standalone terminal math identity`

### T-703: Record Ghostty compatibility evidence

- Scope: In a real Kitty-graphics terminal (Ghostty primary), render multi-block documents and
  record placement, scrollback scroll, mouse wheel, keyboard fallback, replace, invalid
  preservation, and clean-exit results. Kitty/WezTerm stay P1 until the same matrix passes.
- Dependencies: T-702, T-302, T-400
- Acceptance: `AT-2-700`, `AT-2-701`
- Evidence: Runtime
- Commit: `docs(compat): record ghostty standalone evidence`

Progress (T-701, T-702): committed via `3e279e5` plus the Phase 6 docs. Evidence:
`docs/evidence/2026-08-02-tmath-v2-phase6.md`. T-703 remains a manual Ghostty runtime step with
the exact procedure recorded in that evidence; it must be completed before any runtime release
claim.

---

## Phase 7 - Release Gate

The goal of Phase 7 is to make the standalone product releasable: clean and reproducible build
with no local paths, agreed versions, `0.2.0` release preparation, sealed-gate validation, and a
recorded P1/P2 backlog. Publishing the tag is not done here; only the release-gate preparation is
completed.

### T-801: Reproducible clean build with no local paths

- Scope: Verify a clean checkout builds identically for Rust and TypeScript, confirm no
  user-specific absolute paths or runtime artifacts leak into the build, and confirm the declared
  validation surface (`npm ci`, `npm run check`, `npm test`, `npm run test:integration`, `npm run
  build`, `npm run smoke:render`, `cargo test`, `cargo clippy --all-targets`) passes from a clean
  tree.
- Dependencies: T-102, T-602
- Acceptance: `AT-2-003`, `AT-2-004`, `AT-2-005`
- Evidence: Static, Install
- Commit: `test(release): verify a reproducible clean build`

### T-802: Agree versions and prepare the 0.2.0 release

- Scope: Align `package.json`, `Cargo.toml`, and the changelog heading on `0.2.0`; add a
  `docs/RELEASE.md` (or equivalent) release checklist; record sealed-gate results and remaining
  limitations.
- Dependencies: T-801
- Acceptance: `AT-2-003`, `AT-2-706`
- Evidence: Static
- Commit: `chore(release): prepare the 0.2.0 release gate`

### T-803: Record the post-V2 backlog

- Scope: Record Linux, Windows, `watch`, additional terminals, and shared-memory/file media as
  P1/P2 post-V2 backlog with clear "planned/unsupported" labeling.
- Dependencies: T-802
- Acceptance: refresh of `AT-2-700`, `AT-2-701` labeling
- Evidence: Static
- Commit: `docs(release): record the post-v2 backlog`

Progress (T-801/T-802/T-803): see `docs/evidence/2026-08-02-tmath-v2-phase7.md`.

---

## Phase 8 - Agent Integration (tmux viewer)

The goal of Phase 8 is a P1 standalone extension: `tmath agent` watches a tmux
pane running a coding agent (Claude Code, Codex, opencode, Cursor, pi, and
similar), proves the newest answer boundary from `capture-pane` snapshots, and
sends the answer document over a bounded Unix socket to `tmath agent-viewer`,
which renders Markdown + math through the existing one-shot renderer and shows
it as a scrollback-anchored image in a right-side viewer pane. This phase is
not part of the `0.2.0` release gate.

### T-901: Detect agent answer boundaries from pane snapshots

- Scope: Add `tmath-core::agent::boundary` — `find_answer(baseline, completion)`
  returning display text for a new answer, with trailing-prompt and repainted
  working-frame stripping, and `None` for prompt-only, pure-repaint, and
  unrecoverable rewrites. Mirrors the strategies in `answer-corpus.json`; assert
  corpus-derived cases in Rust unit tests.
- Dependencies: none
- Acceptance: `AT-2-801`
- Evidence: Unit
- Commit: `feat(agent): detect agent answer boundaries from pane snapshots`

### T-902: Build tmux commands and the bounded message channel

- Scope: Add `tmath-core::agent::tmux` (validated `PaneId`, `split-window`
  with pane-id capture, `capture-pane` with bounded history, pane-kill and
  pane-existence commands) and `tmath-core::agent::codec` (length-prefixed JSON
  frames: `document`/`quit`, bounded decoder with resync). No shell evaluation
  of agent content.
- Dependencies: T-901
- Acceptance: `AT-2-802`
- Evidence: Unit
- Commit: `feat(agent): build tmux commands and bounded message channel`

### T-903: Add `tmath agent` and `tmath agent-viewer` commands

- Scope: `tmath agent` binds a temp-dir Unix socket, splits one viewer pane
  running `tmath agent-viewer <socket>`, polls the source pane, detects and
  debounces the newest answer, and sends it. `tmath agent-viewer` connects,
  renders each document through the one-shot renderer, replaces the previous
  placement, maps wheel/arrows to a shifted re-placement, and closes on
  `q`/`Ctrl-C`. Both fail closed and log no content. Extract the shared
  renderer-spawn helpers first.
- Dependencies: T-902
- Acceptance: `AT-2-800`, `AT-2-803`, `AT-2-804`
- Evidence: Unit, Contract, Integration
- Commit: `feat(cli): add tmath agent watcher and agent-viewer commands`

### T-904: Add tmux DCS passthrough and Ghostty evidence

- Scope: Wrap Kitty APC commands in `ESC P ... ESC \\` when `$TMUX` is set
  (superseded by T-907 structured `TerminalOp` transport). Record
  `scripts/smoke-agent-tmux.sh` results and
  direct-Ghostty placement evidence; record the tmux image-relay limitation
  (AT-2-806) with the fail-closed diagnostic it produces. Rewrite
  `docs/getting-started.md` and the compatibility matrix for the tmux setup
  and its limitation.
- Dependencies: T-903
- Acceptance: `AT-2-805`, `AT-2-806`, `AT-2-807`
- Evidence: Unit, Static, Runtime
- Commit: `docs(agent): record tmux passthrough and ghostty evidence`

### T-905: Probe real agents and record the boundary matrix

- Scope (P1, not release-gated): Run Claude Code, Codex, opencode, Cursor, and
  pi under `tmath agent`, record the completed-answer boundary and render
  result per agent, and extend the boundary rules for prompt styles not yet
  recognized (plain-text inline markers such as pi's `Current prompt > ...`).
- Dependencies: T-904
- Acceptance: `AT-2-801`, `AT-2-803` for each recorded agent
- Evidence: Runtime
- Commit: `test(agent): record real-agent boundary matrix`

### T-906: User-local install and agent skill

- Scope: Add `scripts/install.sh` (`npm run install:local`) that builds the
  release binary + renderer, installs them under
  `~/.local/share/tmath/app` (launcher in `~/.local/bin/tmath`), links a
  `tmath` SKILL.md into Claude Code/Codex/Cursor/opencode/pi skill dirs, and
  runs `tmath diagnose` as a post-install gate. Add renderer auto-discovery in
  `renderer_worker_path()` (env, then `<exe>/../renderer/dist/...`) so no
  `TMATH_RENDER_WORKER` is required. Add per-agent usage docs
  (`docs/coding-agents.md`) and fix the one-shot render subprocess entry/exit
  so it runs under a symlinked path (macOS `/tmp`) and flushes stdout.
- Dependencies: T-904
- Acceptance: `AT-2-808`
- Evidence: Static, Install, Integration
- Commit: `feat(install): user-local installer, auto-discovery, and agent skill`

### T-907: Correct and separate the tmux graphics transport

- Scope: Replace whole-buffer passthrough with structured pane-local and Kitty
  graphics operations. Double every embedded `ESC` in each independently
  wrapped Kitty APC; keep terminal modes, cursor movement, color CSI,
  placeholder cells, and line breaks in tmux's normal pane output.
- Dependencies: T-904
- Acceptance: `AT-2-805`
- Evidence: Unit, Integration
- Commit: `fix(tmux): separate graphics passthrough from pane output`

### T-908: Make viewer replacement and scrolling real

- Scope: Clear stale placeholder cells when a replacement becomes shorter and
  scroll long answers by cropping an RGBA viewport before replacing the image.
  Append each accepted answer below a bounded in-memory composite instead of
  discarding earlier answers, and normalize captured Unicode box tables back to
  Markdown tables before rendering.
- Dependencies: T-907
- Acceptance: `AT-2-803`, `AT-2-804`
- Evidence: Unit, Integration, Runtime
- Commit: `fix(agent): scroll the rendered answer viewport`

### T-909: Record terminal and coding-agent runtime matrices

- Scope: Verify visible pixels, pane clipping, redraw, resize, detach/attach,
  clean exit, and long-answer scrolling under Ghostty + tmux and cmux + tmux.
  Repeat completed-answer detection for Claude Code, Codex, Cursor CLI, pi,
  and opencode. Do not count a placement log or visible placeholder grid as a
  pixel pass.
- Dependencies: T-907, T-908
- Acceptance: `AT-2-806`, `AT-2-810`, `AT-2-811`
- Evidence: Runtime
- Commit: `test(agent): verify tmux terminal and agent matrices`

### T-910: Add the directory allowlist and its CLI

- Scope: Add `tmath-core`/`tmath` support for a per-directory opt-in list —
  new `engine/crates/tmath/src/agent_allowlist.rs` with `tmath agent-enable
  [<dir>]`, `tmath agent-disable [<dir>]`, and `tmath agent-allowed [<dir>]`.
  The allowlist file lives at
  `${XDG_CONFIG_HOME:-$HOME/.config}/tmath/agent-allowlist`, one
  canonicalized absolute path per line. `agent-allowed` matches by `Path`
  component (directory itself or any descendant), not string prefix, and is
  silent on both streams so a shell hot path can call it on every launch.
  Update `help_text()` and the `help_mentions_commands_and_options` test.
- Dependencies: T-909
- Acceptance: `AT-2-812`, `AT-2-813`
- Evidence: Unit
- Commit: `feat(agent): add directory allowlist for auto-watch`

### T-911: Auto-install the shell integration snippet

- Scope: `scripts/install.sh` writes `$APP/shell/tmath-agent.sh` and appends
  a marker-delimited block (`# >>> tmath shell integration >>>` /
  `# <<< tmath shell integration <<<`) to `~/.zshrc` and `~/.bashrc` that
  sources it. Re-running the installer replaces the block in place instead of
  duplicating it. `TMATH_SKIP_SHELL_INTEGRATION=1` skips rc editing entirely.
- Dependencies: T-910
- Acceptance: `AT-2-814`
- Evidence: Static, Install
- Commit: `feat(install): auto-install shell integration snippet`

### T-912: Add the coding-agent launcher wrapper

- Scope: `$APP/shell/tmath-agent.sh` defines `alias claude/codex/opencode/
  cursor-agent/pi` (alias, not a shell function, because `cursor-agent`
  contains a hyphen) that call `__tmath_wrap_agent <real-cmd> "$@"`. The
  wrapper checks `command -v` for both the real command and `tmath`, then
  `tmath agent-allowed`, and passes through untouched on any miss.
- Dependencies: T-911
- Acceptance: `AT-2-815`
- Evidence: Unit, Integration
- Commit: `feat(shell): add coding-agent launcher wrapper`

### T-913: Auto-start the watcher in and outside tmux

- Scope: Inside `$TMUX`, `__tmath_wrap_agent` starts `tmath agent
  --source-pane $TMUX_PANE` in the background before running the real
  command in place. Outside tmux with an interactive TTY (`[ -t 0 ] && [ -t
  1 ]`), it builds an explicit two-pane tmux session (agent pane running the
  real command, second pane running `tmath agent --source-pane <agent-pane>`
  directly) and attaches — a plain `tmux new-session <cmd>` never sources rc
  files, so the wrapper cannot rely on re-firing inside the new session.
  Outside tmux and non-interactive (pipes/redirects), the command runs
  untouched and `tmath` never starts.
- Dependencies: T-912
- Acceptance: `AT-2-816`
- Evidence: Unit, Integration
- Commit: `feat(shell): auto-start the watcher in and outside tmux`

### T-914: Prevent duplicate watchers on the same pane

- Scope: `__tmath_wrap_agent`'s in-tmux path takes a pane-id-scoped lock file
  (`${XDG_RUNTIME_DIR:-${TMPDIR:-/tmp}}/tmath-agent-pane-<id>.lock`) using
  `noclobber` for atomic create, storing the watcher's PID and reclaiming the
  lock via `kill -0` if the previous watcher died. This stays a shell-side
  concern; `tmath agent` itself keeps no multi-watcher restriction so a
  manual second watcher (e.g. a different `--percent`) remains possible.
- Dependencies: T-913
- Acceptance: `AT-2-817`
- Evidence: Unit, Integration
- Commit: `fix(shell): lock the pane before auto-starting a watcher`

### T-915: Add auto-watch smoke tests and docs

- Scope: Add `scripts/smoke-agent-allowlist.sh` (enable/disable/allowed
  round-trip, subdirectory match, sibling-directory rejection) and
  `scripts/smoke-install-shell-integration.sh` (idempotent rc block
  install/replace against a temporary `HOME`, `TMATH_SKIP_SHELL_INTEGRATION`
  honored). Document the opt-in flow in `docs/getting-started.md` and
  `docs/coding-agents.md`, and add the `CHANGELOG.md` entry.
- Dependencies: T-914
- Acceptance: `AT-2-812`-`AT-2-817` (documented, evidence attached)
- Evidence: Static, Integration
- Commit: `test(agent): add allowlist and shell-integration smoke tests`

### T-916: Drop the dedicated watcher pane from the outside-tmux launch

- Scope: `__tmath_start_in_new_tmux_session` creates a single-pane session
  running the wrapped command and starts the watcher through
  `__tmath_start_watcher_for_pane` (background process of the launching
  shell, pane-scoped lock included) instead of `tmux split-window`, so the
  session shows only the wrapped command plus the watcher's own viewer split
  — no third pane with watcher logs. `__tmath_env_prefix` is removed:
  transport env (`TMATH_TMUX_TRANSPORT`, `TMATH_DPR`, `TMATH_DEBUG_LOG`)
  reaches the watcher by ordinary inheritance. Supersedes the two-pane shape
  described in T-913.
- Dependencies: T-913, T-914
- Acceptance: `AT-2-816`, `AT-R-301`
- Evidence: Integration (`scripts/smoke-agent-wrapper-tmux.sh`)
- Commit: `fix(shell): start the outside-tmux watcher without its own pane`
