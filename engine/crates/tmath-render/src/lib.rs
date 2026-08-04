//! Native Markdown + LaTeX renderer for tmath (V3).

mod block;
mod error;
mod hash;
mod limits;
mod markdown;
mod options;
mod prose;
mod scanner;
mod typst_doc;

pub use block::{Block, BlockKind};
pub use error::{ErrorCode, RenderError, SafeErrorDetails, SafeErrorRecord, SafeLimitKind};
pub use hash::content_hash;
pub use limits::{Limits, ScaledLimits};
pub use markdown::parse_blocks;
pub use options::{RenderOptions, RenderOptionsError, DARK_THEME_TEXT_COLOR};
pub use prose::{render_prose_block, RenderedImage};
pub use scanner::{scan_latex, Formula, ScannerLimits};
pub use typst_doc::{compose_block, TypstSource};
