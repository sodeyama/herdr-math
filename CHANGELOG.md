# Changelog

All notable changes to Terminal Math will be documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and releases use
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
- `tmath agent` / `tmath agent-viewer`: watch a tmux pane running a coding agent and show each
  finished answer (prose + typeset math) in a viewer pane, with tmux passthrough support.
- One-command install (`scripts/install.sh`, `npm run install:local`): builds and installs the
  binary + renderer to `~/.local/share/tmath`, a `~/.local/bin/tmath` launcher, renderer
  auto-discovery (no `TMATH_RENDER_WORKER` needed), and a `tmath` skill linked into coding-agent
  skill directories (Claude Code, Codex, Cursor, opencode, pi).

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

### Security

- Document text and LaTeX source are excluded from durable state and logs.
- Remote resources, trusted links, TeX execution, shell evaluation, and input-selected
  executable paths are denied in both the Rust and TypeScript layers.

The preceding Herdr plugin implementation is versioned as `0.1.0` (see the tag and the
superseded `specs/herdr-math-v1/`).
