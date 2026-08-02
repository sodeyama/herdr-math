# Terminal Math V2 Acceptance Tests

## Status

- Specification state: Draft
- Target release: `0.2.0` (first standalone release without a Herdr runtime)
- Last updated: August 2, 2026
- Canonical plan: `../plans/main.md`
- Executable tasks: `../tasks/main.md`
- Predecessor spec: `../../../specs/herdr-math-v1/tests/main.md`

This document is a **plan**, not verified release behavior. Cases become acceptance
evidence only when the corresponding implementation phase lands and the stated evidence
is produced. A skipped, retried, flaky, partially implemented, or manually assumed case
is not a pass.

## Purpose

This document defines the acceptance contract for the V2 standalone refactor of Herdr
Math into a terminal math / document renderer (`tmath`) that renders Markdown + LaTeX as
scrollback-anchored Kitty placements with mouse interaction. Every P0 case for the
declared scope must pass with the required evidence before the `0.2.0` release gate.

## Priority

- **P0**: Required for the declared phase or release.
- **P1**: Required before claiming the associated optional platform, terminal, or capability.
- **P2**: Post-V2 exploration; not part of the `0.2.0` gate.

## Evidence Types

- **Unit**: deterministic automated test of a pure module (Rust or TypeScript).
- **Contract**: automated validation against recorded or generated schema-compatible fixtures
  (Kitty escape bytes, DECRQM replies, IPC messages).
- **Integration**: automated test using the real modules against a fake terminal / piped IPC.
- **Render**: real local renderer output with image assertions.
- **Runtime**: real Kitty-graphics-capable terminal (Ghostty primary) with no Herdr runtime present.
- **Install**: clean build and install of the published artifact.
- **Static**: source, dependency, license, secret, or artifact inspection.

Runtime evidence must record the date, operating system, architecture, outer terminal and
version, commands used, expected result, observed result, and redacted screenshot or
structured log where appropriate.

## Test Environment Rules

1. Rust and TypeScript unit tests run from a clean dependency installation.
2. Terminal compatibility is tested in the actual terminal, not inferred from generic Kitty
   graphics documentation.
3. No V2 acceptance fixture may contain credentials, private transcripts, real home-directory
   paths, or an environment dump.
4. `tmath` and `tmath-render` must build and run without a Herdr runtime, socket, manifest, or
   `HERDR_*` environment variables.
5. Phase 0 Rust cases may use a fake termios / virtual-terminal harness, but every escape byte
   and mouse decode must be asserted against the documented Kitty / xterm sequences.

## A. Standalone Identity and CLI

### AT-2-001 - Standalone product identity

- Priority: P0
- Evidence: Static
- Given the repository metadata, package manifests, and `Cargo.toml`
- When the public identity is inspected
- Then the product is described as a standalone terminal math / document renderer named
  `tmath`, the repository is `sodeyama/herdr-math`, and no public text claims Herdr plugin
  behavior or uses the `herdr-plugin` topic.

### AT-2-002 - No Herdr runtime dependency

- Priority: P0
- Evidence: Static, Integration
- Given a clean cloned checkout with the required Rust and Node toolchains
- When the declared build and `tmath render` are run with no Herdr installed and no
  `HERDR_*` environment variables present
- Then the build succeeds, runtime reads no socket and no manifest, and no Herdr API is
  called.

### AT-2-003 - Version agreement

- Priority: P0
- Evidence: Static
- When a release is prepared
- Then the release tag, the crate/workspace version, the package version, and the changelog
  heading use the same semantic version.

### AT-2-004 - No user-specific absolute paths

- Priority: P0
- Evidence: Static
- When source, tests, build files, generated output, and runtime behavior are inspected
- Then no local username, home-directory fallback, prototype path, or default socket path is
  present, and runtime artifacts go to a platform state directory.

### AT-2-005 - Clean build reproducibility

- Priority: P0
- Evidence: Static, Install
- Given two clean checkouts at the same commit with the locked dependencies and toolchains
- When the declared build runs
- Then both build outputs are functionally equivalent and the build does not mutate source.

## B. Phase 0 - Rust Terminal Surface

### AT-2-100 - Kitty placement transmit chunking

- Priority: P0
- Evidence: Unit
- Given a small RGBA canvas and a large RGBA canvas
- When the Kitty transmit sequence is built
- Then the first chunk carries `a=T,f=32,o=z,s=<width>,v=<height>,t=d,i=<id>,q=2` plus the
  placement keys, the payload is zlib-compressed and base64-encoded, chunks are at most the
  configured chunk size, intermediate chunks carry `m=1`, the final chunk carries `m=0`, and
  every chunk is terminated by `ESC \`.

### AT-2-101 - Virtual placement keys for scrollback anchoring

- Priority: P0
- Evidence: Unit
- Given a placement anchored to real cells rather than the cursor
- When the placement keys are emitted
- Then they are `U=1,c=<cols>,r=<rows>` and never include cursor keys `p=1` or `C=1`.

### AT-2-102 - Cursor placement keys

- Priority: P0
- Evidence: Unit
- Given legacy cursor-anchored placement
- When the placement keys are emitted
- Then they are `p=1,C=1`.

### AT-2-103 - Placeholder grid encoding

- Priority: P0
- Evidence: Unit
- Given an image id, columns, and rows
- When the placeholder grid is built
- Then it writes one combining-character cell per column/row, encodes the image id as a
  `38;2;r;g;b` foreground color, positions each row absolutely, terminates with `39m`, and
  clamps at the addressable diacritic limit.

### AT-2-104 - Kitty delete sequence

- Priority: P0
- Evidence: Unit
- Given a non-tmux terminal
- When the delete sequence is built
- Then it is `ESC _ G a=d,d=A,q=2 ESC \` and is scoped to our images.

### AT-2-105 - Kitty media/format probe

- Priority: P0
- Evidence: Unit, Contract
- Given a `t=name` medium query and a `t=file` medium query
- When the `a=q` probe sequence is built
- Then it carries `Gi=<id>,a=q,t=<t>,f=32,s=<w>,v=<h>` with a base64 name payload, and the
  DECRQM/probe reply decoder distinguishes supported, unsupported, and absent replies.

### AT-2-106 - Terminal raw-mode init

- Priority: P0
- Evidence: Integration
- Given a real or fake tty
- When the terminal is initialized
- Then raw termios is applied with saved attributes for reset, mode-enable strings for
  all-motion mouse (`?1003h`), SGR mouse (`?1006h`), pixel mouse (`?1016h`), and bracketed
  paste (`?2004h`) are written, and the alternate screen is **not** entered so the main
  buffer scrollback is preserved.

### AT-2-107 - Terminal reset and clean exit

- Priority: P0
- Evidence: Integration
- Given an initialized terminal
- When the process exits on any path (`q`, `Ctrl-C`, error)
- Then saved termios is restored, reporting modes are disabled, and no raw mouse or graphics
  mode remains active.

### AT-2-108 - Cell-size probe and fallback

- Priority: P0
- Evidence: Unit, Integration
- Given a terminal that answers `ESC[6;<h>;<w>t`
- When cell size is queried
- Then the parser returns the reported width/height pair and rejects zero or malformed reports.
- And when the report is absent, the winsize `ws_xpixel`/`ws_ypixel` divided by the cell
  counts are used as the fallback.

### AT-2-109 - Pixel-mouse capability probe

- Priority: P0
- Evidence: Unit, Contract
- Given a `DECRQM ?1016` reply
- When the probe reply is parsed
- Then `1`/`3` report pixel mouse as supported and other values report unsupported.

### AT-2-110 - SGR mouse decode

- Priority: P0
- Evidence: Unit
- Given SGR mouse sequences `<b;x;yM` (press/motion) and `<b;x;ym` (release) for wheel,
- When the decoder runs
- Then it returns the button, modifier bits (shift/alt/ctrl), kind (down/up/move/scroll),
- and the `x`/`y` coordinates, and rejects zero coordinates.

### AT-2-111 - Cell-to-pixel coordinate conversion

- Priority: P0
- Evidence: Unit
- Given cell coordinates and a measured cell size
- When coordinates are converted without pixel mouse
- Then the result is the center of the addressed cell.

### AT-2-112 - Scroll state machine

- Priority: P0
- Evidence: Unit
- Given wheel deltas and a content max
- When ticks and steps run
- Then the target is clamped to `[0, max]`, the smooth profile eases monotonically toward the
  target over multiple frames, the brake profile settles faster once the stream goes quiet,
  a follow target may exceed a stale max, and the state never reports settled while position
  or velocity differ.

### AT-2-113 - Native scroll helper messages

- Priority: P0
- Evidence: Unit
- Given the helper line protocol `s` (scroll), `z` (zoom), `m` (cursor), `w` (window), and
  `scale`
- When the helper output is parsed
- Then scroll lines carry `delta_y`, `phase`, `momentum`, `precise`, and optional `delta_x`
  and cursor point; window lines parse `x y w h` or `none`; and scale updates are delivered
  to subscribers.

### AT-2-114 - Native helper build

- Priority: P0
- Evidence: Static, Integration
- Given macOS
- When the workspace builds
- Then `build.rs` compiles `native-scroll-helper.swift` with `swiftc` into the output
  directory and sets `NATIVE_SCROLL_HELPER`, or skips cleanly off-macOS.

## C. Render Transport (Phase 1)

### AT-2-200 - Versioned JSON IPC contract

- Priority: P0
- Evidence: Contract
- Given the declared IPC protocol version
- When Rust and `tmath-render` exchange a request and response
- Then the request carries the protocol version, render options, and a bounded ordered block
  list, and the response carries dimensions, byte size, and the PNG payload or a reference,
  with all fields schema-compatible.

### AT-2-201 - One-shot renderer process lifecycle

- Priority: P0
- Evidence: Integration
- Given a render request
- When the renderer subprocess is spawned
- Then it reads exactly one bounded request, writes exactly one bounded response, and exits;
- no long-running renderer process remains.

### AT-2-202 - IPC size, timeout, and trust limits

- Priority: P0
- Evidence: Integration, Unit
- Given an oversized request, an oversized response, a render exceeding the timeout, or input
  the trust policy rejects
- When the IPC boundary processes it
- Then it returns a stable bounded error, terminates the subprocess, and leaks nothing.

### AT-2-203 - Render trust policy

- Priority: P0
- Evidence: Static, Integration
- Given input with raw HTML, remote links, or scripts
- When the renderer runs
- Then it applies the allowlisted Markdown subset, renders remote resources disabled,
- rejects denied LaTeX commands such as `\href` and `\includegraphics`, and never executes
- raw HTML, links, scripts, or TeX binaries.

## D. Placement and Scrollback Anchoring (Phase 2)

### AT-2-300 - One placement per block in the main buffer

- Priority: P0
- Evidence: Integration, Runtime
- Given a document with multiple blocks
- When `tmath render` runs in a real Kitty-graphics terminal
- Then each block is transmitted as one virtual placement (`U=1,c,r`) onto real cells in the
  main screen buffer in source order, the placeholder grid is written so the cells scroll
  with the shell scrollback, and no Herdr-viewer or alternate-screen redraw is involved.

### AT-2-301 - Images scroll with scrollback

- Priority: P0
- Evidence: Runtime
- Given placements written to the main buffer
- When the user scrolls the terminal back and forth
- Then the images move with the real cells and do not stay pinned to the viewport.

### AT-2-302 - Replace and delete a block

- Priority: P0
- Evidence: Integration
- Given a previously valid placement for a block id
- When the block is re-rendered or removed
- Then the stale image is deleted (`a=d`) before or as part of the replacement, and no orphan
  image remains.

### AT-2-303 - Fail-closed placement

- Priority: P0
- Evidence: Integration
- Given invalid input, missing Kitty support, a render timeout, or a payload rejection for a
  block
- When it is encountered mid-document
- Then earlier valid placements remain intact, the process reports the failure, and no
  uncertain or misaligned placement is emitted.

### AT-2-304 - No Kitty support

- Priority: P0
- Evidence: Integration, Runtime
- Given a terminal that does not answer the graphics probe
- When `tmath render` runs
- Then it prints a clear compatibility message and exits non-zero without emitting partial
  images.

## E. Input Loop (Phase 3)

### AT-2-400 - Mouse wheel scrolling

- Priority: P0
- Evidence: Runtime
- Given a rendered document taller than the viewport
- When the user scrolls the mouse wheel over the terminal
- Then the placements scroll according to the smooth scroll state machine.

### AT-2-401 - Keyboard fallback scrolling

- Priority: P0
- Evidence: Runtime
- Given a rendered document
- When the user presses arrow keys, `PgUp`/`PgDn`, `j`/`k`, or `g`/`G`
- Then the document scrolls as documented even when mouse input is unavailable.

### AT-2-402 - Trackpad precision via native helper

- Priority: P1
- Evidence: Runtime
- Given macOS and a built native helper
- When the user scrolls a trackpad over the terminal
- Then precise deltas drive the scroll state machine and pinch reports as zoom events.

### AT-2-403 - Bounded input parsing

- Priority: P0
- Evidence: Unit
- Given arbitrary or malformed escape/input bytes
- When the parser runs
- Then it never allocates unbounded memory, caps per-frame work, and recovers at the next
  valid event boundary.

### AT-2-404 - Clean exit and reset on any path

- Priority: P0
- Evidence: Integration
- Given an initialized terminal and any exit trigger
- When the process terminates
- Then the terminal is reset and no partial graphics state remains.

## F. Scanner, Renderer, and Limits (carried from V1)

### AT-2-500 - Scanner delimiters

- Priority: P0
- Evidence: Unit
- Given `$...$`, `$$...$$`, `\(...\)`, and `\[...\]` in source
- When scanning runs
- Then formulas are returned in source order with correct byte offsets, code and escaped
  currency are ignored, and scanner input limits are enforced.

### AT-2-501 - Renderer limits preserved

- Priority: P0
- Evidence: Unit, Render
- Given the V1 policy limits for formula count, per-formula length, aggregate length, render
  duration, image dimensions, raw PNG bytes, and base64 payload size, plus the new limits for
  concurrent placements and total placement pixels
- When any limit is exceeded
- Then a stable bounded error is returned and previous valid placements remain intact.

### AT-2-502 - Renderer corpus compatibility

- Priority: P0
- Evidence: Render
- Given the fixed release corpus of prose plus powers, fractions, roots, sums, integrals,
  aligned equations, matrices, Greek letters, and Unicode
- When rendered through the standalone pipeline
- Then every valid case produces a non-empty transparent PNG with expected prose and math in
  source order and bounded dimensions.

## G. Privacy, Security, and Recovery

### AT-2-600 - No content in logs or state

- Priority: P0
- Evidence: Unit, Integration, Static
- Given sentinel document text, formulas, and local paths
- When success and every error path run
- Then logs and durable state contain none of those sentinel values and only allowlisted
  fields (event/status names, measured counts, byte sizes, timing, hashes, error codes).

### AT-2-601 - No network

- Priority: P0
- Evidence: Static, Integration
- When the renderer and `tmath` run under a network-deny harness
- Then no DNS, HTTP, remote font, or telemetry request occurs, and nothing is uploaded.

### AT-2-602 - No execution of user input

- Priority: P0
- Evidence: Static
- When production source is inspected
- Then no user input reaches a shell, `child_process` shell evaluation, `eval`, dynamic
  executable import, or TeX binary, in either the Rust or TypeScript layer.

### AT-2-603 - Invalid input preserves earlier placements

- Priority: P0
- Evidence: Integration
- Given a document with a valid prefix and an invalid or over-limit block
- When rendering completes
- Then the valid prefix placements remain and the invalid block changes nothing.

## H. Compatibility, Documentation, and Release

### AT-2-700 - Ghostty primary compatibility

- Priority: P0
- Evidence: Runtime
- Given the release versions of `tmath` and Ghostty on macOS
- When first placement, scrollback scroll, mouse scroll, keyboard fallback, replace, invalid
  preservation, and clean-exit cases run
- Then all pass without raw escape text, focus loss, or partial images, with no Herdr runtime
  present.

### AT-2-701 - Kitty and WezTerm compatibility

- Priority: P1
- Evidence: Runtime
- Kitty or WezTerm is listed as verified only after the same matrix as AT-2-700 passes.

### AT-2-702 - English public surface

- Priority: P0
- Evidence: Static
- When README, docs, specs, CLI help, logs, comments, release notes, and contribution files
  are scanned
- Then user-facing content is English except fixtures explicitly testing multilingual
  behavior.

### AT-2-703 - Required public documentation

- Priority: P0
- Evidence: Static
- Before `0.2.0` release, the repository contains accurate installation, usage, privacy,
  security, compatibility, contribution, license, changelog, and known-limit documentation
  describing the standalone product, with V1 explicitly superseded.

### AT-2-704 - Secret and artifact scan

- Priority: P0
- Evidence: Static
- When the complete release tree and Git diff are scanned
- Then no credential pattern, local transcript, username, home path, unredacted screenshot,
  state file, lock file, generated diagnostic log, or private fixture is present.

### AT-2-705 - No Herdr contract remains

- Priority: P0
- Evidence: Static
- When the repository is scanned after Phase 6
- Then `herdr-plugin.toml`, `src/herdr`, `src/viewer`, `src/graphics`, `src/manifest`,
  `src/on-*.ts`, `src/startup.ts`, and all `HERDR_*` reads are absent with no dangling
  imports, and `npm run check` and `cargo test` pass.

### AT-2-706 - Standalone release install

- Priority: P0
- Evidence: Install, Runtime
- Given the proposed immutable release tag
- When a clean user installs that tag and runs the documented first-use flow
- Then installation and runtime pass without a development checkout, unpublished files, a
  Herdr runtime, or another repository's dependencies.

## I. Agent Integration (tmux viewer)

Agent integration is a P1 extension to the standalone CLI, not part of the
`0.2.0` release gate. `tmath agent` watches a tmux pane running a coding agent
(Claude Code, Codex, opencode, Cursor, pi, and similar) and shows each
finished answer as a rendered Markdown + math image in a separate viewer pane.

### AT-2-800 - Agent pane split and channel

- Priority: P1
- Evidence: Contract, Integration, Runtime
- Given `tmath agent` running inside a tmux session
- When it starts
- Then it opens one right-side viewer pane running `tmath agent-viewer`, binds
  a bounded Unix socket in the platform temp directory, and emits a one-line
  banner naming the source and viewer panes.

### AT-2-801 - Answer boundary detection

- Priority: P1
- Evidence: Unit
- Given consecutive `tmux capture-pane` snapshots of a pane running a coding
  agent
- When the newest answer is captured
- Then the detector returns that answer's display text (`$...$`/`$$...$$` math
  and prose), strips a trailing prompt glyph and repainted working frames, and
  rejects prompt-only, pure-repaint, and unrecoverable rewrites with no answer.

### AT-2-802 - Bounded message channel

- Priority: P1
- Evidence: Unit, Integration
- Given a document larger than the renderer request limit or a malformed frame
- When it crosses the Unix socket
- Then the frame is rejected with a stable error, the buffer stays bounded, and
  the viewer keeps its previous image (fail closed).

### AT-2-803 - Viewer render and replace

- Priority: P1
- Evidence: Unit, Render, Runtime
- Given a new answer document
- When the viewer receives it
- Then it renders through the one-shot renderer subprocess, replaces the
  previous placement by image id, and keeps the previous image intact on any
  render, limit, or decode failure.

### AT-2-804 - Viewer scroll and clean exit

- Priority: P1
- Evidence: Unit, Integration
- Given a rendered answer taller than the viewer pane
- When the user scrolls with the wheel or arrow keys in the viewer pane
- Then the placement is re-emitted at a shifted home row, and `q`/`Ctrl-C`
  close the viewer cleanly.

### AT-2-805 - tmux passthrough envelope

- Priority: P1
- Evidence: Unit
- Given the process runs with `$TMUX` set
- When any Kitty sequence or placement is written
- Then it is wrapped in the tmux DCS passthrough envelope
  (`ESC P ... ESC \\`), and unmodified otherwise.

### AT-2-806 - Ghostty tmux image relay

- Priority: P1
- Evidence: Runtime
- Given a Ghostty-attached tmux session with `allow-passthrough` enabled
- When an answer renders in the viewer pane
- Then a real image is displayed; until recorded, image display inside tmux
  remains a verified limitation with a clear fail-closed diagnostic.

### AT-2-807 - No-content logging

- Priority: P1
- Evidence: Static, Integration
- Given watcher/viewer success and every error path
- Then logs contain only event names, pane ids, counts, byte sizes, and hashes;
  never answer text, formulas, or document bytes.

## Release Acceptance Rule

Release `0.2.0` only when:

1. Every P0 case applicable to the declared platforms and terminals is passed.
2. Every result has current evidence from the public implementation.
3. P1 and P2 gaps are described as unsupported or planned, not implied as working.
4. The task checklist contains no incomplete release-gate task.
5. The final clean-tag install test passes after all release files are committed.
6. The Rust workspace, CLI, IPC, placement, and input-loop implementation all pass without a
   Herdr runtime present.
