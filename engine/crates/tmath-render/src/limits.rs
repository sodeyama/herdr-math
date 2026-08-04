use crate::{ErrorCode, RenderError, SafeLimitKind};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Instant;

/// Finite render limits expressed at device pixel ratio 1.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Limits {
    pub source_bytes_per_block: u64,
    pub blocks_per_document: u64,
    pub render_duration_ms: u64,
    pub image_width_px: u32,
    pub image_height_px: u32,
    pub image_pixels: u64,
    pub raw_png_bytes: u64,
    /// Finite bound on one formula's embedded SVG byte length (`MathImage::svg`,
    /// UTF-8 markup, not a raster). SVG is vector output and has no pixel
    /// dimensions to bound, so this is the sole per-formula size check
    /// before the formula is embedded in the composed Typst page; the
    /// page's final rasterized PNG still goes through `image_width_px`/
    /// `image_height_px`/`image_pixels`/`raw_png_bytes` unchanged.
    pub math_svg_bytes: u64,
    /// The agent-viewer's bound on how many blocks' PNG bytes stay retained
    /// (D7's "cached blocks"): blocks whose distance from the current
    /// visibility window exceeds this budget on either side have their
    /// retained PNG evicted and are re-rendered on scroll-back (AT-3-504).
    /// A plain byte count, not pixel-scaled — retention is about how many
    /// answer blocks a session keeps in memory at once, not image
    /// resolution. Unused by stream/watch sessions, which never retain PNGs
    /// at all (see `TerminalSink::retain_pngs`).
    pub retained_window_blocks: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            source_bytes_per_block: 64 * 1024,
            blocks_per_document: 4096,
            render_duration_ms: 1000,
            image_width_px: 4096,
            image_height_px: 16_384,
            image_pixels: 33_554_432,
            raw_png_bytes: 512 * 1024,
            math_svg_bytes: 512 * 1024,
            retained_window_blocks: 200,
        }
    }
}

impl Limits {
    pub fn scaled(self, device_pixel_ratio: u8) -> ScaledLimits {
        let device_pixel_ratio = device_pixel_ratio.clamp(1, 4);
        let linear_scale = u32::from(device_pixel_ratio);
        let area_scale = u64::from(device_pixel_ratio).pow(2);

        ScaledLimits {
            source_bytes_per_block: self.source_bytes_per_block,
            blocks_per_document: self.blocks_per_document,
            render_duration_ms: self.render_duration_ms,
            image_width_px: self.image_width_px.saturating_mul(linear_scale),
            image_height_px: self.image_height_px.saturating_mul(linear_scale),
            image_pixels: self.image_pixels.saturating_mul(area_scale),
            raw_png_bytes: self.raw_png_bytes.saturating_mul(area_scale),
            // SVG markup is vector output generated at the logical font
            // size; unlike a raster, its byte length does not grow with
            // device pixel ratio, so this limit is not DPR-scaled.
            math_svg_bytes: self.math_svg_bytes,
            device_pixel_ratio,
        }
    }

    pub fn check_source_bytes_per_block(self, actual: u64) -> Result<(), RenderError> {
        check_limit(
            actual,
            self.source_bytes_per_block,
            ErrorCode::RendererInputLimit,
            // The stable contract has no prose-block source kind.
            SafeLimitKind::ResponseDocumentBytes,
        )
    }

    pub fn check_blocks_per_document(self, actual: u64) -> Result<(), RenderError> {
        check_limit(
            actual,
            self.blocks_per_document,
            ErrorCode::RendererInputLimit,
            SafeLimitKind::ResponseDocumentBlocks,
        )
    }

    pub fn check_render_duration_ms(self, actual: u64) -> Result<(), RenderError> {
        check_limit(
            actual,
            self.render_duration_ms,
            ErrorCode::RendererTimeout,
            SafeLimitKind::RenderDurationMs,
        )
    }

    pub fn check_image_width_px(self, actual: u32) -> Result<(), RenderError> {
        check_limit(
            u64::from(actual),
            u64::from(self.image_width_px),
            ErrorCode::ImageTooLarge,
            SafeLimitKind::ImageWidthPx,
        )
    }

    pub fn check_image_height_px(self, actual: u32) -> Result<(), RenderError> {
        check_limit(
            u64::from(actual),
            u64::from(self.image_height_px),
            ErrorCode::ImageTooLarge,
            SafeLimitKind::ImageHeightPx,
        )
    }

    pub fn check_image_pixels(self, actual: u64) -> Result<(), RenderError> {
        check_limit(
            actual,
            self.image_pixels,
            ErrorCode::ImageTooLarge,
            SafeLimitKind::ImagePixels,
        )
    }

    pub fn check_raw_png_bytes(self, actual: u64) -> Result<(), RenderError> {
        check_limit(
            actual,
            self.raw_png_bytes,
            ErrorCode::ImageTooLarge,
            SafeLimitKind::RawPngBytes,
        )
    }

    pub fn check_math_svg_bytes(self, actual: u64) -> Result<(), RenderError> {
        check_limit(
            actual,
            self.math_svg_bytes,
            ErrorCode::ImageTooLarge,
            SafeLimitKind::MathSvgBytes,
        )
    }
}

/// Render limits after DPR scaling.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScaledLimits {
    pub source_bytes_per_block: u64,
    pub blocks_per_document: u64,
    pub render_duration_ms: u64,
    pub image_width_px: u32,
    pub image_height_px: u32,
    pub image_pixels: u64,
    pub raw_png_bytes: u64,
    pub math_svg_bytes: u64,
    pub device_pixel_ratio: u8,
}

impl ScaledLimits {
    pub fn check_source_bytes_per_block(self, actual: u64) -> Result<(), RenderError> {
        check_limit(
            actual,
            self.source_bytes_per_block,
            ErrorCode::RendererInputLimit,
            SafeLimitKind::ResponseDocumentBytes,
        )
    }

    pub fn check_blocks_per_document(self, actual: u64) -> Result<(), RenderError> {
        check_limit(
            actual,
            self.blocks_per_document,
            ErrorCode::RendererInputLimit,
            SafeLimitKind::ResponseDocumentBlocks,
        )
    }

    pub fn check_render_duration_ms(self, actual: u64) -> Result<(), RenderError> {
        check_limit(
            actual,
            self.render_duration_ms,
            ErrorCode::RendererTimeout,
            SafeLimitKind::RenderDurationMs,
        )
    }

    pub fn check_image_width_px(self, actual: u32) -> Result<(), RenderError> {
        check_limit(
            u64::from(actual),
            u64::from(self.image_width_px),
            ErrorCode::ImageTooLarge,
            SafeLimitKind::ImageWidthPx,
        )
    }

    pub fn check_image_height_px(self, actual: u32) -> Result<(), RenderError> {
        check_limit(
            u64::from(actual),
            u64::from(self.image_height_px),
            ErrorCode::ImageTooLarge,
            SafeLimitKind::ImageHeightPx,
        )
    }

    pub fn check_image_pixels(self, actual: u64) -> Result<(), RenderError> {
        check_limit(
            actual,
            self.image_pixels,
            ErrorCode::ImageTooLarge,
            SafeLimitKind::ImagePixels,
        )
    }

    pub fn check_raw_png_bytes(self, actual: u64) -> Result<(), RenderError> {
        check_limit(
            actual,
            self.raw_png_bytes,
            ErrorCode::ImageTooLarge,
            SafeLimitKind::RawPngBytes,
        )
    }

    pub fn check_math_svg_bytes(self, actual: u64) -> Result<(), RenderError> {
        check_limit(
            actual,
            self.math_svg_bytes,
            ErrorCode::ImageTooLarge,
            SafeLimitKind::MathSvgBytes,
        )
    }
}

fn check_limit(
    actual: u64,
    limit: u64,
    code: ErrorCode,
    limit_kind: SafeLimitKind,
) -> Result<(), RenderError> {
    if actual <= limit {
        Ok(())
    } else {
        Err(RenderError::limit_exceeded(code, limit_kind, limit, actual))
    }
}

/// Cooperative wall-clock deadline shared by one block render.
///
/// Rust cannot safely interrupt a Typst or RaTeX call while it is executing.
/// The renderer therefore uses two levels of protection: cheap checkpoints
/// between pipeline stages fail the render as soon as control returns, while
/// the measured duration is returned to callers so a higher-level supervisor
/// can observe healthy renders and enforce a harder isolation boundary later.
pub(crate) struct RenderDeadline {
    started: Instant,
    limit_ms: u64,
}

pub(crate) fn render_guard() -> Result<MutexGuard<'static, ()>, RenderError> {
    static HOLDER: OnceLock<Mutex<()>> = OnceLock::new();
    HOLDER.get_or_init(|| Mutex::new(())).lock().map_err(|_| {
        RenderError::new(
            crate::SafeErrorRecord {
                code: ErrorCode::RendererFailed,
                retryable: false,
                details: None,
            },
            "render engine holder was poisoned",
        )
    })
}

impl RenderDeadline {
    pub(crate) fn new(limit_ms: u64) -> Self {
        Self {
            started: Instant::now(),
            limit_ms,
        }
    }

    pub(crate) fn checkpoint(&self) -> Result<u64, RenderError> {
        let duration_ms = self.elapsed_ms();
        if self.limit_ms == 0 || duration_ms > self.limit_ms {
            Err(RenderError::deadline_exceeded(self.limit_ms, duration_ms))
        } else {
            Ok(duration_ms)
        }
    }

    fn elapsed_ms(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ErrorCode, SafeLimitKind};

    fn assert_limit_error(
        error: crate::RenderError,
        code: ErrorCode,
        kind: SafeLimitKind,
        limit: u64,
        actual: u64,
    ) {
        let record = error.safe_record();
        assert_eq!(record.code, code);
        let details = record.details.as_ref().unwrap();
        assert_eq!(details.limit_kind, Some(kind));
        assert_eq!(details.limit, Some(limit));
        assert_eq!(details.actual, Some(actual));
    }

    #[test]
    fn defaults_match_the_per_block_policy() {
        let limits = Limits::default();
        assert_eq!(limits.source_bytes_per_block, 64 * 1024);
        assert_eq!(limits.blocks_per_document, 4096);
        assert_eq!(limits.render_duration_ms, 1000);
        assert_eq!(limits.image_width_px, 4096);
        assert_eq!(limits.image_height_px, 16_384);
        assert_eq!(limits.image_pixels, 33_554_432);
        assert_eq!(limits.raw_png_bytes, 512 * 1024);
        assert_eq!(limits.math_svg_bytes, 512 * 1024);
        assert_eq!(limits.retained_window_blocks, 200);
    }

    #[test]
    fn checks_accept_limits_and_reject_values_past_them() {
        let limits = Limits::default();

        assert!(limits
            .check_source_bytes_per_block(limits.source_bytes_per_block)
            .is_ok());
        assert_limit_error(
            limits
                .check_source_bytes_per_block(limits.source_bytes_per_block + 1)
                .unwrap_err(),
            ErrorCode::RendererInputLimit,
            SafeLimitKind::ResponseDocumentBytes,
            limits.source_bytes_per_block,
            limits.source_bytes_per_block + 1,
        );

        assert!(limits
            .check_blocks_per_document(limits.blocks_per_document)
            .is_ok());
        assert_limit_error(
            limits
                .check_blocks_per_document(limits.blocks_per_document + 1)
                .unwrap_err(),
            ErrorCode::RendererInputLimit,
            SafeLimitKind::ResponseDocumentBlocks,
            limits.blocks_per_document,
            limits.blocks_per_document + 1,
        );

        assert!(limits
            .check_render_duration_ms(limits.render_duration_ms)
            .is_ok());
        assert_limit_error(
            limits
                .check_render_duration_ms(limits.render_duration_ms + 1)
                .unwrap_err(),
            ErrorCode::RendererTimeout,
            SafeLimitKind::RenderDurationMs,
            limits.render_duration_ms,
            limits.render_duration_ms + 1,
        );

        assert!(limits.check_image_width_px(limits.image_width_px).is_ok());
        assert_limit_error(
            limits
                .check_image_width_px(limits.image_width_px + 1)
                .unwrap_err(),
            ErrorCode::ImageTooLarge,
            SafeLimitKind::ImageWidthPx,
            u64::from(limits.image_width_px),
            u64::from(limits.image_width_px) + 1,
        );

        assert!(limits.check_image_height_px(limits.image_height_px).is_ok());
        assert_limit_error(
            limits
                .check_image_height_px(limits.image_height_px + 1)
                .unwrap_err(),
            ErrorCode::ImageTooLarge,
            SafeLimitKind::ImageHeightPx,
            u64::from(limits.image_height_px),
            u64::from(limits.image_height_px) + 1,
        );

        assert!(limits.check_image_pixels(limits.image_pixels).is_ok());
        assert_limit_error(
            limits
                .check_image_pixels(limits.image_pixels + 1)
                .unwrap_err(),
            ErrorCode::ImageTooLarge,
            SafeLimitKind::ImagePixels,
            limits.image_pixels,
            limits.image_pixels + 1,
        );

        assert!(limits.check_raw_png_bytes(limits.raw_png_bytes).is_ok());
        assert_limit_error(
            limits
                .check_raw_png_bytes(limits.raw_png_bytes + 1)
                .unwrap_err(),
            ErrorCode::ImageTooLarge,
            SafeLimitKind::RawPngBytes,
            limits.raw_png_bytes,
            limits.raw_png_bytes + 1,
        );

        assert!(limits.check_math_svg_bytes(limits.math_svg_bytes).is_ok());
        assert_limit_error(
            limits
                .check_math_svg_bytes(limits.math_svg_bytes + 1)
                .unwrap_err(),
            ErrorCode::ImageTooLarge,
            SafeLimitKind::MathSvgBytes,
            limits.math_svg_bytes,
            limits.math_svg_bytes + 1,
        );
    }

    #[test]
    fn scaling_matches_hidpi_policy() {
        let limits = Limits::default();
        let scaled = limits.scaled(2);
        assert_eq!(scaled.image_width_px, limits.image_width_px * 2);
        assert_eq!(scaled.image_height_px, limits.image_height_px * 2);
        assert_eq!(scaled.image_pixels, limits.image_pixels * 4);
        assert_eq!(scaled.raw_png_bytes, limits.raw_png_bytes * 4);
        // SVG byte length does not grow with DPR (see `Limits::scaled`'s
        // `math_svg_bytes` doc comment): it stays exactly the unscaled limit.
        assert_eq!(scaled.math_svg_bytes, limits.math_svg_bytes);
    }

    #[test]
    fn scaling_clamps_dpr_and_saturates_instead_of_overflowing() {
        let limits = Limits {
            image_width_px: u32::MAX,
            image_height_px: u32::MAX,
            image_pixels: u64::MAX,
            raw_png_bytes: u64::MAX,
            math_svg_bytes: u64::MAX,
            ..Limits::default()
        };

        let scaled = limits.scaled(4);
        assert_eq!(scaled.device_pixel_ratio, 4);
        assert_eq!(scaled.image_width_px, u32::MAX);
        assert_eq!(scaled.image_height_px, u32::MAX);
        assert_eq!(scaled.image_pixels, u64::MAX);
        assert_eq!(scaled.raw_png_bytes, u64::MAX);
        assert_eq!(scaled.math_svg_bytes, u64::MAX);

        assert_eq!(limits.scaled(0).device_pixel_ratio, 1);
        assert_eq!(limits.scaled(u8::MAX).device_pixel_ratio, 4);
    }
}
