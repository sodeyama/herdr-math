# Terminal Math Architecture

## Status

- Current release: **0.3.0** (native-only renderer; Node/Chromium path removed in T3-502)
- Last updated: August 6, 2026
- Canonical plan: `specs/terminal-math-v3/plans/main.md`
- Acceptance contract: `specs/terminal-math-v3/tests/main.md`
- Superseded: `specs/terminal-math-v2/` (historical reference for the Node subprocess era)

This document describes the shipped architecture. Behavior labeled **planned** is not released
behavior; it becomes verified only when the acceptance tests and runtime evidence exist.

## System Context

Terminal Math is a standalone CLI: `tmath render` reads a Markdown/LaTeX document, renders it
in-process to transparent PNG blocks, and places them into the main terminal buffer with the
Kitty graphics protocol so they scroll with the shell scrollback.

```
[ input: file | pipe | stdin ]  ──►  tmath (Rust CLI)
                                        │
                                        │ parse/split blocks, render in-process
                                        ▼
                              [ tmath-render (Rust crate) ]
                                        │
                                        │ RaTeX math + Typst prose → PNG per block
                                        ▼
                              [ Rust terminal frontend (tmath-core) ]
                                        │
                                        │ raw mode, Kitty negotiation,
                                        │ mouse + scroll parsing, scroll state machine
                                        ▼
                              [ Kitty-graphics terminal: Ghostty / kitty / WezTerm ]
                                        (scrollback-anchored placement per block)
```

## Architectural Decisions

1. **Runtime**: a single Rust binary owns rendering and the terminal-facing layer. The
   LaTeX/Markdown→PNG pipeline runs in-process in `engine/crates/tmath-render` (RaTeX +
   Typst); there is no Node subprocess and no browser.
2. **Input source**: a standalone CLI. Content comes from a file, a pipe, or stdin. Agent
   integration watches tmux panes or structured transcripts; there is no plugin runtime.
3. **Rendering model**: scrollback-anchored, block-based. One Kitty placement per semantic
   block in the main screen buffer, with virtual placement (`U=1,c,r`) and a placeholder grid.
4. **Product identity**: a standalone terminal math/document renderer named `tmath`. No Herdr
   socket, manifest, or `HERDR_*` contract.

## Process Responsibilities

### `tmath` (Rust binary)

The CLI owns the terminal:

- Enables raw mode and the reporting modes (`?1003h` all-motion mouse, `?1006h` SGR mouse,
  `?1016h` pixel mouse, `?2004h` bracketed paste); it does **not** enter the alternate screen, so
  the main-buffer scrollback is preserved.
- Reads the document from the chosen input (bounded).
- Splits and renders blocks through the in-process native engine (`tmath-render`).
- Negotiates Kitty support; if unsupported it prints a clear message and exits non-zero.
- Transmits placements into the main buffer and writes placeholder grids.
- Runs the input loop: mouse wheel/SGR/pixel deltas feed the scroll state machine; keys drive the
  fallback; `q`/`Ctrl-C` reset the terminal on any exit path.
- `tmath diagnose` reports stdout terminal status, PATH launcher health, tmux graphics route,
  and Kitty graphics support.

### `tmath-render` (Rust crate, in-process)

The native renderer in `engine/crates/tmath-render`:

- Splits Markdown into semantic blocks (pulldown-cmark, offset-based) and scans math with a
  Rust port of the V2 scanner corpus.
- Renders math with RaTeX and prose with Typst as a library; inline formulas embed on the text
  baseline through a static file resolver.
- Uses only fonts embedded in the binary; Typst package resolution, network, and filesystem access
  are structurally unavailable. User text enters Typst exclusively as escaped string literals.
- Enforces per-block limits (source bytes, image dimensions/pixels/bytes scaled by dpr², render
  deadline) and per-formula fail-closed error badges; error records carry no input text.
- Spawns no subprocess; content-hash caching is byte-deterministic per (block, options).

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

### 6. Scanner and Markdown (`engine/crates/tmath-render/src/scanner.rs`, `markdown.rs`)

Rust ports of the V2 conservative `$...$` / `$$...$$` / `\(...\)` / `\[...\]` scanner and the
strict allowlisted Markdown subset, with the V2 fixture corpus retained as acceptance input.

### 7. CLI (`engine/crates/tmath/src/main.rs`)

Parses commands and options, reads the bounded document, drives the native render engine,
places in a terminal or prints a summary/event stream, and reports diagnostics.

### 8. Agent integration

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
  source pane (or tails a structured transcript when available), debounces, and
  emits each answer document to the viewer over a bounded Unix socket.
- `tmath agent-viewer` (in the viewer pane) splits each received document
  into semantic blocks (`tmath_render::parse_blocks_limited`), rendered
  per-block through the resident `RenderCache`, and diffed against the
  previous document's blocks by a `PlacementPlanner` (`Keep`/`Append`/
  `Replace`/`Remove`, never-reused block ids). Placement operations are
  emitted through the same per-block Kitty-placement emitter stream mode
  uses (`tmath::native_stream`): unchanged blocks are never re-rendered or
  re-transmitted, and a shorter replacement answer removes its stale
  trailing placements instead of leaving orphan placeholder cells. There is
  no composite RGBA buffer. New blocks always append below the last
  placement in flowing order (follow mode); a manual scroll input
  disengages follow, and `End`/`F` re-engage it, but the viewer currently
  relies on the pane's own scrollback for viewing earlier blocks rather than
  re-anchoring a visibility window — a full visibility-driven re-emission
  (AT-3-503) is not yet implemented. The viewer closes on `q`/Ctrl-C.
- Under `$TMUX`, graphics and pane-local output are separate operations. The
  default route DCS-wraps each Kitty APC (every embedded `ESC` doubled) as
  tmux passthrough, so graphics bytes are serialized through tmux's own
  output queue against every other pane's output; it requires
  `allow-passthrough on` (checked when the route is defaulted, refused with
  an actionable message otherwise). The optional client-tty route
  (`TMATH_TMUX_TRANSPORT=client-tty`) writes Kitty APC commands directly to
  the validated visible client tty; it bypasses tmux relay quirks but its
  writes are NOT serialized against tmux's own output — concurrent heavy
  pane output can tear an escape mid-sequence and desync the outer
  terminal's parser (observed live 2026-08-05 as raw base64 sprayed over the
  window), which is why it is no longer the default. Cursor movement,
  terminal modes, color CSI, line breaks, and Unicode placeholders remain
  normal tmux pane output on both routes. Graphics probes are skipped
  because replies cannot be routed reliably, and cell size comes from
  winsize; outside tmux probing stays mandatory.

Recorded behavior (P1): controlled image pixels display in Ghostty 1.3.1 and
cmux 0.64.12 through tmux 3.5a using the client-tty graphics route. The corrected
DCS passthrough route also displays controlled pixels in Ghostty 1.3.1. The
client tty is accepted only when tmux reports a `/dev/tty*` character device
owned by the current user and the opened descriptor retains the same identity.
No screenshot or rendered bytes are retained.

Outer-terminal gate: inside tmux with `TMATH_TMUX_TRANSPORT` unset, the route
is selected only after classifying the outer terminal as Verified (advertised
termname contains ghostty/kitty/wezterm, or the validated client tty's process
ancestry reaches a known terminal), Unverified (named in the refusal message),
or NoClient (no attached client). The two refusal states carry distinct
bounded messages, and `tmath diagnose` prints the gate inputs (attached client
count, termname, allow-passthrough, transport env) plus the full refusal
reason. Because tmux-spawned commands inherit the tmux server's environment,
the shell wrapper forwards `TMATH_TMUX_TRANSPORT`/`TMATH_DPR`/`TMATH_DEBUG_LOG`
as an explicit `env` prefix on watcher command lines, and the watcher forwards
them again to the viewer pane. A viewer that has emitted successfully treats a
NoClient refusal as transient: it skips emissions (placements stay intact) and
exits only after 30 consecutive unavailable emissions.

## Concurrency and Atomicity

Single-invocation semantics keep concurrency simple:

- Rendering is in-process; no child renderer subprocess is spawned.
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
- **Capability failure**: no Kitty graphics or no terminal for stdout.
  `tmath diagnose` explains the corrective action; `tmath render` exits non-zero
  without emitting partial images.
- **Transient runtime failure**: a per-block render timeout. Fail closed; do not
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

- `Cargo.toml` is the Rust workspace contract; `package.json` holds the optional repository
  security scan (`npm run security:check`).
- Version tags and `Cargo.toml` versions must agree.
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
- Integration tests drive the CLI non-tty path, native stream/watch modes, and agent pipelines.
- Renderer tests use golden corpora, pixel hashes, and injection fixtures in the Rust crates.
- `npm run security:check` scans Rust production sources and release files for environment dumps,
  network APIs, credential formats, private paths, symbolic links, and runtime artifacts; the
  Rust workspace adds static privacy gates in `cargo test`.
- Runtime smoke tests use a real Ghostty terminal; install tests use a clean build.

Every acceptance-test id in the specification maps to at least one automated or recorded manual
test.

## Compatibility Decisions

- macOS arm64 is the primary target; Linux and Windows are P1/P2 and are not claimed.
- Ghostty 1.3.1 is the primary verified-target terminal; kitty and WezTerm are P1.
- The Kitty graphics protocol is the only image transport.
- No Herdr runtime, manifest, socket, or `HERDR_*` environment variable is required.

Changing one of these decisions requires updating the plan, tests, [compatibility
matrix](compatibility.md), user documentation, and release checklist together.

## Primary References

- [Kitty graphics protocol](https://sw.kovidgoyal.net/kitty/graphics-protocol/)
- [Kitty keyboard protocol](https://sw.kovidgoyal.net/kitty/keyboard-protocol/)
- [xterm controls](https://invisible-island.net/xterm/ctlseqs/ctlseqs.html)
- [Ghostty](https://ghostty.org/docs/features)
