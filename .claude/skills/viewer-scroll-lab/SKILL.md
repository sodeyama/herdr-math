---
name: viewer-scroll-lab
description: >
  Self-observation loop for the tmath agent viewer: feed synthetic content,
  inject scroll/key input into the viewer pane via tmux, and verify the real
  rendered pixels by taking screenshots the agent can read back — so
  render/placement/scroll fixes can be verified autonomously without asking
  the human to look at the screen each round. Trigger when iterating on
  viewer rendering, placement, scrolling, follow mode, or the status bar and
  a real-terminal check is needed (keywords: scroll lab, self observe,
  自己観測, スクロール検証, 実機観測, viewer 検証).
---

# Viewer scroll lab (self-observation loop)

A three-part harness that lets the agent drive and observe the real
`tmath agent` viewer running in the user's Ghostty + tmux session, closing
the verify loop without human eyes.

## Prerequisites

- The session runs inside tmux with Ghostty as the outer terminal.
- A release build exists (`cargo build --release -p tmath`).
- Screen-recording permission for the terminal app (screencapture).
- tmux options for the lab: `allow-passthrough on` (graphics route) and
  `mouse on` if a human will also scroll. Record prior values and restore
  them when the lab session ends.

## Scripts

All scripts live in `scripts/` next to this file. Shot output goes to a
temp/scratch directory, never into the repository.

1. `feed.py init | blocks N [delay] | clean` — creates a synthetic
   transcript `zz-scrolllab.jsonl` inside this project's Claude transcript
   directory (derived from `$HOME` + the cwd slug) and appends synthetic
   assistant messages (paragraphs, display math, bold intros, inline code)
   so the watcher streams controllable content into the viewer. `clean`
   removes the file. Never reads or edits real transcripts.
2. `drive.sh <pane> up|down N [interval_ms]` and `drive.sh <pane> end` —
   injects SGR wheel escapes / the End key directly into the viewer pane's
   stdin via `tmux send-keys -H`, timestamping each injection (latency
   measurements diff these against viewer log lines).
3. `observe.sh <label> [outdir] [x y w h]` — captures the outer terminal
   WINDOW by CGWindowID (`screencapture -l`, occlusion-immune — a full-screen
   shot silently photographs whatever app is frontmost instead) to
   `shot-<label>.png`, optionally cropping to window-relative pixels via
   `crop.py` for glyph-level zooms. `OBSERVE_APP` overrides the window-owner
   match (default `ghostty`). The agent then Reads the PNG to inspect the
   actual rendered pixels.

## Standard cycle

```sh
SKILL=scripts   # this skill's scripts dir
python3 $SKILL/feed.py init
# restart `tmath agent --source-pane <conversation-pane>` AFTER init so the
# watcher's newest-transcript selection picks the lab file; include
# TMATH_VIEWER_LOG=1 when event/offset logs are needed (they print into the
# viewer pane) and tee the agent log for timing.
python3 $SKILL/feed.py blocks 40 0.15      # content taller than the pane
$SKILL/observe.sh baseline <outdir>        # screenshot -> Read it
$SKILL/drive.sh <viewer-pane> up 10 50     # inject scroll
$SKILL/observe.sh after-scroll <outdir>    # screenshot -> compare
```

Interpretation aids:

- `tmux capture-pane -p -t <viewer-pane>` shows the text layer (status-bar
  text, log lines, placeholder rows) — image content itself only appears in
  screenshots.
- Latency: diff `drive.sh` timestamps against `TMATH_VIEWER_LOG=1` event
  lines.
- Always `feed.py clean` and restore tmux options when the lab ends; kill
  the lab agent pane (`q` in the viewer, then kill the split panes).

## Cautions

- Screenshots capture the whole screen: treat them as local-only artifacts
  (scratch dir), never commit them (repository privacy rules).
- The synthetic transcript makes the watcher ignore the real conversation
  transcript for the lab session; restart the agent without the lab file to
  return to normal watching.
- Injected wheel events go to the viewer regardless of tmux `mouse`; the
  `mouse on` option only matters for a human's physical wheel.
