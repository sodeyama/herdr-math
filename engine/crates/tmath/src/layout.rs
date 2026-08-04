//! Terminal-fitted auto layout for the native render paths.
//!
//! `tmath render --engine native`, `tmath watch`, and `tmath agent-viewer`
//! previously rasterized against fixed defaults (480 pt content width, 14 pt
//! font) regardless of the connected terminal's actual pane geometry, so the
//! rendered image often did not fit the pane and its font size did not match
//! the surrounding terminal text. [`terminal_fit_layout`] derives content
//! width, font size, and device pixel ratio from the terminal's measured
//! cell size and pane column count instead.
//!
//! This auto-fit applies only when a real terminal was connected and its
//! geometry measured (`connected.is_some()` at the call site). The
//! non-terminal summary/event-line paths — piped `tmath render --engine
//! native -` with no tty, the CLI's plain "ok width=... height=..." report,
//! and every hermetic hash/byte-parity test that drives those paths — keep
//! today's fixed defaults (480 pt / 14 pt / dpr 1) so their recorded output
//! stays stable. Explicit `--content-width`/`--font-size` CLI values always
//! override the derived numbers, exactly as before this change.

/// Assumed CSS-pixel width of one terminal cell on a standard-density
/// display; device pixel ratio is derived by comparing this against the
/// terminal's actually reported (physical-pixel) cell width.
const ASSUMED_CELL_WIDTH_PX: f64 = 8.0;

/// Fixed default content width (pt) used when no terminal is connected.
pub(crate) const DEFAULT_CONTENT_WIDTH_PT: f64 = 480.0;
/// Fixed default font size (pt) used when no terminal is connected.
pub(crate) const DEFAULT_FONT_SIZE_PT: f64 = 14.0;

const MIN_FONT_SIZE_PT: f64 = 10.0;
const MAX_FONT_SIZE_PT: f64 = 24.0;
const MIN_CONTENT_WIDTH_PT: f64 = 200.0;
const MAX_CONTENT_WIDTH_PT: f64 = 4096.0;
/// Columns reserved as a margin so the placed image's cell grid stays
/// narrower than the pane and nothing overflows or wraps.
const PANE_MARGIN_COLS: u32 = 2;
/// Typical terminal line-height-to-font-size ratio: a rendered glyph height
/// of roughly 0.62x the cell height matches the terminal's own text size.
const FONT_TO_CELL_HEIGHT_RATIO: f64 = 0.62;

/// Terminal-fitted layout derived from a connected terminal's measured cell
/// size and pane column count.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct TerminalFitLayout {
    pub content_width_pt: f64,
    pub font_size_pt: f64,
    pub device_pixel_ratio: u8,
}

/// Derives a content width, font size, and device pixel ratio that fit the
/// connected terminal's pane, so rendered images match the pane width and
/// the surrounding text size.
///
/// - `device_pixel_ratio` is `round(cell_w_px / 8.0)` clamped to `[1, 4]`
///   (the sole implementation of this ratio; every native call site uses
///   this function instead of computing it separately).
/// - `font_size_pt` is `(cell_h_px / dpr) * 0.62`, clamped to `[10, 24]` pt,
///   fixed for the session (never varies per block).
/// - `content_width_pt` is `((pane_cols - 2) * cell_w_px) / dpr`, clamped to
///   `[200, 4096]` pt, so the placement's cell grid is about `pane_cols - 2`
///   columns wide and fits inside the pane.
pub(crate) fn terminal_fit_layout(
    cell_w_px: u32,
    cell_h_px: u32,
    pane_cols: u32,
) -> TerminalFitLayout {
    let device_pixel_ratio = device_scale_factor((cell_w_px, cell_h_px));
    let dpr_f64 = f64::from(device_pixel_ratio);

    let font_size_pt = ((f64::from(cell_h_px) / dpr_f64) * FONT_TO_CELL_HEIGHT_RATIO)
        .clamp(MIN_FONT_SIZE_PT, MAX_FONT_SIZE_PT);

    let usable_cols = pane_cols.saturating_sub(PANE_MARGIN_COLS);
    let content_width_pt = ((f64::from(usable_cols) * f64::from(cell_w_px)) / dpr_f64)
        .clamp(MIN_CONTENT_WIDTH_PT, MAX_CONTENT_WIDTH_PT);

    TerminalFitLayout {
        content_width_pt,
        font_size_pt,
        device_pixel_ratio: device_pixel_ratio as u8,
    }
}

/// Rounds the terminal's reported physical cell width against the assumed
/// standard-density cell width, clamped to a sane HiDPI range, so PNGs are
/// rasterized at the density the terminal will actually display them at.
///
/// This is the single implementation of the device pixel ratio calculation;
/// every call site (one-shot render, stream mode, watch, and the agent
/// viewer) goes through this function or [`terminal_fit_layout`], which
/// calls it internally.
pub(crate) fn device_scale_factor(cell: (u32, u32)) -> u32 {
    let ratio = f64::from(cell.0) / ASSUMED_CELL_WIDTH_PX;
    (ratio.round() as u32).clamp(1, 4)
}

/// Resolves the effective content width in points: an explicit CLI override
/// (already in pixels, treated as CSS/Typst points like the rest of the
/// native pipeline) takes precedence; otherwise the terminal-fit value when
/// connected, otherwise the fixed default.
pub(crate) fn resolve_content_width_pt(
    explicit_px: Option<u32>,
    fitted: Option<TerminalFitLayout>,
) -> f64 {
    explicit_px
        .map(f64::from)
        .or_else(|| fitted.map(|layout| layout.content_width_pt))
        .unwrap_or(DEFAULT_CONTENT_WIDTH_PT)
}

/// Resolves the effective font size in points, with the same override
/// precedence as [`resolve_content_width_pt`].
pub(crate) fn resolve_font_size_pt(
    explicit_px: Option<u32>,
    fitted: Option<TerminalFitLayout>,
) -> f64 {
    explicit_px
        .map(f64::from)
        .or_else(|| fitted.map(|layout| layout.font_size_pt))
        .unwrap_or(DEFAULT_FONT_SIZE_PT)
}

/// Resolves the effective device pixel ratio: the terminal-fit value when a
/// terminal is connected, otherwise 1 (matching the fixed-default,
/// non-terminal path).
pub(crate) fn resolve_device_pixel_ratio(fitted: Option<TerminalFitLayout>) -> u8 {
    fitted.map_or(1, |layout| layout.device_pixel_ratio)
}

/// Derives the terminal-fitted layout from a connected terminal's measured
/// cell size and pane column count. `None` when no terminal is connected, or
/// when the pane column count could not be measured — every call site then
/// falls back to the fixed defaults through [`resolve_content_width_pt`],
/// [`resolve_font_size_pt`], and [`resolve_device_pixel_ratio`].
pub(crate) fn fitted_layout_for_connected(
    connected: &Option<(
        tmath_core::terminal::Terminal<tmath_core::terminal::StdioTty>,
        (u32, u32),
    )>,
) -> Option<TerminalFitLayout> {
    let (terminal, cell) = connected.as_ref()?;
    let pane_cols = terminal.size().ok()?.cols;
    Some(terminal_fit_layout(cell.0, cell.1, pane_cols))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retina_like_cell_yields_a_dpr_in_range_and_readable_font() {
        let layout = terminal_fit_layout(20, 40, 120);
        assert!(
            (2..=3).contains(&layout.device_pixel_ratio),
            "dpr {} out of expected 2-3 range",
            layout.device_pixel_ratio
        );
        assert!(
            (MIN_FONT_SIZE_PT..=MAX_FONT_SIZE_PT).contains(&layout.font_size_pt),
            "font size {} out of the readable clamp range",
            layout.font_size_pt
        );
        let expected_width =
            (f64::from(120 - PANE_MARGIN_COLS) * 20.0) / f64::from(layout.device_pixel_ratio);
        assert!(
            (layout.content_width_pt - expected_width).abs() < 1e-9,
            "width {} did not match (cols-2)*cell/dpr = {expected_width}",
            layout.content_width_pt
        );
    }

    #[test]
    fn standard_density_cell_yields_dpr_one_and_font_near_ten() {
        let layout = terminal_fit_layout(8, 16, 100);
        assert_eq!(layout.device_pixel_ratio, 1);
        // 16 * 0.62 = 9.92, just under the 10pt floor, so the clamp applies
        // and the resolved font size is exactly the floor.
        assert_eq!(layout.font_size_pt, MIN_FONT_SIZE_PT);
    }

    #[test]
    fn font_size_clamps_to_the_readable_range_at_both_ends() {
        let tiny_cell = terminal_fit_layout(4, 4, 80);
        assert_eq!(tiny_cell.font_size_pt, MIN_FONT_SIZE_PT);

        let huge_cell = terminal_fit_layout(32, 200, 80);
        assert_eq!(huge_cell.font_size_pt, MAX_FONT_SIZE_PT);
    }

    #[test]
    fn content_width_clamps_to_the_bounded_range_at_both_ends() {
        let narrow_pane = terminal_fit_layout(8, 16, 3);
        assert_eq!(narrow_pane.content_width_pt, MIN_CONTENT_WIDTH_PT);

        let huge_pane = terminal_fit_layout(8, 16, 4000);
        assert_eq!(huge_pane.content_width_pt, MAX_CONTENT_WIDTH_PT);
    }

    #[test]
    fn device_scale_factor_matches_standard_and_hidpi_cells() {
        assert_eq!(device_scale_factor((8, 16)), 1, "standard-density cell");
        assert_eq!(device_scale_factor((16, 32)), 2, "2x Retina cell");
        assert_eq!(device_scale_factor((24, 48)), 3, "3x Retina cell");
    }

    #[test]
    fn device_scale_factor_clamps_to_the_supported_range() {
        assert_eq!(device_scale_factor((1, 2)), 1, "tiny cell clamps to 1x");
        assert_eq!(device_scale_factor((200, 400)), 4, "huge cell clamps to 4x");
    }

    #[test]
    fn explicit_overrides_take_precedence_over_the_fitted_layout() {
        let fitted = terminal_fit_layout(20, 40, 120);
        assert_eq!(resolve_content_width_pt(Some(800), Some(fitted)), 800.0);
        assert_eq!(resolve_font_size_pt(Some(18), Some(fitted)), 18.0);
    }

    #[test]
    fn no_terminal_falls_back_to_fixed_defaults() {
        assert_eq!(
            resolve_content_width_pt(None, None),
            DEFAULT_CONTENT_WIDTH_PT
        );
        assert_eq!(resolve_font_size_pt(None, None), DEFAULT_FONT_SIZE_PT);
        assert_eq!(resolve_device_pixel_ratio(None), 1);
    }

    #[test]
    fn connected_terminal_without_explicit_override_uses_the_fitted_layout() {
        let fitted = terminal_fit_layout(20, 40, 120);
        assert_eq!(
            resolve_content_width_pt(None, Some(fitted)),
            fitted.content_width_pt
        );
        assert_eq!(
            resolve_font_size_pt(None, Some(fitted)),
            fitted.font_size_pt
        );
        assert_eq!(
            resolve_device_pixel_ratio(Some(fitted)),
            fitted.device_pixel_ratio
        );
    }
}
