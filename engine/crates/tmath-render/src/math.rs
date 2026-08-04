use std::io::Cursor;

use ratex_layout::{layout, to_display_list, LayoutOptions};
use ratex_parser::parser::parse;
use ratex_render::{render_to_png, RenderOptions as RatexRenderOptions};
use ratex_types::color::Color;
use ratex_types::math_style::MathStyle;

use crate::{
    limits::{render_guard, RenderDeadline},
    ErrorCode, Limits, RenderError, RenderOptions, SafeErrorRecord, DARK_THEME_TEXT_COLOR,
};

/// A transparent RaTeX raster and its logical baseline metrics.
#[derive(Clone, Debug, PartialEq)]
pub struct MathImage {
    pub png: Vec<u8>,
    pub width_px: u32,
    pub height_px: u32,
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
    let render_options = RatexRenderOptions {
        font_size: font_size_pt as f32,
        padding: 0.0,
        background_color: Color {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.0,
        },
        device_pixel_ratio: f32::from(options.device_pixel_ratio.clamp(1, 4)),
        ..RatexRenderOptions::default()
    };
    let png = render_to_png(&display_list, &render_options).map_err(invalid_latex_error)?;
    deadline.checkpoint()?;
    let (width_px, height_px) = png_dimensions(&png).map_err(|message| {
        RenderError::new(
            SafeErrorRecord {
                code: ErrorCode::RendererFailed,
                retryable: false,
                details: None,
            },
            message,
        )
    })?;
    let scaled_limits = limits.scaled(options.device_pixel_ratio);
    scaled_limits.check_image_width_px(width_px)?;
    scaled_limits.check_image_height_px(height_px)?;
    scaled_limits.check_image_pixels(u64::from(width_px) * u64::from(height_px))?;
    scaled_limits.check_raw_png_bytes(png.len() as u64)?;
    deadline.checkpoint()?;

    Ok(MathImage {
        png,
        width_px,
        height_px,
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

fn png_dimensions(png: &[u8]) -> Result<(u32, u32), &'static str> {
    let reader = png::Decoder::new(Cursor::new(png))
        .read_info()
        .map_err(|_| "RaTeX returned an invalid PNG")?;
    Ok((reader.info().width, reader.info().height))
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
    use std::io::Cursor;

    use super::*;
    use crate::{ErrorCode, SafeLimitKind};

    fn rgba_and_opaque_pixels(png_bytes: &[u8]) -> (bool, usize) {
        let mut decoder = png::Decoder::new(Cursor::new(png_bytes));
        decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::ALPHA);
        let mut reader = decoder.read_info().unwrap();
        let mut output = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut output).unwrap();
        let bytes = &output[..info.buffer_size()];
        match info.color_type {
            png::ColorType::Rgba => (
                bytes.chunks_exact(4).any(|pixel| pixel[3] == 0),
                bytes.chunks_exact(4).filter(|pixel| pixel[3] > 0).count(),
            ),
            png::ColorType::GrayscaleAlpha => (
                bytes.chunks_exact(2).any(|pixel| pixel[1] == 0),
                bytes.chunks_exact(2).filter(|pixel| pixel[1] > 0).count(),
            ),
            _ => (false, 0),
        }
    }

    #[test]
    fn display_formulas_are_transparent_and_scale_with_dpr() {
        for latex in [
            r"\sqrt{x}",
            r"\frac{a+b}{c+d}",
            r"\begin{aligned}a&=b+c\\d&=e-f\end{aligned}",
        ] {
            let dpr1 =
                render_formula(latex, true, &RenderOptions::new(480.0, 12.0, 1).unwrap()).unwrap();
            let dpr2 =
                render_formula(latex, true, &RenderOptions::new(480.0, 12.0, 2).unwrap()).unwrap();
            let (transparent1, opaque1) = rgba_and_opaque_pixels(&dpr1.png);
            let (transparent2, opaque2) = rgba_and_opaque_pixels(&dpr2.png);

            assert!(transparent1 && transparent2);
            assert!(opaque1 > 0 && opaque2 > 0);
            assert!((dpr2.width_px as f64 / dpr1.width_px as f64 - 2.0).abs() < 0.25);
            assert!((dpr2.height_px as f64 / dpr1.height_px as f64 - 2.0).abs() < 0.25);
            let opaque_ratio = opaque2 as f64 / opaque1 as f64;
            assert!((2.4..=5.5).contains(&opaque_ratio), "{opaque_ratio}");
        }
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
    fn oversized_formula_reports_the_scaled_pixel_limit() {
        let options = RenderOptions::new(480.0, 900.0, 1).unwrap();
        let mut latex = "x".to_owned();
        for _ in 0..32 {
            latex = format!(r"\frac{{x}}{{{latex}}}");
        }
        let limits = Limits {
            render_duration_ms: 60_000,
            ..Limits::default()
        };
        let deadline = RenderDeadline::new(limits.render_duration_ms);
        let error =
            render_formula_with_deadline(&latex, true, &options, &limits, &deadline).unwrap_err();
        assert_eq!(error.safe_record().code, ErrorCode::ImageTooLarge);
        assert_eq!(
            error.safe_record().details.as_ref().unwrap().limit_kind,
            Some(SafeLimitKind::ImagePixels)
        );
    }
}
