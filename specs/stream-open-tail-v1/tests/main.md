# Stream Open-Tail V1 Acceptance Tests

## Status

- Contract state: **Draft** — ids are stable; no test is implemented or passed
  yet.
- Plan: `../plans/main.md`
- Task checklist: `../tasks/main.md`
- Conventions follow `specs/terminal-math-v2/tests/main.md`: a failed,
  skipped, retried, or unimplemented case is not a pass; terminal cases
  record evidence under `docs/evidence/`.

Test id scheme: `AT-S-<group><nn>` — groups: 1 open-tail bundling,
2 stream/one-shot equivalence, 3 terminal-level integrity.

Background: streaming a display formula chunk-by-chunk lets pulldown-cmark
misread the unclosed formula's raw LaTeX (line-leading `+`, `-`, `#`, `---`)
as Markdown structure, splitting it into several blocks. When the closing
delimiter arrives the blocks merge into one `DisplayMath`, producing an
interior divergence the plain stream sink can only handle by deleting images
in place (leaving blank rows) and appending at the bottom. See the plan's
root-cause chain; every case below pins the fix that bundles the open
formula into a single provisional tail block.

## Group 1 — Open-tail bundling (StreamSplitter unit level)

- **AT-S-101** Unclosed `\[` formula containing line-leading `+`, `-`, `#`,
  and `---` lines: for every chunk boundary (byte-stride sweep of the input,
  strides at least 1, 3, 7, 64), each intermediate revision's block list
  contains the unclosed span as exactly one trailing `Paragraph` block —
  never split into multiple blocks, never classified as
  `List`/`Heading`/`ThematicBreak`.
- **AT-S-102** Formula completion is a pure tail update: at the revision
  where the closing `\]` arrives, `stable_prefix` covers every block before
  the formula, and the plan derived from the revision contains no `Remove`
  and no `Replace` of a non-tail block (planner-level assertion).
- **AT-S-103** Bare-bracket display math (`[ ... ]` with a LaTeX hint,
  including a multi-line body with a blank line): same guarantees as
  AT-S-101/102, and `tail_open` stays `true` while the bracket is unclosed
  (this is new behavior — today bare brackets have no unclosed detection).
- **AT-S-104** `$$` display math across chunks keeps its existing behavior:
  AT-S-101/102 guarantees hold (regression guard for the delimiters that
  already worked).
- **AT-S-105** EOF finality: a stream that ends (`finish()`) with an
  unclosed opener parses the full text exactly like the one-shot parser —
  block-for-block equal to `parse_blocks_limited` over the same bytes — so
  an unclosed formula degrades to visible raw text, never to a stuck tail.
- **AT-S-106** Fenced code immunity: `\[`, `$$`, and line-leading `[` inside
  a fenced code block are never treated as openers; the fence streams and
  closes exactly as today.
- **AT-S-107** Oversized open tail fails closed: an opener followed by more
  than `source_bytes_per_block` bytes without closing produces the same
  stable limit error the one-shot parser reports for an oversized block,
  and the splitter reports the failure on every subsequent push (sticky
  failure, matching existing splitter semantics).

## Group 2 — Stream/one-shot equivalence (corpus level)

- **AT-S-201** Real-answer corpus replay: a fixture modeled on the field
  answer (Japanese prose, four consecutive `\[...\]` formulas with
  line-leading `+`/`(` lines, lists, headings, thematic breaks, and one
  bare-bracket formula) streamed at multiple strides yields a final
  revision whose block list equals the one-shot
  `parse_blocks_limited` result byte-for-byte (extends AT-3-401's chunk
  parity to this corpus).
- **AT-S-202** No interior divergence over the whole replay: across every
  revision of the AT-S-201 replay, every `Replace` in the derived plans
  targets the then-current tail block and no `Remove` is ever planned
  (the summary-event reproduction that exposed the bug — 7 non-tail
  replaces — must count zero after the fix).

## Group 3 — Terminal-level integrity

- **AT-S-301** Row-budget parity on a fake tty: streaming the AT-S-201
  fixture through the fake-tty harness ends with the cursor exactly
  `sum(rows(block)) + inter-block line feeds` rows below the start
  anchor — no leftover blank rows from replaced placements (this is the
  terminal-byte-level expression of "no gaps").
- **AT-S-302** Real-terminal evidence (release gate, manual): piping a
  streamed LLM answer (`ask ... | tmath render -` or an equivalent
  paced feeder) into a real Ghostty session shows no blank regions
  between blocks and preserves document order; screenshot recorded under
  `docs/evidence/` with private content redacted.
