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

Phases 0-3 (Rust terminal surface, render transport, placement, input loop) are complete;
resume with **T-501** (Phase 4, CLI and document composition) next. T-302's real-terminal
scrollback observation and the Phase 3 mouse-wheel runtime step remain manual Ghostty
verification documented in the phase evidence; no runtime-release claim is made until they run.

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

## Phase 5 - Hardening and Security (outline)

- **T-601**: Revisit the threat model for the Rust/TS split; enforce all aggregate limits.
- **T-602**: Privacy audit for the new code paths (no content in logs/state).
- **T-603**: Fuzz the mouse/escape parser and the scanner.

## Phase 6 - Compatibility and Documentation (outline)

- **T-701**: Remove `herdr-plugin.toml`, `src/herdr`, `src/viewer`, `src/graphics`,
  `src/manifest`, `src/on-*.ts`, `src/startup.ts`, and all `HERDR_*` reads; fix dangling imports.
- **T-702**: Update `AGENTS.md`, `docs/concept.md`, `docs/architecture.md`, README, and
  security/privacy docs to the standalone product; mark V1 superseded.
- **T-703**: Record Ghostty evidence; mark kitty/WezTerm as P1.

## Phase 7 - Release Gate (outline)

- **T-801**: Clean build, install, validation; no local paths; reproducible build.
- **T-802**: Version tags agree; prepare `0.2.0` release notes.
- **T-803**: P1/P2 post-V2 backlog (Linux, Windows, `watch`, additional terminals).
