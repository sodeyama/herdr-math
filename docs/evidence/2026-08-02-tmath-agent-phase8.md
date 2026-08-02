# Terminal Math Agent Integration Evidence

Date: 2026-08-02
Scope: `tmath agent` tmux viewer feature (Phase 8 addition).

## Result

- **Automated pipeline**: PASS. The watcher detects a finished agent answer,
  proves the boundary, and emits the answer document to the viewer channel;
  the viewer fails closed cleanly when the outer terminal does not relay Kitty
  graphics.
- **Renderer placement**: PASS in a direct Ghostty 1.3.1 terminal (transparent
  PNG placed and displayed, width=360 height=100).
- **Images inside a Ghostty-attached tmux pane**: NOT VERIFIED (see
  "Known limitations"). The Kitty `a=q` probe does not receive a reply when
  tunneled through tmux passthrough, so the viewer fails closed in that setup.

## Environment

- Platform: macOS on arm64
- tmux: 3.5a
- Ghostty: 1.3.1
- Node: 22.x; renderer: TS subprocess at `dist/renderer/subprocess.js`
- `tmath` built from `target/debug/tmath` at commit under test

## Automated pipeline smoke (headless)

Command: `scripts/smoke-agent-tmux.sh`

The script creates a detached tmux session (no Kitty-capable client), runs a
fake coding agent in one pane, runs `tmath agent` in a control pane, and
writes an answer with math to the agent pane.

Observed (redacted, counts only):

```text
tmath agent: watching %NN → %MM; q/Ctrl-C to stop
tmath agent: document_sent bytes=187        # answer detected + emitted
tmath agent: no viewer connected ...        # detached: viewer failed closed
PASS: watcher emitted the answer document and the viewer failed closed cleanly.
```

The detached session cannot display images (no Kitty-capable client), so the
viewer exits after a clean failure on its graphics probe. The watcher keeps
running and drops documents until a viewer reconnects.

## Direct Ghostty placement (renderer path shared with the viewer)

`tmath render --content-width 360 /tmp/doc.md` run in a direct Ghostty window
(effective PTY via `script`):

```text
kitty graphics: supported
<placeholder grid bytes shown, then>
placed width=360 height=100 image_id=1
```

This exercises the exact probe + placement + placeholder-grid path the viewer
uses, confirming the renderer is healthy in a Kitty-capable terminal.

## Known limitations

- **tmux image passthrough**: inside a tmux pane (Ghostty-attached), the
  `a=q` graphics probe reply is not relayed back on this setup, so the viewer
  reports `no Kitty graphics support` and fails closed. `tmath` now wraps
  emit bytes in the tmux DCS passthrough envelope (`ESC P ... ESC \\`) when
  `$TMUX` is set (see `kitty::wrapped_for_tty`), which is the documented way to
  carry Kitty sequences through tmux; resolving the reply relay for Ghostty is
  a P1 follow-up. Terminals whose tmux passthrough relays both directions
  (e.g. kitty) are expected to work but are not yet recorded here.
- The watcher requires the renderer worker via `TMATH_RENDER_WORKER`.
- Answer boundaries for prompt styles that are plain text with an inline
  marker (for example pi's `Current prompt > ...`) are not recognized yet;
  `❯`, `›`, and `┃ prompt:` are.
- A viewer pane is not recreated automatically after it is closed.

## Privacy

Logs above contain only event names and byte counts; no answer or formula
text. Socket files live under the platform temp directory.
