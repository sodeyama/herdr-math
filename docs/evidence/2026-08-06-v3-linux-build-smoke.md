# V3 Phase 5 — Linux x86_64 build and pipe render smoke

Date: 2026-08-06  
Task: T3-504 (partial — CI build smoke; real Kitty terminal evidence pending)  
Acceptance: AT-3-803 (build + fake-tty/pipe render only)

## Scope

This record covers automated **build and non-tty render smoke** on Linux x86_64 in
GitHub Actions. It does **not** claim Linux runtime support in user-facing
compatibility docs until a real Kitty-graphics terminal session is recorded.

## CI job

Workflow: `.github/workflows/ci.yml` → `linux-x86_64` on `ubuntu-latest`.

Commands:

```sh
cargo test --workspace
cargo build --release -p tmath
scripts/smoke-footprint.sh
scripts/smoke-render-pipe.sh
```

## Expected results

- Release binary builds on Linux x86_64.
- Footprint gate: artifact ≤ 60 MiB; no Node/Chromium dynamic dependencies in `ldd`.
- Pipe render: `event=append` and `event=done` on stdout for a bounded fixture document.

## Real-terminal gap

Linux remains **unverified** for placement, scrollback scroll, mouse input, and agent
viewer behavior until the same matrix used for Ghostty on macOS is repeated on a
Kitty-capable Linux terminal and linked here.
