# Terminal Math with Coding Agents

`tmath` is designed to sit beside a coding agent so that **messages and
equations come out typeset in a separate viewer pane** instead of raw LaTeX in
the shell. It is agent-agnostic: the watcher reads the pane's output, so the
same command works for Claude Code, Codex, opencode, Cursor Agent, and pi.

After [one `tmath` install](getting-started.md#install), every agent also gets
the `tmath` skill linked into its skills directory, so you can ask an agent to
"show this as math" and it will pipe the Markdown/LaTeX through `tmath render`.

## Setup once (tmux)

```sh
# inside tmux (tmath enables passthrough automatically; manual is optional)
tmux set-option -t <window> -w allow-passthrough on   # requires 3.2+
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

## Per-agent notes

All agents appear the same to the watcher (a pane + text). The differences are
the prompt glyph the watcher recognizes and known boundary limitations.

| Agent | Command | Prompt glyph | Boundary support | Notes |
|---|---|---|---|---|
| Claude Code | `claude` | `❯` | Yes | Verified prompt glyph; matches the corpus fixture. |
| Codex | `codex` | `›` | Yes | Working frames (`• Working …`) are not treated as answers. |
| opencode | `opencode` | `┃ prompt:` | Yes | `┃ answer:` lines are kept as content; only `┃ prompt:` acts as the boundary. |
| Cursor Agent | `cursor-agent` | `>` | Partial | Plain-text prompts are recognized when they start a line; aggressive in-progress repaints can reset the boundary. |
| pi | `pi` | `Current prompt > …` (inline) | Not yet | Prompts are inline plain text; answer-boundary detection for pi is a P1 item (spec T-905). |

General guidance:

- **Streaming answers**: `--wait-ms <ms>` (default 600) controls how long text
  must settle before it is emitted; lower it (e.g. `--wait-ms 200`) for more
  aggressive updates, raise it if answers arrive in parts.
- **Long answers**: `--history <lines>` (default 500) captures scrollback so an
  answer taller than one screen is not lost.
- **Boundary confusion** (big repaint, pane cleared, resize): the watcher
  fails closed and logs `boundary_failed`; it re-anchors on the next stable
  answer rather than rendering a broken split.
- **Terminals**: inside tmux, queries cannot round-trip, so the viewer uses
  optimistic passthrough; it requires `allow-passthrough on` and a
  Kitty-graphics outer terminal. Outside tmux, `tmath render` probes normally
  and fails closed when Kitty is missing.

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
- Viewer pane opens then closes — the outer terminal did not relay the image:
  confirm `allow-passthrough on` on the tmux window and run `tmath diagnose`.
- Nothing updates — the agent repainted the whole pane (boundary reset); wait
  for the next finished answer, and check the watcher stderr for
  `boundary_failed`.
- `kitty graphics: unsupported` in `tmath diagnose` — the current terminal does
  not support the Kitty graphics protocol.
