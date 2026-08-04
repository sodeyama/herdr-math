//! Native Markdown + LaTeX renderer for tmath (V3).

mod block;
mod error;
mod hash;
mod limits;
mod options;
mod scanner;

pub use block::{Block, BlockKind};
pub use error::{ErrorCode, RenderError, SafeErrorDetails, SafeErrorRecord, SafeLimitKind};
pub use hash::content_hash;
pub use limits::{Limits, ScaledLimits};
pub use options::{RenderOptions, RenderOptionsError, DARK_THEME_TEXT_COLOR};
pub use scanner::{scan_latex, Formula, ScannerLimits};
