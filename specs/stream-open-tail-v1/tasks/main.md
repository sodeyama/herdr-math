# Stream Open-Tail V1 Tasks

## Status

- Checklist state: **In progress** — Phases 1-2 complete (commits 5b4a76c, 5fdad7e); Phases 3-5 open.
- Plan: `../plans/main.md`
- Acceptance contract: `../tests/main.md`

Each implementation task lands as one commit; progress updates to this file
land as separate docs commits, per AGENTS.md.

## Phase 1 — Detection helper

- [x] **T-S-101** Implement `open_display_math_start(text) -> Option<usize>`
  in `engine/crates/tmath-render/src/stream.rs` (or as a small exposed
  scanner helper), covering `$$`, `\[`, and line-leading bare `[` openers
  with the scanner's plausibility heuristics, honoring fenced-code
  tracking. Unit tests for each opener kind, fence immunity, and prose
  false-positive resistance. (AT-S-106 groundwork)

## Phase 2 — Splitter bundling

- [x] **T-S-201** Bundle the open span in `StreamSplitter::revise`
  (non-EOF): parse only `text[..start]`, append the open span as one
  provisional `Paragraph` tail block, enforce `source_bytes_per_block`
  on it. (AT-S-101, AT-S-103, AT-S-104, AT-S-107)
- [x] **T-S-202** Unify `tail_open` with the new helper; delete
  `has_unclosed_display_math`'s `$$`/`\]`-only probing. (AT-S-103)
- [x] **T-S-203** EOF path parity: `finish()` parses the full text
  unchanged; add the one-shot equivalence assertion. (AT-S-105)

## Phase 3 — Corpus and planner guarantees

- [ ] **T-S-301** Add the real-answer corpus fixture (sanitized, English
  test comments; Japanese prose retained as Unicode-behavior content per
  AGENTS.md) and the multi-stride replay test. (AT-S-201)
- [ ] **T-S-302** Planner-level no-interior-divergence assertion across
  the replay: every Replace targets the current tail; zero Removes.
  (AT-S-102, AT-S-202)

## Phase 4 — Terminal-level verification

- [ ] **T-S-401** Fake-tty row-budget parity test. (AT-S-301)
- [ ] **T-S-402** Real Ghostty streamed-answer evidence run, recorded
  under `docs/evidence/`. (AT-S-302, release gate)

## Phase 5 — Docs

- [ ] **T-S-501** Update `docs/coding-agents.md` (or architecture notes)
  describing the open-tail bundling behavior and its EOF finality, in the
  same change that lands the behavior if any public contract wording
  changes.
