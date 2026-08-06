# V3 Phase 5 — Release binary footprint (macOS arm64)

Date: 2026-08-06  
Task: T3-503  
Acceptance: AT-3-801

## Command

```sh
cargo build --release -p tmath
scripts/smoke-footprint.sh
```

## Recorded result (reference machine)

| Metric | Value |
| --- | ---: |
| Platform | macOS arm64 (Apple Silicon) |
| Binary | `target/release/tmath` |
| Size | 43.0 MiB (45,086,672 bytes) |
| Cap | 60 MiB |
| Dynamic deps (`otool -L`) | `libSystem`, `libiconv`, `CoreFoundation` only |

No Node.js, Chromium, Playwright, or npm artifacts are linked.

## CI

The same smoke runs on macOS and Linux x86_64 in `.github/workflows/ci.yml`.
