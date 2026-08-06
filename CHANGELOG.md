# Changelog

All notable changes to Terminal Math will be documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and releases use
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Removed

- The deprecated Node/KaTeX/Chromium renderer (`src/renderer/*`, `--engine node`,
  `TMATH_RENDER_WORKER`, Playwright postinstall). Rendering is native-only
  (RaTeX + Typst in-process). Install no longer requires Node.js or npm for
  runtime; `scripts/install.sh` builds the Rust binary only.

### Added

- Phase 5 portability smokes: release footprint gate (`scripts/smoke-footprint.sh`,
  ≤ 60 MiB, no Node/browser dynamic deps), pipe render smoke, install-without-Node
  smoke, and a Linux x86_64 CI job (AT-3-801..803 build scope).

### Fixed

- `scripts/install.sh` reads the workspace version from the root `Cargo.toml` again
  (the crate manifest uses `version.workspace = true`).
- The outside-tmux auto-watch launch no longer opens a dedicated watcher pane:
  the watcher runs as a background process, so the session shows only the
  wrapped command and the viewer pane. Transport env
  (`TMATH_TMUX_TRANSPORT`, `TMATH_DPR`, `TMATH_DEBUG_LOG`) now reaches the
  watcher by ordinary environment inheritance.

## [0.3.0] - 2026-08-06

First tagged release.

### Added

- Native engine as the default render path (`tmath render` without `--engine node`):
  RaTeX math + Typst prose, in-process, embedded fonts, no subprocess.
- Agent streaming hardening: transcript re-resolution, idle capture fallback, and
  streaming replay tests (T3-404/405).
- Runtime reliability (specs/runtime-reliability-v1, all phases): atomic PATH-launcher
  install with a foreign-file warning, `tmath diagnose` PATH-launcher and version-skew
  checks, wrapper failure visibility, the fail-closed tmux outer-terminal gate with
  distinct no-client/unverified refusals and gate inputs in `diagnose`, transport env
  propagation to tmux-spawned watchers and viewers, a transient no-client retry budget
  in the viewer, hermetic private-socket tmux smokes, and a CI job running the full
  smoke suite.
- Viewer polish from the live scroll-lab close-out: immediate status-bar updates when
  wheel scrolling disengages follow, text brightness parity with terminal fonts
  (near-white theme color + alpha-gamma edge lift), and a transient scrollbar.
- Phase 6 hardening: deterministic fuzz coverage for the stream splitter and delta
  codec, pathological limit tests (AT-3-703), and a `cargo test`-based performance
  suite (`engine/crates/tmath/tests/performance.rs`).
- Rust CI gates: `cargo test`, `cargo clippy`, and `cargo fmt --check` on macOS arm64.

### Changed

- The default tmux graphics route is now passthrough (serialized through tmux's output
  queue; requires `allow-passthrough on`, checked automatically). The previous
  client-tty default could tear escape sequences against concurrent tmux output under
  heavy streaming; client-tty remains as an explicit `TMATH_TMUX_TRANSPORT=client-tty`
  opt-in.
- V2 specification triad marked superseded; V3 is the current acceptance contract.
- `npm run test:performance` now runs the Rust performance suite instead of a missing
  Vitest spec.

### Fixed

- A block whose retained PNG could not be re-rendered no longer fails the whole
  visible-window sync on every pass (the field-observed recurring
  `sync_failed (RendererFailed)`); it fails closed per block.

### Notes

- V3 Phase 5 (T3-502) removed the Node browser renderer; single-binary packaging
  (T3-503) and Linux smoke evidence (T3-504) remain open.

## [0.2.0] - Unreleased

### Added

- Standalone `tmath` CLI: `render <file | ->` with bounded reads and composition options, and
  `diagnose` capability checks.
- Rust terminal frontend: raw mode, Kitty negotiation, SGR/pixel mouse parsing, bounded input
  decoding, smooth scroll state machine, and the macOS native scroll helper.
- Scrollback-anchored placements with virtual placement (`U=1,c,r`) and a placeholder grid so
  images scroll with the shell scrollback; replacement and scoped deletion.
- Versioned `tmath-render/1` JSON IPC between the Rust CLI and the one-shot TypeScript render
  subprocess.
- Local KaTeX, Chromium, and sharp PNG rendering with network denial and strict limits.
- Conservative `$...$`, `$$...$$`, `\(...\)`, and `\[...\]` scanning and strict allowlisted
  Markdown rendering.
- Bounded, privacy-preserving logs, stable error records, and fuzz/adversarial parser coverage.
- The Herdr plugin contract (`herdr-plugin.toml`, `src/herdr`, viewer, graphics, manifest,
  boundary, state, events, config, presentation, diagnostics) removed; V1 is superseded.
- `tmath agent` / `tmath agent-viewer`: watch a tmux pane running a coding
  agent and show each finished answer (prose + typeset math) in a viewer pane.
  The default validated client-tty graphics route supports Ghostty and cmux;
  `TMATH_TMUX_TRANSPORT=passthrough` selects stable tmux DCS passthrough.
- One-command install (`scripts/install.sh`, `npm run install:local`): builds and installs the
  binary + renderer to `~/.local/share/tmath`, a `~/.local/bin/tmath` launcher, renderer
  auto-discovery (no `TMATH_RENDER_WORKER` needed), and a `tmath` skill linked into coding-agent
  skill directories (Claude Code, Codex, Cursor, opencode, pi).
- Opt-in shell auto-watch: `tmath agent-enable`/`agent-disable`/`agent-allowed` manage a
  per-directory allowlist, and `scripts/install.sh` sources a shell wrapper from
  `~/.zshrc`/`~/.bashrc` (`TMATH_SKIP_SHELL_INTEGRATION=1` to skip) that starts a background
  `tmath agent` watcher automatically when `claude`, `codex`, `opencode`, `cursor-agent`, or `pi`
  runs in an allowlisted directory.

### Fixed

- One-shot render subprocess: the entry check now resolves symlinks (macOS `/tmp` → `/private/tmp`)
  so the worker runs when its path is under a symlinked directory, and the process drains stdout
  before exiting so larger responses are never truncated.
- `tmath render -` with a piped document no longer fails with
  `Inappropriate ioctl for device`: when stdin is not a terminal the original
  stdout descriptor is used for raw mode, probes, and input. (A freshly opened
  `/dev/tty` descriptor was tried first, but macOS `poll(2)` reports its
  readiness as `POLLPRI` rather than `POLLIN`, which made capability probes
  time out and leak the terminal's reply.) `tmath render` also assumes tmux
  passthrough inside tmux, matching `tmath agent-viewer`, and enables the
  window's `allow-passthrough` option automatically (best-effort) when running
  under tmux.
- `tmath render -` no longer holds the terminal after placing: when the
  document comes from a pipe, the image is placed and the command returns
  immediately (the image stays in the terminal scrollback), instead of
  blocking the shell in an input loop the pipeline cannot drive.
- `tmath render` places the image directly below the current command line
  (cursor-relative home row) instead of at the top row of the terminal.
- `tmath render` no longer adds a spurious blank line before the image when
  the shell has already moved the cursor to the start of a line (e.g. right
  after a piped `tmath render -`): the cursor column is queried via `CSI 6n`
  and the placement only advances a line when the cursor is not already at
  column 1.
- tmux graphics now double embedded `ESC` bytes and wrap each Kitty APC
  independently; terminal modes, cursor movement, and placeholder cells remain
  pane-local. Agent viewer scrolling crops and replaces the visible RGBA
  viewport, and replacement clears stale placeholder cells.
- Agent answer boundaries now cover pi contextual repaints, opencode sliding
  capture windows, and Cursor CLI tool-activity prefixes.

### Security

- Document text and LaTeX source are excluded from durable state and logs.
- Remote resources, trusted links, TeX execution, shell evaluation, and input-selected
  executable paths are denied in both the Rust and TypeScript layers.

The preceding Herdr plugin implementation is versioned as `0.1.0` (see the tag and the
superseded `specs/herdr-math-v1/`).
