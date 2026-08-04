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
- [ ] **T3-102** Port the scanner (`scan-latex.ts`) to Rust with the V2 fixture
      corpus. (AT-3-102)
- [ ] **T3-103** Markdown mapping: pulldown-cmark → Typst structured content
      (no string-concatenated markup); syntect code blocks; inert links/images.
      (AT-3-101, AT-3-701 groundwork)
- [ ] **T3-104** Math rendering: RaTeX display + inline embedding; per-formula
      fail-closed error badges. (AT-3-103, AT-3-104)
- [ ] **T3-105** Per-block limits and deadlines (D7); determinism.
      (AT-3-202, AT-3-203, AT-3-205)
- [ ] **T3-106** Wire `tmath render --engine native` (default remains node);
      no-subprocess assertion. (AT-3-204)
- [ ] **T3-107** Docs commit: amend `AGENTS.md` required-architecture section and
      `docs/architecture.md` for the resident in-process engine.

## Phase 2 — Incremental pipeline

- [ ] **T3-201** Streaming block splitter (chunk-safe, UTF-8-safe, unterminated
      constructs as literal). (AT-3-401, AT-3-404)
- [ ] **T3-202** Render cache: content hashing, bounded LRU, eviction.
      (AT-3-304, AT-3-305)
- [ ] **T3-203** Placement planner: prefix reuse, append, tail replace, interior
      edit re-anchoring. (AT-3-301..303)
- [ ] **T3-204** `tmath render -` stream mode with boundary-driven emission and
      tail coalescing. (AT-3-402, AT-3-403)
- [ ] **T3-205** `tmath watch <file>` (FS events, changed-block re-render).
      (AT-3-405)

## Phase 3 — Viewer v2

- [ ] **T3-301** Per-block placements in `agent-viewer`; delete composite RGBA
      path. (AT-3-501, AT-3-505)
- [ ] **T3-302** Follow mode + disengage/re-engage. (AT-3-502)
- [ ] **T3-303** Visibility-driven scroll re-emission from cache. (AT-3-503)
- [ ] **T3-304** Bounded history + re-render on scroll-back. (AT-3-504)
- [ ] **T3-305** Recorded terminal evidence: many-placement behavior in Ghostty
      (+ tmux route). (supports AT-3-501/503; evidence file)

## Phase 4 — Agent sources

- [ ] **T3-401** Delta socket protocol (versioned document/append/replace-tail);
      codec fixtures incl. malformed/out-of-order. (AT-3-601)
- [ ] **T3-402** Claude Code transcript adapter with JSONL fixtures; graceful
      degradation to capture. (AT-3-602)
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
