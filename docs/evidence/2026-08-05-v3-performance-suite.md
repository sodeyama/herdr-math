# V3 performance suite evidence (T3-602)

Date: 2026-08-05  
Machine: macOS arm64 (local reference)  
Suite: `engine/crates/tmath/tests/performance.rs`

## Commands

Registration gate (always runs in CI via `cargo test`):

```sh
cargo test -p tmath performance_suite_is_registered
```

Reference-machine release gates:

```sh
cargo test -p tmath --release performance -- --ignored
```

Equivalent npm entry point (AT-3-905):

```sh
npm run test:performance
```

## Coverage mapping

| Acceptance id | Harness | Notes |
|---------------|---------|-------|
| AT-3-901 (G1) | `warm_block_render_meets_g1` | Warm corpus block render p50/p95 |
| AT-3-903 (G4) | `cold_start_render_meets_g4` | Subprocess `tmath render -` cold start |
| AT-3-902 (G2) | `streaming_transcript_replay_meets_g2_on_release_builds` in `agent_viewer.rs` | Existing release-only append latency test |
| AT-3-904 | streaming replay fixture (`tests/fixtures/agents/streaming-transcript.jsonl`) | Covered by T3-404 replay tests |
| AT-3-905 | `performance_suite_is_registered` + npm script re-home | Dead Vitest path removed |

## Result

- Registration test: PASS (`cargo test -p tmath performance_suite_is_registered`)
- Release gates: run locally with `--release --ignored` before tagging; CI runs the
  non-ignored Rust suite only.
