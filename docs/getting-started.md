# Getting Started

Terminal Math 0.2.0 is under development. This document describes the planned standalone
workflow. Nothing below is a release claim until the release-gate tasks complete and real
Ghostty evidence is recorded.

## Install (one command)

From a checkout, or anywhere (the script clones the repository on first use):

```sh
bash scripts/install.sh
# equivalent: npm run install:local
# one-liner, like terminal-browser:
# curl -fsSL https://raw.githubusercontent.com/sodeyama/terminal-math/main/scripts/install.sh | bash
```

The installer builds and places everything under your user data directory and
puts a `tmath` launcher on `~/.local/bin`:

- `tmath` binary: `~/.local/share/tmath/app/bin/tmath`
- renderer (Node + local Chromium): `~/.local/share/tmath/app/renderer`
- the `tmath` agent skill linked into Claude Code, Codex, Cursor, opencode,
  and pi skills directories.

After install the binary locates its renderer automatically — no
`TMATH_RENDER_WORKER` setup is needed. `tmath diagnose` verifies everything
(`--prefix <dir>` / `TMATH_INSTALL_ROOT` change the target; `TMATH_SKIP_TESTS=1`
skips the post-install check). Add `~/.local/bin` to `PATH` if the installer
detects it is missing.

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
the main terminal buffer so it scrolls with the shell scrollback.

- With a file argument in a terminal, the document stays interactive: mouse wheel and keyboard
  scroll it, and `q` or Ctrl-C returns to the shell.
- With a piped document (`tmath render -`), the image is placed and the command returns right
  away, since the pipeline cannot receive key input; scroll the image with the normal terminal
  scrollback (or use `tmath agent` for an interactive viewer pane).

If the built render subprocess is located elsewhere, set `TMATH_RENDER_WORKER` to its path.

## Show a coding agent's answers in a viewer pane (experimental)

`tmath agent` watches a tmux pane running a coding agent (Claude Code, Codex,
opencode, Cursor, pi, and similar) and shows each finished answer as rendered
Markdown + math in a right-hand viewer pane. This is a P1/experimental
feature; the `0.2.0` release does not depend on it.

```sh
# Optional, only when forcing TMATH_TMUX_TRANSPORT=passthrough:
tmux set-option -t <window> -w allow-passthrough on

# Pane 1: run your coding agent.
# Pane 2: watch pane 1 (use its pane id, e.g. %0):
tmath agent --source-pane %0
```

The watcher creates the viewer pane, prints `tmath agent: watching ...` once,
and then logs only bounded status to stderr. It passes the renderer worker
path to the viewer automatically. `q`/Ctrl-C stops the watcher. Inside the
viewer pane, the wheel and arrow keys scroll the current answer and `q`/
Ctrl-C closes it.

Inside tmux, query replies are not reliable, so the viewer uses a
graphics-only route to the attached client's tty by default. Cursor movement,
terminal modes, and Unicode placeholder cells continue through tmux normally,
which preserves pane clipping and redraw. This route is verified with Ghostty
1.3.1 and cmux 0.64.12 on tmux 3.5a. Set
`TMATH_TMUX_TRANSPORT=passthrough` to force the standards-based route; each
Kitty APC is independently ESC-doubled and requires tmux 3.3+ with
`allow-passthrough on`. Unknown outer terminals fail closed before placeholder
output. Outside tmux, `tmath render` probes normally.

Per-agent notes (Claude Code, Codex, opencode, Cursor Agent, pi): see
[Coding agents](coding-agents.md).

### Auto-watch (opt-in, per directory)

Starting `tmath agent` by hand still requires finding the source pane id.
`scripts/install.sh` also installs an opt-in shell integration that wraps
`claude`, `codex`, `opencode`, `cursor-agent`, and `pi` and starts the watcher
automatically — but only inside directories you explicitly allowlist. Right
after install, the allowlist is empty, so nothing changes until you opt in:

```sh
# Allow auto-watch for the current directory (and its subdirectories):
tmath agent-enable

# Remove it again:
tmath agent-disable

# Check whether the current directory is allowlisted (silent, exit code only):
tmath agent-allowed
```

Once a directory is allowlisted, running `claude` (or another wrapped
command) there starts a `tmath agent` watcher in the background for that
pane — inside tmux, in the current pane; outside tmux with an interactive
terminal, in a new two-pane tmux session (agent pane + watcher pane) that
gets attached automatically. Non-interactive invocations (pipes, redirects)
always pass through untouched.

Set `TMATH_SKIP_SHELL_INTEGRATION=1` before running the installer to skip the
`~/.zshrc`/`~/.bashrc` edit entirely.

## Diagnose

```sh
tmath diagnose        # installed binary
./target/debug/tmath diagnose   # from a build checkout
```

Diagnostics report only allowlisted versions, capabilities, statuses, counts, and stable error
codes. They do not print document text, equations, environment contents, or local paths.

Common results:

- `renderer subprocess: not found`: the binary could not find its renderer; set
  `TMATH_RENDER_WORKER` or re-run `scripts/install.sh`.
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
