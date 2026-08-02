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

Use `tmath` when the reader will benefit from **typeset math or a clean
Markdown rendering** instead of raw LaTeX or prose in the conversation, for
example:

- A derivation with `$...$` or `$$...$$` formulas.
- A short Markdown document (headings, lists, code) worth visual formatting.

Do not use it for trivial single-line text; plain output is fine then.

## Commands

```sh
# Render a file (places one scrollback-anchored image per block)
tmath render ./notes.md

# Render a document from stdin (pipe your markdown/latex text)
printf 'By Fubini, $\\int_0^1 \\int_0^1 x y\\,dx\\,dy = 1/4$.\n' | tmath render -

# Compose the image width / font size
tmath render --content-width 800 --font-size 18 ./notes.md

# Show the finished answers of a coding agent in a viewer pane (run inside tmux)
tmath agent --source-pane %0
```

`tmath diagnose` verifies the renderer, node, terminal, and Kitty support.
`tmath --help` lists every command.

## Requirements

- A terminal that supports the Kitty graphics protocol (Ghostty is the verified
  primary; kitty and WezTerm are P1). `tmath diagnose` reports whether the
  current terminal supports it.
- Node.js 22+ and the installed renderer (handled by `scripts/install.sh`; the
  binary locates the renderer automatically after install).
- `tmath agent` additionally requires tmux (3.2+) with
  `tmux set-option -w allow-passthrough on`.

## Behavior and privacy

- Formulas and Markdown render locally with KaTeX and a local browser; remote
  resources, scripts, raw HTML, and user CSS are never loaded or executed.
- Document text, formulas, and rendered bytes are never written to logs,
  durable state, or any network service.
- `tmath agent` prints only event names, pane ids, counts, and byte sizes to
  its logs (never answer content), and stores nothing beyond a temp-dir socket
  that is removed on exit.

## When to stop

If the terminal does not support Kitty graphics, `tmath` will say so clearly
(`tmath diagnose`) and fail closed; fall back to inline text in that case.
