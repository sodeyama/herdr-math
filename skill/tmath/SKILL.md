---
name: tmath
description: >-
  Render Markdown and LaTeX as scrollable images in the terminal, and watch a
  coding agent's answers so math and messages show typeset in a viewer pane.
---

# Terminal Math (tmath)

`tmath` renders `$...$` / `$$...$$` equations and a strict allowlisted Markdown
subset (headings, emphasis, lists, quotes, tables, code blocks, inert links) as
transparent images placed into the terminal with the Kitty graphics protocol.
It runs fully locally in any Kitty-graphics-capable terminal (Ghostty, kitty,
WezTerm). No network, no TeX engine, no plugin runtime.

## When to use this skill

Use `tmath agent` when the reader will benefit from **typeset math or a clean
Markdown rendering** in the side viewer pane instead of raw LaTeX in the agent
shell, for example:

- A derivation with `$...$` or `$$...$$` formulas.
- A short Markdown document (headings, lists, code) worth visual formatting.

**Never run `tmath render` from a coding-agent shell tool** (Claude Code, Cursor,
Codex, and similar). Those UIs capture stdout as plain text, so Kitty graphics
payloads appear as unreadable base64 and corrupt the chat pane. Keep using plain
text in the agent shell; let the already-running `tmath agent` viewer typeset
finished answers automatically.

Do not use this skill for trivial single-line text; plain output is fine then.

## Commands

```sh
# Recommended for coding agents: watch their tmux pane and open a viewer
tmath agent --source-pane %0

# Manual one-off render in a normal terminal (not from a coding-agent shell tool)
tmath render ./notes.md
tmath render --content-width 800 --font-size 18 ./notes.md
```

`tmath diagnose` verifies the terminal and Kitty support.
`tmath --help` lists every command.

## Requirements

- A terminal that supports the Kitty graphics protocol (Ghostty is the verified
  primary; kitty and WezTerm are P1). `tmath diagnose` reports whether the
  current terminal supports it.
- `tmath agent` requires tmux 3.2+ and opens a separate viewer pane. Graphics
  use the attached tmux client tty by default, while placeholder cells remain
  in tmux for clipping and redraw. Set
  `TMATH_TMUX_TRANSPORT=passthrough` to force the standards-based DCS route
  (which requires tmux 3.3+ and `tmux set-option -w allow-passthrough on`).

## Behavior and privacy

- Formulas and Markdown render locally in-process (RaTeX + Typst, embedded fonts); remote
  resources, scripts, raw HTML, and user CSS are never loaded or executed.
- Document text, formulas, and rendered bytes are never written to logs,
  durable state, or any network service.
- `tmath agent` prints only event names, pane ids, counts, and byte sizes to
  its logs (never answer content), and stores nothing beyond a temp-dir socket
  that is removed on exit.
- Coding-agent shell tools capture stdout instead of exposing a real terminal.
  **Do not run `tmath render` from those tools.** Start `tmath agent` in another
  tmux pane and let the viewer typeset answers; keep plain text/LaTeX in the
  agent shell.

## When to stop

If the terminal does not support Kitty graphics, `tmath` will say so clearly
(`tmath diagnose`) and fail closed; fall back to inline text in that case.
