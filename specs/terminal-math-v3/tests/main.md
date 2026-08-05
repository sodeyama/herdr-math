# Terminal Math V3 Acceptance Tests

## Status

- Contract state: **Draft** — ids are stable; no test is implemented or passed yet.
- Plan: `../plans/main.md`
- Task checklist: `../tasks/main.md`
- Conventions follow `specs/terminal-math-v2/tests/main.md`: a failed, skipped,
  retried, or unimplemented case is not a pass; performance and terminal cases
  record evidence under `docs/evidence/`.

Test id scheme: `AT-3-<group><nn>` — groups: 0 feasibility, 1 fidelity, 2 engine,
3 incremental, 4 streaming input, 5 viewer, 6 agent sources, 7 security/privacy,
8 portability/install, 9 performance.

## Group 0 — Feasibility (Phase 0 gate)

- **AT-3-001** RaTeX inline embedding: a Typst paragraph containing three RaTeX
  inline-math boxes (ascender-heavy, descender-heavy, plain) renders with each
  box's baseline aligned to the text baseline within 1 px at dpr 1 and 2 px at
  dpr 2, and the paragraph wraps across lines without clipping any box. Evidence:
  annotated screenshots + measured offsets.
- **AT-3-002** Cold start: process start → first block placed (fake-tty harness,
  warm OS caches) completes in ≤ 300 ms p50 over 10 runs, with **zero system font
  directory reads** (verified by fs tracing or by building with no font-dir code
  path).
- **AT-3-003** Golden corpus parity: the V2 renderer corpus plus the RaTeX coverage
  probe (≥ 22 constructs incl. `align*`, `cases`, `\boxed`, mhchem, CJK `\text{}`)
  renders with the native engine; per-case image diffs against V2 output are
  recorded and each divergence is explicitly accepted or rejected in the evidence
  file. Rejected divergences block the default-engine flip (AT-3-206).

## Group 1 — Fidelity

- **AT-3-101** Markdown subset: headings h1–h6, emphasis, nested lists, block
  quote, table (header + alignment), fenced code with syntect highlighting,
  inline code, hr, and inert links/images render; links/images carry no target and
  trigger no I/O.
- **AT-3-102** Math delimiters: `$...$`, `$$...$$`, `\(...\)`, `\[...\]` all render;
  scanner skip rules (fenced/inline code, escaped currency, unclosed delimiters,
  shell/price patterns) match the V2 scanner corpus verbatim (ported fixtures).
- **AT-3-103** Invalid LaTeX in one formula fails closed per formula: the block
  renders with a bounded error badge for that formula, sibling formulas and all
  other blocks render normally, and the error record carries only allowlisted
  fields.
- **AT-3-104** Transparency + dpr: output PNGs have a transparent background at
  dpr 1–4; glyph raster density matches the requested dpr (probe: stroke width in
  device pixels scales with dpr).

## Group 2 — Engine contract

- **AT-3-201** The resident engine serves ≥ 1,000 sequential renders with stable
  RSS (no monotonic growth beyond cache budget) and no file-descriptor leak.
- **AT-3-202** Per-block limits: block count, per-block source length, per-image
  pixel and byte caps (dpr-scaled per D7), and the 1,000 ms per-block render
  deadline are each enforced with the existing stable error codes; earlier
  placements stay intact.
- **AT-3-203** Error mapping: every `SafeErrorRecord` code emitted by the native
  engine is from the existing allowlisted set; no new code leaks input text.
- **AT-3-204** No subprocess: rendering a document spawns zero child processes
  (asserted via process accounting in the integration harness).
- **AT-3-205** Determinism: rendering the same block twice with the same options
  yields byte-identical PNGs (required for cache correctness).
- **AT-3-206** Engine flip gate: `--engine native` output passes AT-3-1xx and the
  accepted golden corpus before becoming the default; `--engine node` still works
  for one release afterward.

## Group 3 — Incremental pipeline

- **AT-3-301** Append reuse: given a placed 10-block document, appending block 11
  re-renders exactly one block and transmits exactly one new placement; the 10
  existing placements' bytes are not re-sent (asserted on the fake-tty byte
  stream).
- **AT-3-302** Tail replace: mutating only the final block deletes and replaces
  only that block's placement (scoped delete by image id).
- **AT-3-303** Interior edit: mutating block k of n re-renders blocks whose hash
  changed and re-anchors only k..n placements; blocks 1..k-1 are untouched.
- **AT-3-304** Cache bounds: with a cache budget of B pixels, rendering a document
  exceeding B evicts LRU entries, never exceeds the budget, and evicted blocks
  re-render correctly on demand.
- **AT-3-305** Hash discipline: cache keys incorporate source, layout options, and
  dpr; changing any of them misses the cache. Hashes never appear in placement
  authorization decisions.

## Group 4 — Streaming input

- **AT-3-401** Stream mode: feeding a document through a pipe in 10 chunks split at
  arbitrary byte offsets (including mid-UTF-8, mid-fence, mid-`$$`) produces the
  same final placements as a one-shot render (fail-closed intermediate states
  allowed, corruption not).
- **AT-3-402** Block-boundary emission: after a chunk completes a block (blank
  line / closing fence / closing `$$`), that block is placed without waiting for
  any settle timer (measured: no artificial delay ≥ 50 ms between boundary and
  transmit in the harness).
- **AT-3-403** Tail coalescing: a tail block updated 100 times in rapid succession
  results in ≤ 1 in-flight render at any moment and a final placement matching the
  last content (latest-wins).
- **AT-3-404** Unterminated constructs: an unclosed fence or `$$` at the current
  stream end renders as literal text and is upgraded once the terminator arrives.
- **AT-3-405** `tmath watch`: an edit to the watched file re-renders only from the
  first changed block; the reaction is event-driven (no fixed-interval full-file
  re-read on quiescent files).

## Group 5 — Viewer

- **AT-3-501** Per-block placement: the viewer holds one placement per block; no
  composite RGBA buffer exists; appending an answer block is O(new block) in bytes
  transmitted (asserted on the byte stream).
- **AT-3-502** Follow mode: with follow engaged, appended blocks auto-scroll into
  view; any manual scroll disengages follow; `End` re-engages it.
- **AT-3-503** Scroll re-emission: scrolling re-emits only placements whose
  visibility changed, from cached PNGs; total bytes transmitted per scroll step are
  bounded by the visible-block budget, independent of history length.
- **AT-3-504** Bounded history: blocks beyond the history budget lose their
  placements and are re-rendered on scroll-back; memory stays within the cache
  budget during a 1,000-block session.
- **AT-3-505** Shrinking answer: a replacement answer shorter than its predecessor
  leaves no stale placeholder cells or orphan placements.

## Group 6 — Agent sources

- **AT-3-601** Delta protocol: the watcher→viewer socket carries versioned
  `document` / `append` / `replace-tail` frames; oversized, malformed, truncated,
  duplicate, and out-of-order frames are rejected fail-closed with the previous
  viewer state intact (fixture-driven, port of V2 codec tests plus delta cases).
- **AT-3-602** Transcript adapter (Claude Code): given recorded JSONL fixtures
  (streamed append, multi-message answer, rotation, truncation, malformed lines),
  the adapter emits the assistant Markdown deltas exactly, read-only and bounded;
  malformed input degrades to the capture adapter without crashing.
- **AT-3-603** Streaming end-to-end: replaying a recorded agent transcript at
  realistic timing shows each completed block placed while later text is still
  streaming; completed-block append latency meets G2 budgets (recorded evidence).
- **AT-3-604** Capture fallback: with no transcript available, V2 capture behavior
  (boundary heuristics, settle timers) still works, now feeding the incremental
  pipeline so an unchanged answer prefix transmits zero bytes.
- **AT-3-605** Privacy: transcript and pane content never appear in logs, error
  records, or durable state (scan-based assertion, extending the V2 privacy
  tests to the transcript path).

## Group 7 — Security / privacy

- **AT-3-701** Typst injection corpus: `#eval`, `#import`, `#include`, `#read`,
  show/set rules, and raw Typst markup fed through every Markdown context
  (paragraph, heading, list item, table cell, code block, link text, math text)
  render as literal text; no Typst code executes (asserted by rendering output and
  by the world exposing no file/package capability).
- **AT-3-702** No network, no package resolution: the engine performs zero network
  syscalls and resolves zero Typst packages during a full-corpus render (traced in
  the integration harness); fonts resolve only from embedded assets.
- **AT-3-703** Resource exhaustion: pathological inputs (deeply nested lists,
  giant tables, formula bombs within RaTeX's bounded expansion, 10 MB single
  block) hit a limit error within the per-block deadline; the process stays
  responsive and earlier placements survive.
- **AT-3-704** Static privacy gates: the V2 security scan (`security:check`
  equivalent, re-homed after Node removal) and the Rust static privacy tests pass
  over the new crates; no content, paths, or credentials in logs or artifacts.

## Group 8 — Portability / install

- **AT-3-801** Single binary: a clean checkout builds one self-contained `tmath`
  ≤ 60 MB; `ldd`/`otool` shows no Node, browser, or unexpected dynamic deps.
- **AT-3-802** Clean install: `scripts/install.sh` on a machine without Node and
  without network access beyond the artifact fetch installs a working `tmath`
  (render smoke passes).
- **AT-3-803** Linux: build + render smoke (fake-tty) pass on Linux x86_64;
  a real Kitty-graphics terminal run is recorded before Linux is claimed in
  compatibility docs.
- **AT-3-804** Removal completeness: after Phase 5, no references to Playwright,
  Chromium, `TMATH_RENDER_WORKER`, or `node` remain in runtime code or install
  scripts (scan-based).

## Group 9 — Performance (reference machine, recorded evidence)

- **AT-3-901** Warm block render (G1): p50 ≤ 10 ms, p95 ≤ 30 ms over the corpus
  (math and prose blocks measured separately).
- **AT-3-902** Append end-to-end (G2): input delta → placement bytes written, p50
  ≤ 50 ms, p95 ≤ 150 ms in the fake-tty harness.
- **AT-3-903** Cold start (G4): ≤ 300 ms p50 (same protocol as AT-3-002, on the
  release build).
- **AT-3-904** Streaming throughput: replaying a 200-block transcript at 2 blocks/s
  keeps append latency within G2 for every block (no queue growth).
- **AT-3-905** The performance suite is wired into `cargo test`-adjacent tooling
  and CI-runnable; the dead `test:performance` npm script is removed or replaced
  in the same change.
