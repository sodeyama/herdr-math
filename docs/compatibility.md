# Compatibility

This document separates the verified target combinations from those that remain unverified or
unsupported. Nothing here claims a released build; release readiness is a separate gate.

## Verified target

The implementation is developed and validated toward this combination:

| Dimension | Value |
|---|---|
| Operating system | macOS (primary target) |
| Architecture | arm64 |
| Outer terminal | Ghostty 1.3.1 stable |
| Node.js | 22 or later |
| Rust | recent stable |

A release claim is made only after the release-gate tasks complete: clean build, unit,
integration, rendering, security, and real Ghostty runtime tests.

## Required runtime capability

- A Kitty-graphics-capable terminal; `tmath diagnose` probes support and `tmath render` exits
  non-zero with a clear message when it is missing.
- A real terminal for stdout to receive the image; piped stdout prints a bounded text summary
  instead.
- Node.js 22 or later for the render subprocess.

## Unverified and unsupported claims

The following combinations are not verified and must not be described as supported:

- macOS x64;
- Linux and Windows;
- Kitty, WezTerm, and other outer terminals (P1);
- remote sessions or non-graphics transports.

Newer compatible versions may work, but they remain expected rather than verified until the
release matrix is repeated. A future compatibility claim must include clean installation,
rendering, placement, scrollback scroll, mouse and keyboard scrolling, failure preservation, and
clean-exit evidence.

## Agent integration (P1, experimental)

`tmath agent` / `tmath agent-viewer` (Phase 8) run inside tmux 3.2+ and rely
on the tmux passthrough envelope (`allow-passthrough on`). Verified on a
direct Ghostty terminal (placements display correctly) and on a
Ghostty 1.3.1 + tmux 3.5a pane (a real placement was placed through
passthrough). tmux cannot relay query replies, so inside tmux probing is
optimistic and the cell size comes from winsize; the forwarded PNG's visual
appearance in the user's terminal is a manual eyeball step. kitty and WezTerm
tmux relay behavior is P1.

## Evidence

- [Phase 0 terminal surface](evidence/2026-08-02-tmath-v2-phase0.md)
- [Phase 1 render transport](evidence/2026-08-02-tmath-v2-phase1.md)
- [Phase 2 placement](evidence/2026-08-02-tmath-v2-phase2.md)
- [Phase 3 input loop](evidence/2026-08-02-tmath-v2-phase3.md)
- [Phase 4 CLI and composition](evidence/2026-08-02-tmath-v2-phase4.md)
- [Phase 5 hardening](evidence/2026-08-02-tmath-v2-phase5.md)
