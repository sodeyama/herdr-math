# Terminal Math V3 Task Checklist

## Status

- Checklist state: **Draft — no task started**
- Plan: `../plans/main.md`
- Acceptance contract: `../tests/main.md`
- Rules: one logical change per commit; a task is complete only when its listed
  acceptance tests pass with required evidence; spec/doc updates land as separate
  documentation commits immediately after the implementation commit.

## Phase 0 — Feasibility spikes (gate)

- [x] **T3-001** Spike: RaTeX SVG/PNG inline boxes embedded in a Typst paragraph
      with baseline metrics; record offsets and wrapping behavior.
      (AT-3-001 PASS; commit `9c263dd`; evidence
      `docs/evidence/2026-08-04-v3-inline-math-baseline-spike.md`)
- [x] **T3-002** Spike: embedded-font engine construction (RaTeX OFL set +
      `typst-assets` subset), no system font scan; measure cold start.
      (AT-3-002 PASS at spike level, ~9-12 ms p50 vs 300 ms budget; commit
      `a7ed163`; evidence `docs/evidence/2026-08-04-v3-cold-start-spike.md`;
      fs-trace variant deferred to Phase 1)
- [x] **T3-003** Build the golden corpus: V2 renderer corpus + coverage probe +
      historically problematic formulas; render with V2 and native spike; record
      accepted/rejected divergences. (AT-3-003 PASS, native 31/31, no rejected
      divergence; commit `ddc9535`; evidence
      `docs/evidence/2026-08-04-v3-golden-corpus-spike.md`)
- [x] **T3-004** Decision checkpoint: proceed with RaTeX+Typst, or revise the plan
      to the MathJax+resvg fallback. Update `plans/main.md` status either way.
      (Decision: **proceed with RaTeX + Typst**; recorded in the AT-3-003
      evidence file and the plan status block)

## Phase 1 — Native render crate

- [x] **T3-101** Create `engine/crates/tmath-render`: block model, options,
      `SafeErrorRecord` mapping; pin `ratex-*`/`typst*` versions.
      (commit `31fc843`; 16 unit tests, clippy clean)
- [x] **T3-102** Port the scanner (`scan-latex.ts`) to Rust with the V2 fixture
      corpus. (AT-3-102 PASS; commit `7901b92`; 28 scanner tests incl. a
      fixture-sync guard over `answer-corpus.json`)
- [x] **T3-103** Markdown mapping: pulldown-cmark → Typst structured content
      (no string-concatenated markup); syntect code blocks; inert links/images.
      (AT-3-101 groundwork + AT-3-701 injection corpus; commit `13f31f5`;
      60 crate tests green)
- [x] **T3-104** Math rendering: RaTeX display + inline embedding; per-formula
      fail-closed error badges. (AT-3-103/104 covered at crate level; commit
      `4e0f94f`; 66 crate tests green; supervisor visual smoke on a mixed
      document confirmed badge, DisplayMath promotion, inline baselines)
- [x] **T3-105** Per-block limits and deadlines (D7); determinism.
      (AT-3-202/203/205 covered at crate level; commit `20bd570`; 72 crate
      tests green incl. marker-based no-leak audit and byte-determinism)
- [x] **T3-106** Wire `tmath render --engine native` (default remains node);
      no-subprocess assertion. (AT-3-204 PASS via hermetic empty-PATH tests
      in both directions; commit `0ea63c4`; real-terminal visual check still
      pending for AT-3-206)
- [x] **T3-107** Docs commit: amend `AGENTS.md` required-architecture section and
      `docs/architecture.md` for the resident in-process engine.
      (dual-engine migration state documented; native engine labeled opt-in
      and experimental until AT-3-206)

## Phase 2 — Incremental pipeline

- [x] **T3-201** Streaming block splitter (chunk-safe, UTF-8-safe, unterminated
      constructs as literal). (AT-3-401/404 covered at crate level; commit
      `a83734d`; 88 crate tests green)
- [x] **T3-202** Render cache: content hashing, bounded LRU, eviction.
      (AT-3-304/305 covered at crate level; commit `4a9d6b0`; 96 crate tests
      green; errors never cached)
- [x] **T3-203** Placement planner: prefix reuse, append, tail replace, interior
      edit re-anchoring. (AT-3-301..303 op-level coverage; commit `26030fe`;
      byte-level fake-tty assertions land with the T3-204 wiring)
- [x] **T3-204** `tmath render -` stream mode with boundary-driven emission and
      tail coalescing. (AT-3-402/403 PASS via hermetic event-line tests;
      commit `e791f79`; interior in-place re-anchoring deferred to Phase 3)
- [x] **T3-205** `tmath watch <file>` (FS events, changed-block re-render).
      (AT-3-405 PASS via hermetic event-line tests; commit `33527c2`;
      known gap: no dedicated SIGTERM-exit test yet — carried to Phase 6
      hardening)

## Phase 3 — Viewer v2

- [x] **T3-301** Per-block placements in `agent-viewer`; delete composite RGBA
      path. (AT-3-501/505 covered at unit level plus shared stream-emitter
      integration tests; commit `2e21aa4`; live run placed a 13-block answer
      per-block. Includes T3-302 follow mode at basic level; AT-3-503
      visibility-window scrolling deferred to T3-303. Terminal-fit auto
      layout added alongside in `9e83de7`.)
- [x] **T3-302** Follow mode + disengage/re-engage. (AT-3-502 covered by
      hermetic unit tests: pure `Viewport` state machine, disengage-on-scroll /
      End-`F` re-engage, offset stability across appends while disengaged;
      commit `5754153`. Known placeholder: append/scroll trigger a full-window
      redraw (emit-then-redraw) until T3-303's visibility-diff re-emission;
      real-terminal evidence lands with T3-305.)
- [x] **T3-303** Visibility-driven scroll re-emission from cache. (AT-3-503 PASS
      at the hermetic byte level: id-based window sync deletes only departures,
      re-places the window from retained PNGs, erases residual rows, and a
      2,000-block-history scroll step costs byte-identical output to a 20-block
      one; append writes are suppressed state-only while follow is disengaged.
      Also fixes the placement budget acting as a de facto 64-block history cap
      in viewer mode. Commit `25803cb`; AT-3-503 wording refined in
      `tests/main.md` to record the whole-window re-placement policy;
      real-terminal evidence lands with T3-305.)
- [x] **T3-304** Bounded history + re-render on scroll-back. (AT-3-504 covered
      hermetically: `Limits::retained_window_blocks` fixed-radius keep-alive
      evicts retained PNGs on every append and window sync; scroll-back
      restores via RenderCache hit or source re-render, fail-closed per block;
      1,000-block session stays within the retained budget. Commit `d6629ee`.
      Known gaps for Phase 6: the `TerminalSink` full flow is verified by
      pure-function tests plus review (no fake-tty sink harness yet), and one
      tmath-render lib test flaked once under heavy parallel load —
      timing-sensitivity to audit in T3-602.)
- [ ] **T3-305** Recorded terminal evidence: many-placement behavior in Ghostty
      (+ tmux route). (supports AT-3-501/503; evidence file)

## Phase 4 — Agent sources

- [x] **T3-401** Delta socket protocol (versioned document/append/replace-tail);
      codec fixtures incl. malformed/out-of-order. (AT-3-601 PASS at codec
      level: strict last+1 sequencing, invalidate-until-next-Document resync,
      UTF-8 tail-boundary checks, aggregate reassembly cap at
      IPC_MAX_REQUEST_BYTES, unconditional V2 Document acceptance; 28 codec
      tests + viewer wiring tests. Commit `6c177dc`. Watcher-side delta
      emission lands with T3-402/403.)
- [x] **T3-402** Claude Code transcript adapter with JSONL fixtures; graceful
      degradation to capture. (AT-3-602 PASS with synthesized inline JSONL
      fixtures: EOF-start read-only tail, bounded per-poll reads, rotation and
      truncation recovery, malformed-line skip, char-boundary-safe block
      truncation, Document/Append delta emission with blank-line separators
      verified at the reassembled-document level; I/O failures degrade to the
      capture adapter. Commit `b8f0a8f`. ReplaceTail emission is an unused
      seam until a transcript format rewrites tails in place.)
- [ ] **T3-403** Rewire capture adapter through the incremental pipeline.
      (AT-3-604)
- [ ] **T3-404** Streaming end-to-end replay evidence. (AT-3-603)
- [ ] **T3-405** Privacy scan extension to the transcript path. (AT-3-605)

## Phase 5 — Node removal + portability

- [ ] **T3-501** Flip default engine to native after AT-3-206 passes; keep
      `--engine node` one release.
- [ ] **T3-502** Remove TS renderer/scanner, Playwright/sharp/KaTeX/markdown-it/
      highlight.js, postinstall browser fetch, `TMATH_RENDER_WORKER`; re-home the
      security scan. (AT-3-804, AT-3-704)
- [ ] **T3-503** Single-binary packaging + install.sh without Node; footprint
      gate. (AT-3-801, AT-3-802)
- [ ] **T3-504** Linux x86_64 build + smoke; record real-terminal evidence before
      claiming support. (AT-3-803)
- [ ] **T3-505** Docs commit: README, getting-started, compatibility, licensing
      (OFL font attribution, THIRD_PARTY_NOTICES), CHANGELOG.

## Phase 6 — Hardening + release gate 0.3.0

- [ ] **T3-601** Fuzz splitter + delta decoder; injection corpus. (AT-3-701..703)
- [ ] **T3-602** Performance suite in CI-runnable form; reference-machine evidence.
      (AT-3-901..905)
- [ ] **T3-603** Full release gate: clean build/install, real Ghostty (+ tmux)
      smoke, version agreement, no secrets/paths scan.
- [ ] **T3-604** Mark V2 spec superseded; final docs pass.
