//! Native Markdown + LaTeX renderer for tmath (V3).

mod block;
mod error;
mod hash;
mod limits;
mod markdown;
mod math;
mod options;
mod prose;
mod scanner;
mod typst_doc;

pub use block::{Block, BlockKind};
pub use error::{ErrorCode, RenderError, SafeErrorDetails, SafeErrorRecord, SafeLimitKind};
pub use hash::content_hash;
pub use limits::{Limits, ScaledLimits};
pub use markdown::parse_blocks;
pub use math::{render_formula, MathImage};
pub use options::{RenderOptions, RenderOptionsError, DARK_THEME_TEXT_COLOR};
pub use prose::{render_prose_block, RenderedImage};
pub use scanner::{scan_latex, Formula, ScannerLimits};
pub use typst_doc::{compose_block, TypstSource};

/// A rendered block image and any formula-local safe errors.
pub type RenderedBlock = RenderedImage;

/// Renders one semantic block through the math or prose path.
pub fn render_block(block: &Block, options: &RenderOptions) -> Result<RenderedBlock, RenderError> {
    if block.kind != BlockKind::DisplayMath {
        return render_prose_block(block, options);
    }

    let candidate = block.source.trim();
    let formulas = scan_latex(candidate, &ScannerLimits::default())?;
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
    let image = render_formula(&formula.latex, true, options)?;
    prose::render_display_math_block(block.index, image, options)
}

#[cfg(test)]
mod render_tests {
    use super::*;

    #[test]
    fn public_entry_dispatches_display_math_and_prose() {
        let blocks = parse_blocks("$$E=mc^2$$\n\nordinary $a+b$\n");
        let math = render_block(&blocks[0], &RenderOptions::default()).unwrap();
        let prose = render_block(&blocks[1], &RenderOptions::default()).unwrap();
        assert!(!math.png.is_empty());
        assert!(!prose.png.is_empty());
        assert!(math.formula_errors.is_empty());
        assert!(prose.formula_errors.is_empty());
    }
}
