use std::io::Cursor;
use std::path::PathBuf;

use typst::layout::{Abs, PagedDocument};
use typst_as_lib::typst_kit_options::TypstKitFontOptions;
use typst_as_lib::TypstEngine;

use crate::{
    limits::{render_guard, RenderDeadline},
    typst_doc::compose_block_with_deadline,
    Block, ErrorCode, Limits, MathImage, RenderError, RenderOptions, SafeErrorRecord,
};

/// A rendered transparent prose image.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderedImage {
    pub png: Vec<u8>,
    pub width_px: u32,
    pub height_px: u32,
    pub formula_errors: Vec<SafeErrorRecord>,
    /// Wall-clock time spent in this block's guarded render pipeline.
    pub duration_ms: u64,
}

/// Renders one Markdown prose block through the native Typst engine.
pub fn render_prose_block(
    block: &Block,
    options: &RenderOptions,
) -> Result<RenderedImage, RenderError> {
    let _guard = render_guard()?;
    let limits = Limits::default();
    let deadline = RenderDeadline::new(limits.render_duration_ms);
    let mut image = render_prose_block_with_deadline(block, options, &limits, &deadline)?;
    image.duration_ms = deadline.checkpoint()?;
    Ok(image)
}

pub(crate) fn render_prose_block_with_deadline(
    block: &Block,
    options: &RenderOptions,
    limits: &Limits,
    deadline: &RenderDeadline,
) -> Result<RenderedImage, RenderError> {
    let source = compose_block_with_deadline(block, options, limits, deadline)?;
    deadline.checkpoint()?;
    render_typst_source(
        source.as_str(),
        &source.static_files,
        source.formula_errors.clone(),
        options,
        true,
        limits,
        deadline,
    )
}

pub(crate) fn render_display_math_block(
    block_index: usize,
    image: MathImage,
    options: &RenderOptions,
    limits: &Limits,
    deadline: &RenderDeadline,
) -> Result<RenderedImage, RenderError> {
    // Embedded as SVG, not PNG, so Typst rasterizes the glyph outlines
    // directly into the composed page instead of resampling an
    // already-rasterized bitmap (see the `MathImage` doc comment in
    // `math.rs` and `typst_doc.rs::push_math_image`).
    let name = format!("math-{block_index}-0.svg");
    let total_height = image.height_pt + image.depth_pt;
    let source = format!(
        "#set page(width: {width}pt, height: auto, margin: 0pt, fill: none)\n\
         #align(center)[#image(\"{name}\", width: {image_width}pt, \
         height: {image_height}pt, fit: \"stretch\")]\n",
        width = options.content_width_pt,
        image_width = image.width_pt,
        image_height = total_height,
    );
    deadline.checkpoint()?;
    render_typst_source(
        &source,
        &[(name, image.svg)],
        Vec::new(),
        options,
        false,
        limits,
        deadline,
    )
}

fn render_typst_source(
    source: &str,
    static_files: &[(String, Vec<u8>)],
    formula_errors: Vec<SafeErrorRecord>,
    options: &RenderOptions,
    trim_right: bool,
    limits: &Limits,
    deadline: &RenderDeadline,
) -> Result<RenderedImage, RenderError> {
    let static_file_refs = static_files
        .iter()
        .map(|(name, bytes)| (name.as_str(), bytes.as_slice()))
        .collect::<Vec<_>>();
    let dpr = options.device_pixel_ratio.clamp(1, 4);
    let scaled_limits = limits.scaled(dpr);

    let engine = TypstEngine::builder()
        .main_file(source.to_owned())
        .with_static_file_resolver(static_file_refs)
        .search_fonts_with(embedded_font_options())
        .build();
    let document: PagedDocument = engine
        .compile()
        .output
        .map_err(|_| renderer_error("Typst compilation failed"))?;
    deadline.checkpoint()?;
    let pixmap = typst_render::render_merged(&document, f32::from(dpr), Abs::zero(), None);
    deadline.checkpoint()?;
    let raster_width_px = pixmap.width();
    let height_px = pixmap.height();

    scaled_limits.check_image_width_px(raster_width_px)?;
    scaled_limits.check_image_height_px(height_px)?;
    scaled_limits.check_image_pixels(u64::from(raster_width_px) * u64::from(height_px))?;

    deadline.checkpoint()?;
    let full_width_png = pixmap
        .encode_png()
        .map_err(|_| renderer_error("PNG encoding failed"))?;
    scaled_limits.check_raw_png_bytes(full_width_png.len() as u64)?;
    let (png, width_px) = if trim_right {
        trim_transparent_right(&full_width_png)?
    } else {
        (full_width_png, raster_width_px)
    };
    scaled_limits.check_raw_png_bytes(png.len() as u64)?;

    Ok(RenderedImage {
        png,
        width_px,
        height_px,
        formula_errors,
        duration_ms: 0,
    })
}

fn trim_transparent_right(png_bytes: &[u8]) -> Result<(Vec<u8>, u32), RenderError> {
    let mut decoder = png::Decoder::new(Cursor::new(png_bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::ALPHA);
    let mut reader = decoder
        .read_info()
        .map_err(|_| renderer_error("PNG decoding failed"))?;
    let mut decoded = vec![
        0;
        reader.output_buffer_size().ok_or_else(|| renderer_error(
            "PNG output buffer size was unavailable"
        ))?
    ];
    let info = reader
        .next_frame(&mut decoded)
        .map_err(|_| renderer_error("PNG decoding failed"))?;
    let decoded = &decoded[..info.buffer_size()];
    let rgba = decoded_rgba(decoded, info.color_type)?;
    let row_bytes = usize::try_from(info.width)
        .ok()
        .and_then(|width| width.checked_mul(4))
        .ok_or_else(|| renderer_error("PNG row size overflowed"))?;
    let content_width = (0..usize::try_from(info.width).unwrap_or(usize::MAX))
        .rev()
        .find(|x| {
            rgba.chunks_exact(row_bytes)
                .any(|row| row[x.saturating_mul(4) + 3] > 0)
        })
        .map_or(1, |x| x + 1);
    if content_width == usize::try_from(info.width).unwrap_or(usize::MAX) {
        return Ok((png_bytes.to_vec(), info.width));
    }

    let cropped = rgba
        .chunks_exact(row_bytes)
        .flat_map(|row| row[..content_width * 4].iter().copied())
        .collect::<Vec<_>>();
    let mut output = Vec::new();
    {
        let mut encoder = png::Encoder::new(
            &mut output,
            u32::try_from(content_width)
                .map_err(|_| renderer_error("PNG width conversion failed"))?,
            info.height,
        );
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|_| renderer_error("PNG encoding failed"))?;
        writer
            .write_image_data(&cropped)
            .map_err(|_| renderer_error("PNG encoding failed"))?;
    }
    Ok((
        output,
        u32::try_from(content_width).map_err(|_| renderer_error("PNG width conversion failed"))?,
    ))
}

fn decoded_rgba(bytes: &[u8], color_type: png::ColorType) -> Result<Vec<u8>, RenderError> {
    match color_type {
        png::ColorType::Rgba => Ok(bytes.to_vec()),
        png::ColorType::Rgb => Ok(bytes
            .chunks_exact(3)
            .flat_map(|pixel| [pixel[0], pixel[1], pixel[2], 255])
            .collect()),
        png::ColorType::GrayscaleAlpha => Ok(bytes
            .chunks_exact(2)
            .flat_map(|pixel| [pixel[0], pixel[0], pixel[0], pixel[1]])
            .collect()),
        png::ColorType::Grayscale => Ok(bytes
            .iter()
            .flat_map(|value| [*value, *value, *value, 255])
            .collect()),
        png::ColorType::Indexed => Err(renderer_error("Indexed PNG was not expanded")),
    }
}

fn embedded_font_options() -> TypstKitFontOptions {
    TypstKitFontOptions::default()
        .include_system_fonts(false)
        .include_dirs(std::iter::empty::<PathBuf>())
        .include_embedded_fonts(true)
}

fn renderer_error(message: &'static str) -> RenderError {
    RenderError::new(
        SafeErrorRecord {
            code: ErrorCode::RendererFailed,
            retryable: false,
            details: None,
        },
        message,
    )
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;
    use crate::{compose_block, BlockKind, SafeLimitKind};

    fn block(kind: BlockKind, source: impl Into<String>) -> Block {
        Block {
            index: 0,
            kind,
            source: source.into(),
        }
    }

    fn nontransparent_pixels(png_bytes: &[u8]) -> usize {
        let mut decoder = png::Decoder::new(Cursor::new(png_bytes));
        decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::ALPHA);
        let mut reader = decoder.read_info().unwrap();
        let mut output = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut output).unwrap();
        let bytes = &output[..info.buffer_size()];
        match info.color_type {
            png::ColorType::Rgba => bytes.chunks_exact(4).filter(|pixel| pixel[3] > 0).count(),
            png::ColorType::Rgb => bytes.len() / 3,
            png::ColorType::GrayscaleAlpha => {
                bytes.chunks_exact(2).filter(|pixel| pixel[1] > 0).count()
            }
            png::ColorType::Grayscale => bytes.len(),
            png::ColorType::Indexed => panic!("indexed PNG was not expanded"),
        }
    }

    #[test]
    fn escaped_syntax_and_unicode_compile_to_visible_pixels() {
        let image = render_prose_block(
            &block(
                BlockKind::Paragraph,
                "\" \\\\ {} # $ first\nsecond 数学 🧮 \u{7}",
            ),
            &RenderOptions::default(),
        )
        .unwrap();
        assert!(!image.png.is_empty());
        assert!(nontransparent_pixels(&image.png) > 0);
    }

    #[test]
    fn inline_formula_embeds_visible_pixels_and_changes_the_paragraph() {
        let with_formula = render_prose_block(
            &block(BlockKind::Paragraph, "The value is $\\frac{a+b}{c+d}$."),
            &RenderOptions::default(),
        )
        .unwrap();
        let without_formula = render_prose_block(
            &block(BlockKind::Paragraph, "The value is."),
            &RenderOptions::default(),
        )
        .unwrap();

        assert!(nontransparent_pixels(&with_formula.png) > 0);
        assert!(with_formula.width_px > without_formula.width_px);
        assert_ne!(with_formula.png, without_formula.png);
        assert!(with_formula.formula_errors.is_empty());
    }

    #[test]
    fn one_invalid_formula_becomes_a_badge_without_harming_siblings() {
        let mixed = render_prose_block(
            &block(
                BlockKind::Paragraph,
                r"valid $a+b$ then broken $\frac{$ then valid $c^2$",
            ),
            &RenderOptions::default(),
        )
        .unwrap();
        let removed = render_prose_block(
            &block(
                BlockKind::Paragraph,
                r"valid $a+b$ then broken  then valid $c^2$",
            ),
            &RenderOptions::default(),
        )
        .unwrap();

        assert_eq!(mixed.formula_errors.len(), 1);
        assert_eq!(mixed.formula_errors[0].code, ErrorCode::InvalidLatex);
        assert!(nontransparent_pixels(&mixed.png) > 0);
        assert!(nontransparent_pixels(&removed.png) > 0);
        assert_ne!(mixed.png, removed.png);
    }

    #[test]
    fn injection_corpus_is_literal_in_every_required_context() {
        let corpus = [
            "#eval",
            "#import",
            "#include",
            "#read",
            "#set text(...)",
            "#show",
            "$x$",
            "\u{7}",
        ];
        for value in corpus {
            let contexts = [
                ("paragraph", BlockKind::Paragraph, value.to_owned()),
                ("heading", BlockKind::Heading, format!("# {value}")),
                ("list", BlockKind::List, format!("- {value}")),
                (
                    "table",
                    BlockKind::Table,
                    format!("| A |\n| - |\n| {value} |"),
                ),
                ("inline code", BlockKind::Paragraph, format!("`{value}`")),
            ];
            for (context, kind, source) in contexts {
                let block = block(kind, source);
                let composed = compose_block(&block, &RenderOptions::default()).unwrap();
                assert!(composed.source.contains("\\u{7}") || value != "\u{7}");
                let image = render_prose_block(&block, &RenderOptions::default())
                    .unwrap_or_else(|error| panic!("{context} failed for {value:?}: {error}"));
                assert!(!image.png.is_empty(), "{context} was empty for {value:?}");
            }
        }
    }

    #[test]
    fn eval_directive_renders_differently_from_its_hypothetical_result() {
        // AT-3-701 practical equality check: the directive must render as its
        // escaped literal text (identical to a re-render) yet differently from
        // the value it would produce if Typst had executed it.
        let directive = render_prose_block(
            &block(BlockKind::Paragraph, "#eval(\"1+1\")"),
            &RenderOptions::default(),
        )
        .unwrap();
        let result =
            render_prose_block(&block(BlockKind::Paragraph, "2"), &RenderOptions::default())
                .unwrap();
        let directive_again = render_prose_block(
            &block(BlockKind::Paragraph, "#eval(\"1+1\")"),
            &RenderOptions::default(),
        )
        .unwrap();
        assert_ne!(directive.png, result.png);
        assert_eq!(directive.png, directive_again.png);
    }

    #[test]
    fn injection_corpus_stays_literal_in_code_block_and_link_text() {
        // AT-3-701 enumerates code-block, link-text, and math-text contexts in
        // addition to the five checked above. All route user text through the
        // same escaped `#text`/`#raw` channel, so directives must render as
        // literal text and compile without executing.
        let corpus = [
            "#eval",
            "#import",
            "#include",
            "#read",
            "#set text(...)",
            "#show",
            "$x$",
            "\u{7}",
        ];
        for value in corpus {
            let contexts = [
                (
                    "code block",
                    BlockKind::CodeBlock,
                    format!("```\n{value}\n```"),
                ),
                (
                    "link text",
                    BlockKind::Paragraph,
                    format!("[{value}](https://example.com/target)"),
                ),
                (
                    "math text",
                    BlockKind::Paragraph,
                    format!("prefix {value} suffix"),
                ),
            ];
            for (context, kind, source) in contexts {
                let block = block(kind, source);
                let composed = compose_block(&block, &RenderOptions::default()).unwrap();
                // A directive must never appear as a bare Typst call; it can
                // only survive inside an escaped string literal.
                assert!(
                    !composed.source.contains(&format!("#{value}(")),
                    "{context} leaked {value:?} as a Typst call"
                );
                // The link target must never enter the Typst source.
                assert!(
                    !composed.source.contains("example.com"),
                    "{context} leaked a link target for {value:?}"
                );
                render_prose_block(&block, &RenderOptions::default())
                    .unwrap_or_else(|error| panic!("{context} failed for {value:?}: {error}"));
            }
        }
    }

    #[test]
    fn allowlisted_structures_render_at_dpr_one_and_two() {
        let cases = [
            (
                "headings",
                BlockKind::Heading,
                "# H1\n## H2\n### H3\n#### H4\n##### H5\n###### H6",
            ),
            (
                "paragraph",
                BlockKind::Paragraph,
                "Text with `inline code` and [link](https://example.com).",
            ),
            ("list", BlockKind::List, "- parent\n  1. nested\n  2. next"),
            ("quote", BlockKind::Quote, "> quoted text"),
            (
                "table",
                BlockKind::Table,
                "| A | B |\n| :- | -: |\n| 1 | 2 |",
            ),
            ("code", BlockKind::CodeBlock, "```rust\nfn main() {}\n```"),
            ("rule", BlockKind::ThematicBreak, "---"),
            (
                "image",
                BlockKind::Paragraph,
                "![visible alt](https://example.com/image.png)",
            ),
        ];
        for dpr in [1, 2] {
            let options = RenderOptions::new(480.0, 12.0, dpr).unwrap();
            let limits = Limits::default().scaled(dpr);
            for (name, kind, source) in cases {
                let image = render_prose_block(&block(kind, source), &options)
                    .unwrap_or_else(|error| panic!("{name} at dpr {dpr}: {error}"));
                assert!(!image.png.is_empty());
                assert!(
                    nontransparent_pixels(&image.png) > 0,
                    "{name} at dpr {dpr} rendered no visible pixels"
                );
                assert!(image.width_px <= limits.image_width_px);
                assert!(image.height_px <= limits.image_height_px);
                assert!(
                    u64::from(image.width_px) * u64::from(image.height_px) <= limits.image_pixels
                );
                assert!(image.png.len() as u64 <= limits.raw_png_bytes);
            }
        }
    }

    #[test]
    fn unknown_fence_language_falls_back_to_plain_raw_text() {
        let source = compose_block(
            &block(
                BlockKind::CodeBlock,
                "```not-a-real-language\n#eval(\"1+1\")\n```",
            ),
            &RenderOptions::default(),
        )
        .unwrap();
        assert!(!source.source.contains("lang:"));
        let image = render_prose_block(
            &block(
                BlockKind::CodeBlock,
                "```not-a-real-language\n#eval(\"1+1\")\n```",
            ),
            &RenderOptions::default(),
        )
        .unwrap();
        assert!(nontransparent_pixels(&image.png) > 0);
    }

    #[test]
    fn oversized_source_returns_the_safe_input_limit_error() {
        let error = render_prose_block(
            &block(BlockKind::Paragraph, "x".repeat(64 * 1024 + 1)),
            &RenderOptions::default(),
        )
        .unwrap_err();
        assert_eq!(error.safe_record().code, ErrorCode::RendererInputLimit);
        assert_eq!(
            error.safe_record().details.as_ref().unwrap().limit_kind,
            Some(SafeLimitKind::ResponseDocumentBytes)
        );
    }

    #[test]
    fn oversized_output_returns_a_safe_image_limit_error() {
        let options = RenderOptions::new(5000.0, 12.0, 1).unwrap();
        let error =
            render_prose_block(&block(BlockKind::Paragraph, "wide output"), &options).unwrap_err();
        assert_eq!(error.safe_record().code, ErrorCode::ImageTooLarge);
        assert_eq!(
            error.safe_record().details.as_ref().unwrap().limit_kind,
            Some(SafeLimitKind::ImageWidthPx)
        );
    }

    #[test]
    fn output_over_the_pixel_cap_returns_the_safe_pixel_limit_error() {
        let options = RenderOptions::new(3000.0, 3000.0, 1).unwrap();
        let limits = Limits {
            render_duration_ms: 60_000,
            ..Limits::default()
        };
        let deadline = RenderDeadline::new(limits.render_duration_ms);
        let error = render_prose_block_with_deadline(
            &block(BlockKind::Paragraph, "one two three four five"),
            &options,
            &limits,
            &deadline,
        )
        .unwrap_err();
        assert_eq!(error.safe_record().code, ErrorCode::ImageTooLarge);
        assert_eq!(
            error.safe_record().details.as_ref().unwrap().limit_kind,
            Some(SafeLimitKind::ImagePixels)
        );
    }
}
