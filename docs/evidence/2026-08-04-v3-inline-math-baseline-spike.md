# Evidence: RaTeX Inline Math Baselines in Typst (AT-3-001)

- Date: August 4, 2026
- Environment: macOS arm64 (Darwin 25.5.0), Rust stable, offline build
- Pinned dependencies: `ratex-*` 0.1.14, `typst-as-lib` 0.14.4 (typst 0.13.1), `png` 0.18
- Harness: `scripts/experiments/native-engine-spike` (commit `9c263dd`)
- Reproduce: `cargo test --offline` inside the spike crate; rendered pages and
  `summary.json` are written to the crate-local `out/` directory (not committed).

## Result: PASS

RaTeX-rendered formulas embed as inline Typst boxes
(`box(width, height: height+depth, baseline: depth, image(...))`) with the RaTeX
`DisplayList` em metrics scaled by the text size (1 em = 12 pt at 12 pt text).

Measured signed baseline offsets (math baseline minus text baseline, device px):

| Probe | Formula | dpr 1 | dpr 2 | Threshold |
|---|---|---:|---:|---|
| Ascender-heavy | `\hat{A}^{2^{x}}` | 0 | -1 | ±1 / ±2 |
| Descender-heavy | `\sqrt{y_{j_{q}}}` | 0 | 0 | ±1 / ±2 |
| Plain | `x+y` | 0 | 0 | ±1 / ±2 |

Wrapping probe: 12 inline math boxes at page width 480 pt wrapped to 2 lines;
1,155 of 1,176 expected math-colored opaque pixels survived composition
(ratio 0.9821, threshold ≥ 0.95). Visual inspection of the composed pages
confirms glyph-level baseline alignment and un-clipped wrapping.

## Method

- Math renders in a distinct color; a green rule is injected into a copy of the
  RaTeX display list across `y = height` (the math baseline). Red 1 pt Typst
  marker boxes cross the text baseline. The test decodes the composed PNG and
  compares the independently detected red and green marker rows. A guard asserts
  the detected rows differ between probes (no fixed-row shortcut).
- Line count and clipping are measured from marker-row grouping and opaque-pixel
  survival with alpha/color thresholds (no fragile byte equality).

## Caveats carried into Phase 1

1. The text baseline is located via Typst marker boxes placed with
   `box(baseline:)`, i.e. the measurement validates the RaTeX-metric-to-box
   mapping and relies on Typst's box-baseline semantics for the glyph baseline;
   visual inspection confirms glyph alignment, but a glyph-pixel-derived
   baseline check would make the acceptance test fully self-contained.
2. The composed pages embed RaTeX output as PNG. The pinned `ratex-svg` default
   feature set emits `<text>` nodes referencing KaTeX-family fonts, which
   Typst's embedded font set cannot resolve; standalone/vector embedding needs
   the `ratex-svg` `standalone`/`embed-fonts` features (a deliberate Phase 1
   decision, not a blocker).
3. The dpr 2 ascender offset of -1 px comes from raster quantization
   (integer-pixel RaTeX canvas stretched to an exact pt box); within threshold.
