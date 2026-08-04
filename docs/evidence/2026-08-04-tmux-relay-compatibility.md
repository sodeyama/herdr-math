# Evidence: Kitty Graphics Through tmux Behind a Terminal-Session Relay

- Date: August 4, 2026
- Chain under test: cmux (Ghostty-based) ← session relay daemon (pty owner,
  parent pid 1) ← tmux 3.5a ← tmath
- Method: controlled byte probes emitted into live panes, plus recorded
  emission dumps from `tmath render` (pty harness with pixel winsize),
  verified visually (screenshots reviewed by the supervising agent).

## Findings

| Probe | Result |
|---|---|
| Raw APC direct placement, tmux passthrough (DCS) | displays |
| Raw APC direct placement, written to client tty | displays |
| RGBA + zlib (`f=32,o=z`), single chunk, virtual placement | displays |
| Raw RGB, 5-chunk `m=1` continuation, virtual placement | displays |
| Virtual placement + Unicode placeholders, indexed fg id (1, 16, 43, 44) | displays |
| Virtual placement + Unicode placeholders, 24-bit RGB fg id | **blank** |

Root cause of the blank case: the session relay re-renders cells and does not
preserve 24-bit foreground colors, destroying the placeholder-to-image id
association. Fixed in commit `7d0542e` by encoding ids ≤ 255 with the
256-indexed foreground form (both placeholder emitters; a duplicate emitter
existed in `placement.rs` in addition to `kitty.rs`).

Additional findings fixed the same day:

- `tmath` refused the tmux route because neither the advertised termname
  (`xterm-256color`) nor the client-tty owner ancestry can reach the real
  terminal when a relay daemon (parent pid 1) owns the pty. An explicit
  `TMATH_TMUX_TRANSPORT` value now asserts the outer terminal (`6c167be`);
  the default remains fail-closed.
- Stream-mode probe replies were read from a fresh `/dev/tty` descriptor,
  whose readiness surfaces as `POLLPRI` on macOS and was missed by the
  `POLLIN` poll; replies now use the cloned stdout descriptor (`5b7e1c9`).

## Verified behavior after the fixes

- `tmath render --engine native` inside a tmux pane in this chain displays
  correctly typeset output (heading + inline math confirmed on screen).
- The V2 `tmath agent` watcher pipeline runs end to end (boundary detection,
  render, transmit, placeholder grid), but the legacy composite-image viewer
  displays its image compressed into the top placeholder rows in this chain.
  The V3 Phase 3 viewer replaces that composite path with the per-block
  placement path shown to render correctly, so the legacy defect is recorded
  here rather than fixed in place.

These results also answer part of the long-open V2 items AT-2-806/810/811
for this specific outer-terminal chain.
