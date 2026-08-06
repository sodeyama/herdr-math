---
name: demo-gif-record
description: >
  Record a sanitized public demo GIF of tmath agent in Ghostty: isolated tmux
  session, synthetic Q&A, scroll capture, and privacy-checked output under
  docs/media/. Use when the user asks for a demo GIF, README media, Claude Code
  demo recording, or to re-shoot claude-code-demo.gif (keywords: demo gif,
  デモ gif, 録画, screencapture, Ghostty demo).
---

# Demo GIF recording (tmath agent)

Record `docs/media/claude-code-demo.gif`: a Ghostty window showing a coding-agent
pane plus the `tmath agent` viewer, streaming synthetic math/Markdown, scrolling
through ~two pages of content, with no real transcripts or personal data.

## Confirmed recipe (default)

Use the script defaults — do not change unless the user asks:

| Setting | Value |
|---------|-------|
| Ghostty size | **140×40** cells |
| Terminal font (`GHOSTTY_FONT_SIZE`) | **12** pt |
| tmath render (`TMATH_FONT_SIZE_PT`) | **16** pt |
| Output GIF width | **720** px |
| `TMATH_DPR` | **2** (applied only when winsize reports logical pixels) |

```sh
cargo build --release -p tmath
bash scripts/record-claude-demo-gif.sh
```

Takes ~60s (50s capture + setup). Output: `docs/media/claude-code-demo.gif`.

## Prerequisites

- macOS with screen-recording permission for the terminal (`screencapture -l`)
- Ghostty, tmux 3.3+, ffmpeg, python3 (PyObjC/Quartz for window lookup)
- Release `tmath` built from this repo (the script builds if missing)

## Terminal font vs tmath font

These are **independent**:

- **`GHOSTTY_FONT_SIZE`** — shell prompt and raw agent text in the left pane.
- **`TMATH_FONT_SIZE_PT`** — typeset math/Markdown in the viewer pane.

Setting Ghostty `font-size` alone does **not** change viewer typography. The
script sets `TMATH_FONT_SIZE_PT` on `tmath agent`; `agent_watcher` **must**
forward it to `agent-viewer` (tmux split panes do not inherit the watcher's env).

## Why viewer text was tiny (fixed)

Two bugs caused unreadably small viewer text during early demo runs:

1. **`TMATH_FONT_SIZE_PT` not forwarded** to the viewer pane → auto-fit (~12 pt)
   won instead of the requested 16 pt.
2. **`TMATH_DPR=2` double-scaled** the cell when winsize already reported physical
   pixels → placements shrank. Fix: skip the DPR override when auto-detected DPR
   is already > 1 (`layout::resolve_dpr_override`).

Always rebuild `tmath` after changing either path before re-recording.

## Configuration overrides

| Variable | Default | Meaning |
|----------|---------|---------|
| `GHOSTTY_COLS` | `140` | Ghostty/tmux width in **cells** |
| `GHOSTTY_ROWS` | `40` | Ghostty/tmux height in **cells** |
| `GHOSTTY_FONT_SIZE` | `12` | Ghostty UI monospace font (pt) |
| `TMATH_FONT_SIZE_PT` | `16` | tmath **rendered** font size (pt) |
| `GIF_WIDTH` | `720` | Output GIF width in pixels (height scales) |
| `TMATH_BIN` | `target/release/tmath` | Binary copied into the isolated demo |

## What the script does

1. Starts an **isolated tmux server** (`tmux -L tmath-demo-$$`) under `/tmp`.
2. Opens a dedicated Ghostty window (140×40 by default) attached to that session.
3. Runs `tmath agent` in the background with `TMATH_DPR=2` and `TMATH_FONT_SIZE_PT`.
4. Drives synthetic demo content (`demo-stream-answer.py long`) and viewer scroll.
5. Captures the Ghostty window at 8 fps, crops the title bar, encodes a GIF.
6. **Privacy gate**: fails if sample frames contain home paths or error strings.

Scratch artifacts live under `/tmp/tmath-public-demo-$$` and are removed on exit.

## Supporting scripts

| Script | Role |
|--------|------|
| `scripts/record-claude-demo-gif.sh` | Full automated record pipeline |
| `scripts/demo-stream-answer.py` | Synthetic streaming answers |
| `scripts/demo-drive-scroll.sh` | Manual scroll injection |

## Agent workflow

When asked to record or re-shoot the demo:

1. `cargo build --release -p tmath`
2. Run `bash scripts/record-claude-demo-gif.sh` with `block_until_ms` ≥ 120000.
3. Read `docs/media/claude-code-demo.gif` to verify content and scroll.
4. Do **not** commit unless the user asks.

## Troubleshooting

| Symptom | Check |
|---------|-------|
| Viewer text too small | Rebuild `tmath`; confirm status bar shows `16pt`; see "Why viewer text was tiny" |
| `Ghostty demo window not found` | Ghostty installed; screen-recording permission |
| `boundary_failed during demo` | Shorten demo content or fix viewer limits |
| Recording interrupted | Re-run; `/tmp` scratch is cleaned on exit |

## Cautions

- Never capture the user's real tmux session or Claude transcripts.
- Do not commit intermediate PNG frames or `/tmp` scratch output.
