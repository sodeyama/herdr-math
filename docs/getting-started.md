# Getting Started

This guide covers installing Terminal Math 0.3.0, rendering documents, and running the
live typeset viewer for coding agents. The verified platform is macOS with Ghostty and
tmux; see [Compatibility](compatibility.md) for the full matrix.

## Install (one command)

From anywhere (the script clones the repository on first use), or from a checkout:

```sh
curl -fsSL https://raw.githubusercontent.com/sodeyama/terminal-math/main/scripts/install.sh | bash
# from a checkout: bash scripts/install.sh   (equivalent: npm run install:local)
```

The installer builds the release binary and places everything under your user data
directory:

- `tmath` binary: `~/.local/share/tmath/app/bin/tmath`
- launcher script (written atomically at a fresh inode) into a user bin directory,
  chosen from your environment in this order: `$XDG_BIN_HOME` if set; the directory of
  an existing `tmath` launcher on `PATH` (so an upgrade never leaves a second, shadowed
  copy); the first of `~/.local/bin`, `~/bin` that exists and is on `PATH`; otherwise
  `~/.local/bin`. It never writes into other toolchains' bin directories or anywhere
  outside `$HOME`, and it never edits `PATH` (it prints the command if needed).
- optional Node renderer for the deprecated `--engine node` path:
  `~/.local/share/tmath/app/renderer`
- the `tmath` agent skill linked into the Claude Code, Codex, Cursor, opencode, and pi
  skill directories
- **only with `--with-shell-integration`** (or `TMATH_WITH_SHELL_INTEGRATION=1` for the
  `curl | bash` form): a marker-delimited auto-watch snippet in
  `~/.zshrc`/`~/.bashrc` (see [Auto-watch](#auto-watch-opt-in-per-directory); it does
  nothing until you allowlist a directory). Without the flag, rc files are never
  touched; an existing marked block from a previous opt-in keeps being updated so it
  cannot go stale. Remove it by deleting the marked block.

The installer finishes by running `tmath diagnose`. Useful installer knobs:
`TMATH_INSTALL_ROOT` changes the target, `TMATH_SKIP_TESTS=1` skips the post-install
check, `TMATH_SKIP_SHELL_INTEGRATION=1` prevents any rc-file edit including updates to
an existing block.

To update later, re-run the same install command. Never copy a freshly built binary over
`~/.local/bin/tmath` by hand — see [Diagnose](#diagnose) for why that breaks on macOS.

## Requirements

- macOS arm64 (the primary target)
- A Rust toolchain (the installer builds from source)
- A Kitty-graphics-capable terminal: Ghostty (verified), kitty or WezTerm (expected,
  unverified)
- For the viewer inside tmux: tmux 3.3+ with `allow-passthrough on` (the default
  graphics route; `tmath diagnose` checks it)
- Node.js 22+ and npm — **optional**, only for the deprecated `--engine node` render
  path

Rendering is fully local and in-process: RaTeX for math, Typst as a library for the
Markdown subset, fonts embedded in the binary. No network access, no browser, no TeX
installation.

## Render a document

```sh
# Render a Markdown/LaTeX file and place it in the terminal
tmath render ./notes.md

# Read the document from stdin
cat notes.md | tmath render -

# Composition options
tmath render --content-width 800 --font-size 18 ./notes.md
```

Terminal Math renders `$...$` and `$$...$$` equations (plus `\(...\)` / `\[...\]`) and
the strict allowlisted Markdown subset (headings, emphasis, lists, quotes, tables, code
blocks, inert links). Images are transparent PNGs placed into the main terminal buffer,
so they scroll with the shell scrollback like ordinary output.

- With a file argument in a terminal, the document stays interactive: mouse wheel and
  keyboard scroll it, and `q` or Ctrl-C returns to the shell.
- With a piped document (`tmath render -`), the image is placed and the command returns
  right away; scroll with the normal terminal scrollback.
- The deprecated Node/KaTeX engine remains available as `tmath render --engine node`
  (requires the optional renderer install and Node.js).

## Show a coding agent's answers in a viewer pane

`tmath agent` watches a tmux pane running a coding agent (Claude Code, Codex, opencode,
Cursor Agent, pi, and similar) and shows each finished answer as rendered Markdown +
math in a separate viewer pane. For Claude Code it reads the session transcript
directly, so answers stream into the viewer as they are written; for other agents it
falls back to watching the pane content.

```sh
# One-time tmux setup (usually already on; `tmath diagnose` verifies it):
tmux set-option -g allow-passthrough on

# Pane A: run your coding agent.
# Any other pane: watch pane A by its pane id (tmux display-message -p '#{pane_id}'):
tmath agent --source-pane %0
```

The watcher creates the viewer pane, prints `tmath agent: watching %A → %B` once, and
then logs only bounded status lines. `q`/Ctrl-C in the watcher stops it, and the viewer
pane closes with it.

### Inside the viewer

- **Follow mode** (default): the view stays pinned to the newest answer as content
  streams in. The status bar's last word reads `following`.
- **Scroll back**: mouse wheel (with momentum), arrow keys, `PageUp`/`PageDown`,
  `Home`/`End`, or `j`/`k`/`g`/`G`. The first wheel notch away from the bottom
  disengages follow — the status bar flips to `scrolled` and a transient scrollbar
  appears in the last column (it auto-hides about a second after motion stops). New
  answers arriving while you are scrolled back do **not** move your view.
- **Re-engage follow**: press `End` or `F`, or simply scroll back down to the bottom.
- **Quit**: `q` or Ctrl-C in the viewer pane.
- The status bar (row 1) shows the block count, font size, and follow state.

### Watcher options

- `--percent <n>` — viewer pane width as a percentage of the window (default 40).
- `--wait-ms <ms>` (default 600) — how long text must settle before an answer is
  emitted; lower for snappier updates, raise if answers arrive in fragments.
- `--poll-ms <ms>` — pane polling interval.
- `--history <lines>` (default 500) — scrollback captured per answer, for answers
  taller than one screen.

### Graphics transport inside tmux

Inside tmux, the viewer sends image data through **tmux passthrough** by default: each
Kitty graphics command is DCS-wrapped and flows through tmux's own output queue, so it
is serialized against everything else tmux writes. This requires
`allow-passthrough on` (tmux 3.3+); when the option is off, the viewer refuses with an
actionable message instead of silently showing nothing.

`TMATH_TMUX_TRANSPORT=client-tty` selects the alternative route that writes graphics
directly to the attached client's tty. It exists for terminals whose passthrough relay
is broken, but its writes are not serialized against tmux's own output — under heavy
concurrent streaming a write can tear an escape sequence and corrupt the display — so
only set it when passthrough does not work in your terminal.

The outer-terminal gate fails closed: with no attached client, or an outer terminal
that does not advertise a Kitty-capable termname, the viewer refuses (the message names
the reason) rather than emitting graphics that would appear as garbage. Setting
`TMATH_TMUX_TRANSPORT` explicitly overrides the gate — that is your assertion that the
outer terminal renders Kitty graphics.

Per-agent notes (Claude Code, Codex, opencode, Cursor Agent, pi): see
[Coding agents](coding-agents.md).

## Auto-watch (opt-in, per directory)

Starting `tmath agent` by hand requires finding the source pane id. The installer can
also install a shell integration that wraps `claude`, `codex`, `opencode`,
`cursor-agent`, and `pi` and starts the watcher automatically. It is doubly opt-in:
the rc snippet is only installed when you run the installer with
`--with-shell-integration`, and even then it only acts inside directories you
explicitly allowlist. Right after install the allowlist is empty, so nothing changes
until you opt in:

```sh
tmath agent-enable      # allow auto-watch for the current directory (and subdirs)
tmath agent-disable     # remove it again
tmath agent-allowed     # check (silent; exit code only)
```

Once a directory is allowlisted, running `claude` (or another wrapped command) there
starts a watcher in the background for that pane — inside tmux, in the current pane;
outside tmux with an interactive terminal, in a new two-pane tmux session (agent pane +
watcher pane) that gets attached automatically. Non-interactive invocations (pipes,
redirects) always pass through untouched, and a broken `tmath` on `PATH` produces one
warning line (`agent-allowed failed (exit N)`) and passes through rather than blocking
your agent.

## Diagnose

```sh
tmath diagnose
```

Diagnostics report only allowlisted versions, capabilities, statuses, counts, and
stable error codes — never document text, environment contents, or local paths. Inside
tmux it also prints the graphics-gate inputs: attached client count, the outer
terminal's termname, the `allow-passthrough` value, the transport env, and the selected
route (or the full refusal reason).

Common results:

- `path launcher: broken (exit <code>)`: the `tmath` found on `PATH` cannot run. On
  macOS, `exit 137` (SIGKILL with no output) usually means the file was overwritten in
  place — for example by copying a freshly built binary over `~/.local/bin/tmath` —
  which poisons the kernel's code-signature cache for that inode. Never `cp` a binary
  over the launcher; re-run `scripts/install.sh`, which replaces it atomically at a
  fresh inode.
- `path launcher: version skew (...)`: the `tmath` on `PATH` is a different version
  than the binary you are running; re-run `scripts/install.sh`.
- `tmux graphics route: unavailable (...)`: the outer-terminal gate refused; the
  message names the cause (no attached client, unverified termname, or
  `allow-passthrough` off) and the override.
- `stdout: not a terminal`: image transport needs a real terminal (piping output only
  prints a text summary).
- `kitty graphics: unsupported`: the attached terminal does not support the Kitty
  graphics protocol.
- `renderer subprocess: unavailable`: only relevant for `--engine node`; the native
  default does not need it.

## Build from a checkout (development)

```sh
cargo build            # tmath binary in target/debug/tmath
cargo test --workspace
npm ci && npm run build   # deprecated node engine + TS tooling
```

## Known limits

- Only `$...$`, `$$...$$`, `\(...\)`, and `\[...\]` math delimiters are parsed, and only
  the allowlisted Markdown subset is rendered by a local parser. Raw HTML, images,
  scripts, custom CSS, and color directives are not supported.
- Formulas in code spans, fenced code, prices, shell variables, and ambiguous delimiter
  runs are rejected.
- Strict formula count, source length, image dimension, byte, placement, and time
  limits apply; on any failure, earlier placements remain intact.
- Document text and LaTeX source are never written to durable state or logs.
- macOS arm64 with Ghostty is the only verified terminal combination; kitty and WezTerm
  are expected but unverified; Linux and Windows are not yet supported.

See [Compatibility](compatibility.md), [Architecture](architecture.md), and the official
[Kitty graphics protocol](https://sw.kovidgoyal.net/kitty/graphics-protocol/) for more
detail.
