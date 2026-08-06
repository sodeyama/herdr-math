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
| Rust | recent stable |

A release claim is made only after the release-gate tasks complete: clean build, unit,
integration, rendering, security, and real Ghostty runtime tests.

## Required runtime capability

- A Kitty-graphics-capable terminal; `tmath diagnose` probes support and `tmath render` exits
  non-zero with a clear message when it is missing.
- A real terminal for stdout to receive the image; piped stdout prints a bounded text summary
  instead.

## Build-verified (not runtime-verified)

GitHub Actions runs a **Linux x86_64** job that builds the release binary, passes the
60 MiB footprint gate, and completes a pipe render smoke (`scripts/smoke-render-pipe.sh`).
See [Linux build smoke evidence](evidence/2026-08-06-v3-linux-build-smoke.md). This does
**not** make Linux a supported platform for end users until real Kitty-graphics terminal
evidence is recorded.

## Unverified and unsupported claims

The following combinations are not verified and must not be described as supported:

- macOS x64;
- Linux and Windows for interactive terminal use (Linux **build** smoke only; see
  [build-verified](#build-verified-not-runtime-verified));
- Kitty, WezTerm, and other outer terminals (P1);
- remote sessions or non-graphics transports.

Newer compatible versions may work, but they remain expected rather than verified until the
release matrix is repeated. A future compatibility claim must include clean installation,
rendering, placement, scrollback scroll, mouse and keyboard scrolling, failure preservation, and
clean-exit evidence.

## Agent integration

`tmath agent` / `tmath agent-viewer` run inside tmux and require a
Kitty-capable outer terminal plus a valid graphics route. As of 0.3.0 the
default route is tmux passthrough (requires `allow-passthrough on`; serialized
against other pane output), with client-tty available as an explicit
`TMATH_TMUX_TRANSPORT=client-tty` opt-in for terminals whose passthrough relay
is broken. Live evidence: Ghostty + tmux 3.5a renders and scrolls streamed
answers over passthrough (2026-08-05 scroll-lab session, runtime-reliability-v1
close-out); earlier controlled pixels were also observed via client-tty on
Ghostty 1.3.1 and cmux 0.64.12. Resize, detach/attach, multiple clients, and
the complete live-agent matrix remain unverified.

## Evidence

- [Linux build smoke (CI, 2026-08-06)](evidence/2026-08-06-v3-linux-build-smoke.md)
- [Phase 0 terminal surface](evidence/2026-08-02-tmath-v2-phase0.md)
- [Phase 1 render transport](evidence/2026-08-02-tmath-v2-phase1.md)
- [Phase 2 placement](evidence/2026-08-02-tmath-v2-phase2.md)
- [Phase 3 input loop](evidence/2026-08-02-tmath-v2-phase3.md)
- [Phase 4 CLI and composition](evidence/2026-08-02-tmath-v2-phase4.md)
- [Phase 5 hardening](evidence/2026-08-02-tmath-v2-phase5.md)
- [tmux graphics and agent viewer](evidence/2026-08-03-tmath-tmux-graphics.md)
