# Stream Open-Tail V1 Plan

## Status

- Plan state: **Draft**
- Acceptance contract: `../tests/main.md`
- Task checklist: `../tasks/main.md`
- Scope: stop incremental streaming (`tmath render -` fed from a pipe) from
  leaving permanent blank gaps and reordered blocks when a display formula
  arrives across multiple chunks. This plan touches the stream splitter only
  (`engine/crates/tmath-render/src/stream.rs`); it does not change the
  one-shot parser semantics, the render pipeline, or the terminal sink.

## Incident summary (2026-08-06, macOS/Ghostty)

Observed: `ask '<prompt>' | tmath render -` (an LLM CLI streaming its answer
into tmath) renders every formula correctly, but large blank regions (5-15
terminal rows) remain between blocks, and some blocks appear out of document
order. The gaps appeared after commit `9c2b3a7` ("fix(markdown): protect
display formulas spanning a blank line").

## Root cause chain (verified by live reproduction)

Each step below was confirmed against the real streamed answer text
(97 blocks, four consecutive `\[...\]` formulas among them), replayed into
`tmath render --engine native -` in 64-byte chunks with a 10 ms delay:

1. **Piped stdin always selects the incremental stream path.** `main.rs`'s
   `render()` routes `input == "-" && !stdin.is_terminal()` to
   `render_native_stream` regardless of whether stdout is a terminal, so a
   streaming producer drives one revision per drained chunk batch.
2. **An unclosed display formula is parsed as ordinary Markdown.** Until the
   closing `\]` arrives, the formula's raw LaTeX is plain paragraph text to
   pulldown-cmark. Real formulas contain line-leading `+`, `-`, `(`, `#`,
   and `---` tokens; pulldown-cmark reads those as list items, headings, or
   thematic breaks and **splits the unclosed formula into multiple blocks**
   (reproduced: an unclosed `\Lambda_n` update formula parsed as 3 blocks
   because of two line-leading `+` lines).
3. **Formula completion then merges those blocks into one.** When `\]`
   arrives, `parse_blocks_limited` (with the placeholder protection added in
   `9c2b3a7`) correctly recognizes the whole span as one `DisplayMath`
   block. The revision therefore replaces *multiple previously-placed
   blocks* with one — an **interior divergence**, not a pure tail update
   (reproduced: 7 non-tail replaces out of 107 in one answer).
4. **The plain stream sink cannot re-anchor an interior block.**
   `native_stream.rs::TerminalSink::replace` only does an in-place
   `tail_replace_operations` when the replaced block is the current tail and
   its top row is still reachable. For every other case it deletes the Kitty
   image **leaving its old cells blank** and appends the replacement at the
   bottom (an explicitly documented Phase-2 limitation). `remove` likewise
   deletes the image without reclaiming rows. The leftover rows are the
   observed gaps; the append-at-bottom is the observed reordering.

Why this surfaced only now: before `9c2b3a7`, the split-up unclosed formula
**never merged back** — `is_complete_display_math` only upgraded a paragraph
that was exactly one complete formula, so the broken-apart blocks stayed
broken (the original "formula rendered as raw text" bug this epic fixed).
That older bug masked the interior-divergence path; fixing block merging
exposed it.

Secondary defect found during investigation:
`stream.rs::has_unclosed_display_math` only probes `$$` and `\]` as closing
delimiters. Bare-bracket display math (`[ ... ]`, supported since
`cfeb35d`) has no unclosed-detection at all, so its `tail_open` signal is
wrong during streaming.

## Fix strategy: bundle the open formula into one tail block

Kill the divergence at its source. While the stream is still open, any text
from the start of an **unclosed display-math opener** to the end of the
buffer must be withheld from pulldown-cmark and carried as one provisional
`Paragraph` tail block:

- While the formula is open, every revision keeps that span as a single
  block, so the terminal sink only ever sees same-position tail replaces
  (which `tail_replace_operations` handles in place, clearing and reusing
  the same rows).
- When the closing delimiter arrives, the merge is a plain tail replace of
  one block by one block — no interior divergence, no leftover rows, no
  reordering.
- At EOF (`finish()`), an unclosed formula is final: parse the full text
  exactly as today so one-shot semantics and every existing hermetic test
  stay unchanged.

### Implementation outline

1. Add `open_display_math_start(text) -> Option<usize>` to `stream.rs`: the
   byte offset of the earliest display-math opener that never closes before
   the end of `text`. Openers: `$$`, `\[`, and a line-leading bare `[` that
   passes the same plausibility heuristics the scanner uses
   (`is_plausible_block_latex` equivalent: LaTeX hint present, no full-width
   punctuation, closing candidate not a Markdown link). Reuse the scanner's
   fence tracking so fenced code never counts. Prefer implementing this by
   exposing a small scanner helper over duplicating delimiter logic.
2. In `StreamSplitter::revise(eof = false)`: when
   `open_display_math_start` returns `Some(start)`, run
   `parse_blocks_limited(&text[..start])` and append one synthetic
   `Paragraph` block with source `text[start..]` (subject to the same
   per-block byte cap; a cap violation fails closed exactly as today).
   When it returns `None`, or when `eof` is true, parse the full text as
   today.
3. Replace `has_unclosed_display_math` and the `\]`/`$$`-only probing with
   the new helper so `tail_open` and the bundling decision cannot drift
   apart. This also fixes the bare-bracket `tail_open` gap.
4. Hash flow is unchanged: the bundled tail block hashes like any other
   block, so `stable_prefix` naturally covers everything before the open
   formula.

### Explicitly out of scope

- The interior-divergence fallback in `TerminalSink::replace`/`remove`
  (delete-leaving-rows + append-at-bottom) stays as is. Re-anchoring
  interior blocks in the plain stream path requires explicit
  viewport/history state and belongs to the Phase-3 viewer work already
  tracked in `specs/terminal-math-v3`. This plan removes the only known
  trigger on the stream path; `tmath watch` and the agent-viewer have their
  own region/batch machinery and are not changed here.
- No renderer, scanner-contract, or placement changes.

## Risks

- **False-positive opener detection** (e.g. prose that begins a line with
  `[` and never closes) would hold the tail open until EOF; the text still
  renders on `finish()`, so the failure mode is delayed display, not loss.
  The bare-bracket opener therefore reuses the scanner's plausibility
  heuristics, and `$$`/`\[` openers are unambiguous.
- **Large open formulas**: the bundled tail is subject to
  `source_bytes_per_block`, so a pathological never-closing opener cannot
  grow without bound; it fails closed exactly like an oversized block today.
