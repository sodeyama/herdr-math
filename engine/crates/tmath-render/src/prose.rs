use std::io::Cursor;
use std::path::PathBuf;

use typst::layout::{Abs, PagedDocument};
use typst_as_lib::typst_kit_options::TypstKitFontOptions;
use typst_as_lib::TypstEngine;

use crate::{
    limits::{render_guard, RenderDeadline},
    typst_doc::compose_block_with_deadline,
    Block, ErrorCode, Limits, MathImage, RenderError, RenderOptions, SafeErrorRecord,
    DARK_THEME_TEXT_COLOR,
};

/// CJK prose coverage (D-CJK): M PLUS 2, vendored under `assets/fonts/`
/// alongside its OFL license text (`assets/fonts/OFL.txt`). `typst-assets`'s
/// embedded font set (pulled in via `search_fonts_with`/`typst-kit`, see
/// `embedded_font_options` below) covers Latin prose and math but has no CJK
/// glyphs, so Japanese text renders as tofu without this. M PLUS 2 (a
/// clean gothic Japanese typeface with heavier strokes than Klee One, the
/// prior choice — Klee One's handwriting-style strokes read as too thin),
/// per the user's chosen font.
///
/// `google/fonts`'s `ofl/mplus2/` carries only a variable font
/// (`MPLUS2[wght].ttf`); its own `upstream_info.md` names the true upstream
/// as `coz-m/MPLUS_FONTS` (commit `84c56ab8d094484cf18c555c12e9ef7708fa4fa5`
/// per that provenance record), which publishes the static per-weight TTFs
/// vendored here (`fonts/MPLUS2/ttf/MPLUS2-{Regular,Bold}.ttf`) under the
/// same OFL license (`OFL.txt` is byte-identical in both repos, 4387
/// bytes). Embedded directly with `include_bytes!` — no system font scan,
/// no network fetch — per AGENTS.md's native-engine font constraints.
///
/// Unlike Klee One, this family has a true Bold (700) static cut, so
/// `weight: "bold"` heading/strong emission resolves to the real bold face
/// rather than a nearest-weight substitute (see
/// `the_two_static_weights_are_named_and_bold_resolves_to_the_true_bold_face`
/// below, which pins this down empirically rather than assuming it).
static M_PLUS_2_REGULAR: &[u8] = include_bytes!("../assets/fonts/MPlus2-Regular.ttf");
static M_PLUS_2_BOLD: &[u8] = include_bytes!("../assets/fonts/MPlus2-Bold.ttf");

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
    // A prose page's right edge is transparent margin (`INTER_BLOCK_MARGIN_EM`,
    // see `typst_doc.rs`), not just "past the content" — `trim_transparent_right`
    // must stop this many px short of the ink's own right edge, or the pane-edge
    // margin gets cropped away as if it were ordinary trailing whitespace.
    let right_margin_px = (crate::typst_doc::INTER_BLOCK_MARGIN_EM
        * options.font_size_pt
        * f64::from(options.device_pixel_ratio.clamp(1, 4)))
    .round() as u32;
    render_typst_source(
        source.as_str(),
        &source.static_files,
        source.formula_errors.clone(),
        options,
        Some(right_margin_px),
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
    // Same D-LINE inter-block margin every other block page gets, on all
    // four sides (see `typst_doc::INTER_BLOCK_MARGIN_EM`), so a standalone
    // display-math block stacks against neighboring prose blocks with the
    // same gap as any other block pair, and clears the pane's left/right
    // edges the same way a prose block does.
    let block_margin_pt = crate::typst_doc::INTER_BLOCK_MARGIN_EM * options.font_size_pt;
    let source = format!(
        "#set page(width: {width}pt, height: auto, margin: {block_margin}pt, fill: none)\n\
         #align(center)[#image(\"{name}\", width: {image_width}pt, \
         height: {image_height}pt, fit: \"stretch\")]\n",
        width = options.content_width_pt,
        block_margin = block_margin_pt,
        image_width = image.width_pt,
        image_height = total_height,
    );
    deadline.checkpoint()?;
    render_typst_source(
        &source,
        &[(name, image.svg)],
        Vec::new(),
        options,
        None,
        limits,
        deadline,
    )
}

/// Renders a standalone display-math block's `[invalid latex]` error badge
/// (AT-3-103: invalid LaTeX in one formula fails closed PER FORMULA — the
/// block still renders, with a bounded badge, rather than the whole render
/// aborting). This is the display-math counterpart to
/// `typst_doc.rs::push_text_with_math`'s inline badge path — same literal
/// badge text, same `#raw(..., block: ...)` construct, same fixed string
/// with nothing derived from the invalid LaTeX source (per AGENTS.md, the
/// error record and the visible badge both carry only allowlisted content,
/// never the rejected formula text itself). Uses the same page/margin/font
/// setup every other block gets so a badge block stacks and reads exactly
/// like a normal display-math block, just with `#raw` text where the image
/// would be.
pub(crate) fn render_display_math_error_badge(
    formula_error: SafeErrorRecord,
    options: &RenderOptions,
    limits: &Limits,
    deadline: &RenderDeadline,
) -> Result<RenderedImage, RenderError> {
    let block_margin_pt = crate::typst_doc::INTER_BLOCK_MARGIN_EM * options.font_size_pt;
    let source = format!(
        "#set page(width: {width}pt, height: auto, margin: {block_margin}pt, fill: none)\n\
         #set text(font: {fonts}, size: {font_size}pt, fill: rgb(\"{color}\"))\n\
         #align(center)[#raw(\"[invalid latex]\", block: true)]\n",
        width = options.content_width_pt,
        block_margin = block_margin_pt,
        fonts = crate::typst_doc::font_fallback_list(options.cjk_font),
        font_size = options.font_size_pt,
        color = DARK_THEME_TEXT_COLOR,
    );
    deadline.checkpoint()?;
    render_typst_source(
        &source,
        &[],
        vec![formula_error],
        options,
        None,
        limits,
        deadline,
    )
}

fn render_typst_source(
    source: &str,
    static_files: &[(String, Vec<u8>)],
    formula_errors: Vec<SafeErrorRecord>,
    options: &RenderOptions,
    trim_right_reserve_px: Option<u32>,
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
        .fonts([M_PLUS_2_REGULAR, M_PLUS_2_BOLD])
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
    let (png, width_px) = if let Some(reserve_px) = trim_right_reserve_px {
        trim_transparent_right(&full_width_png, reserve_px)?
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

/// Crops trailing transparent columns off the right edge, but stops
/// `reserve_px` short of the actual ink's right edge so the page's own
/// pane-edge right margin (`INTER_BLOCK_MARGIN_EM`, transparent by
/// construction — it is Typst page margin, not content) survives instead
/// of being cropped away as ordinary trailing whitespace. Never trims
/// closer than `reserve_px` even if the ink itself extends past
/// `info.width - reserve_px` (a content_width_pt this tight is already
/// clamped elsewhere; this function only needs to not UNDER-reserve).
fn trim_transparent_right(
    png_bytes: &[u8],
    reserve_px: u32,
) -> Result<(Vec<u8>, u32), RenderError> {
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
    let ink_right_edge = (0..usize::try_from(info.width).unwrap_or(usize::MAX))
        .rev()
        .find(|x| {
            rgba.chunks_exact(row_bytes)
                .any(|row| row[x.saturating_mul(4) + 3] > 0)
        })
        .map_or(1, |x| x + 1);
    let full_width = usize::try_from(info.width).unwrap_or(usize::MAX);
    let content_width = ink_right_edge
        .saturating_add(reserve_px as usize)
        .min(full_width);
    if content_width == full_width {
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

    /// PART 2 pane-edge margins: a trimmed prose block's right edge must
    /// stay transparent for `INTER_BLOCK_MARGIN_EM * font_size_pt` px past
    /// the actual ink, proving `trim_transparent_right`'s reserve keeps the
    /// intended right margin instead of cropping it away as ordinary
    /// trailing whitespace (the regression this fix targets: before the
    /// reserve existed, ANY transparent trailing space — margin or not —
    /// was cropped to the bare content edge).
    #[test]
    fn trimmed_prose_block_preserves_the_right_pane_margin() {
        let options = RenderOptions::new(480.0, 12.0, 1).unwrap();
        let image =
            render_prose_block(&block(BlockKind::Paragraph, "Short line."), &options).unwrap();

        let expected_margin_px =
            (crate::typst_doc::INTER_BLOCK_MARGIN_EM * options.font_size_pt).round() as u32;
        assert!(
            expected_margin_px > 0,
            "the derived pane-edge margin must be a positive, non-zero px value"
        );

        let mut decoder = png::Decoder::new(Cursor::new(&image.png));
        decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::ALPHA);
        let mut reader = decoder.read_info().unwrap();
        let mut output = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut output).unwrap();
        let rgba = &output[..info.buffer_size()];
        assert_eq!(info.width, image.width_px);

        // The rightmost `expected_margin_px` columns must be fully
        // transparent (alpha 0) in EVERY row — that is the reserved right
        // margin, and it must not have been trimmed away.
        let row_bytes = (info.width as usize) * 4;
        for row in rgba.chunks_exact(row_bytes) {
            for col in (info.width - expected_margin_px)..info.width {
                let alpha = row[(col as usize) * 4 + 3];
                assert_eq!(
                    alpha, 0,
                    "column {col} (within the last {expected_margin_px}px reserved margin) \
                     must be fully transparent, got alpha={alpha}"
                );
            }
        }
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
        // A single short word at a huge font size stays on one line (no
        // wrap), so its height is governed by TARGET_LINE_ADVANCE_EM's
        // fixed per-line metrics rather than by wrapped line count. Width
        // and font size are both kept under their own caps
        // (`image_width_px`/`image_height_px`), but their product exceeds
        // `image_pixels` (see `output_over_the_pixel_cap...`'s sibling
        // width/height tests above for the standalone caps) — this must
        // trip `ImagePixels`, not either per-dimension cap alone.
        let options = RenderOptions::new(4000.0, 9500.0, 1).unwrap();
        let limits = Limits {
            render_duration_ms: 60_000,
            ..Limits::default()
        };
        let deadline = RenderDeadline::new(limits.render_duration_ms);
        let error = render_prose_block_with_deadline(
            &block(BlockKind::Paragraph, "x"),
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

    // --- CJK prose coverage (D-CJK) ---

    /// Direct font-book proof, independent of the render pipeline: parse the
    /// vendored OTF bytes exactly as `typst-as-lib` does internally
    /// (`Font::info`/`FontInfo::iter` walk the same `name`/`cmap` tables) and
    /// assert the family name and glyph coverage the fallback chain in
    /// `typst_doc.rs` depends on. This is the most hermetic signal for "the
    /// font book resolves a Japanese codepoint to the new font" — it does not
    /// depend on rasterization, DPI, or antialiasing at all.
    #[test]
    fn the_vendored_font_resolves_hiragana_kanji_and_japanese_punctuation() {
        let infos: Vec<_> = typst::text::FontInfo::iter(M_PLUS_2_REGULAR).collect();
        assert_eq!(infos.len(), 1, "one face in the Regular TTF");
        let info = &infos[0];
        assert_eq!(info.family, "M PLUS 2");

        // Hiragana (あ U+3042), a common kanji (日 U+65E5), and Japanese
        // full-width punctuation (。U+3002) — the three script categories
        // AT's Unicode-coverage requirement calls out.
        for codepoint in ['あ', '日', '。'] {
            assert!(
                info.coverage.contains(codepoint as u32),
                "M PLUS 2 should cover {codepoint:?}"
            );
        }

        // A Private Use Area codepoint, for contrast: no real font assigns
        // glyphs here, so this proves `coverage.contains` reports real
        // per-glyph coverage rather than trivially returning true for
        // everything (which would make the assertions above meaningless).
        assert!(!info.coverage.contains(0xE000));
    }

    /// Unlike Klee One (the prior CJK family, which had no Bold cut and
    /// fell back to a nearest-weight SemiBold), M PLUS 2 vendors a true
    /// Bold (700) static face. Rather than assume that changes the
    /// resolved weight, this drives the actual selection algorithm Typst
    /// uses internally: build a `FontBook` from both vendored `FontInfo`s
    /// (mirroring what `typst-as-lib`'s `.fonts([...])` registers) and call
    /// `FontBook::select` with the family name and a bold `FontVariant`,
    /// exactly as `#text(weight: "bold")` resolves one.
    #[test]
    fn the_two_static_weights_are_named_and_bold_resolves_to_the_true_bold_face() {
        use typst::text::{FontBook, FontInfo, FontStyle, FontVariant, FontWeight};

        let regular_infos: Vec<_> = FontInfo::iter(M_PLUS_2_REGULAR).collect();
        let bold_infos: Vec<_> = FontInfo::iter(M_PLUS_2_BOLD).collect();
        assert_eq!(regular_infos.len(), 1, "one face in the Regular TTF");
        assert_eq!(bold_infos.len(), 1, "one face in the Bold TTF");

        let regular = regular_infos[0].clone();
        let bold = bold_infos[0].clone();
        assert_eq!(regular.family, "M PLUS 2");
        assert_eq!(bold.family, "M PLUS 2");
        assert_eq!(regular.variant.weight, FontWeight::REGULAR);
        assert_eq!(
            bold.variant.weight,
            FontWeight::BOLD,
            "M PLUS 2's Bold cut must report the true 700 weight, not a \
             nearest-weight substitute like Klee One's SemiBold (600) did"
        );

        let book = FontBook::from_infos([regular, bold.clone()]);
        let bold_variant = FontVariant::new(
            FontStyle::Normal,
            FontWeight::BOLD,
            typst::text::FontStretch::default(),
        );
        let selected = book
            .select("m plus 2", bold_variant)
            .expect("the family must resolve at all for a bold request");
        assert_eq!(
            book.info(selected).unwrap().variant.weight,
            FontWeight::BOLD,
            "with an exact 700 cut available (weight distance 0), Typst's \
             nearest-weight selection must land on the real Bold face"
        );
    }

    /// End-to-end proof: a Japanese prose block renders to a non-empty image
    /// with visible glyph pixels through the real `render_prose_block`
    /// pipeline (the same path `compose_block_with_deadline` and
    /// `render_typst_source` take for any other block).
    #[test]
    fn japanese_prose_renders_visible_glyphs_through_the_full_pipeline() {
        let image = render_prose_block(
            &block(
                BlockKind::Paragraph,
                "日本語のプローズをテストします。ひらがな、漢字、句読点。",
            ),
            &RenderOptions::default(),
        )
        .unwrap();
        assert!(!image.png.is_empty());
        assert!(
            nontransparent_pixels(&image.png) > 0,
            "Japanese prose rendered no visible pixels — likely tofu or an empty page"
        );
    }

    /// Rendering the same visual "weight" of Japanese text at two different
    /// lengths must produce different images, the same hermetic signal
    /// `inline_formula_embeds_visible_pixels_and_changes_the_paragraph` uses
    /// above for math: if Japanese glyphs were silently dropped (e.g. the
    /// font failed to load and every CJK codepoint rendered as nothing), a
    /// short and a long Japanese string would collapse to visually identical
    /// (near-empty) output instead of differing.
    #[test]
    fn longer_japanese_prose_produces_a_visibly_different_image_than_shorter_prose() {
        let short = render_prose_block(
            &block(BlockKind::Paragraph, "日本語。"),
            &RenderOptions::default(),
        )
        .unwrap();
        let long = render_prose_block(
            &block(
                BlockKind::Paragraph,
                "日本語のテキストを長くして、グリフの被覆を確認します。",
            ),
            &RenderOptions::default(),
        )
        .unwrap();
        assert!(nontransparent_pixels(&short.png) > 0);
        assert!(nontransparent_pixels(&long.png) > 0);
        assert!(long.width_px > short.width_px);
        assert_ne!(short.png, long.png);
    }

    // --- Line rhythm (D-LINE) ---

    /// Hard-break a fixed number of ASCII lines (deterministic line count,
    /// no wrapping) and return the rendered image height in px.
    fn n_line_height_px(lines: u32) -> u32 {
        let text = (0..lines)
            .map(|i| format!("Line {i} of prose text for measurement."))
            .collect::<Vec<_>>()
            .join("\\\n");
        render_prose_block(
            &block(BlockKind::Paragraph, text),
            &RenderOptions::default(),
        )
        .unwrap()
        .height_px
    }

    /// The per-line vertical advance (image height delta per extra line)
    /// must land within tolerance of `font_size_pt * TARGET_LINE_ADVANCE_EM`
    /// (DPR 1, so 1pt ~= 1px). Compares two widely separated line counts so
    /// a single line's fixed top/bottom padding cancels out of the average,
    /// leaving just the per-line advance.
    #[test]
    fn n_line_paragraph_height_matches_the_target_line_advance_within_tolerance() {
        let short = n_line_height_px(4);
        let long = n_line_height_px(20);
        let measured_advance = f64::from(long - short) / 16.0;
        let expected_advance =
            RenderOptions::default().font_size_pt * crate::typst_doc::TARGET_LINE_ADVANCE_EM;
        assert!(
            (measured_advance - expected_advance).abs() < 0.75,
            "measured per-line advance {measured_advance:.3}px should be within 0.75px \
             of the {expected_advance:.3}px target (font_size * TARGET_LINE_ADVANCE_EM)"
        );
    }

    /// A heading block must render taller than a same-width bare text line
    /// (its larger font size plus its own line box), and a heading followed
    /// by a paragraph must be taller still (the block-level spacing between
    /// them), so heading text never visually collides with the line above
    /// or below it.
    #[test]
    fn a_heading_is_taller_than_a_bare_text_line_and_leaves_room_before_its_paragraph() {
        let bare_line = render_prose_block(
            &block(BlockKind::Paragraph, "Following paragraph text."),
            &RenderOptions::default(),
        )
        .unwrap();
        let heading_only = render_prose_block(
            &block(BlockKind::Heading, "# Heading"),
            &RenderOptions::default(),
        )
        .unwrap();
        let heading_and_paragraph = render_prose_block(
            &block(BlockKind::Heading, "# Heading\n\nFollowing paragraph text."),
            &RenderOptions::default(),
        )
        .unwrap();

        assert!(
            heading_only.height_px > bare_line.height_px,
            "a heading's larger font size should make its line box taller \
             than a same-content bare text line"
        );
        assert!(
            heading_and_paragraph.height_px > heading_only.height_px + bare_line.height_px,
            "a heading immediately followed by its paragraph must reserve \
             visible spacing between them, not just stack their bare heights"
        );
    }

    /// D-LINE bold-line-rhythm regression: a live-run bug report claimed
    /// paragraphs/headings containing bold (`#strong`) spans or bold
    /// list-intro lines show uneven line spacing versus plain text, despite
    /// the fixed em-based `top-edge`/`bottom-edge` from `1ad2a6c`. Typst's
    /// own `#strong` show rule only sets `TextElem::delta` (the weight),
    /// never `top-edge`/`bottom-edge` (see `typst_library::model::strong`),
    /// and those edges are configured as `TopEdge::Length`/
    /// `BottomEdge::Length` — a fixed em multiple, not a font-metric- or
    /// bounding-box-derived variant — so per Typst's own semantics no glyph
    /// or weight difference can change the computed line-box height.
    ///
    /// This test proves that hermetically at the render layer: partial-line
    /// bold spans (`**word** rest`), matching the exact pattern the report
    /// describes, must produce byte-identical per-line vertical advance to
    /// the same text with no bold. If a regression reintroduces a
    /// bold-sensitive line metric, this fails.
    #[test]
    fn bold_spans_do_not_change_the_per_line_advance() {
        fn n_line_height_px_partial_bold(lines: u32, bold: bool) -> u32 {
            let text = (0..lines)
                .map(|i| {
                    if bold {
                        format!("**Line {i}** of prose text for measurement.")
                    } else {
                        format!("Line {i} of prose text for measurement.")
                    }
                })
                .collect::<Vec<_>>()
                .join("\\\n");
            render_prose_block(
                &block(BlockKind::Paragraph, text),
                &RenderOptions::default(),
            )
            .unwrap()
            .height_px
        }

        let plain_advance = f64::from(
            n_line_height_px_partial_bold(20, false) - n_line_height_px_partial_bold(4, false),
        ) / 16.0;
        let bold_advance = f64::from(
            n_line_height_px_partial_bold(20, true) - n_line_height_px_partial_bold(4, true),
        ) / 16.0;

        assert!(
            (plain_advance - bold_advance).abs() < 1e-6,
            "bold spans changed the per-line advance: plain={plain_advance:.4}px \
             bold={bold_advance:.4}px — the fixed em-edges should make these identical"
        );
    }

    /// Same regression as above for a Markdown list whose items each start
    /// with a bold lead-in (`- **Term:** rest`, the "bold list-intro line"
    /// pattern from the report), and separately for a heading immediately
    /// followed by a bold-containing paragraph at the live-build 17pt font
    /// size with CJK glyphs mixed in (M PLUS 2's own bold weight, not
    /// NewCM10's Latin-only fallback).
    #[test]
    fn bold_list_intros_and_cjk_bold_headings_do_not_change_line_height() {
        fn n_line_height_px_list(lines: u32, bold_intro: bool) -> u32 {
            let text = (0..lines)
                .map(|i| {
                    if bold_intro {
                        format!("- **Item {i}:** description text for measurement.")
                    } else {
                        format!("- Item {i}: description text for measurement.")
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            render_prose_block(&block(BlockKind::List, text), &RenderOptions::default())
                .unwrap()
                .height_px
        }

        let plain_advance =
            f64::from(n_line_height_px_list(20, false) - n_line_height_px_list(4, false)) / 16.0;
        let bold_advance =
            f64::from(n_line_height_px_list(20, true) - n_line_height_px_list(4, true)) / 16.0;
        assert!(
            (plain_advance - bold_advance).abs() < 1e-6,
            "bold list intros changed the per-line advance: plain={plain_advance:.4}px \
             bold={bold_advance:.4}px"
        );

        let options = RenderOptions::new(480.0, 17.0, 1).unwrap();
        fn n_line_height_px_partial_bold_at(
            options: &RenderOptions,
            lines: u32,
            bold: bool,
        ) -> u32 {
            let text = (0..lines)
                .map(|i| {
                    if bold {
                        format!("**Line {i}** of prose text for measurement 日本語.")
                    } else {
                        format!("Line {i} of prose text for measurement 日本語.")
                    }
                })
                .collect::<Vec<_>>()
                .join("\\\n");
            render_prose_block(&block(BlockKind::Paragraph, text), options)
                .unwrap()
                .height_px
        }
        let plain_advance_cjk = f64::from(
            n_line_height_px_partial_bold_at(&options, 20, false)
                - n_line_height_px_partial_bold_at(&options, 4, false),
        ) / 16.0;
        let bold_advance_cjk = f64::from(
            n_line_height_px_partial_bold_at(&options, 20, true)
                - n_line_height_px_partial_bold_at(&options, 4, true),
        ) / 16.0;
        assert!(
            (plain_advance_cjk - bold_advance_cjk).abs() < 1e-6,
            "CJK bold spans at the live 17pt size changed the per-line advance: \
             plain={plain_advance_cjk:.4}px bold={bold_advance_cjk:.4}px"
        );

        let heading_plain = render_prose_block(
            &block(BlockKind::Heading, "# Heading\n\nFollowing paragraph text."),
            &options,
        )
        .unwrap();
        let heading_bold = render_prose_block(
            &block(
                BlockKind::Heading,
                "# Heading\n\n**Following** paragraph text.",
            ),
            &options,
        )
        .unwrap();
        assert_eq!(
            heading_plain.height_px, heading_bold.height_px,
            "a heading followed by a bold-mixed paragraph must render the same \
             height as the same heading followed by a plain paragraph"
        );
    }

    /// D-LINE uniform inter-block spacing (the re-routed "bold breaks the
    /// line spacing" symptom): each block renders as its own zero-*outer*-
    /// gap Typst page, and the viewer/stream emitter stacks block images
    /// with zero gap between them — so before `INTER_BLOCK_MARGIN_EM`
    /// existed, the visual gap BETWEEN two stacked blocks was always
    /// exactly 0px, regardless of the 1.6em (`TARGET_LINE_ADVANCE_EM`)
    /// rhythm applied *within* one block between its own lines. Headings
    /// and bold-led lines are disproportionately followed by a new
    /// Markdown block (a paragraph), so that block-adjacency gap of 0 is
    /// what a live-run bug report actually perceived as bold-specific line
    /// spacing — `bold_spans_do_not_change_the_per_line_advance` above
    /// already disproved a true bold/weight effect at the render layer.
    ///
    /// This proves the fix hermetically: two single-line blocks stacked
    /// (as the viewer does, at zero gap) must equal one two-line block's
    /// height, i.e. `2 * INTER_BLOCK_MARGIN_EM` (top of block B + bottom of
    /// block A) reproduces exactly the same ink-to-ink gap Typst's own
    /// `par.leading` inserts between two lines in one block.
    #[test]
    fn stacked_single_line_blocks_match_one_two_line_block_height() {
        let one_line = render_prose_block(
            &block(BlockKind::Paragraph, "Line A of prose text."),
            &RenderOptions::default(),
        )
        .unwrap();
        let two_line = render_prose_block(
            &block(
                BlockKind::Paragraph,
                "Line A of prose text.\\\nLine B of prose text.",
            ),
            &RenderOptions::default(),
        )
        .unwrap();

        let stacked_two_single_line_blocks = 2 * one_line.height_px;
        assert_eq!(
            stacked_two_single_line_blocks, two_line.height_px,
            "two single-line blocks stacked at the viewer's zero gap \
             (2 * {}px = {stacked_two_single_line_blocks}px) must match one \
             two-line block's height ({}px) — the designed inter-block \
             margin should make block adjacency indistinguishable from \
             line adjacency within a block",
            one_line.height_px, two_line.height_px
        );
    }

    /// A heading block and a paragraph block must emit the EXACT same
    /// `#set page(margin: ...)` value — proven directly from the composed
    /// Typst source (via `compose_block`, not an empirical height
    /// measurement), since `block_margin_pt` is computed once from the
    /// outer `options.font_size_pt` and written into `#set page(...)`
    /// before any body content — including a heading's own `#text(size:
    /// ...em, weight: "bold")` override — is ever emitted. This is the
    /// most hermetic possible proof that the margin is a page property
    /// independent of `BlockKind` and of a heading's larger inner font
    /// size: reading the literal generated margin value, not inferring it
    /// from rendered pixel heights. The margin is now uniform on all four
    /// sides (`#set page(margin: {value}pt, ...)`, PART 2's pane-edge
    /// horizontal margins reusing the same `INTER_BLOCK_MARGIN_EM`
    /// constant as the vertical rhythm — see its doc comment), so a single
    /// value covers both axes.
    #[test]
    fn heading_and_paragraph_blocks_emit_the_identical_page_margin() {
        let options = RenderOptions::default();
        let paragraph_source =
            compose_block(&block(BlockKind::Paragraph, "Paragraph text."), &options).unwrap();
        let heading_source =
            compose_block(&block(BlockKind::Heading, "# Heading text"), &options).unwrap();

        let paragraph_margin = extract_page_margin(&paragraph_source.source);
        let heading_margin = extract_page_margin(&heading_source.source);
        assert_eq!(
            paragraph_margin, heading_margin,
            "a heading and a paragraph must emit the identical #set page(margin: ...) clause"
        );

        let expected_margin_pt = crate::typst_doc::INTER_BLOCK_MARGIN_EM * options.font_size_pt;
        assert_eq!(
            paragraph_margin,
            format!("margin: {expected_margin_pt}pt"),
            "margin clause did not match the expected uniform value derived from \
             INTER_BLOCK_MARGIN_EM"
        );
    }

    /// Pulls the `margin: ...pt` clause out of a composed block's `#set
    /// page(...)` line, for exact source-level comparison.
    fn extract_page_margin(source: &str) -> String {
        let start = source
            .find("margin: ")
            .expect("margin clause must be present");
        let rest = &source[start..];
        let end = rest.find("pt,").expect("margin clause must end with pt,");
        rest[..end + 2].to_owned()
    }

    /// The re-route's explicit requirement: display-math blocks
    /// (`render_display_math_block`, a SEPARATE Typst page from
    /// `compose_block_with_deadline`'s prose pages) must get the identical
    /// derived margin, so a standalone `$$...$$` block stacks against
    /// neighboring text blocks with the same designed gap in both
    /// directions (math→text and text→math), not flush.
    #[test]
    fn display_math_blocks_get_the_same_inter_block_margin_as_text_blocks() {
        let options = RenderOptions::new(480.0, 12.0, 1).unwrap();
        let expected_margin_pt = crate::typst_doc::INTER_BLOCK_MARGIN_EM * options.font_size_pt;
        assert!(
            expected_margin_pt > 0.0,
            "the derived inter-block margin must be a positive, non-zero pt value"
        );

        // The display-math page has exactly one content element (the
        // formula image, placed via `#align(center)[#image(..., width:
        // image.width_pt, height: image.height_pt + image.depth_pt, fit:
        // "stretch")]`) and no other variable-height content — no line-box
        // padding, no leading. So the rendered block's height at DPR 1 (1pt
        // ~= 1px) must equal the formula's own exact content height plus
        // `2 * expected_margin_pt`, with nothing else contributing. This is
        // an exact equality, not a statistical estimate.
        let image =
            crate::math::render_formula(r"\frac{a}{b}", true, &options).expect("must render");
        let formula_content_height_pt = image.height_pt + image.depth_pt;
        let limits = Limits::default();
        let deadline = RenderDeadline::new(limits.render_duration_ms);
        let math_block = render_display_math_block(0, image, &options, &limits, &deadline).unwrap();

        let expected_total_height_px =
            (formula_content_height_pt + 2.0 * expected_margin_pt).round() as u32;
        assert_eq!(
            math_block.height_px,
            expected_total_height_px,
            "a display-math block's rendered height must equal its formula's own \
             content height ({formula_content_height_pt:.3}pt) plus exactly \
             2*INTER_BLOCK_MARGIN_EM*font_size_pt ({:.3}pt) of margin — the same \
             derived margin every text block gets",
            2.0 * expected_margin_pt
        );
    }

    /// Inline `` `raw` `` spans must not read visibly smaller than
    /// surrounding CJK prose — Typst's built-in `raw` show rule defaults to
    /// 0.8em, which measured ink-row height ~0.83x a plain-text control at
    /// the live 15pt/dpr2 geometry (task #24). `compose_block`'s `#show
    /// raw: set text(size: RAW_TEXT_SIZE_EM em)` rule undoes that: an
    /// inline raw span's ink height must now match a plain-text control
    /// within a small tolerance (whole-pixel rounding, not the ~7px gap
    /// the unfixed default produced).
    #[test]
    fn inline_raw_spans_match_surrounding_text_ink_height() {
        fn ink_row_span(png_bytes: &[u8], width_px: u32, height_px: u32) -> u32 {
            let mut decoder = png::Decoder::new(Cursor::new(png_bytes));
            decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::ALPHA);
            let mut reader = decoder.read_info().unwrap();
            let mut output = vec![0; reader.output_buffer_size().unwrap()];
            let info = reader.next_frame(&mut output).unwrap();
            let bytes = &output[..info.buffer_size()];
            let mut first_row = None;
            let mut last_row = 0u32;
            for y in 0..height_px {
                let row_start = (y * width_px * 4) as usize;
                let row_end = row_start + (width_px * 4) as usize;
                let row_has_ink = bytes[row_start..row_end]
                    .chunks_exact(4)
                    .any(|pixel| pixel[3] > 0);
                if row_has_ink {
                    first_row.get_or_insert(y);
                    last_row = y;
                }
            }
            last_row - first_row.unwrap_or(0) + 1
        }

        let options = RenderOptions::new(672.0, 15.0, 2).unwrap();
        let raw_only = render_prose_block(
            &block(BlockKind::Paragraph, "`is_genuine_user_text`"),
            &options,
        )
        .unwrap();
        let plain_only = render_prose_block(
            &block(BlockKind::Paragraph, "is_genuine_user_text"),
            &options,
        )
        .unwrap();

        let raw_span = ink_row_span(&raw_only.png, raw_only.width_px, raw_only.height_px);
        let plain_span = ink_row_span(&plain_only.png, plain_only.width_px, plain_only.height_px);

        assert_eq!(
            raw_only.height_px, plain_only.height_px,
            "raw must not change the block's line-box height"
        );
        let diff = raw_span.abs_diff(plain_span);
        assert!(
            diff <= 2,
            "an inline raw span's ink height ({raw_span}px) must closely match              plain text's ({plain_span}px) once RAW_TEXT_SIZE_EM restores raw              to body size — Typst's unfixed 0.8em default would show a ~7px gap"
        );
    }

    /// Fenced block code gets the same size restoration as inline raw
    /// (task #24's measurement found no meaningful difference between
    /// inline and block reduction, so both go through the same
    /// `RAW_TEXT_SIZE_EM` rule rather than keeping two separate sizes).
    #[test]
    fn block_code_also_matches_surrounding_text_ink_height() {
        fn ink_row_span(png_bytes: &[u8], width_px: u32, height_px: u32) -> u32 {
            let mut decoder = png::Decoder::new(Cursor::new(png_bytes));
            decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::ALPHA);
            let mut reader = decoder.read_info().unwrap();
            let mut output = vec![0; reader.output_buffer_size().unwrap()];
            let info = reader.next_frame(&mut output).unwrap();
            let bytes = &output[..info.buffer_size()];
            let mut first_row = None;
            let mut last_row = 0u32;
            for y in 0..height_px {
                let row_start = (y * width_px * 4) as usize;
                let row_end = row_start + (width_px * 4) as usize;
                let row_has_ink = bytes[row_start..row_end]
                    .chunks_exact(4)
                    .any(|pixel| pixel[3] > 0);
                if row_has_ink {
                    first_row.get_or_insert(y);
                    last_row = y;
                }
            }
            last_row - first_row.unwrap_or(0) + 1
        }

        let options = RenderOptions::new(672.0, 15.0, 2).unwrap();
        let block_code = render_prose_block(
            &block(
                BlockKind::CodeBlock,
                "```
is_genuine_user_text
```",
            ),
            &options,
        )
        .unwrap();
        let plain_only = render_prose_block(
            &block(BlockKind::Paragraph, "is_genuine_user_text"),
            &options,
        )
        .unwrap();

        let block_span = ink_row_span(&block_code.png, block_code.width_px, block_code.height_px);
        let plain_span = ink_row_span(&plain_only.png, plain_only.width_px, plain_only.height_px);
        let diff = block_span.abs_diff(plain_span);
        assert!(
            diff <= 2,
            "block code's ink height ({block_span}px) must closely match \
             plain text's ({plain_span}px) once RAW_TEXT_SIZE_EM restores it \
             to body size"
        );
    }

    /// Regression guard for task #24's fallback-order decision: measured
    /// ink height for NewCM10 Latin text vs. M PLUS 2 CJK text at equal pt
    /// (31px vs. 29px at the live 15pt/dpr2 geometry, ~6.5% apart) was
    /// judged NOT large enough to justify reordering the prose font
    /// fallback list (`("NewCM10", "M PLUS 2")`, fixed by
    /// `font_fallback_list_always_starts_with_the_primary_latin_font` in
    /// `typst_doc.rs`). If a future font change makes the two scripts
    /// diverge past a clearly-visible amount, that decision should be
    /// revisited — this test catches such drift rather than re-deciding it
    /// silently.
    #[test]
    fn latin_and_cjk_ink_heights_stay_within_the_accepted_balance_tolerance() {
        fn ink_row_span(png_bytes: &[u8], width_px: u32, height_px: u32) -> u32 {
            let mut decoder = png::Decoder::new(Cursor::new(png_bytes));
            decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::ALPHA);
            let mut reader = decoder.read_info().unwrap();
            let mut output = vec![0; reader.output_buffer_size().unwrap()];
            let info = reader.next_frame(&mut output).unwrap();
            let bytes = &output[..info.buffer_size()];
            let mut first_row = None;
            let mut last_row = 0u32;
            for y in 0..height_px {
                let row_start = (y * width_px * 4) as usize;
                let row_end = row_start + (width_px * 4) as usize;
                let row_has_ink = bytes[row_start..row_end]
                    .chunks_exact(4)
                    .any(|pixel| pixel[3] > 0);
                if row_has_ink {
                    first_row.get_or_insert(y);
                    last_row = y;
                }
            }
            last_row - first_row.unwrap_or(0) + 1
        }

        let options = RenderOptions::new(672.0, 15.0, 2).unwrap();
        let latin_only = render_prose_block(
            &block(BlockKind::Paragraph, "Hxypg quick brown fox"),
            &options,
        )
        .unwrap();
        let cjk_only = render_prose_block(
            &block(BlockKind::Paragraph, "漢字仮名文章日本語吾輩猫"),
            &options,
        )
        .unwrap();

        let latin_span = ink_row_span(&latin_only.png, latin_only.width_px, latin_only.height_px);
        let cjk_span = ink_row_span(&cjk_only.png, cjk_only.width_px, cjk_only.height_px);
        let ratio = f64::from(cjk_span) / f64::from(latin_span);
        assert!(
            (0.85..=1.0).contains(&ratio),
            "Latin ({latin_span}px) vs CJK ({cjk_span}px) ink-height balance drifted \
             outside the accepted range (ratio={ratio:.3}); re-evaluate the prose \
             font fallback order if this fails"
        );
    }
}
