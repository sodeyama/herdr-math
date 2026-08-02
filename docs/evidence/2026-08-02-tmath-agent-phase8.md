# Terminal Math Agent Integration Evidence

Date: 2026-08-02
Scope: `tmath agent` tmux viewer feature (Phase 8 addition).

## Result

- **Automated pipeline (headless)**: PASS. The watcher detects a finished agent
  answer, proves the boundary, and emits the answer document; the viewer
  connects, receives it, and (in a Kitty-capable setup) places the image.
- **Placement inside a Ghostty-attached tmux pane**: PASS at the pipeline
  level — the auto-viewer connected and placed a real image in the pane
  (`placed image=1 rows=10 bytes=4980`), with the placeholder grid visible in
  the pane. The transmit is a fire-and-forget Kitty sequence carried through
  tmux passthrough (`ESC Ptmux; ... ESC \`), which tmux cannot reply to but
  does forward.
- **Direct Ghostty placement**: PASS (`placed width=360 height=100
  image_id=1`).

## Investigation and root causes

Three distinct issues blocked the tmux path and were fixed:

1. **Passthrough envelope**: tmux only forwards a DCS opened with its private
   prefix (`ESC Ptmux; ... ESC \`); a generic `ESC P` or a bare Kitty APC is
   consumed by tmux. `kitty::dcs_wrap` now uses `\ePtmux;`.
2. **Queries cannot round-trip through tmux**: the `a=q` graphics probe and
   `CSI 16t` cell-size query (tmux answers with character counts, not pixels)
   cannot be answered reliably inside tmux. The viewer therefore skips the
   probe when `$TMUX` is set (optimistic passthrough, with a stderr warning)
   and derives the cell size from winsize, which tmux reports in real pixels
   (`2044x1335` for a `292x89` pane -> 7x15 px cells).
3. **Viewer inherited the wrong environment**: `tmux split-window` starts the
   viewer with the server environment, so `TMATH_RENDER_WORKER` was lost and
   the viewer exited before connecting. The watcher now passes the worker path
   explicitly on the viewer command line.

## Commands used

```sh
tmux set-option -t <session> -w allow-passthrough on
env TMATH_RENDER_WORKER=$PWD/dist/renderer/subprocess.js \
  $PWD/target/debug/tmath agent --source-pane %NN
```

Observed watcher log (redacted: pane ids and byte counts only):

```text
tmath agent: watching %64 → %68; q/Ctrl-C to stop
tmath agent: document_sent bytes=74
tmath agent: document_sent bytes=146
```

Observed auto-viewer pane (placeholder grid plus placement line):

```text
<placeholder grid glyphs>
agent-viewer: placed image=1 rows=10 bytes=4980
```

## Privacy

Logs contain only event names, pane ids, counts, and byte sizes; never answer
or formula text. Sockets live under the platform temp directory and are
removed on watcher exit.

## Remaining caveats

- tmux cannot deliver query replies, so the viewer operates optimistically
  inside tmux: `allow-passthrough on` and a Kitty-graphics-capable outer
  terminal are required; otherwise nothing is displayed (no crash).
- Visual confirmation of the PNG in the user's Ghostty window is a manual
  eyeball check; the placement and transmit succeeded without error.
