# Terminal Math with Coding Agents

`tmath` is designed to sit beside a coding agent so that **messages and
equations come out typeset in a separate viewer pane** instead of raw LaTeX in
the shell. It is agent-agnostic: the watcher reads the pane's output, so the
same command works for Claude Code, Codex, opencode, Cursor Agent, and pi.

After [one `tmath` install](getting-started.md#install), every agent also gets
the `tmath` skill linked into its skills directory, so you can ask an agent to
"show this as math" and it will pipe the Markdown/LaTeX through `tmath render`.
The installer also offers an opt-in shell integration
([Auto-watch](#auto-watch-opt-in-per-directory) below) that starts the
watcher automatically for allowlisted directories, so you never have to find
a pane id by hand.

## Setup once (tmux)

```sh
# Optional: only needed when forcing the DCS passthrough route
tmux set-option -t <window> -w allow-passthrough on   # requires 3.3+
which tmath && tmath diagnose                         # verify install
```

Run a coding agent in **pane A**, then from any other pane start the watcher
pointing at pane A:

```sh
# find pane A's id
tmux list-panes -F '#{pane_id}  #{(pane_active ? "*" : "")} #{pane_current_command}'

# watch pane A (example: %3)
tmath agent --source-pane %3
```

`tmath agent` opens a right-hand viewer pane, updates it with each finished
answer (prose + typeset `$...$`/`$$...$$` math), logs only bounded status to
stderr, and stops on `q`/Ctrl-C. Scroll the viewer with the wheel or arrow
keys; `q`/Ctrl-C closes it.

## Auto-watch (opt-in, per directory)

Instead of finding a pane id and running `tmath agent` by hand, install the
shell integration once and let it start the watcher for you:

```sh
tmath agent-enable            # allowlist the current directory (+ subdirectories)
tmath agent-disable           # remove it again
tmath agent-allowed            # check the current directory (silent, exit code only)
```

`scripts/install.sh` installs `$APP/shell/tmath-agent.sh` and sources it from
`~/.zshrc`/`~/.bashrc` via a marker-delimited block (`TMATH_SKIP_SHELL_INTEGRATION=1`
skips this). It wraps `claude`, `codex`, `opencode`, `cursor-agent`, and `pi`:

- **Not allowlisted, or `tmath` missing**: the real command runs unmodified;
  `tmath` never runs.
- **Allowlisted, inside tmux**: a `tmath agent --source-pane <this pane>`
  watcher starts in the background, then the real command runs in the
  foreground of the same pane. A pane-scoped lock file prevents starting a
  second watcher for a pane that already has one; a lock left behind by a
  watcher that died is reclaimed automatically.
- **Allowlisted, outside tmux, interactive terminal**: a new tmux session is
  created with the agent command in one pane and the watcher in a second pane
  (an explicit two-pane session, since a plain `tmux new-session <cmd>` never
  sources shell rc files), and the session is attached.
- **Allowlisted, outside tmux, non-interactive** (piped or redirected):
  passes through untouched, same as not being allowlisted.

The allowlist lives at `${XDG_CONFIG_HOME:-~/.config}/tmath/agent-allowlist`
(one canonicalized absolute path per line) and starts empty after install, so
nothing changes until you run `tmath agent-enable`.

## Per-agent notes

All agents appear the same to the watcher (a pane + text). The differences are
the prompt glyph the watcher recognizes and known boundary limitations.

| Agent | Command | Prompt glyph | Boundary support | Notes |
|---|---|---|---|---|
| Claude Code | `claude` | `❯` | Yes | Verified prompt glyph; matches the corpus fixture. |
| Codex | `codex` | `›` | Yes | Working frames (`• Working …`) are not treated as answers. |
| opencode | `opencode` | `┃ prompt:` | Yes | `┃ answer:` lines are kept as content; only `┃ prompt:` acts as the boundary. |
| Cursor Agent | `cursor-agent` | `>` | Yes | Tool-activity prefixes are removed before the final answer is rendered. |
| pi | `pi` | `Current prompt > …` (inline) | Yes | Repeated contextual prompt anchors recover answers after full-screen repaint or capture truncation. |

General guidance:

- **Streaming answers**: `--wait-ms <ms>` (default 600) controls how long text
  must settle before it is emitted; lower it (e.g. `--wait-ms 200`) for more
  aggressive updates, raise it if answers arrive in parts.
- **Long answers**: `--history <lines>` (default 500) captures scrollback so an
  answer taller than one screen is not lost.
- **Boundary confusion** (big repaint, pane cleared, resize): the watcher
  fails closed and logs `boundary_failed`; it re-anchors on the next stable
  answer rather than rendering a broken split.
- **Terminals**: inside tmux, queries cannot round-trip reliably. The default
  route sends only Kitty graphics commands to the attached client tty while
  cursor movement and placeholder cells stay in tmux. This works around DCS
  relay differences in Ghostty and cmux. Set
  `TMATH_TMUX_TRANSPORT=passthrough` to use individually wrapped, ESC-doubled
  DCS commands instead. Outside tmux, `tmath render` probes normally and fails
  closed when Kitty is missing.
- **Blurry math on a Retina display, inside tmux**: also because queries
  cannot round-trip inside tmux, the viewer falls back to the terminal's
  reported window size to estimate the pixel density (device pixel ratio).
  On some terminal/tmux combinations that fallback itself reports logical
  pixels rather than physical ones, so the viewer under-estimates the ratio
  and the terminal upscales the rendered image, producing soft, crushed
  glyphs. If math looks blurry only inside a tmux-hosted `tmath agent-viewer`
  pane (not in a directly-run `tmath render`), set `TMATH_DPR` to your
  display's actual scale factor — `TMATH_DPR=2` for a standard Retina
  display, `TMATH_DPR=3` for some higher-density laptop panels. `tmath agent`
  forwards the variable to the viewer pane it spawns automatically, so
  setting it once in the shell you run `tmath agent` from is enough. Values
  outside `1`-`4`, or set outside tmux, are ignored and the automatic
  estimate is used instead.
- **Captured tool stdout**: a coding agent's shell tool may capture stdout, so
  an agent-launched `tmath render -` is not guaranteed to reach the visible
  terminal. The watcher + viewer pane is the standard agent workflow.
- **Configuring font size**: `tmath` reads `config.toml` from the platform
  config directory (`$XDG_CONFIG_HOME/tmath/config.toml`, or
  `$HOME/.config/tmath/config.toml` when `XDG_CONFIG_HOME` is unset) if
  present. It currently has one key:
  ```toml
  font_size_pt = 15.0
  ```
  `font_size_pt` is a number (integer or float) in `[10, 24]`; a value
  outside that range, or of the wrong type, is ignored with a warning and
  the setting falls back to the next precedence level. Unrecognized keys
  are ignored with a warning naming the key. The full precedence order,
  highest first: the `--font-size` CLI flag (`tmath render`/`tmath watch`)
  > the `TMATH_FONT_SIZE_PT` environment variable > `config.toml`'s
  `font_size_pt` > the terminal auto-fit calculation > the fixed default.
  `tmath agent-viewer` has no CLI flag of its own (it is spawned by `tmath
  agent`), so its precedence is env > config > auto-fit > default; it reads
  the config file itself at startup and logs the resolved source
  (`agent-viewer: font_size source=<cli|env|config|auto-fit|default>
  value=<pt>`) — never any other config content. A missing config file is
  silent (not a warning); the file only ever holds small numeric settings,
  never document content.

## Let an agent show math to you

Because the `tmath` skill is installed for each agent, you can simply ask:

```text
Show that derivation as typeset math with tmath.
```

The agent will pipe the Markdown/LaTeX through `tmath render -`, placing a
readable image next to the conversation. If the terminal lacks Kitty support
(`tmath diagnose`), the agent falls back to inline text.

## Troubleshooting

- `tmath: renderer subprocess not found` — re-run the installer, or set
  `TMATH_RENDER_WORKER=/abs/path/dist/renderer/subprocess.js`.
- Viewer pane opens then closes — run `tmath diagnose` inside the source tmux
  session and confirm that a visible client tty and a known Kitty terminal are
  reported. For forced DCS passthrough, also confirm `allow-passthrough on`.
- Nothing updates — the agent repainted the whole pane (boundary reset); wait
  for the next finished answer, and check the watcher stderr for
  `boundary_failed`.
- `kitty graphics: unsupported` in `tmath diagnose` — the current terminal does
  not support the Kitty graphics protocol.
- Auto-watch never starts — check `tmath agent-allowed` in the directory you
  expect it to fire from (silent; exit `0` means allowlisted, exit `1` means
  not). If it is allowlisted and still nothing happens, confirm the shell rc
  file was updated (`grep -A2 'tmath shell integration' ~/.zshrc`) and that
  you started a new shell after installing.
