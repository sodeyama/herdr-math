# Terminal-math V2 Phase 0 Evidence

Date: August 2, 2026

## Scope

This evidence covers the Phase 0 Rust terminal surface for the V2 standalone `tmath`
refactor, on branch `feat/tmath-v2-phase0` (worktree `herdr-math-v2-phase0`). Phase 0 ports
the terminal-facing pieces of `terminal-browser`'s `pixel-core` crate into a self-contained
Rust workspace that has no Herdr coupling. The TypeScript renderer is untouched in this phase.

## Environment

- Rust: 1.92.0 (`cargo 1.92.0`)
- macOS on arm64; Swift available at `/usr/bin/swift`
- Outer terminal: not consulted in Phase 0 (fake-tty harness only)

## Deliverables

- `Cargo.toml` — workspace with `edition = "2021"`, `resolver = "2"`,
  `[workspace.lints.rust] unsafe_code = "deny"`, member `engine/crates/tmath-core`.
- `engine/crates/tmath-core` — crate `tmath-core` v0.2.0 with modules:
  - `kitty.rs` — Kitty placed-transmit chunking, virtual/cursor placements,
    `placeholder_grid`, delete, and `a=q` medium probes.
  - `terminal.rs` — `Tty` trait, `StdioTty` (termios), raw mode, reporting-mode
    enable/reset (never the alternate screen), cell-size probe and winsize fallback,
    pixel-mouse `DECRQM ?1016` probe.
  - `mouse.rs` — SGR mouse decode, CSI key dispatch, cell-to-pixel conversion.
  - `scroll.rs` — `ScrollState` and `Smooth` easing profile.
  - `native.rs` — macOS helper spawn, line-protocol parsing (`s`/`z`/`m`/`w`/`scale`),
    subscriber model.
  - `native-scroll-helper.swift` + `build.rs` — compiled by `swiftc` only on macOS;
    `NATIVE_SCROLL_HELPER` points at the built helper.
- `.gitignore` — `target` output ignored.

## Validation

```sh
cargo test      # 37 passed, 0 failed
cargo clippy --all-targets   # clean, no warnings
cargo fmt --check            # OK
cargo build     # succeeds; swiftc produces native-scroll-helper
```

- `cargo build -vv` confirms `cargo:rustc-env=NATIVE_SCROLL_HELPER=<out>/native-scroll-helper`.
- Static scans of `engine/` found no `unsafe`, no Herdr identifiers, and no local absolute paths.
- `cargo metadata` and the lockfile resolve with no user-specific paths.

## Acceptance status

Phase 0 cases passed with unit evidence:

- AT-2-100 through AT-2-105 (Kitty escapes): transmit chunking, placement keys,
  placeholder encoding, delete, probes.
- AT-2-106 through AT-2-109 (terminal init/reset): modes without alternate screen,
  clean reset, cell-size probe + fallback, pixel-mouse probe.
- AT-2-110, AT-2-111 (mouse): SGR decode and cell-to-pixel.
- AT-2-112 (scroll): easing, clamping, brake, settle.
- AT-2-113, AT-2-114 (native): line protocol and macOS build.

Runtime and install evidence are deferred to Phase 2+ (real Ghostty placement) and the
release gate. No release claim is made from Phase 0 evidence alone.

## Commits

- `958b9f3` `chore(rust): scaffold terminal-math workspace` (T-101)
- `102d014` `feat(kitty): transmit placed scrollback-anchored images` (T-102)
- `764d8c6` `feat(terminal): initialize and reset the main-buffer terminal` (T-103)
- `aad464d` `feat(terminal): decode SGR and pixel mouse input` (T-104)
- `d21b7c7` `feat(scroll): animate document scrolling with smoothing` (T-105)
- `9a8a60f` `feat(native): add macOS trackpad scroll helper` (T-106)
