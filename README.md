# terminal-math

Render LaTeX and Markdown as scrollable images in your terminal.

terminal-math is a standalone terminal renderer (no plugin runtime required). It renders
`$...$` and `$$...$$` equations and a strict allowlisted Markdown subset to transparent images,
transmits them into the terminal with the Kitty graphics protocol, and anchors them to terminal
cells so they scroll with the shell's scrollback. Mouse wheel and keyboard both scroll the
rendered document.

It runs in any Kitty-graphics-capable terminal such as Ghostty, kitty, or WezTerm.

> **Status: in development.** This repository is transitioning from the Herdr Math plugin
> (v0.1.0) to the standalone terminal-math renderer. The refactor plan is in
> [specs/terminal-math-v2/plans/main.md](specs/terminal-math-v2/plans/main.md). Core
> implementation is complete through Phase 5; release-gate evidence (real Ghostty run,
> install, and tagged release) is still outstanding. A P1/experimental agent
> integration (`tmath agent`, tmux viewer) is in Phase 8; see
> [getting-started.md](docs/getting-started.md).

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/sodeyama/herdr-math/main/scripts/install.sh | bash
# from a checkout: bash scripts/install.sh  (or npm run install:local)
```

Installs the `tmath` binary, its local renderer, and the coding-agent skill
under `~/.local/share/tmath` with a launcher on `~/.local/bin`. No
`TMATH_RENDER_WORKER` setup is needed.

## Planned use

```sh
# Render a Markdown/LaTeX document and anchor the images in the terminal
terminal-math render ./notes.md
terminal-math render -          # read from stdin

# Show a coding agent's finished answers (with rendered math) in a viewer pane
tmath agent --source-pane %0    # inside tmux, watching pane %0 (experimental)
```

## Product boundaries

- Renders `$...$` and `$$...$$` math plus a strict, allowlisted Markdown subset. No raw HTML,
  remote resources, user CSS/color directives, or scripts.
- Renders locally with KaTeX. It never executes TeX binaries, shell input, user JavaScript, or
  remote resources.
- Never uploads document content, equations, images, logs, or telemetry to a network service.
- Images are transparent PNGs placed into the main terminal buffer so they scroll with the
  shell. No viewer pane, no plugin runtime required.

## Documentation

- [Concept and product boundaries](docs/concept.md)
- [Architecture](docs/architecture.md)
- [Compatibility](docs/compatibility.md)
- [Coding agents (Claude Code, Codex, opencode, Cursor, pi)](docs/coding-agents.md)
- [Getting started and troubleshooting](docs/getting-started.md)
- [Release checklist](docs/RELEASE.md)
- [Post-V2 backlog](docs/backlog.md)
- [Privacy](PRIVACY.md)
- [Security](SECURITY.md)
- [Support](SUPPORT.md)
- [Contributing](CONTRIBUTING.md)
- [Changelog](CHANGELOG.md)

## Development

```sh
cargo build       # Rust terminal frontend (Kitty graphics, mouse/scroll)
npm ci            # TypeScript renderer dependencies
npm run check
npm test
```

Read [AGENTS.md](AGENTS.md) before contributing. Public documentation, code comments, logs,
commits, and release material are written in English.

terminal-math is licensed under the [MIT License](LICENSE). Third-party runtime notices are in
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
