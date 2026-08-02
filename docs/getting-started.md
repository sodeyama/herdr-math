# Getting Started

Terminal Math 0.2.0 is under development. This document describes the planned standalone
workflow. Nothing below is a release claim until the release-gate tasks complete and real
Ghostty evidence is recorded.

## Requirements

- macOS arm64 (the primary target)
- A Rust toolchain for the terminal frontend
- Node.js 22 or later and npm for the render subprocess
- A Kitty-graphics-capable terminal: Ghostty 1.3.1 (verified target), kitty or WezTerm (P1)

Terminal Math does not call Ghostty APIs and does not require Glowing Bear, a plugin runtime, or
a browser window.

## Build

Clone the repository and enter the checkout:

```sh
npm ci
npm run audit:browser
npm run build
cargo build
```

`npm ci` installs the locked dependencies and the local Chromium headless shell; `npm run
install:browser` repairs only the locked browser artifacts, and `npm run audit:browser` verifies
them. `cargo build` produces the `tmath` binary in `target/debug/tmath`.

## Render a document

```sh
# Render a Markdown/LaTeX file and place it in the terminal
./target/debug/tmath render ./notes.md

# Read the document from stdin
cat notes.md | ./target/debug/tmath render -

# Composition options
./target/debug/tmath render --content-width 800 --font-size 18 ./notes.md
```

Terminal Math renders `$...$` and `$$...$$` equations and the strict allowlisted Markdown subset
(headings, emphasis, lists, quotes, tables, code blocks, inert links). The image is placed into
the main terminal buffer so it scrolls with the shell scrollback. Mouse wheel and keyboard scroll
a tall rendered document; `q` or Ctrl-C resets the terminal.

If the built render subprocess is located elsewhere, set `TMATH_RENDER_WORKER` to its path.

## Show a coding agent's answers in a viewer pane (experimental)

`tmath agent` watches a tmux pane running a coding agent (Claude Code, Codex,
opencode, Cursor, pi, and similar) and shows each finished answer as rendered
Markdown + math in a right-hand viewer pane. This is a P1/experimental
feature; the `0.2.0` release does not depend on it.

```sh
# Inside tmux. tmux 3.2+, with passthrough enabled for the window:
tmux set-option -t <window> -w allow-passthrough on

# Pane 1: run your coding agent.
# Pane 2: watch pane 1 (use its pane id, e.g. %0):
env TMATH_RENDER_WORKER=/abs/path/dist/renderer/subprocess.js \
  /abs/path/target/debug/tmath agent --source-pane %0
```

The watcher creates the viewer pane, prints `tmath agent: watching ...` once,
and then logs only bounded status to stderr. `q`/Ctrl-C stops it. Inside the
viewer pane, the wheel and arrow keys scroll the current answer and `q`/
Ctrl-C closes it.

Known limitation: inside a tmux pane, Kitty images require the outer terminal
to relay both the passthrough sequence and its reply. On the verified
Ghostty 1.3.1 + tmux 3.5a setup the reply is not relayed yet, so the viewer
fails closed with `no Kitty graphics support`. A direct (non-tmux) Ghostty
terminal displays placements correctly.

## Diagnose

```sh
./target/debug/tmath diagnose
```

Diagnostics report only allowlisted versions, capabilities, statuses, counts, and stable error
codes. They do not print document text, equations, environment contents, or local paths.

Common results:

- `renderer subprocess: missing`: `TMATH_RENDER_WORKER` is not set; point it at
  `dist/renderer/subprocess.js` after `npm run build`.
- `node: missing`: install Node.js 22 or later.
- `stdout: not a terminal`: image transport needs a real terminal (piping output only prints a
  text summary).
- `kitty graphics: unsupported`: the attached terminal does not support the Kitty graphics
  protocol.

## Help and version

```sh
./target/debug/tmath --help
./target/debug/tmath --version
```

## Known limits

- Only `$...$`, `$$...$$`, `\(...\)`, and `\[...\]` math delimiters are parsed, and only the
  allowlisted Markdown subset is rendered by a local parser. Raw HTML, images, scripts, custom
  CSS, and color directives are not supported.
- Formulas in code spans, fenced code, prices, shell variables, and ambiguous delimiter runs are
  rejected.
- Strict formula count, source length, image dimension, byte, placement, and time limits apply.
- Earlier placements remain on invalid input, limits, timeout, or graphics failure.
- Document text and LaTeX source are never written to durable state or logs.
- macOS arm64 with Ghostty is the only currently verified terminal combination; kitty and WezTerm
  are P1; Linux and Windows are not yet supported.

See [Compatibility](compatibility.md), [Architecture](architecture.md), and the official
[Kitty graphics protocol](https://sw.kovidgoyal.net/kitty/graphics-protocol/) for more detail.
