# Terminal Math V2 Architecture

## Status

- Target release: `0.2.0` (first standalone release; no Herdr runtime)
- Last updated: August 2, 2026
- Canonical plan: `specs/terminal-math-v2/plans/main.md`
- Acceptance contract: `specs/terminal-math-v2/tests/main.md`

This document describes the target architecture. Behavior labeled **planned** is not released
behavior; it becomes verified only when the acceptance tests and runtime evidence exist.

## System Context

Terminal Math is a standalone CLI: `tmath render` reads a Markdown/LaTeX document, renders it to
a transparent PNG locally, and places it into the main terminal buffer with the Kitty graphics
protocol so it scrolls with the shell scrollback.

```
[ input: file | pipe | stdin ]  ──►  tmath (Rust CLI)
                                        │
                                        │ parses doc → formula/block list
                                        │ spawns renderer subprocess (one-shot)
                                        ▼
                              [ tmath-render (TS) ]
                                        │
                                        │ KaTeX + allowlisted Markdown → transparent PNG
                                        │ returns PNG bytes + dimensions over stdout
                                        ▼
                              [ Rust terminal frontend ]
                                        │
                                        │ raw mode, Kitty negotiation,
                                        │ mouse + scroll parsing, scroll state machine
                                        ▼
                              [ Kitty-graphics terminal: Ghostty / kitty / WezTerm ]
                                        (one scrollback-anchored placement per block)
```

## Architectural Decisions

1. **Runtime**: a Rust engine owns the terminal-facing layer; the LaTeX/Markdown→PNG pipeline
   stays in TypeScript and runs as a short-lived render subprocess. Rust owns the terminal and
   the render transport.
2. **Input source**: a standalone CLI. Content comes from a file, a pipe, or stdin. There are no
   events, no pane watching, and no plugin runtime.
3. **Rendering model**: scrollback-anchored. One persistent Kitty placement per block in the main
   screen buffer, glued to real cells with a virtual placement (`U=1,c,r`) and a placeholder
   grid.
4. **Product identity**: a standalone terminal math/document renderer named `tmath`. No Herdr
   socket, manifest, or `HERDR_*` contract.

## Process Responsibilities

### `tmath` (Rust binary)

The CLI owns the terminal:

- Enables raw mode and the reporting modes (`?1003h` all-motion mouse, `?1006h` SGR mouse,
  `?1016h` pixel mouse, `?2004h` bracketed paste); it does **not** enter the alternate screen, so
  the main-buffer scrollback is preserved.
- Reads the document from the chosen input (bounded).
- Sends the document to the render subprocess and receives the PNG payload.
- Negotiates Kitty support; if unsupported it prints a clear message and exits non-zero.
- Transmits the placement into the main buffer at the cursor row and writes the placeholder grid.
- Runs the input loop: mouse wheel/SGR/pixel deltas feed the scroll state machine; keys drive the
  fallback; `q`/`Ctrl-C` reset the terminal on any exit path.
- `tmath diagnose` reports renderer subprocess availability, node, stdout terminal status, and
  Kitty graphics support.

### `tmath-render` (TypeScript, one-shot)

The renderer subprocess reuses the existing `src/renderer/*` pipeline:

- Reads exactly one bounded JSON request on stdin.
- Runs the scanner (when formulas are not pre-supplied), composes the allowlisted Markdown
  document, and renders with KaTeX/Chromium/sharp to a transparent PNG.
- Applies the render timeout and the KaTeX `trust: false` policy.
- Writes exactly one bounded JSON response (dimensions, byte size, base64 PNG, or a stable error
  record) on stdout, then exits.

### IPC contract

- Protocol: `tmath-render/1` (shared constants in `src/renderer/ipc-contract.ts` and
  `engine/crates/tmath-core/src/ipc.rs`).
- Bounded request/response sizes, a render timeout, and an explicit trust policy.
- Later, optional shared-memory/file media can avoid pushing large payloads through a pipe.

## Component Boundaries

### 1. Kitty escapes (`engine/crates/tmath-core/src/kitty.rs`)

Constructs the Kitty graphics byte sequences: placed transmit chunking
(`a=T,f=32,o=z,s,v,t=d,i,q=2`), virtual placement `U=1,c,r`, cursor placement `p=1,C=1`, the
placeholder grid, scoped delete (`d=I,i=<id>`), and `a=q` probes. No bytes are written here;
callers forward the sequences.

### 2. Terminal init/reset (`terminal.rs`)

A `Tty` trait abstracts stdin/stdout and termios so escape construction and probe replies are
unit-tested against a fake device. It enables raw mode and the reporting modes, measures cell
size (`CSI 6;h;w t` with winsize fallback), probes pixel mouse (`DECRQM ?1016`) and Kitty
graphics (`a=q`), and always restores the terminal on reset.

### 3. Mouse and input decoding (`mouse.rs`, `input.rs`)

`parse_sgr_mouse` decodes SGR `<b;x;y` reports (wheel/button/motion/release and modifier bits);
`input.rs` provides a bounded incremental decoder that buffers capped bytes and emits one event at
a time (mouse, keys, bracketed paste, focus), skipping garbage to the next valid boundary.

### 4. Scroll driver (`scroll.rs`, `scroll_driver.rs`)

`ScrollState` + `Smooth` provide exponential easing and braking. `ScrollDriver` maps wheel deltas
(±3 rows), arrows/`j`/`k` (±1 row), `PgUp`/`PgDn`/`g`/`G` (±page), and `Home`/`End` (extrema)
through the state machine, clamping to `[0, max]`. `is_exit_signal` recognizes `q` and `Ctrl-C`.

### 5. Placement (`placement.rs`)

The tracker assigns image ids, enforces concurrent-placement and total-pixel limits, and stacks
blocks in source order. `emit_placed_block_cursor` transmits the virtual placement over the
cells at the current cursor, so an interactive or piped render appears directly under the
command line; it advances one line first only when the cursor is not already at the start of
a line (checked outside `tmux` via `CSI 6n`; inside `tmux` it always advances, since the
report answers with the pane-relative cursor rather than the outer terminal's), so a render
invoked right after the shell's own newline does not push the image down an extra row.
Replacement emits a scoped delete first. Emissions are structured as `TerminalOp::Local`
(pane bytes) and `TerminalOp::Graphics` (Kitty APC commands) so tmux transports
can route them separately.

### 6. Scanner (`src/scanner/scan-latex.ts`)

A stateful conservative scanner for `$...$`, `$$...$$`, `\(...\)`, and `\[...\]` that skips fenced
and inline code, escaped currency, unclosed delimiters, and shell/price patterns, and enforces
input, delimiter-run, formula-count, and formula-character limits.

### 7. Renderer (`src/renderer/*`)

- `render.ts` enforces count/length/aggregate/timeout/dimension/byte limits and maps errors.
- `document.ts` composes prose and math segments in source order.
- `markdown.ts` renders the strict allowlisted Markdown subset with raw HTML disabled.
- `browser-backend.ts` renders through local Chromium (remote resources denied) and sharp.
- `ipc-contract.ts` + `subprocess.ts` implement the one-shot JSON transport.
- `runtime-check.ts` verifies the local browser runtime.

### 8. CLI (`engine/crates/tmath/src/main.rs`)

Parses commands and options, reads the bounded document, drives the render subprocess, places in
a terminal or prints a summary, and reports diagnostics.

### 9. Agent integration (P1, experimental)

Phase 8 adds two `tmath` subcommands that reuse the renderer and placement
pipeline to show a coding agent's finished answers in a separate tmux pane:

- `tmath-core::agent::boundary` — `find_answer(baseline, completion)` over
  `tmux capture-pane` snapshots: exact/stable-prefix detection, repainted
  working-frame and trailing-prompt stripping, and fail-closed rejection of
  prompt-only, pure-repaint, and unrecoverable rewrites.
- `tmath-core::agent::tmux` — a fixed allowlisted `tmux` CLI surface (split /
  capture / display / kill) with validated pane ids; no agent content reaches
  a shell.
- `tmath-core::agent::codec` — bounded uns across a Unix socket with
  length-prefixed JSON frames (`document`/`quit`).
- `tmath agent` (watcher) owns the socket, splits the viewer pane, polls the
  source pane, debounces, and emits each answer document. It passes the
  renderer worker path to the viewer on the command line (tmux panes start
  with the server environment).
- `tmath agent-viewer` (in the viewer pane) renders each document through the
  one-shot renderer, replaces the previous placement by image id (clearing
  stale placeholder cells when the new image is shorter), scrolls long answers
  by cropping the RGBA viewport and replacing the placement, and closes on
  `q`/Ctrl-C.
- Under `$TMUX`, graphics and pane-local output are separate operations. The
  default route writes only Kitty APC commands to the validated visible client
  tty; cursor movement, terminal modes, color CSI, line breaks, and Unicode
  placeholders remain normal tmux pane output. The optional stable-tmux route
  independently DCS-wraps each Kitty APC with every embedded `ESC` doubled.
  Graphics probes are skipped because replies cannot be routed reliably, and
  cell size comes from winsize; outside tmux probing stays mandatory.

Recorded behavior (P1): controlled image pixels display in Ghostty 1.3.1 and
cmux 0.64.12 through tmux 3.5a using the client-tty graphics route. The corrected
DCS passthrough route also displays controlled pixels in Ghostty 1.3.1. The
client tty is accepted only when tmux reports a `/dev/tty*` character device
owned by the current user and the opened descriptor retains the same identity.
No screenshot or rendered bytes are retained.

## Concurrency and Atomicity

Single-invocation semantics keep concurrency simple:

- The render subprocess is one-shot; at most one request is written and one response read.
- The IPC boundary enforces request/response byte caps and a render timeout.
- Placement reserve is atomic against the limits; a rejected block emits nothing and leaves the
  tracker unchanged.
- No durable shared state exists; every invocation is self-contained.

## Limits

Renderer limits (from `src/core/limits.ts`):

| Limit | Value |
|---|---|
| Scanner input bytes | 1 MiB |
| Delimiter runs / run length | 4096 / 8 |
| Formulas per answer | 20 |
| Characters per formula | 2000 |
| Aggregate formula characters | 10,000 |
| Response document bytes / lines / blocks | 256 KiB / 4000 / 512 |
| Render duration | 8000 ms |
| Image width / height / pixels | 4096 / 16384 / 32 MiB |
| Raw PNG bytes / base64 payload | 512 KiB / 700 KiB |

Placement limits (Rust): 64 concurrent placements and 64 MiB total placed pixels, with the CLI
document read capped at the request byte limit.

## Error Model

- **User-input rejection**: invalid LaTeX or a configured limit. Keep earlier placements, emit a
  bounded diagnostic, fail closed.
- **Capability failure**: no Kitty graphics, no terminal for stdout, or a missing renderer
  dependency. `tmath diagnose` explains the corrective action; `tmath render` exits non-zero
  without emitting partial images.
- **Transient runtime failure**: a render timeout or subprocess failure. Fail closed; do not
  retry in a tight loop.

Errors serialize only allowlisted fields (code, retryable flag, limit kind, bounded numeric
details) with no input text.

## Logging

- Logs and diagnostics contain only event/status names, bounded counts, byte sizes, timing, and
  stable error codes.
- Document text, formula source, rendered bytes, and paths never appear in logs or durable
  state.
- There is no telemetry and no network upload.

## Packaging and Release

- `Cargo.toml` is the Rust workspace contract; `package.json` declares the renderer build and
  validation commands.
- The Rust binary locates the built render subprocess via `TMATH_RENDER_WORKER`.
- Version tags, `Cargo.toml`, and `package.json` must agree.
- Release requires a real Ghostty session, a clean build, and a security/artifact scan.

## Compatibility Matrix

| Dimension | Required evidence |
|---|---|
| Operating system | Clean dependency build and runtime smoke |
| CPU architecture | Native dependency installation and render smoke |
| Outer terminal | Real placement, scrollback scroll, mouse scroll, keyboard fallback, clean-exit |
| Kitty probe | Fails closed with a clear diagnostic when unsupported |

The current implementation is verified on macOS arm64 and Ghostty 1.3.1 is installed but not yet
recorded as a passed full-matrix run. See [Compatibility](compatibility.md) for the exact matrix.

## Test Architecture

- Unit tests cover Kitty escapes, terminal init/probes, mouse parsing, the input decoder, the
  scroll state machine, placement limits, scanning, limits, and error mapping.
- Contract tests validate the `tmath-render/1` IPC fixtures and escape/probe bytes.
- Integration tests run the built render subprocess end to end and drive the CLI non-tty path.
- Renderer tests use a fixed formula corpus and image assertions.
- `npm run security:check` scans source (including `.rs`/`.swift`) and release files for
  environment dumps, network APIs, dynamic execution, credential formats, private paths,
  symbolic links, and runtime artifacts; the Rust workspace adds static privacy gates in
  `cargo test`.
- Runtime smoke tests use a real Ghostty terminal; install tests use a clean build.

Every acceptance-test id in the specification maps to at least one automated or recorded manual
test.

## V2 Compatibility Decisions

- macOS arm64 is the primary target; Linux and Windows are P1/P2 and are not claimed.
- Ghostty 1.3.1 is the primary verified-target terminal; kitty and WezTerm are P1.
- Node.js 22 or later is required for the render subprocess.
- The Kitty graphics protocol is the only image transport.
- V2 does not require Herdr, a manifest, a socket, or any `HERDR_*` environment variable.

Changing one of these decisions requires updating the plan, tests, [compatibility
matrix](compatibility.md), user documentation, and release checklist together.

## Primary References

- [Kitty graphics protocol](https://sw.kovidgoyal.net/kitty/graphics-protocol/)
- [Kitty keyboard protocol](https://sw.kovidgoyal.net/kitty/keyboard-protocol/)
- [xterm controls](https://invisible-island.net/xterm/ctlseqs/ctlseqs.html)
- [Ghostty](https://ghostty.org/docs/features)
