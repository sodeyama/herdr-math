# tmux Graphics and Agent Viewer Evidence

Date: August 3, 2026

## Environment

- macOS arm64
- tmux 3.5a
- Ghostty 1.3.1
- cmux 0.64.12
- `tmath` 0.2.0 development build

Controlled fixtures contained only product test text and simple equations. The
temporary window captures used for visual inspection were deleted immediately
and are not repository artifacts.

## Results

### Transport byte contract

PASS:

- Every Kitty APC upload chunk is an independent graphics operation.
- Stable tmux DCS wrapping doubles each embedded `ESC`.
- Cursor movement, terminal modes, placeholder cells, color CSI, and line
  breaks remain pane-local output.
- Delete and replacement commands use the same graphics route.

Evidence: deterministic Rust unit and integration tests in `kitty.rs`,
`placement.rs`, and `render_transport.rs`.

### Ghostty + tmux

PASS for controlled pixel display:

- The default validated client-tty route displayed
  `Ghostty tmux test: $E=mc^2$` as image pixels.
- The forced `TMATH_TMUX_TRANSPORT=passthrough` route displayed
  `Ghostty passthrough: $a^2+b^2=c^2$` as image pixels.
- No placeholder-glyph wall was visible in either controlled window.

Ghostty 1.3.1 retains its upstream wide Unicode-placeholder sizing limitation.
Resize and detach/attach remain outside this observation.

### cmux + tmux

PASS for controlled pixel display:

- A fresh cmux terminal attached to a fresh tmux session.
- The default client-tty route displayed
  `cmux tmux test: $E=mc^2$` as image pixels.
- Pane-local output remained owned by tmux, and no placeholder-glyph wall was
  visible.

The controlled tmux session and cmux workspace were removed after the test.
Resize and detach/attach remain outside this observation.

### Agent viewer

PASS for the controlled Ghostty + tmux workflow:

- The watcher detected a completed synthetic Claude-style answer and opened a
  separate viewer pane.
- The viewer displayed image pixels through the default graphics route.
- A 180-line answer produced a 96-row image; `PageDown` replaced it with a
  cropped lower viewport, proving that scrolling changes the visible content.
- Replacement clears stale placeholder cells before writing the new viewport.

The deterministic boundary corpus passes for Claude Code, Codex, Cursor CLI,
pi, and opencode. Live read-only response smokes produced the controlled
`TMATH_AGENT_OK $E=mc^2$` line with:

- Cursor Agent 2026.07.23-e383d2b
- pi 0.83.0
- opencode 1.18.10

Claude Code 2.1.220 stopped at its configured USD budget, and Codex CLI 0.146.0
reported an account usage-limit denial. Those are not runtime passes and were
not retried or bypassed. The completed live smokes validate agent availability
and response content, while the separate tmux fixture validates watcher
boundary-to-viewer delivery.

## Remaining runtime scope

The following still require separate evidence before a full compatibility
claim: pane resize, tmux detach/attach, multiple attached clients, nested tmux,
and live end-to-end watcher responses from every coding-agent version. Claude
Code and Codex additionally require available account budget/quota.
