use ratex_layout::{layout, to_display_list, LayoutOptions};
use ratex_parser::parser::parse;
use ratex_svg::{render_to_svg, SvgOptions};
use ratex_types::color::Color;
use ratex_types::math_style::MathStyle;

use crate::{
    limits::{render_guard, RenderDeadline},
    ErrorCode, Limits, RenderError, RenderOptions, SafeErrorRecord, DARK_THEME_TEXT_COLOR,
};

/// A transparent RaTeX vector formula and its logical baseline metrics.
///
/// The formula is embedded into the composed Typst page as SVG rather than a
/// pre-rasterized PNG. A pre-rasterized bitmap gets resampled a second time
/// when Typst rasterizes the full page, and math boxes commonly sit at
/// fractional-pt baseline offsets, so that second resample bilinearly
/// smears the glyph strokes — prose stays sharp only because Typst
/// rasterizes its text vectors directly at final resolution. SVG glyph
/// outlines go through that same one-shot rasterization path as prose text
/// (see `typst_doc.rs::push_math_image`), so both come out equally sharp.
#[derive(Clone, Debug, PartialEq)]
pub struct MathImage {
    /// A standalone SVG document with glyphs embedded as `<path>` outlines
    /// (feature `embed-fonts`/`standalone` on `ratex-svg`), never `<text>`
    /// elements with `font-family` references — Typst cannot resolve
    /// external font names from SVG, so a `<text>`-based SVG would compile
    /// with invisible or substituted glyphs.
    pub svg: Vec<u8>,
    pub width_pt: f64,
    pub height_pt: f64,
    pub depth_pt: f64,
}

/// Renders one LaTeX formula with RaTeX.
pub fn render_formula(
    latex: &str,
    display: bool,
    options: &RenderOptions,
) -> Result<MathImage, RenderError> {
    // Queueing for the resident engine is not charged to the block's execution
    // budget; the cooperative deadline starts once this render owns the engine.
    let _guard = render_guard()?;
    let limits = Limits::default();
    let deadline = RenderDeadline::new(limits.render_duration_ms);
    render_formula_with_deadline(latex, display, options, &limits, &deadline)
}

pub(crate) fn render_formula_with_deadline(
    latex: &str,
    display: bool,
    options: &RenderOptions,
    limits: &Limits,
    deadline: &RenderDeadline,
) -> Result<MathImage, RenderError> {
    let ast = parse(latex).map_err(invalid_latex_error)?;
    let layout_options = LayoutOptions {
        style: if display {
            MathStyle::Display
        } else {
            MathStyle::Text
        },
        color: theme_text_color(),
        ..LayoutOptions::default()
    };
    let layout_box = layout(&ast, &layout_options);
    let display_list = to_display_list(&layout_box);
    deadline.checkpoint()?;
    let font_size_pt = options.font_size_pt;
    let width_pt = display_list.width * font_size_pt;
    let height_pt = display_list.height * font_size_pt;
    let depth_pt = display_list.depth * font_size_pt;
    let svg_options = SvgOptions {
        font_size: font_size_pt,
        padding: 0.0,
        embed_glyphs: true,
        ..SvgOptions::default()
    };
    let svg = render_to_svg(&display_list, &svg_options).into_bytes();
    deadline.checkpoint()?;
    // The formula has no pixel dimensions to check (it is vector, not
    // raster); its size is bounded on SVG byte length alone. The composed
    // page's rasterized PNG still goes through the unchanged
    // `check_image_width_px`/`check_image_height_px`/`check_image_pixels`
    // checks in `prose.rs` once Typst renders the full page.
    let scaled_limits = limits.scaled(options.device_pixel_ratio);
    scaled_limits.check_math_svg_bytes(svg.len() as u64)?;
    deadline.checkpoint()?;

    Ok(MathImage {
        svg,
        width_pt,
        height_pt,
        depth_pt,
    })
}

fn theme_text_color() -> Color {
    let bytes = DARK_THEME_TEXT_COLOR.as_bytes();
    debug_assert_eq!(bytes.len(), 7);
    let channel = |start| {
        let value = u8::from_str_radix(&DARK_THEME_TEXT_COLOR[start..start + 2], 16)
            .expect("fixed theme color must be valid hexadecimal");
        f32::from(value) / 255.0
    };
    Color {
        r: channel(1),
        g: channel(3),
        b: channel(5),
        a: 1.0,
    }
}

fn invalid_latex_error(error: impl std::fmt::Display) -> RenderError {
    RenderError::new(
        SafeErrorRecord {
            code: ErrorCode::InvalidLatex,
            retryable: false,
            details: None,
        },
        format!("RaTeX failed: {error}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ErrorCode, SafeLimitKind};

    /// Confirms the SVG carries real vector outlines, not KaTeX `<text>`
    /// elements referencing external `font-family` names that Typst cannot
    /// resolve (see the `MathImage::svg` doc comment). A `<path>` element is
    /// the outline-embedding signal from `ratex-svg`'s `standalone` glyph
    /// path; a stray `<text>`/`font-family` would mean glyphs silently fell
    /// back to the unresolvable KaTeX webfont reference instead.
    fn assert_is_well_formed_outline_svg(svg_bytes: &[u8]) {
        let svg = std::str::from_utf8(svg_bytes).expect("SVG must be valid UTF-8");
        assert!(svg.starts_with("<svg "), "not an SVG document: {svg}");
        assert!(svg.trim_end().ends_with("</svg>"), "unterminated SVG");
        assert!(
            !svg.contains("<text"),
            "glyphs fell back to <text> elements instead of embedded outlines: {svg}"
        );
        assert!(
            !svg.contains("font-family"),
            "SVG references an external font-family Typst cannot resolve: {svg}"
        );
        assert!(
            svg.contains("<path"),
            "expected at least one glyph outline <path>: {svg}"
        );
    }

    #[test]
    fn display_formulas_embed_as_well_formed_outline_svg_at_every_dpr() {
        for latex in [
            r"\sqrt{x}",
            r"\frac{a+b}{c+d}",
            r"\begin{aligned}a&=b+c\\d&=e-f\end{aligned}",
        ] {
            for dpr in [1, 2] {
                let image =
                    render_formula(latex, true, &RenderOptions::new(480.0, 12.0, dpr).unwrap())
                        .unwrap();
                assert_is_well_formed_outline_svg(&image.svg);
                assert!(image.width_pt > 0.0, "{latex} at dpr {dpr}");
                assert!(image.height_pt > 0.0, "{latex} at dpr {dpr}");
            }
        }
    }

    #[test]
    fn svg_bytes_are_identical_across_device_pixel_ratios() {
        // Unlike the old PNG raster, SVG is device-pixel-ratio independent:
        // it is Typst's page rasterization (unchanged, still DPR-scaled)
        // that produces the final pixels. The vector source and the logical
        // metrics must be identical regardless of DPR.
        let latex = r"\frac{a+b}{c+d}";
        let dpr1 =
            render_formula(latex, true, &RenderOptions::new(480.0, 12.0, 1).unwrap()).unwrap();
        let dpr2 =
            render_formula(latex, true, &RenderOptions::new(480.0, 12.0, 2).unwrap()).unwrap();
        assert_eq!(dpr1.svg, dpr2.svg);
        assert_eq!(dpr1.width_pt, dpr2.width_pt);
        assert_eq!(dpr1.height_pt, dpr2.height_pt);
        assert_eq!(dpr1.depth_pt, dpr2.depth_pt);
    }

    #[test]
    fn invalid_latex_is_safe_and_does_not_serialize_input() {
        let input = r"\frac{PRIVATE_FRAGMENT";
        let error = render_formula(input, false, &RenderOptions::default()).unwrap_err();
        assert_eq!(error.safe_record().code, ErrorCode::InvalidLatex);
        assert!(!error.safe_record().retryable);
        let json = serde_json::to_string(error.safe_record()).unwrap();
        assert!(!json.contains("PRIVATE_FRAGMENT"));
        assert!(!json.contains("frac"));
    }

    #[test]
    fn oversized_formula_reports_the_scaled_svg_byte_limit() {
        // SVG glyph-outline markup is far more compact per formula than a
        // rasterized PNG, so a formula that used to blow the old pixel cap
        // does not reliably blow a realistic SVG byte cap. Set a tight
        // `math_svg_bytes` limit directly instead, so the test verifies the
        // check itself rather than depending on formula complexity to
        // organically exceed the (generous, real-world-sized) default.
        let options = RenderOptions::new(480.0, 12.0, 1).unwrap();
        let latex = r"\frac{a+b}{c+d}";
        let limits = Limits {
            render_duration_ms: 60_000,
            math_svg_bytes: 16,
            ..Limits::default()
        };
        let deadline = RenderDeadline::new(limits.render_duration_ms);
        let error =
            render_formula_with_deadline(latex, true, &options, &limits, &deadline).unwrap_err();
        assert_eq!(error.safe_record().code, ErrorCode::ImageTooLarge);
        assert_eq!(
            error.safe_record().details.as_ref().unwrap().limit_kind,
            Some(SafeLimitKind::MathSvgBytes)
        );
    }
}
