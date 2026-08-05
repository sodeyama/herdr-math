# V3 Phase 6 hardening evidence (T3-601, T3-604)

Date: 2026-08-05

## T3-601 — Fuzz + injection + pathological limits

| Acceptance id | Evidence |
|---------------|----------|
| AT-3-701 | Existing injection corpus in `engine/crates/tmath-render/src/prose.rs` |
| AT-3-703 | `pathological_inputs_hit_finite_limits_without_panicking` in `lib.rs` |
| AT-3-601 fuzz | `adversarial_chunk_streams_never_panic_or_diverge_from_one_shot_parse` (`stream.rs`), `adversarial_byte_streams_never_panic_or_grow_unbounded` + `adversarial_delta_sequences_never_panic_and_fail_closed` (`codec.rs`) |

Command:

```sh
cargo test -p tmath-core codec::tests::adversarial
cargo test -p tmath-render stream::tests::adversarial
cargo test -p tmath-render pathological_inputs_hit_finite_limits
```

## T3-604 — V2 superseded

Added superseded banners to:

- `specs/terminal-math-v2/plans/main.md`
- `specs/terminal-math-v2/tests/main.md`
- `specs/terminal-math-v2/tasks/main.md`
