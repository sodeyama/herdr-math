# Evidence: Golden Corpus, V2 vs Native Engine (AT-3-003)

- Date: August 4, 2026
- Environment: macOS arm64 (Darwin 25.5.0), offline build, Node 22.21.1 for the
  V2 side
- Harness: `scripts/experiments/native-engine-spike` `golden` binary/test
  (commit `ddc9535`); per-case table in the spike `REPORT.md`
- Reproduce: `cargo test --offline --test golden` inside the spike crate;
  artifacts regenerate under the crate-local `out/golden/` (per-engine PNGs,
  side-by-side composites, `index.json`).

## Result: PASS — native renders 31/31 cases; every divergence accepted

Corpus: 9 V2 `formula-corpus.json` valid cases + 22 coverage probes + 1
Markdown document sample = 31 cases rendered by both engines.

| Engine | ok | failed |
|---|---:|---:|
| Native (RaTeX + Typst) | 31 | 0 |
| V2 (KaTeX + Chromium) | 29 | 2 (`\ce` mhchem, `\pu` → `invalid_latex`) |

## Divergence review (accept/reject)

Each divergence was reviewed on the side-by-side composites by the supervising
agent; screenshots are reproducible from the harness.

1. **Font flavor** — V2 uses the KaTeX font family, native uses RaTeX's
   Computer Modern set; shapes are close, metrics differ slightly.
   **Accepted**: V3 does not promise glyph-identical output, only KaTeX-grade
   typesetting (plan G7).
2. **Stretchy delimiters (`bmatrix`, `cases`, `vmatrix`)** — in this harness
   run V2 rendered *unstretched* delimiters while native stretched them
   correctly. **Accepted** (native is the better rendering). The V2 behavior
   is recorded as a separate observation; it does not block the native
   engine and disappears with V2's removal in Phase 5.
3. **mhchem/`\pu` coverage** — V2 fails these as `invalid_latex`; native
   renders them. **Accepted**: native is a strict superset on this corpus.
4. **Document styling** — V2: sans-serif GitHub-dark theme; native sample:
   NewCM serif with Typst default table strokes. Structural parity holds
   (heading, bold, inline math on-baseline, list, table, syntect-highlighted
   code). **Accepted for the spike**; Phase 1 must define the target
   typography (font choice, table strokes, code theme) as part of the
   markdown mapping task, not as a fidelity gap.

No divergence was rejected; the default-engine flip gate (AT-3-206) will
re-run this corpus against the Phase 1 implementation.

## Supervisor corrections during review

- Native glyph color was initially RaTeX's default black, making the dark
  composites unreadable; it now matches V2's fixed dark-theme text color.
- The harness's compositing blend discarded alpha (forced 255), turning
  stacked native output opaque; rewritten as alpha-preserving source-over.
  Verified after the fix: 17,002/18,920 pixels fully transparent on the
  representative stacked case.

## Phase 0 decision (T3-004)

All three Phase 0 gates passed: inline baselines within threshold (AT-3-001),
cold start ~9-12 ms p50 vs the 300 ms budget (AT-3-002), and golden corpus
parity with zero rejected divergences (AT-3-003). **Decision: proceed with the
RaTeX + Typst native engine per plan D1/D2; the MathJax+resvg fallback is not
needed.** Carried-forward items: `ratex-svg` standalone/vector embedding
evaluation, glyph-pixel-derived baseline measurement, fs-traced font-scan
verification, and target document typography — all owned by Phase 1 tasks.
