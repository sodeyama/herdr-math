# T3-001 RaTeX Inline Math Spike

## Result

AT-3-001 passes with RaTeX PNG boxes embedded inline in a Typst paragraph.
The measured baseline offsets are within the required tolerance, and a paragraph
containing 12 math boxes wraps to two lines without clipping.

| Probe | Formula | DPR 1 offset | DPR 2 offset |
|---|---|---:|---:|
| Ascender-heavy | `\hat{A}^{2^{x}}` | 0 px | -1 px |
| Descender-heavy | `\sqrt{y_{j_{q}}}` | 0 px | 0 px |
| Plain | `x+y` | 0 px | 0 px |

The wrapping probe retained 1,155 of 1,176 expected math-colored opaque pixels,
for a ratio of 98.2143%. It produced two detected baseline-marker groups, which
correspond to two paragraph lines.

## RaTeX metrics

`ratex-layout` exposes these public fields on `LayoutBox`:

- `width`: horizontal extent in em
- `height`: ascent above the baseline in em
- `depth`: descent below the baseline in em

`ratex-layout::to_display_list` produces a `DisplayList` with public `width`,
`height`, and `depth` fields in the same em coordinate system. Its baseline is
`y = height`. `DisplayList::total_height()` returns `height + depth`. There is no
separate public scalar named `baseline`; the baseline is derived from `height`
when coordinates start at the top.

For the three probes, `LayoutBox` and `DisplayList` metrics were equal. The exact
values from the successful run are recorded in `out/summary.json`.

`ratex-svg` does not return a separate metrics object. `render_to_svg` and
`render_to_svg_with_color_syntax` accept the `DisplayList`, multiply its em
metrics by `SvgOptions::font_size`, add `SvgOptions::padding`, and emit SVG
`viewBox`, `width`, and `height`. The SVG baseline remains
`padding + DisplayList.height * font_size`.

## Scaling and composition

The Typst paragraph uses 12 pt `NewCM10` text from Typst's embedded font assets.
RaTeX uses `MathStyle::Text`. The conversion is:

```text
1 RaTeX em = 12 Typst pt
width_pt = display_list.width * 12
height_pt = display_list.height * 12
depth_pt = display_list.depth * 12
```

Each inline asset is composed as:

```typst
#box(
  width: width_pt,
  height: height_pt + depth_pt,
  baseline: depth_pt,
  image("math-...-dprN.png",
    width: width_pt,
    height: height_pt + depth_pt,
    fit: "stretch"),
)
```

Typst's box baseline is measured upward from the bottom of the box, so setting
it to the RaTeX depth aligns the RaTeX baseline with the Typst text baseline.
Typst renders at 1.0 and 2.0 pixels per point. Matching RaTeX PNG assets are
rendered with `font_size = 12` and `device_pixel_ratio = 1` or `2`.

## File resolver

The binary uses `TypstEngine::builder().with_static_file_resolver(...)`. The
resolver receives the in-memory PNG byte arrays under virtual names such as
`math-ascender-dpr2.png`. Typst source references those names with `image(...)`.
No file-system resolver or package resolver is installed. Font discovery uses
`TypstKitFontOptions` with `include_system_fonts(false)` and
`include_embedded_fonts(true)`.

## Automated pixel measurements

The baseline pages contain lowercase `x` runs adjacent to each math box. Two
short red Typst boxes cross the text baseline. A green RaTeX rule is injected
into a copy of the RaTeX `DisplayList` at `y = display_list.height`, so it crosses
the math baseline before the asset is embedded. The test decodes the composed
PNG with the `png` crate, locates the red and green marker rows independently,
and reports their signed vertical difference. Marker positions vary between
probes because their ascents and depths change the paragraph line box, which
also guards against measuring one hardcoded row.

The wrapping probe renders math glyphs in blue and text plus in-memory baseline
markers in black/red. The test counts blue opaque pixels in the composed page
and compares them with the sum from the standalone transparent RaTeX PNGs. It
also groups the red marker rows to count wrapped lines.

## Fidelity caveats

- Pixel positions are quantized during RaTeX rasterization, Typst image
  placement, and Typst page rasterization. The DPR 2 ascender probe therefore
  measures -1 px while the other offsets are 0 px.
- The clipping comparison uses alpha and color thresholds to tolerate
  anti-aliasing and resampling. It does not require fragile full-byte equality.
- SVG output is generated and retained to document RaTeX's vector geometry and
  metrics. The composed acceptance images use PNG because the pinned
  `ratex-svg` dependency does not enable its `standalone`/`embed-fonts` feature:
  its default SVG glyph path emits KaTeX-family `<text>` nodes, and Typst does
  not have those fonts in its embedded set. PNG embedding is self-contained and
  passes the acceptance thresholds. Enabling standalone SVG would require a
  Cargo feature change outside the pinned dependency contract for this task.

## T3-002 cold start

AT-3-002 passes on the release build. The measurement integration test builds
`coldstart` once with `cargo build --release --offline --bin coldstart`, performs
one unmeasured warmup spawn to populate filesystem and executable caches, and
then records 10 fresh process runs. Wall-clock timing starts immediately before
`Command::output()` spawns the child and ends after child exit, so it includes OS
process startup and stdout capture. The in-process timer starts at the first
statement of `main`.

| Run | Wall-clock | Engine build | First render | In-process total |
|---:|---:|---:|---:|---:|
| 1 | 8.752 ms | 2.310 ms | 2.167 ms | 4.516 ms |
| 2 | 8.146 ms | 2.239 ms | 1.991 ms | 4.267 ms |
| 3 | 8.616 ms | 2.360 ms | 2.246 ms | 4.655 ms |
| 4 | 8.671 ms | 2.295 ms | 2.224 ms | 4.561 ms |
| 5 | 8.895 ms | 2.372 ms | 2.173 ms | 4.600 ms |
| 6 | 8.696 ms | 2.322 ms | 2.111 ms | 4.471 ms |
| 7 | 10.134 ms | 3.075 ms | 2.687 ms | 5.829 ms |
| 8 | 10.041 ms | 3.103 ms | 2.304 ms | 5.466 ms |
| 9 | 9.791 ms | 2.710 ms | 2.420 ms | 5.177 ms |
| 10 | 10.243 ms | 2.498 ms | 2.283 ms | 4.834 ms |

| Metric | p50 | p95 |
|---|---:|---:|
| Wall-clock | 8.824 ms | 10.194 ms |
| In-process total | 4.627 ms | 5.666 ms |

The representative first block contains a bold Typst paragraph with one inline
RaTeX PNG box and a RaTeX display formula below it. Both RaTeX assets are
produced and kept in memory, resolved by Typst's static file resolver, and the
final transparent PNG is encoded in memory at DPR 2. Every measured run emitted
19,177 PNG bytes. The engine configuration exposes 17 embedded Typst font faces,
and the compiled document was also checked to contain at least one used text
font face.

The `engine_build_ms` phase starts at process entry and includes RaTeX parsing,
layout, PNG rasterization, Typst source construction, static resolver setup, and
embedded-font engine construction. `first_render_ms` covers Typst compilation,
font-use verification, page rasterization, and final PNG encoding. The remaining
wall-clock difference is executable loading, dynamic loader/runtime startup,
stdout I/O, and child teardown.

System-font exclusion is structural rather than syscall-traced because this
sandbox has no filesystem tracing facility. Both spike binaries use the same
`embedded_font_options()` construction site, which sets
`include_system_fonts(false)`, supplies an empty explicit font-directory list,
and sets `include_embedded_fonts(true)`. Debug assertions guard those invariants
before `TypstKitFontOptions` is passed to the engine. No filesystem resolver is
installed for the formula assets.

Caveats:

- The acceptance threshold applies to the release binary; debug performance is
  not representative and is not used for the assertion.
- The warmup result was 635.629 ms wall-clock and 14.182 ms in-process. It is
  excluded from the 10 samples because AT-3-002 specifies warm OS caches and
  first-run filesystem/executable cache effects are substantial on this host.
- Results are host- and load-dependent. The machine-readable evidence is written
  to `out/coldstart-summary.json` on every integration-test run.

## T3-003 golden corpus run

31 cases were rendered by both engines: V2 ok 29, native ok 31, both ok 29.
The native engine rendered every case, including both cases the V2 renderer
rejects (`\ce` from mhchem and `\pu`, which V2 reports as `invalid_latex`).
Artifacts live in `out/golden/` (per-engine PNGs, side-by-side `-pair.png`
composites on a dark backdrop, and `index.json`).

| Case | V2 | Native | V2 size | Native size |
|---|---|---|---|---|
| power-inline | ok | ok | 960x48 | 93x20 |
| fraction-and-root | ok | ok | 960x130 | 215x88 |
| sum-and-integral | ok | ok | 960x168 | 191x141 |
| aligned-equations | ok | ok | 960x78 | 199x66 |
| matrix | ok | ok | 960x68 | 124x58 |
| greek-letters | ok | ok | 960x124 | 134x80 |
| unicode-math | ok | ok | 960x44 | 204x22 |
| multiline-gathered | ok | ok | 960x76 | 169x65 |
| probe-01-align | ok | ok | 960x76 | 95x65 |
| probe-02-gather | ok | ok | 960x76 | 58x65 |
| probe-03-cases | ok | ok | 960x86 | 192x73 |
| probe-04-array | ok | ok | 960x68 | 74x58 |
| probe-05-alphabets | ok | ok | 960x36 | 58x22 |
| probe-06-text | ok | ok | 960x36 | 131x18 |
| probe-07-binom | ok | ok | 960x68 | 50x58 |
| probe-08-overbrace | ok | ok | 960x60 | 53x47 |
| probe-09-stackrel | ok | ok | 960x42 | 75x31 |
| probe-10-substack | ok | ok | 960x86 | 68x76 |
| probe-11-big-delimiters | ok | ok | 960x52 | 48x45 |
| probe-12-xrightarrow | ok | ok | 960x36 | 86x26 |
| probe-13-textcolor | ok | ok | 960x36 | 56x19 |
| probe-14-mhchem | `invalid_latex` | ok | -x- | 70x20 |
| probe-15-physical-units | `invalid_latex` | ok | -x- | 116x24 |
| probe-16-cancel | ok | ok | 960x36 | 56x20 |
| probe-17-boxed | ok | ok | 960x44 | 109x38 |
| probe-18-equation-tag | ok | ok | 960x34 | 113x29 |
| probe-19-cjk-text | ok | ok | 960x44 | 192x23 |
| probe-20-nested-fraction | ok | ok | 960x76 | 86x65 |
| probe-21-product | ok | ok | 960x84 | 130x75 |
| probe-22-vmatrix | ok | ok | 960x68 | 66x58 |
| document-markdown | ok | ok | 960x630 | 840x388 |

Factual observations (no accept/reject judgment here; see the evidence doc):

- Font flavor differs: V2 uses the KaTeX font family, the native path uses
  RaTeX's Computer Modern set; glyph shapes are close but not identical.
- In this harness run V2 rendered `bmatrix`/`cases`/`vmatrix` with unstretched
  delimiters, while the native output stretches them correctly.
- The document sample differs in styling: V2 uses a sans-serif GitHub-dark
  theme, the native sample uses NewCM serif and Typst default table strokes;
  structure (heading, bold, inline math, list, table, highlighted code) matches.
- Native stacked output is transparent; a supervisor fix replaced the initial
  alpha-discarding blend with source-over compositing that preserves alpha.
