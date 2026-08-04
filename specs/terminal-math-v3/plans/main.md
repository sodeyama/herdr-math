# Terminal Math V3 Plan — Native Realtime Renderer

## Status

- Plan state: **Phase 0 complete — all three feasibility gates passed
  (AT-3-001/002/003); decision recorded at T3-004: proceed with the
  RaTeX + Typst native engine. Phase 1 not started.**
- Target release: `0.3.0`
- Last updated: August 4, 2026
- Acceptance contract: `../tests/main.md`
- Task checklist: `../tasks/main.md`
- Predecessor spec: `../../terminal-math-v2/plans/main.md` (V2 remains the shipped
  architecture until V3 phases land)

This document is a **plan**, not verified release behavior. All latency and footprint
numbers below are preliminary measurements from the 2026-08-04 investigation on macOS
arm64; each must be re-recorded under `docs/evidence/` by the phase that depends on it.

## Motivation

V2 shipped a correct but slow pipeline. The user-facing goals for V3 are:

1. **Realtime terminal UX**: rendered output should appear "instantly" (tens of
   milliseconds) after input changes, not after a second-plus pipeline delay.
2. **Streaming viewer for coding agents**: when watching a coding agent (Claude Code,
   Codex, ...), rendered blocks should appear incrementally in the viewer pane *while*
   the answer streams, not only after the answer settles.
3. **Small, portable install**: no browser download, no Node runtime requirement,
   Linux-capable.

## Problem Inventory (measured, 2026-08-04)

### P1. Per-render process lifecycle dominates latency

The actual typesetting + rasterization work costs 40–80 ms, but every render pays
540–600 ms steady-state (1,246 ms first run) because a fresh Node process **and** a
fresh Chromium process are launched and torn down per render:

| Stage | Measured |
|---|---:|
| Full one-shot render (steady state) | 540–600 ms |
| Node startup + `dist/renderer` import | ~300–360 ms |
| `chromium.launch` + context + page + close | ~120–170 ms |
| Actual `setContent`/fonts/screenshot/sharp work | 40–80 ms |

The V1 ADR (`docs/decisions/0001-v1-renderer.md`) chose Chromium on the strength of a
42.7 ms *warm* figure that assumed a persistent browser — the shipped one-shot design
(`src/renderer/index.ts:6`, `src/renderer/render.ts:136`) never realizes it. The
one-shot contract is mandated by `AGENTS.md` ("The TypeScript `tmath-render`
subprocess is one-shot"), so this is a specification problem, not only a code problem.

### P2. Chromium is heavyweight, fragile, and macOS-only

- `node_modules` totals 366 MB; the Playwright Chromium headless shell alone is
  199 MB. The installed app tree is 262 MB for a 1.1 MB Rust binary.
- The executable path is hardcoded to a pinned revision and only
  `chrome-headless-shell-mac-{arm64,x64}` are mapped
  (`src/renderer/browser-backend.ts:28-42`); **Linux/Windows cannot render at all**,
  and a Playwright version bump silently breaks the path.
- Each render lays the page out twice (probe pass + final pass) and runs KaTeX twice
  per math segment; `katex.min.css` is re-read from disk every render.

### P3. Agent mode is poll-and-settle, not streaming

`tmath agent` polls `tmux capture-pane` every 250 ms, requires the answer to hold
unchanged for 600 ms (any growth resets the timer, 3,000 ms ceiling), then triggers a
full-document re-render (~550 ms). Floor: **~1.4 s after the agent stops writing**; a
steadily streaming answer is deliberately held to the 3 s ceiling
(`agent_watcher.rs:26-31,196-197`). The documented answer to streaming is to wait
longer (`docs/coding-agents.md:88`), the opposite of the V3 goal.

### P4. Monolithic document image; no incrementality anywhere

- Exactly one PNG per document; per-block rendering does not exist
  (`src/renderer/document.ts`, `browser-backend.ts:96-107`).
- The agent viewer composites all answers into one growing RGBA buffer (up to
  64 Mi pixels = 256 MB RGBA), re-transmits the **entire** composite on every new
  answer, and re-uploads a full cropped copy on every scroll step
  (`agent_viewer.rs:293-323,412,471,509,562`).
- No diffing, no caching, no partial update at any layer.

### P5. Lossy input source for agent answers

`tmux capture-pane` exposes presentation cells, not the agent's Markdown source, so
the watcher must heuristically reverse box-drawing tables back into Markdown and
strip repainted working frames (`tmath-core::agent::boundary`). This is inherently
fragile and discards source fidelity that exists on disk (e.g. Claude Code's
transcript JSONL).

### P6. Transport and density mismatches

- The PNG travels base64-inside-JSON over a pipe read via a 25 ms sleep-poll loop
  (`render.rs:104-114`); stdin is read fully before any work (`subprocess.ts:112`).
- `deviceScaleFactor` (1–4) multiplies rasterized pixels by up to 16× against fixed
  `rawPngBytes` 512 KiB / `base64PayloadBytes` 700 KiB caps sized for dpr 1.
- `package.json` still declares `test:performance` against a spec file that does not
  exist; the only recorded performance evidence predates the one-shot architecture.

## Goals

- **G1 — Warm block render** (math or prose block, engine resident): ≤ 10 ms p50,
  ≤ 30 ms p95.
- **G2 — End-to-end append latency** (new complete block arrives on a streaming
  input → pixels placed in the terminal): ≤ 50 ms p50, ≤ 150 ms p95.
- **G3 — Streaming viewer**: blocks appear incrementally while an agent answer is
  still being produced; no settle-delay for already-complete blocks.
- **G4 — Cold start** (`tmath` process start → first block placed): ≤ 300 ms p50.
- **G5 — Install footprint**: one self-contained binary ≤ 60 MB; no Node, no browser
  download, no postinstall network fetch beyond the binary itself.
- **G6 — Portability**: macOS arm64 primary; Linux x86_64/arm64 buildable and
  smoke-tested (rendering no longer platform-gated).
- **G7 — Fidelity**: KaTeX-grade math output verified against a golden corpus.

## Non-Goals

- No general HTML/CSS engine, no arbitrary Markdown extensions (the strict allowlist
  stands).
- No change to the Kitty-graphics-only transport or the scrollback-anchored model.
- No daemon shared across terminals; the engine is resident *within* a `tmath`
  process, not a system service.
- V3 does not remove the V2 privacy/security invariants; it inherits them all.

## Decisions

### D1. Replace Chromium+Node with a native Rust renderer: RaTeX + Typst

The renderer becomes a Rust crate compiled into `tmath`. Two engines split the work:

- **Math (`$...$`, `$$...$$`, `\(...\)`, `\[...\]`): RaTeX**
  (`ratex-render`/`ratex-parser`/`ratex-layout`/`ratex-svg`, MIT, fonts OFL-1.1) — a
  KaTeX-compatible pure-Rust typesetter. Measured: 0.64 ms warm / 2.4 ms cold per
  formula, 5.3 MB binary contribution, zero C dependencies (tiny-skia + png), true
  RGBA transparency, `device_pixel_ratio` and background-alpha options,
  KaTeX-style parse errors with source locations, and a 22/22 pass on a coverage
  probe including `align*`, `cases`, `\boxed`, mhchem, and CJK `\text{}`.
- **Markdown subset (headings, emphasis, lists, quotes, tables, code blocks, inert
  links): Typst as a library** (`typst` + `typst-render` via `typst-as-lib`,
  Apache-2.0). Measured: 0.25 ms warm compile; a full subset page (headings, nested
  lists, quote, table, syntect-highlighted code block) compiled in 32 ms.
  syntect replaces highlight.js. Transparent page background is supported
  (`#set page(fill: none)`).

Prose blocks that contain inline math embed RaTeX-rendered SVG as inline boxes in
the Typst document, aligned using RaTeX's baseline metrics (height/depth), so LaTeX
math never goes through a LaTeX→Typst syntax conversion. **Phase 0 must validate
this embedding (baseline alignment, line wrapping around inline boxes) before any
other phase starts**; it is the highest technical risk in this plan.

Rejected alternatives (recorded for the ADR):

- **mitex (LaTeX→Typst conversion)**: hard-fails on commands RaTeX renders (e.g.
  `\ce`), requires vendoring MiTeX's Typst shims, effectively unmaintained since
  2024-06. Fidelity risk too high for the primary path.
- **MathJax v4 + resvg sidecar (persistent Node)**: 1.94 ms warm is acceptable but
  keeps the Node runtime requirement and a 161 MB tree, and MathJax's NCM font
  diverges from KaTeX output. Kept as the documented fallback if RaTeX fails
  Phase 0.
- **KaTeX HTML without a browser**: no viable layout engine exists (Satori is
  flex-only and fails silently; Blitz is alpha with no table/inline commitments).
- **Keeping Chromium warm in a persistent worker**: fixes latency (~42 ms) but keeps
  the 199 MB macOS-only browser, the double-layout design, and the Node dependency.

### D2. The renderer is resident and in-process; the one-shot subprocess contract is retired

`tmath` hosts a persistent render engine (fonts loaded once, syntect syntax set
loaded once, RaTeX/Typst worlds reused across renders). There is no render
subprocess, no JSON-over-pipe hop, and no base64 detour for render results; PNG
bytes pass in memory to the placement layer. Font loading must use fonts embedded
in the binary (RaTeX's OFL set + `typst-assets` subset) — **no system font
directory scan** — so cold start is deterministic and the ~576 ms font-search cost
measured for default Typst setup never occurs.

Consequences:

- `AGENTS.md` "Required Architecture" must be amended in the same change that lands
  Phase 1: remove the one-shot TypeScript subprocess requirement and the
  `tmath-render/1` IPC mandate; state the in-process engine and its limit
  enforcement points instead. Per the repository workflow rule, this spec change
  precedes the implementation commit.
- The TypeScript renderer, scanner, and the Node/npm toolchain are removed at the
  end of the migration (Phase 5), not at the start; until then V2 remains the
  shipped path.
- The scanner (`src/scanner/scan-latex.ts`) is ported to Rust with its test corpus;
  the existing fixtures become the port's acceptance corpus.

### D3. Block-based incremental pipeline with content-hash caching

The document model changes from "one PNG per document" to an ordered list of
**blocks** (paragraph, heading, list, quote, table, code block, display math). The
pipeline is:

```
input text ──► block splitter ──► [Block { kind, source, hash }]
                                      │  per-block content hash (BLAKE3 or SHA-256,
                                      │  keyed on source + layout options + dpr)
                                      ▼
                              render cache (bounded LRU)
                                      │  hit  → cached PNG + metrics
                                      │  miss → RaTeX / Typst render
                                      ▼
                              placement planner
                                      │  unchanged prefix → untouched placements
                                      │  new blocks       → append placements
                                      │  changed tail     → scoped delete + replace
                                      ▼
                              Kitty placements (one per block)
```

Invariants:

- A block whose hash is unchanged is never re-rendered and its placement is never
  re-transmitted.
- Appending block N+1 touches only block N+1 (and the streaming tail block, see D4).
- The cache is bounded (entry count and total pixel budget) and eviction only costs
  a re-render, never correctness.
- Hashes are cache keys only, never authorization or boundary checks (inherited
  invariant).

### D4. Streaming-first input: render on block boundaries, not on silence

Poll-and-settle debouncing is replaced by boundary-driven emission:

- **Stream mode** (`tmath render -` on a pipe, and all watcher sources): input is
  consumed incrementally. A block is *complete* when the splitter sees its
  terminator (blank line, closing fence, closing `$$`, table end, EOF). Complete
  blocks render and place immediately.
- The **tail block** (still growing) renders with coalescing: at most one in-flight
  render; when it finishes and the tail changed meanwhile, render again with the
  newest content (effectively latest-wins at up to render speed, ~30 fps capped).
  An unclosed fence or `$$` renders as literal text until closed (fail-closed: no
  speculative math parse of an unterminated region).
- `tmath watch <file>`: filesystem-event driven (kqueue/inotify via `notify`),
  falling back to a bounded poll only where FS events are unavailable. Re-splits
  and re-renders only from the first changed block.

### D5. Agent integration reads structured sources; capture becomes the fallback

The watcher gains **source adapters** with a fixed priority:

1. **Transcript adapter (preferred)**: for agents that persist a structured
   transcript on disk (Claude Code writes JSONL under
   `~/.claude/projects/<project>/`), the watcher tails the transcript file and
   extracts assistant-message text deltas. This yields the *original Markdown
   source* — no box-table reverse-conversion, no repaint stripping, no settle
   heuristics — and streams deltas as they are appended. The adapter must be
   read-only, bounded per read, resilient to rotation/truncation, and must treat
   transcript content with the same privacy rules as pane content (never logged,
   never persisted).
2. **Capture adapter (fallback, V2 behavior)**: `tmux capture-pane` polling with the
   existing boundary heuristics, for agents without a usable transcript. Poll and
   settle parameters remain, but the emitted document now enters the incremental
   pipeline (D3), so an unchanged answer prefix costs nothing.

The watcher→viewer socket protocol is extended from whole-document messages to
`document` **plus** `append`/`replace-tail` delta messages (versioned; a V3 viewer
still accepts whole documents from a V2-style source). Frames stay length-prefixed,
bounded JSON.

### D6. Viewer places per-block images; the composite RGBA buffer is removed

`tmath agent-viewer` stops compositing answers into one growing RGBA image.
Instead it maintains the block list with one Kitty placement per block:

- New blocks are appended below the last placement (append is O(new block), not
  O(total history)).
- **Follow mode** (default): the viewport pins to the newest block, auto-scrolling
  as blocks are appended; any manual scroll input disengages follow; `F`/`End`
  re-engages it. This is the "blocks keep streaming into the pane" UX.
- Scrolling re-emits only placements whose visibility changed, using cached PNGs;
  it never crops or re-uploads a monolithic buffer.
- History is bounded: blocks scrolled beyond a configured history budget have their
  placements deleted and their cache entries evictable; scrolling back re-renders
  on demand.

### D7. Limits are re-based for the block model and HiDPI

All limits stay finite and enforced (inherited invariant). Changes:

- Per-image byte caps (`rawPngBytes`, transmitted-payload bytes) are defined **per
  block** and scaled by `deviceScaleFactor²` up to dpr 4, so Retina rendering is not
  starved by dpr-1 caps.
- New limits: blocks per document, cached blocks, cached pixels, in-flight tail
  renders (=1), transcript read bytes per poll, delta frames per second.
- `renderDurationMs` drops from 8,000 ms to 1,000 ms per block (native engines
  finish in single-digit ms; a second-long block render indicates pathology).
- Placement concurrency rises to cover per-block placement (one per visible block +
  history budget) with a total-pixel budget unchanged at 64 MiB.

### D8. Security and privacy posture of the native engine

- **No Typst package resolution and no network**: the embedded Typst world must be
  constructed with package imports disabled and no file-system root; the only
  reachable assets are fonts embedded in the binary. `#include`/`#read` style
  capabilities are unavailable to input by construction.
- **No markup injection**: user Markdown is parsed in Rust (`pulldown-cmark`,
  CommonMark + tables only) and converted into Typst **structured content through
  the library API, never by string-concatenating user text into Typst markup**.
  User text can therefore never be interpreted as Typst code. An acceptance test
  feeds Typst directives (`#eval`, `#read`, `#import`, show rules) through every
  Markdown context and asserts they render as literal text.
- RaTeX runs with error-on-unknown-command surfaced as the existing
  `invalid_latex` error path; macros remain disabled-equivalent (no user macro
  definitions expand beyond RaTeX's KaTeX-compatible defaults with bounded
  expansion).
- Logging, error records, hashing, and no-network invariants carry over verbatim
  from `AGENTS.md`; the new engine adds no durable state.
- Dependency risk: RaTeX is young (first release 2026-03, dominant single author).
  Pin exact versions of all `ratex-*` and `typst*` crates, vendor the lockfile, and
  keep the golden corpus (G7) as the regression tripwire. The MathJax+resvg sidecar
  remains the documented escape hatch if RaTeX stalls.

## Target Architecture

```
[ input: file | pipe | stdin stream | watch | agent source adapter ]
        │ incremental text
        ▼
[ tmath (single Rust binary) ]
  ├─ block splitter (streaming; port of scan-latex + pulldown-cmark)
  ├─ render engine (resident)
  │    ├─ RaTeX: math → SVG/PNG + baseline metrics
  │    ├─ Typst world: prose blocks (+ inline-math boxes) → PNG
  │    └─ render cache (hash → PNG + metrics, bounded LRU)
  ├─ placement planner (prefix reuse / append / tail replace)
  ├─ terminal surface (unchanged: kitty.rs, terminal.rs, mouse, scroll,
  │    tmux graphics routes, placement tracker)
  └─ agent watcher + viewer (delta socket protocol, per-block viewer)
        ▼
[ Kitty-graphics terminal: Ghostty / kitty / WezTerm; tmux routes unchanged ]
```

Everything below the render engine (Kitty escapes, probes, placement anchoring,
scroll state machine, tmux client-tty / DCS passthrough routes) is **kept from V2
unchanged**; V3 replaces the content pipeline above it.

## Latency Budget (end-to-end, warm, macOS arm64 reference machine)

| Stage | Budget p50 |
|---|---:|
| Delta ingest + block split | 2 ms |
| Cache lookup (unchanged blocks) | 0 ms (no work) |
| Render one new prose block (Typst) | 10 ms |
| Render one new display formula (RaTeX) | 3 ms |
| PNG encode | 3 ms |
| Kitty transmit + placeholder grid (typical block) | 10 ms |
| **Append end-to-end (G2)** | **≤ 50 ms** |

These budgets are enforced by `AT-3-9xx` performance acceptance tests with a real
measurement harness (replacing the dead `test:performance` script).

## Migration Plan

V2 keeps working until Phase 5. Sequencing:

1. Amend `AGENTS.md` (architecture section) and this spec's status as each phase
   lands (docs commit separate from implementation commit, per repo rules).
2. The native engine lands behind `tmath render --engine native` (default stays V2)
   until fidelity (G7) and performance (G1/G2) gates pass; then the default flips
   and `--engine node` remains one release as an escape hatch.
3. Phase 5 removes: `src/renderer`, `src/scanner`, `src/core` (TS), Playwright,
   sharp, KaTeX, markdown-it, highlight.js, the `postinstall` browser download,
   `TMATH_RENDER_WORKER`, and the Node requirement from `scripts/install.sh`.
   `package.json` shrinks to repository tooling only (or is removed if nothing
   remains).
4. CHANGELOG and compatibility docs updated with the new footprint and Linux
   status; V2 spec marked superseded (kept as reference, like V1).

## Phases

- **Phase 0 — Feasibility spikes (gate for everything else)**
  (a) RaTeX SVG inline embedding in Typst with correct baselines and wrapping;
  (b) embedded-font cold start ≤ 300 ms; (c) golden fidelity corpus: render the
  V2 test corpus + RaTeX coverage probe with both V2 and native engines, record
  visual diffs in `docs/evidence/`. Abort criteria: if (a) fails, fall back to
  MathJax+resvg sidecar design and revise this plan before proceeding.
- **Phase 1 — Native render crate** (`engine/crates/tmath-render`): block model,
  Rust scanner port, pulldown-cmark → Typst content mapping, RaTeX math, syntect
  code blocks, limits, error mapping to the existing `SafeErrorRecord` codes.
- **Phase 2 — Incremental pipeline**: streaming splitter, hash cache, placement
  planner (prefix reuse / append / tail replace), `tmath render -` stream mode,
  `tmath watch`.
- **Phase 3 — Viewer v2**: per-block placements, follow mode, visibility-driven
  re-emission, bounded history, removal of the composite RGBA path.
- **Phase 4 — Agent sources**: delta socket protocol, transcript adapter (Claude
  Code JSONL first), capture adapter rewired through the incremental pipeline,
  wrapper/allowlist unchanged.
- **Phase 5 — Node removal + portability**: delete the TS stack and browser
  install, single-binary packaging, Linux build + smoke evidence, install-size and
  cold-start gates.
- **Phase 6 — Hardening + release gate `0.3.0`**: fuzz the splitter and delta
  decoder, injection corpus, performance gates on the reference machine, real
  Ghostty (+ tmux) evidence, docs, release.

## Risk Register

- **Inline-math embedding in Typst** (baseline, wrapping, spacing) is unproven —
  hence Phase 0 gate with an explicit fallback.
- **RaTeX maturity / bus factor ~1**: pinned versions, golden corpus tripwire,
  documented MathJax+resvg fallback; budget for upstreaming fixes.
- **RaTeX coverage gaps** beyond the 22-case probe: the author's ">99.5% KaTeX
  coverage" claim is unaudited; the golden corpus must include the historically
  problematic commands from V1/V2 issue history, and unknown-command failures must
  fail closed per formula (render the block with the formula replaced by the
  bounded error badge, keep other blocks intact).
- **Typst API churn** (0.x): isolate behind a thin `engine::typst` module; pin.
- **Binary size** (~40–60 MB with embedded fonts): acceptable per G5, but font
  subsetting should be evaluated in Phase 5.
- **Per-block placeholder-grid cost**: many small placements instead of one big one
  — placement concurrency and pixel budgets re-based in D7; terminal behavior with
  100+ placements needs recorded evidence in Phase 3.
- **Transcript format drift** (Claude Code JSONL is not a public contract): the
  adapter must degrade gracefully to the capture adapter when parsing fails, and
  the format assumptions live in one module with fixtures.
- **PNG caps vs dpr 4 on tall blocks**: caps re-based per block in D7; Phase 1 must
  verify worst-case (4096-wide table at dpr 4) stays within transmit budgets.

## Definition of Done (V3)

1. `tmath render` and the agent viewer run with **no Node, no Chromium, no
   subprocess** on macOS arm64 and Linux x86_64.
2. G1–G7 acceptance tests pass on the reference machine with recorded evidence.
3. Streaming an answer into the viewer shows completed blocks incrementally with
   follow mode, while earlier blocks' placements are never re-transmitted.
4. Install is a single binary ≤ 60 MB; `scripts/install.sh` performs no browser or
   npm fetch.
5. All V2 privacy/security invariants hold, plus the new injection and no-package
   guarantees (D8), verified by tests.
6. `AGENTS.md`, `docs/architecture.md`, `docs/concept.md`, compatibility docs, and
   the V2 spec status are updated; V2 marked superseded.
