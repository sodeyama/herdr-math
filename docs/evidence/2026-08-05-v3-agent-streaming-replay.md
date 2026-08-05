# Evidence: Agent Transcript Streaming Replay (AT-3-603)

- Date: August 5, 2026
- Environment: macOS arm64, debug `cargo test` harness (hermetic, no tmux)
- Fixture: `tests/fixtures/agents/streaming-transcript.jsonl` (synthesized JSONL only;
  no real Claude Code transcript bytes)
- `tmath` change set: transcript file resolution fix + replay/privacy tests

## Method

Two hermetic tests exercise the full streaming path without a live terminal:

1. **`streaming_transcript_fixture_replay_emits_document_then_replace_tails`**
   (`agent_watcher.rs`): replays the fixture through `TranscriptAdapter` and the
   watcher’s `emit_transcript_delta` / `AnswerHistory` wire path; asserts a
   `Document` resync followed by at least one `ReplaceTail`, and that the
   reassembled document contains paragraphs from multiple fixture lines.

2. **`streaming_transcript_replay_places_blocks_incrementally`**
   (`agent_viewer.rs`): replays the same fixture one JSONL line at a time through
   `TranscriptAdapter` into `apply_incoming_message` / `render_and_place`; asserts
   the placed block count grows before the next line arrives and that each append
   step on this reference machine stays within the G2 p95 ceiling (150 ms).

## Commands

```sh
cargo test -p tmath streaming_transcript
cargo test -p tmath --test privacy_gates
cargo test --release --bin tmath streaming_transcript_replay_meets_g2_on_release -- --ignored
```

## Result: PASS (hermetic AT-3-603 functional gate)

- Incremental placement: block count increases as assistant lines arrive, not only
  after the full fixture is on disk.
- Wire protocol: watcher emits `Document` then `ReplaceTail` for streaming growth.
- G2-style append latency: every measured append step on this machine was ≤ 150 ms
  during the hermetic replay test run.

## Related fix (live Claude Code blank viewer)

The same change set fixes a live failure mode where `tmath agent` opened a **stale**
project JSONL at EOF while Claude Code wrote the current session to a **new** file,
leaving the viewer at “0 blocks”. Resolution now prefers actively modified or
watcher-era session files, re-resolves periodically, and falls back to tmux capture
when the transcript stays idle.

## Limitations

- This evidence is **hermetic** (no Ghostty/tmux byte capture). Real-terminal
  streaming timing under `feed.py` remains optional supplemental evidence for T3-602.
- The fixture is synthesized; it does not prove resilience to every future Claude
  Code JSONL shape drift (AT-3-602 degradation path covers parse failures).
