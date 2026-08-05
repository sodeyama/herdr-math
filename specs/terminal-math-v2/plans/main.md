> **SUPERSEDED**: V2 describes the Node-subprocess renderer architecture shipped as
> `0.2.0`. The current product direction is V3 (`specs/terminal-math-v3/`): native
> in-process rendering with streaming agent integration. Keep this spec as historical
> reference for the V2 release gate; do not treat it as current product guidance.

# Terminal Math V2 Refactor Plan

## Status

- Plan state: Phases 0-7 (release-gate prep) complete; **Phase 8 (agent
  integration + local install, P1) in progress** — `tmath agent`/`agent-viewer`
  implemented, and `scripts/install.sh` provides a one-command user-local
  install with renderer auto-discovery and a coding-agent skill.
- Target release: `0.2.0` (first standalone release without a Herdr runtime)
- Last updated: August 3, 2026
- Acceptance contract: `../tests/main.md`
- Task checklist: `../tasks/main.md`
- Predecessor spec: `../../../specs/herdr-math-v1/plans/main.md`

This document is a **plan**, not verified release behavior. Nothing described here is
released until the corresponding acceptance tests and runtime evidence exist.

## Motivation

Herdr Math V1 is a Herdr plugin: it renders LaTeX from AI-agent responses into a Herdr-managed
side pane using `HERDR_SOCKET_PATH`, `pane.graphics.set`, and Herdr-managed viewer panes. That
architecture couples the product to the Herdr runtime and to the Herdr client-to-pane transport.

The user wants to escape the Herdr premise:

- Remove the Herdr dependency and its plugin lifecycle, manifest, and socket transport.
- Able to be run from a plain terminal (like `terminal-browser`) against any Kitty-graphics-capable
  outer terminal (Ghostty, kitty, WezTerm).
- Enable **mouse events** (currently V1 scrolling is keyboard-only via `parseScrollKeys` in
  `src/viewer.ts`).
- **Freely place scrollable images on the terminal**, glued to real terminal cells so they scroll
  with the shell's scrollback.

Reference implementation: `terminal-browser` at `~/git/terminal-browser`. Its rendering core is a
Rust crate `pixel-core` that owns the terminal (raw mode, Kitty graphics escape sequences, SGR /
pixel-accurate mouse parsing, a smooth-scroll state machine, and an optional macOS native Swift
input helper).

## Decisions (recorded from review)

1. **Runtime**: adopt a Rust engine for the terminal-facing layer (port `pixel-core`'s applicable
   pieces). The LaTeX/Markdown→PNG pipeline stays in TypeScript (KaTeX, markdown-it, sharp), run as a
   short-lived render subprocess. Rust owns the terminal and the render transport.
2. **Input source**: a standalone CLI. Content comes from a file, a pipe, or stdin, e.g.
   `tmath render <doc.md>` or `tmath render -` reading a stream. No Herdr events, no pane-watching.
3. **Rendering model**: scrollback-anchored. One persistent Kitty placement per formula/document
   block in the **main screen buffer**, glued to real cells with a virtual placement (`U=1,c,r`) and
   a placeholder grid, so the images scroll with the shell scrollback. This is the opposite of
   `terminal-browser`'s alternate-screen full-redraw model and is what "scrollable images" means here.
4. **Product identity**: a standalone terminal math / document renderer. The repository keeps its
   current name but drops the "Herdr plugin" identity, `herdr-plugin.toml`, and the `HERDR_*` contract.

## Non-Negotiable Decisions

These carry forward from V1 where they still apply, and are revised where the Herdr premise no longer
holds.

1. Product category: a standalone CLI that renders Markdown + LaTeX as scrollable, mouse-interactive
   images in a Kitty-graphics-capable terminal.
2. V2 syntax: `$...$` and `$$...$$` support first; `\(...\)` / `\[...\]` retained from V1. Render a
   strict allowlisted Markdown subset (headings, emphasis, lists, quotes, tables, code blocks, inert
   links) through a local strict renderer. No raw HTML, no user CSS/color directives, no remote
   resources, no scripts.
3. No network: never upload document content, equations, images, logs, or telemetry.
4. No execution: never run LaTeX engines, `child_process` shell evaluation, `eval`, user JavaScript,
   or TeX binaries. KaTeX runs in a pinned renderer runtime with a trust policy equivalent to
   `trust: false`.
5. Placement model: render each formula/document block as a transparent PNG and transmit it
   through the Kitty graphics protocol to a scrollback-anchored placement. The terms "viewer pane"
   and "graphics placement" are replaced by "Kitty placement".
6. Limits: keep bounded, non-infinite caps for formula count, per-formula length, aggregate length,
   scan input bytes, render duration, image dimensions, raw PNG bytes, base64 payload size, and (new)
   number of concurrent placements and total placement pixels. Caps may be sized generously for
   real-world document and chapter-summary inputs, but every cap stays finite and enforced. Invalid
   input, timeouts, and payload rejection leave previous valid placements intact.
7. Scroll model: mouse wheel and, where the native helper is available, trackpad deltas drive a
   smooth-scroll state machine over the rendered document. Keyboard scrolling (arrow/PgUp/PgDn/j/k/g/G)
   is retained as an accessibility fallback.
8. Fail-closed error model: on boundary confusion, input truncation, or render failure, do not emit
   an uncertain/misaligned placement.
9. Compatibility model: declare only tested platforms and terminals. macOS is the primary target
   (matches V1); Linux and Windows are post-V2 (P1/P2).
10. Public language: English. Portability, predictable install, privacy, and clear English docs are
    product requirements.
11. No user-specific absolute paths. Runtime state goes to a platform state directory; nothing durable
    in the repository.
12. Do not reuse the Herdr socket, manifest, or `HERDR_*` environment contract anywhere.

## Current V1 Assets: Keep vs Drop

**Keep (agent-agnostic, no Herdr coupling)**:

- `src/scanner/scan-latex.ts` — stateful `$...$` / `$$...$$` / `\(...\)` / `\[...\]` scanner.
- `src/core/contracts.ts`, `src/core/errors.ts`, `src/core/limits.ts` — `Formula`, `RenderedImage`,
  `ErrorCode`, `SafeLimitKind`, `POLICY_LIMITS` (prune Herdr-specific error codes such as
  `viewer_open_failed`, `graphics_disabled`, `herdr_protocol_error`).
- `src/renderer/*` — KaTeX → Chromium → sharp PNG pipeline: `index.ts`, `render.ts`, `document.ts`,
  `markdown.ts`, `browser-backend.ts`, `layout.ts`, `runtime-check.ts`. Re-target only the final
  pixel-delivery boundary.
- `src/boundary/*`, `src/state/*`, `src/config/*`, `src/events/lifecycle.ts` — keep only what is
  needed when input is a discrete file/pipe rather than live pane content. The fingerprint boundary
  resolver is still useful for resolving an appended/updated answer from a possibly-shared buffer, but
  the V2 primary path is a clean document, so the boundary stack becomes opportunistic, not required.
- `src/presentation/final-response.ts` — useful only if the tool still parses a raw agent transcript;
  keep as optional, off-by-default.

**Drop / replace (Herdr-coupled)**:

- `src/herdr/*` — `socket-client.ts`, `event-decoder.ts`: replaced by ours-own terminal I/O and
  document input. Entirely removed.
- `src/graphics/placement.ts`, `src/graphics/publisher.ts`: replaced by the Rust Kitty placement
  transmitter. The encoder/policy logic moves into Rust (or a small TS pre-step that emits payload and
  lets Rust place it).
- `src/viewer/*` entirely — `manager.ts`, `runtime.ts`, `transport.ts`, `transport-protocol.ts`,
  `presenter.ts`, `render-layout.ts`, `scroll-frames.ts`, `stack-images.ts`, `ownership.ts`:
  replaced by Rust placement + scroll state machine.
- `src/on-agent-status.ts`, `src/on-pane-closed.ts`, `src/startup.ts`, `src/viewer.ts`,
  `src/diagnose.ts`, `src/manifest/*`, `herdr-plugin.toml`: all removed.

## Target Architecture

Two processes cooperate:

```
[ input: file | pipe | stdin ]  ──►  tmath (Rust CLI)
                                        │
                                        │ parses doc → formula/block list
                                        │ spawns renderer subprocess (one-shot)
                                        ▼
                              [ tmath-render (TS, current renderer/*) ]
                                        │
                                        │ KaTeX + allowlisted Markdown → transparent PNG(s)
                                        │ returns PNG bytes + dimensions over stdout/IPC
                                        ▼
                              [ Rust terminal frontend (ported from pixel-core) ]
                                        │
                                        │ raw mode, Kitty protocol negotiation,
                                        │ mouse + scroll parsing, scroll state machine,
                                        │ (optional) native Swift helper
                                        ▼
                              [ Kitty-graphics terminal: Ghostty / kitty / WezTerm ]
                                        (one scrollback-anchored placement per block,
                                         U=1 virtual placement + placeholder grid)
```

### Process responsibilities

- **`tmath` (Rust binary)** — owns the terminal: enables raw mode and the needed modes
  (`?1003h` all-motion mouse, `?1006h` SGR mouse, `?1016h` pixel mouse, `?2004h` bracketed paste;
  does NOT enter the alt screen, so the main-buffer scrollback is preserved). Reads the document from
  the chosen input. Sends each formula/document block to the renderer subprocess, receives PNG
  payloads, negotiates Kitty support, transmits each placement into the main buffer at the cursor row
  where the block belongs, writes the placeholder grid for real cells, and runs the input loop
  (mouse wheel / SGR / pixel coordinates → scroll state machine; keys → scroll fallback). Leaves the
  terminal in a clean state on exit.
- **`tmath-render` (TS, one-shot)** — the existing `renderer/*` pipeline. Reads a JSON request on
  stdin (or a temp file), produces a transparent PNG, writes a bounded JSON response (dimensions,
  byte size, base64 or a write-once shm/temp path) to stdout, and exits. Spawned per document or per
  block; never long-running.
- **IPC contract** — a small, versioned JSON protocol over stdin/stdout between Rust and the TS
  renderer. Bounded request/response sizes, a render timeout, and an explicit trust policy. Later, an
  optional shared-memory/file medium (`t=s` / `t=f` Kitty media) can avoid pushing large payloads
  through a pipe, mirroring `kitty_transmit_named`.

### Rust module port (from `terminal-browser/engine/crates/pixel-core/src`)

Port only the terminal-facing pieces; drop the browser/electron, layout-tree, compositor, and React
code:

| Port | Source file | Purpose |
|---|---|---|
| Kitty escapes | `kitty.rs` | `kitty_transmit_placed` chunking (`a=T,f=32,o=z,s,v,t=d,i,q=2,m`), virtual placement `U=1,c,r`, `placeholder_grid` (per-cell combining-character encoding), `kitty_delete`, `a=q` media/format probe |
| Terminal init / reset | `terminal.rs` (trimmed) | raw mode via termios, mode-enable/reset strings, cell-size probe (`\x1b[16t`, winsize fallback), pixel-mouse probe (`?1016$p` DECRQM) |
| SGR/pixel mouse parse | `terminal.rs` `parse_sgr_mouse` | `<b;x;y` decoding, wheel/button/motion/release, cells→px |
| Scroll state machine | `scroll/mod.rs`, `scroll/profiles/smooth.rs` | `tick` on input, `step`/`chase` per frame, exponential easing |
| Native input helper | `native-scroll-helper.swift` + `native.rs` | macOS optional trackpad precision + pinch + OS cursor; builds via `build.rs`, parses `s/z/m/w/scale` lines |

The Rust crate owns: stdout transmission, stdin reading (poll + buffered parse), placement tracking
(one `image_id` per block, delete-on-replacement or on block removal), and the input loop.

## Data Flow (a single `render` invocation)

1. `tmath render [-o file| - | file]` reads the document into memory (bounded by aggregate
   policy limits).
2. Scanner produces an ordered list of text/math/doc-format segments with byte offsets.
3. `tmath` computes per-block render requests and calls `tmath-render` once, receiving a bounded
   ordered set of transparent PNGs plus dimensions.
4. Rust enables terminal modes (main buffer), measures cell size, and probes Kitty graphics support.
   If unsupported, prints a clear message and exits non-zero.
5. For each block, `tmath` moves the cursor to a home row and transmits the placement
   (`a=T,f=32,o=z,s,v,t=d,i,q=2,U=1,c,r`), then writes the placeholder grid so the cells exist and
   scroll with scrollback. Blocks are placed in source order down the line.
6. `tmath` enters the input loop: mouse wheel/SGR/pixel deltas feed `ScrollState`; smooth profile
   steps each frame; keys drive the fallback scroll. `q`/`Ctrl-C` exits and resets the terminal.
7. On invalid input, missing Kitty support, render timeout, or payload rejection, previous valid
   placements remain intact and the process fails closed.

## CLI Surface (planned)

```
tmath render <path | ->
tmath agent [--source-pane <id>] [--percent <p>] [--wait-ms <ms>]   # P1
tmath agent-viewer <socket-path>                                    # P1
tmath watch <path>            # re-render on file change (P2)
tmath ls                      # list active placements in this terminal (P2)
tmath --help / --version
```

Place planned command names in `package.json`/Cargo manifests only once they exist; do not document
unbuilt commands as working.

## Phase 8 - Agent Integration (tmux viewer)

A P1 extension for the standalone product: show the finished output of a coding
agent (Claude Code, Codex, opencode, Cursor, pi, ...) in a separate tmux
viewer pane as rendered Markdown + math. This reuses the proven
`tmath-render/1` pipeline and the scrollback-anchored placement model; tmux
replaces the deprecated Herdr pane owner.

```text
[ agent running in a tmux source pane ]
        │  tmux capture-pane (bounded history)
        ▼
[ tmath agent (long-running watcher) ]
        │  answer boundary proven from snapshots (exact/stable prefix,
        │  working-frame repaint, prompt strip), debounce, fail closed
        │  Unix socket: length-prefixed JSON (document/quit), bounded
        ▼
[ tmath agent-viewer (in the viewer pane) ]
        │  one-shot tmath-render subprocess (KaTeX + allowlisted Markdown)
        ▼
[ viewer pane: bounded answers appended into one composite viewport,
  replaced by image id, scrolled by wheel/arrows, q/Ctrl-C closes ]
```

Key behaviors:

- The watcher owns tmux only through a fixed allowlisted `tmux` CLI surface
  (split/capture/display/kill) with validated pane ids; no agent content ever
  reaches a shell.
- Under `$TMUX`, pane-local bytes (cursor moves, modes, placeholder cells) stay
  on stdout while Kitty APC commands use a selected graphics route: by default
  a validated write to the attached client tty, or optionally per-APC DCS
  passthrough with embedded `ESC` bytes doubled.
- The viewer fails closed on graphics-unavailable (outside tmux), over-limit,
  malformed, and render-error paths, keeping the previous image. Inside tmux,
  where queries cannot round-trip, graphics support is assumed for known
  Kitty-capable outer terminals and the chosen route is logged to stderr.
- Captured Unicode box tables are normalized back to the allowlisted Markdown
  table subset before rendering because terminal capture exposes presentation
  cells rather than the coding agent's original Markdown source.
- Private: the watcher passes the renderer worker path (`TMATH_RENDER_WORKER`)
  to the viewer on the command line, because `tmux split-window` starts panes
  with the server environment.
- Privacy: logs carry event names, pane ids, counts, and byte sizes only;
  sockets live under the platform temp directory.

Current correction: the earlier Ghostty 1.3.1 + tmux 3.5a observation proved
only that the watcher/viewer pipeline ran and placeholder cells were printed.
It did not prove that image pixels reached the outer terminal. The legacy tmux
transport must double every embedded `ESC`, wrap each Kitty APC independently,
and leave pane-local cursor, mode, and placeholder bytes unwrapped. Narrow
controlled pixel display is now recorded for Ghostty + tmux and cmux + tmux
(see `docs/evidence/2026-08-03-tmath-tmux-graphics.md`); full acceptance in
AT-2-806, AT-2-810, and AT-2-811 remains pending for resize, detach/attach,
multiple clients, and live end-to-end coding-agent watcher responses.

### Shell auto-watch (opt-in, per directory)

`tmath agent` still requires the user to find the source pane id and start the
watcher by hand. To make the launch-and-forget experience work right after
`curl | bash` install, `scripts/install.sh` installs an opt-in shell
integration that wraps coding-agent launcher commands (`claude`, `codex`,
`opencode`, `cursor-agent`, `pi`) and starts `tmath agent` automatically, but
only inside directories the user has explicitly allowlisted.

```text
[ scripts/install.sh ]
        │  writes $APP/shell/tmath-agent.sh
        │  appends a marker-delimited source line to ~/.zshrc and ~/.bashrc
        ▼
[ ~/.zshrc / ~/.bashrc ]  →  source $APP/shell/tmath-agent.sh
        │  defines alias claude/codex/opencode/cursor-agent/pi
        │  each alias calls __tmath_wrap_agent <real-cmd> "$@"
        ▼
[ __tmath_wrap_agent ]
        │  tmath agent-allowed?  (directory allowlist, exit code only)
        │    no  → exec the real command untouched
        │    yes → in tmux: lock pane id, start `tmath agent --source-pane`
        │          in background, then exec the real command in place
        │       → outside tmux, interactive TTY: build a single-pane tmux
        │          session running the command, start `tmath agent
        │          --source-pane` in the background, then attach
        │       → outside tmux, non-interactive (pipes/redirects): exec the
        │          real command untouched, tmath never runs
```

Key behaviors:

- Directory scoping is a Rust-side allowlist (`tmath agent-enable [<dir>]` /
  `tmath agent-disable [<dir>]` / `tmath agent-allowed [<dir>]`) stored at
  `${XDG_CONFIG_HOME:-$HOME/.config}/tmath/agent-allowlist`, one canonicalized
  absolute path per line. `agent-allowed` matches the directory itself or any
  descendant by `Path` component comparison (not string-prefix matching, so a
  sibling directory with a matching name prefix is never allowed) and is
  silent (no stdout/stderr) since the shell wrapper calls it on every launch.
- The shell wrapper never embeds the allowlist logic itself; it only checks
  the `tmath agent-allowed` exit code, keeping the wrapper thin and the path
  logic testable in Rust.
- Duplicate-watcher prevention is a shell-side concern (a pane-id-scoped lock
  file with a PID liveness check), not a Rust-side constraint, so `tmath
  agent` itself stays free for advanced manual multi-watcher use.
- Outside tmux, a plain `tmux new-session <cmd>` never sources shell rc files,
  so the wrapper cannot re-fire inside the new session; the wrapper instead
  creates the session itself and starts the watcher as a background process
  of the launching shell (the same shape as the in-tmux path), so the only
  extra pane the user sees is the watcher's own viewer split, never a
  dedicated watcher pane.
- Non-interactive invocations (stdin/stdout not both a TTY) always pass
  through untouched, so scripted or piped use of these commands never
  changes behavior.
- This is the first feature that edits a user's shell rc files. Installation
  is opt-in and idempotent (a marker-delimited block that installer re-runs
  replace in place) and skippable with `TMATH_SKIP_SHELL_INTEGRATION=1`.
  Auto-watch itself stays opt-in per directory even after the shell
  integration is installed, since `agent-allowlist` starts empty.

## Repository Layout (target)

```text
Cargo.toml                     # Rust workspace: terminal frontend + ported pixel pieces
engine/                        # Rust crates (kitty, terminal-io, scroll, native)
  crates/tmath-core/
  crates/native-scroll-helper/  # Swift, built by build.rs
   (or reuse terminal-browser as a git dependency where the port is thin)
src/                           # TS renderer + scanner + core (pruned of Herdr)
src/renderer/  src/scanner/  src/core/
tests/
  unit/       # scanner, limits, renderer markdown/doc, scroll math (Rust)
  integration/
  fixtures/
scripts/
docs/
specs/terminal-math-v2/  {tests, plans, tasks}/main.md
```

Remove `herdr-plugin.toml`, the `src/herdr`, `src/viewer`, `src/graphics`, `src/manifest`,
`src/on-*.ts`, `src/startup.ts` tree, and all `HERDR_*` reads.

## Phases

- **Phase 0** — port the Rust terminal surface: Kitty escapes, terminal init/reset, mouse parse,
  scroll state machine, native helper. Add a minimal Rust test harness (fake termios / vt parser) so
  the escape construction and mouse decode are unit-tested before any real terminal is involved.
- **Phase 1** — render transport: define the versioned JSON IPC between Rust and `tmath-render`;
  wire the existing TS renderer as a one-shot subprocess; enforce size/timeout/trust limits at the IPC
  boundary.
- **Phase 2** — placement + scrollback anchoring: transmit one placement per block into the main
  buffer with virtual placement + placeholder grid; verify images scroll with the shell scrollback;
  implement replacement/delete of a stale block.
- **Phase 3** — input loop: mouse wheel → scroll state machine; keyboard fallback; bracketed paste
  hygiene; clean exit/reset; focus events.
- **Phase 4** — CLI: `render` over file/pipe/stdin; document composition (allowlisted Markdown +
  `$..$`/`$$..$$`); diagnostics; version/help.
- **Phase 5** — hardening + security: revisit the threat model for the new Rust/TS split; enforce all
  aggregate limits; privacy audit (no content in logs/state); fuzz the mouse/escape parser and the
  scanner.
- **Phase 6** — compatibility + docs: real Ghostty / kitty / WezTerm evidence, macOS primary; update
  `AGENTS.md`, `docs/concept.md`, `docs/architecture.md`, `README.md`, security/privacy docs to the
  standalone product; port or re-run the V2 acceptance tests.
- **Phase 7** — release gate: clean build, install, validation; no local paths; reproducible build;
  version tags agree; P1/P2 post-V2 backlog (Linux, Windows, `watch`, additional terminals).
- **Phase 8** — agent integration (P1): `tmath agent` tmux watcher + `tmath agent-viewer`
  (boundary detection, bounded socket channel, viewer render/replace/scroll, DCS passthrough);
  record real-agent boundary matrix. Not part of the `0.2.0` gate.

## Advisory / Rollback

- The instructions in `AGENTS.md`, and every source-of-truth doc that references the Herdr plugin
  identity, must be updated in the same change that drops the Herdr contract. Keep V1 tag `v0.1.0`
  intact so users can roll back. Do not delete the V1 spec; mark it superseded.
- Anything described as "planned" here must not be presented as released. Update this status block
  and the acceptance/task docs as each phase lands, in a separate documentation commit after the
  implementation commit.

## Risk Register

- **Kitty protocol variance**: negotiate per-feature (`a=q`), as `terminal-browser` does, rather than
  assuming `_Gi=1` negotiation. Ghostty is the verified primary; kitty/WezTerm are P1.
- **Scrollback anchoring fragility**: virtual placements over blank cells can be dropped by some
  terminals; the placeholder grid mitigates this and must be integration-tested in each supported
  terminal.
- **Rust toolchain / build**: the port adds Cargo/Swift to the build. Keep the renderer TS-only and
  the Rust surface thin and well-scoped to avoid a large, hard-to-review port.
- **Input-loop livelock / parse errors**: fuzz the escape/mouse parser; cap per-frame work; always
  reset the terminal on any exit path.
- **Payload transport**: large PNGs through a pipe are slow; add `t=s`/`t=f` shared-memory/file
  media in a later phase if needed, keeping the bounded-size invariants.
- **tmux image relay**: stable tmux requires one passthrough envelope per Kitty
  APC with embedded `ESC` bytes doubled; placeholder text and pane-local
  terminal controls must remain outside that envelope. Query replies cannot be
  relied on, so runtime compatibility is gated by controlled pixel evidence.
  Narrow controlled pixel display is recorded for Ghostty + tmux and cmux +
  tmux; full resize, detach/attach, multi-client, and live-agent matrix
  acceptance in AT-2-806, AT-2-810, and AT-2-811 remains pending.

## Definition of Done (for the refactor)

1. `tmath render <doc>` renders `$..$` / `$$..$$` and allowlisted Markdown as scrollback-anchored,
   mouse-scrollable placements in Ghostty with no Herdr runtime present.
2. Removing `herdr-plugin.toml`, `src/herdr`, `src/viewer`, `src/graphics`, `src/manifest`, and all
   `HERDR_*` reads leaves no dangling imports (`npm run check` and `cargo test` pass).
3. Mouse wheel scrolls the rendered document; keyboard fallback still works; `q` / `Ctrl-C` restore
   the terminal.
4. Invalid input / missing Kitty support / render timeout fail closed and preserve earlier valid
   placements.
5. All privacy invariants hold: no content in logs, state, or network.
6. Compatibility and documentation are updated to the standalone product, with V1 explicitly
   superseded.
