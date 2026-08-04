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
