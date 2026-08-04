//! Native Markdown + LaTeX renderer for tmath (V3).

mod block;
mod cache;
mod error;
mod hash;
mod limits;
mod markdown;
mod math;
mod options;
mod planner;
mod prose;
mod scanner;
mod stream;
mod typst_doc;

pub use block::{Block, BlockKind};
pub use cache::{CacheBudget, CacheStats, RenderCache};
pub use error::{ErrorCode, RenderError, SafeErrorDetails, SafeErrorRecord, SafeLimitKind};
pub use hash::content_hash;
pub use limits::{Limits, ScaledLimits};
pub use markdown::{parse_blocks, parse_blocks_limited};
pub use math::{render_formula, MathImage};
pub use options::{RenderOptions, RenderOptionsError, DARK_THEME_TEXT_COLOR};
pub use planner::{BlockId, PlacementPlanner, Plan, PlanOp, PlannedBlock};
pub use prose::{render_prose_block, RenderedImage};
pub use scanner::{scan_latex, Formula, ScannerLimits};
pub use stream::{Revision, StreamSplitter};
pub use typst_doc::{compose_block, TypstSource};

/// A rendered block image and any formula-local safe errors.
///
/// For the same block and options, the PNG bytes are deterministic. Combined
/// with [`content_hash`], this makes rendered-block caching sound.
pub type RenderedBlock = RenderedImage;

/// Renders one semantic block through the math or prose path.
pub fn render_block(block: &Block, options: &RenderOptions) -> Result<RenderedBlock, RenderError> {
    render_block_limited(block, options, &Limits::default())
}

/// Renders one semantic block with explicit finite limits.
pub fn render_block_limited(
    block: &Block,
    options: &RenderOptions,
    limits: &Limits,
) -> Result<RenderedBlock, RenderError> {
    // Queue wait is intentionally outside the cooperative deadline. Once the
    // resident engine is owned, all block pipeline stages share one timer.
    let _guard = limits::render_guard()?;
    let deadline = limits::RenderDeadline::new(limits.render_duration_ms);
    let mut rendered = render_block_with_deadline(block, options, limits, &deadline)?;
    rendered.duration_ms = deadline.checkpoint()?;
    Ok(rendered)
}

fn render_block_with_deadline(
    block: &Block,
    options: &RenderOptions,
    limits: &Limits,
    deadline: &limits::RenderDeadline,
) -> Result<RenderedBlock, RenderError> {
    if block.kind != BlockKind::DisplayMath {
        return prose::render_prose_block_with_deadline(block, options, limits, deadline);
    }

    let candidate = block.source.trim();
    let formulas = scan_latex(candidate, &ScannerLimits::default())?;
    deadline.checkpoint()?;
    let formula = formulas
        .into_iter()
        .find(|formula| formula.display && formula.start == 0 && formula.end == candidate.len())
        .ok_or_else(|| {
            RenderError::new(
                SafeErrorRecord {
                    code: ErrorCode::FormulaNotFound,
                    retryable: false,
                    details: None,
                },
                "display-math block did not contain one complete formula",
            )
        })?;
    let image =
        math::render_formula_with_deadline(&formula.latex, true, options, limits, deadline)?;
    prose::render_display_math_block(block.index, image, options, limits, deadline)
}

#[cfg(test)]
mod render_tests {
    use super::*;
    use serde_json::to_string;

    #[test]
    fn public_entry_dispatches_display_math_and_prose() {
        let blocks = parse_blocks("$$E=mc^2$$\n\nordinary $a+b$\n").unwrap();
        let math = render_block(&blocks[0], &RenderOptions::default()).unwrap();
        let prose = render_block(&blocks[1], &RenderOptions::default()).unwrap();
        assert!(!math.png.is_empty());
        assert!(!prose.png.is_empty());
        assert!(math.formula_errors.is_empty());
        assert!(prose.formula_errors.is_empty());
        assert!(math.duration_ms <= Limits::default().render_duration_ms);
        assert!(prose.duration_ms <= Limits::default().render_duration_ms);
    }

    #[test]
    fn zero_duration_limit_fails_at_the_first_checkpoint() {
        let block = Block {
            index: 0,
            kind: BlockKind::Paragraph,
            source: "deadline marker".to_owned(),
        };
        let limits = Limits {
            render_duration_ms: 0,
            ..Limits::default()
        };

        let error = render_block_limited(&block, &RenderOptions::default(), &limits).unwrap_err();
        assert_eq!(error.safe_record().code, ErrorCode::RendererTimeout);
        let details = error.safe_record().details.as_ref().unwrap();
        assert_eq!(details.limit_kind, Some(SafeLimitKind::RenderDurationMs));
        assert_eq!(details.limit, Some(0));
        assert_eq!(details.actual, Some(0));
        assert_eq!(details.duration_ms, Some(0));
    }

    #[test]
    fn public_safe_errors_use_only_allowlisted_codes_and_never_serialize_input() {
        const MARKER: &str = "ZZINJECTED_MARKERZZ";
        const ALLOWLIST: [ErrorCode; 8] = [
            ErrorCode::FormulaNotFound,
            ErrorCode::ScannerInputLimit,
            ErrorCode::InvalidLatex,
            ErrorCode::RendererInputLimit,
            ErrorCode::RendererTimeout,
            ErrorCode::RendererFailed,
            ErrorCode::ImageTooLarge,
            ErrorCode::InternalError,
        ];

        let scanner_error = scan_latex(
            MARKER,
            &ScannerLimits {
                max_input_bytes: 1,
                ..ScannerLimits::default()
            },
        )
        .unwrap_err();
        let invalid_latex = render_formula(
            &format!(r"\frac{{{MARKER}"),
            false,
            &RenderOptions::default(),
        )
        .unwrap_err();
        let source_error = parse_blocks_limited(
            &format!("{MARKER}{}", "x".repeat(32)),
            &Limits {
                source_bytes_per_block: 8,
                ..Limits::default()
            },
        )
        .unwrap_err();
        let block_count_error = parse_blocks_limited(
            &format!("{MARKER} one\n\n{MARKER} two\n"),
            &Limits {
                blocks_per_document: 1,
                ..Limits::default()
            },
        )
        .unwrap_err();
        let block = Block {
            index: 0,
            kind: BlockKind::Paragraph,
            source: MARKER.to_owned(),
        };
        let pixel_error = render_block_limited(
            &block,
            &RenderOptions::default(),
            &Limits {
                image_pixels: 1,
                ..Limits::default()
            },
        )
        .unwrap_err();
        let png_error = render_block_limited(
            &block,
            &RenderOptions::default(),
            &Limits {
                raw_png_bytes: 1,
                ..Limits::default()
            },
        )
        .unwrap_err();
        let deadline_error = render_block_limited(
            &block,
            &RenderOptions::default(),
            &Limits {
                render_duration_ms: 0,
                ..Limits::default()
            },
        )
        .unwrap_err();

        for error in [
            scanner_error,
            invalid_latex,
            source_error,
            block_count_error,
            pixel_error,
            png_error,
            deadline_error,
        ] {
            assert!(ALLOWLIST.contains(&error.safe_record().code));
            assert!(!to_string(error.safe_record()).unwrap().contains(MARKER));
        }
    }

    #[test]
    fn render_block_is_byte_deterministic_for_all_primary_paths_and_dprs() {
        let cases = [
            (
                BlockKind::Paragraph,
                "A prose block with inline math $\\frac{a+b}{c}$.",
            ),
            (BlockKind::DisplayMath, "$$\\sum_{i=1}^{n} i$$"),
            (
                BlockKind::CodeBlock,
                "```rust\nfn deterministic() -> bool { true }\n```",
            ),
        ];

        for dpr in [1, 2] {
            let options = RenderOptions::new(480.0, 12.0, dpr).unwrap();
            for (kind, source) in cases {
                let block = Block {
                    index: 0,
                    kind,
                    source: source.to_owned(),
                };
                let first = render_block(&block, &options).unwrap();
                let second = render_block(&block, &options).unwrap();
                assert_eq!(first.png, second.png, "{kind:?} at dpr {dpr}");
            }
        }
    }
}
