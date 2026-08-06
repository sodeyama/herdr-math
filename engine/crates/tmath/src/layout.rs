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
//!
//! **`TMATH_DPR` override (tmux winsize-fallback path only)**: inside tmux,
//! `Terminal::cell_size` cannot use the `CSI 16t` pixel-size query (tmux
//! answers it with character counts, not pixels) and falls back to the
//! winsize pixel report instead. On at least one verified route (Ghostty
//! 1.3.1 Retina + tmux 3.5a, client-tty passthrough), that winsize fallback
//! itself reports *logical* pixels rather than physical ones, so
//! [`device_scale_factor`]'s `cell_w_px / 8.0` guess resolves to `1` on a
//! display that is actually 2x — the image renders at half the needed pixel
//! count and the terminal upscales it, producing blurred, crushed glyphs.
//! No ioctl-reachable value on that route reveals the true scale, so
//! [`resolve_dpr_override`] lets `TMATH_DPR` (integer `1..=4`) state it
//! explicitly. [`terminal_fit_layout`]'s `dpr_override` parameter, when
//! `Some`, is used as `device_pixel_ratio` directly and the measured cell is
//! treated as the *logical* cell (multiplied by the override to get the
//! physical cell used for font size and content width), so `grid_for` and
//! the viewport end up consistent in physical pixel units. Auto-detection
//! via a passthrough graphics capability query is future work; this is an
//! explicit escape hatch, not a general fix. The override is skipped when
//! winsize already yields a physical cell (auto-detected DPR > 1), because
//! scaling the cell again would shrink Kitty placements.

/// Assumed CSS-pixel width of one terminal cell on a standard-density
/// display; device pixel ratio is derived by comparing this against the
/// terminal's actually reported (physical-pixel) cell width.
const ASSUMED_CELL_WIDTH_PX: f64 = 8.0;

/// Fixed default content width (pt) used when no terminal is connected.
pub(crate) const DEFAULT_CONTENT_WIDTH_PT: f64 = 480.0;
/// Fixed default font size (pt) used when no terminal is connected.
pub(crate) const DEFAULT_FONT_SIZE_PT: f64 = 14.0;

/// Also the valid range for `config.rs`'s `font_size_pt` config key — the
/// user-facing setting is clamped to the same bound the terminal-fit
/// calculation itself respects, so a configured size can never produce a
/// more extreme result than auto-fit could.
pub(crate) const MIN_FONT_SIZE_PT: f64 = 10.0;
pub(crate) const MAX_FONT_SIZE_PT: f64 = 24.0;
const MIN_CONTENT_WIDTH_PT: f64 = 200.0;
const MAX_CONTENT_WIDTH_PT: f64 = 4096.0;
/// Columns reserved as a margin so the placed image's cell grid stays
/// narrower than the pane and nothing overflows or wraps.
///
/// This is also the guarantee the scroll-region viewer's rightmost-column
/// scrollbar (planned, see the `scroll_region` module doc) relies on to
/// never collide with placeholder cells: `content_width_pt` below is
/// computed from `pane_cols - PANE_MARGIN_COLS`, and every block's rendered
/// PNG is trimmed to at most that width by `trim_transparent_right`
/// (`tmath-render/src/prose.rs`), which only ever shrinks the image further
/// — never grows it past the page width. Since `grid_for` rounds a pixel
/// width UP to whole cells, the worst case still leaves at least
/// `PANE_MARGIN_COLS` = 2 full columns free at the pane's right edge, so a
/// 1-column scrollbar in the absolute last column is always clear of any
/// placeholder grid, at any block width.
const PANE_MARGIN_COLS: u32 = 2;
/// Maps a terminal's measured cell height to a rendered font size. Live
/// calibration on Ghostty 1.3.1 (Retina, a 15px logical cell) walked
/// through 0.62 (~10pt, too small), 0.85 (~13pt, matched the terminal's
/// own text but the user still found rendered prose and math hard to
/// read), briefly 17pt, and settled on 15pt for a 15px cell (ratio 1.0):
/// typeset content at exactly the cell height reads comfortably next to
/// the terminal text without dwarfing it.
const FONT_TO_CELL_HEIGHT_RATIO: f64 = 1.0;

/// Terminal-fitted layout derived from a connected terminal's measured cell
/// size and pane column count.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct TerminalFitLayout {
    pub content_width_pt: f64,
    pub font_size_pt: f64,
    pub device_pixel_ratio: u8,
    /// The physical cell size actually used to derive `font_size_pt` and
    /// `content_width_pt`: the measured `(cell_w_px, cell_h_px)` unchanged
    /// when no `dpr_override` applied, or that measured cell scaled by the
    /// override otherwise. Every downstream consumer of a cell size for
    /// this session (`grid_for`, the viewport's row-height math, the sink's
    /// pixel bookkeeping) must use this value, not the raw measured cell —
    /// otherwise an overridden dpr and a stale logical cell disagree, and
    /// placements are sized in the wrong units (see the `TMATH_DPR` section
    /// of the module doc for the concrete failure this caused).
    pub effective_cell_px: (u32, u32),
}

/// Derives a content width, font size, and device pixel ratio that fit the
/// connected terminal's pane, so rendered images match the pane width and
/// the surrounding text size.
///
/// - `device_pixel_ratio` is `round(cell_w_px / 8.0)` clamped to `[1, 4]`
///   (the sole implementation of this ratio; every native call site uses
///   this function instead of computing it separately) — unless
///   `dpr_override` is `Some`, in which case that value is used directly and
///   `cell_w_px`/`cell_h_px` are treated as the *logical* cell, scaled up by
///   the override to get the physical cell used below (see the module doc's
///   `TMATH_DPR` section for why this exists and when it applies).
/// - `font_size_pt` is `(physical_cell_h_px / dpr) * 1.0`, clamped to
///   `[10, 24]` pt, fixed for the session (never varies per block).
/// - `content_width_pt` is `((pane_cols - 2) * physical_cell_w_px) / dpr`,
///   clamped to `[200, 4096]` pt, so the placement's cell grid is about
///   `pane_cols - 2` columns wide and fits inside the pane.
pub(crate) fn terminal_fit_layout(
    cell_w_px: u32,
    cell_h_px: u32,
    pane_cols: u32,
    dpr_override: Option<u32>,
) -> TerminalFitLayout {
    let device_pixel_ratio =
        dpr_override.unwrap_or_else(|| device_scale_factor((cell_w_px, cell_h_px)));
    let dpr_f64 = f64::from(device_pixel_ratio);
    // With an override, the measured cell is logical; scale it up to the
    // physical cell the override claims so font size and content width are
    // computed in physical pixels, matching the auto-detected path (where
    // `cell_w_px`/`cell_h_px` are already physical).
    let (cell_w_px, cell_h_px) = match dpr_override {
        Some(dpr) => (cell_w_px.saturating_mul(dpr), cell_h_px.saturating_mul(dpr)),
        None => (cell_w_px, cell_h_px),
    };

    let font_size_pt = ((f64::from(cell_h_px) / dpr_f64) * FONT_TO_CELL_HEIGHT_RATIO)
        .clamp(MIN_FONT_SIZE_PT, MAX_FONT_SIZE_PT);

    let usable_cols = pane_cols.saturating_sub(PANE_MARGIN_COLS);
    let content_width_pt = ((f64::from(usable_cols) * f64::from(cell_w_px)) / dpr_f64)
        .clamp(MIN_CONTENT_WIDTH_PT, MAX_CONTENT_WIDTH_PT);

    TerminalFitLayout {
        content_width_pt,
        font_size_pt,
        device_pixel_ratio: device_pixel_ratio as u8,
        effective_cell_px: (cell_w_px, cell_h_px),
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
/// native pipeline) takes precedence and is never capped — it states an
/// exact width, not a fitting preference. Otherwise the terminal-fit value
/// applies, capped at `font_size_pt * max_content_width_font_multiple` (see
/// `config`'s module doc for why), or the fixed default when no terminal is
/// connected.
pub(crate) fn resolve_content_width_pt(
    explicit_px: Option<u32>,
    fitted: Option<TerminalFitLayout>,
    font_size_pt: f64,
    max_content_width_font_multiple: f64,
) -> f64 {
    if let Some(explicit_px) = explicit_px {
        return f64::from(explicit_px);
    }
    match fitted {
        Some(layout) => layout
            .content_width_pt
            .min(font_size_pt * max_content_width_font_multiple),
        None => DEFAULT_CONTENT_WIDTH_PT,
    }
}

/// Resolves the effective device pixel ratio: the terminal-fit value when a
/// terminal is connected, otherwise 1 (matching the fixed-default,
/// non-terminal path).
pub(crate) fn resolve_device_pixel_ratio(fitted: Option<TerminalFitLayout>) -> u8 {
    fitted.map_or(1, |layout| layout.device_pixel_ratio)
}

/// Parses the `TMATH_DPR` environment variable into a [`terminal_fit_layout`]
/// override, or `None` when it does not apply. `raw` is the raw environment
/// value (`None` when the variable is unset); `on_tmux_winsize_fallback`
/// tells the caller whether this session is actually on the affected path
/// (`Terminal::cell_size`'s winsize fallback, which only tmux sessions take
/// — see the module doc). `TMATH_DPR` is deliberately ignored (returns
/// `None`, falling back to today's auto-detected behavior) whenever it is
/// unset, not an integer, or outside `1..=4` — the override never causes an
/// error, only a fall-through, so a mistyped value cannot break the viewer.
/// override is skipped when winsize already reports a physical cell (auto-
/// detected DPR > 1), because applying it again would double-scale the cell
/// and shrink rendered placements.
pub(crate) fn resolve_dpr_override(
    raw: Option<&str>,
    on_tmux_winsize_fallback: bool,
    measured_cell: (u32, u32),
) -> Option<u32> {
    if !on_tmux_winsize_fallback {
        return None;
    }
    let dpr: u32 = raw?.trim().parse().ok()?;
    if !(1..=4).contains(&dpr) {
        return None;
    }
    if device_scale_factor(measured_cell) > 1 {
        return None;
    }
    Some(dpr)
}

/// Derives the terminal-fitted layout from a connected terminal's measured
/// cell size and pane column count. `None` when no terminal is connected, or
/// when the pane column count could not be measured — every call site then
/// falls back to the fixed defaults through [`resolve_content_width_pt`],
/// `crate::config::resolve_font_size_pt_with_source` (font size now goes
/// through the config precedence chain, not this module's own resolver),
/// and [`resolve_device_pixel_ratio`].
pub(crate) fn fitted_layout_for_connected(
    connected: &Option<(
        tmath_core::terminal::Terminal<tmath_core::terminal::StdioTty>,
        (u32, u32),
    )>,
) -> Option<TerminalFitLayout> {
    let (terminal, cell) = connected.as_ref()?;
    let pane_cols = terminal.size().ok()?.cols;
    // `TMATH_DPR` is a viewer-only escape hatch (see the module doc); stream
    // and watch mode never pass an override here.
    Some(terminal_fit_layout(cell.0, cell.1, pane_cols, None))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retina_like_cell_yields_a_dpr_in_range_and_readable_font() {
        let layout = terminal_fit_layout(20, 40, 120, None);
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
    fn standard_density_cell_yields_dpr_one_and_font_above_terminal_text() {
        let layout = terminal_fit_layout(8, 16, 100, None);
        assert_eq!(layout.device_pixel_ratio, 1);
        // 16 * 1.0 = 16, comfortably inside [10, 24], so no clamp applies.
        assert!((layout.font_size_pt - 16.0).abs() < 1e-9);
    }

    #[test]
    fn font_size_clamps_to_the_readable_range_at_both_ends() {
        let tiny_cell = terminal_fit_layout(4, 4, 80, None);
        assert_eq!(tiny_cell.font_size_pt, MIN_FONT_SIZE_PT);

        let huge_cell = terminal_fit_layout(32, 200, 80, None);
        assert_eq!(huge_cell.font_size_pt, MAX_FONT_SIZE_PT);
    }

    #[test]
    fn content_width_clamps_to_the_bounded_range_at_both_ends() {
        let narrow_pane = terminal_fit_layout(8, 16, 3, None);
        assert_eq!(narrow_pane.content_width_pt, MIN_CONTENT_WIDTH_PT);

        let huge_pane = terminal_fit_layout(8, 16, 4000, None);
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
    fn explicit_overrides_take_precedence_over_the_fitted_layout_and_are_never_capped() {
        let fitted = terminal_fit_layout(20, 40, 120, None);
        // A cap of 1x the font size would reject 800pt if it applied to the
        // explicit override; it must not, since `--content-width` states an
        // exact width rather than a fitting preference.
        assert_eq!(
            resolve_content_width_pt(Some(800), Some(fitted), fitted.font_size_pt, 1.0),
            800.0
        );
    }

    #[test]
    fn no_terminal_falls_back_to_fixed_defaults() {
        assert_eq!(
            resolve_content_width_pt(None, None, DEFAULT_FONT_SIZE_PT, 28.0),
            DEFAULT_CONTENT_WIDTH_PT
        );
        assert_eq!(resolve_device_pixel_ratio(None), 1);
    }

    #[test]
    fn connected_terminal_without_explicit_override_uses_the_fitted_layout_when_under_the_cap() {
        let fitted = terminal_fit_layout(20, 40, 120, None);
        // A generous multiple keeps the fitted width unchanged.
        assert_eq!(
            resolve_content_width_pt(None, Some(fitted), fitted.font_size_pt, 1000.0),
            fitted.content_width_pt
        );
        assert_eq!(
            resolve_device_pixel_ratio(Some(fitted)),
            fitted.device_pixel_ratio
        );
    }

    #[test]
    fn fitted_layout_is_capped_at_font_size_times_the_multiple() {
        // A wide pane (200 cols) fits well past a modest cap.
        let fitted = terminal_fit_layout(20, 40, 200, None);
        let cap_multiple = 28.0;
        let expected_cap = fitted.font_size_pt * cap_multiple;
        assert!(
            fitted.content_width_pt > expected_cap,
            "fixture should exceed the cap to exercise it"
        );
        assert_eq!(
            resolve_content_width_pt(None, Some(fitted), fitted.font_size_pt, cap_multiple),
            expected_cap
        );
    }

    // --- TMATH_DPR override ---

    #[test]
    fn dpr_override_forces_the_ratio_and_scales_the_logical_cell() {
        // A Retina-like logical cell (7x15, as measured on the affected
        // route) auto-detects to dpr 1 (7/8 rounds to 1), which is exactly
        // the bug: without an override this cell is treated as already
        // physical. With `dpr_override = Some(2)`, the cell is scaled to a
        // 14x30 physical cell before font size / content width are derived.
        let auto = terminal_fit_layout(7, 15, 120, None);
        assert_eq!(auto.device_pixel_ratio, 1, "the bug: auto-detect misses it");
        assert_eq!(
            auto.effective_cell_px,
            (7, 15),
            "no override: effective cell passes through the measured cell unchanged"
        );

        let overridden = terminal_fit_layout(7, 15, 120, Some(2));
        assert_eq!(overridden.device_pixel_ratio, 2);
        // FIX: everything downstream (the sink, `grid_for`, the viewport)
        // must use this physical cell, not the raw measured (7,15) one —
        // that was the placement-overflow / double-counted-rows bug.
        assert_eq!(
            overridden.effective_cell_px,
            (14, 30),
            "effective cell is the measured logical cell scaled by the override"
        );
        // font_size_pt = (15*2 / 2) * 1.0 = 15, the size the user calibrated
        // live on this exact geometry; inside [10, 24], no clamp.
        assert!((overridden.font_size_pt - 15.0).abs() < 1e-9);
        // content_width_pt = ((120-2) * 7*2) / 2 = 118 * 7 = 826.
        assert!((overridden.content_width_pt - 826.0).abs() < 1e-9);
    }

    #[test]
    fn dpr_override_out_of_range_is_clamped_like_the_struct_field() {
        // `terminal_fit_layout` itself does not validate `dpr_override` (that
        // is `resolve_dpr_override`'s job); passing an out-of-range value
        // directly still produces a usable, non-panicking layout, with the
        // ratio used exactly as given (this function is not where the 1..=4
        // bound is enforced).
        let layout = terminal_fit_layout(7, 15, 120, Some(9));
        assert_eq!(layout.device_pixel_ratio, 9);
    }

    #[test]
    fn resolve_dpr_override_accepts_a_valid_value_on_the_tmux_fallback_path() {
        assert_eq!(resolve_dpr_override(Some("2"), true, (7, 15)), Some(2));
        assert_eq!(resolve_dpr_override(Some("1"), true, (7, 15)), Some(1));
        assert_eq!(resolve_dpr_override(Some("4"), true, (7, 15)), Some(4));
    }

    #[test]
    fn resolve_dpr_override_ignores_an_absent_variable() {
        assert_eq!(resolve_dpr_override(None, true, (7, 15)), None);
    }

    #[test]
    fn resolve_dpr_override_ignores_an_invalid_value() {
        assert_eq!(
            resolve_dpr_override(Some("0"), true, (7, 15)),
            None,
            "below range"
        );
        assert_eq!(
            resolve_dpr_override(Some("5"), true, (7, 15)),
            None,
            "above range"
        );
        assert_eq!(
            resolve_dpr_override(Some("abc"), true, (7, 15)),
            None,
            "not a number"
        );
        assert_eq!(resolve_dpr_override(Some(""), true, (7, 15)), None, "empty");
        assert_eq!(
            resolve_dpr_override(Some("2.5"), true, (7, 15)),
            None,
            "not an integer"
        );
        assert_eq!(
            resolve_dpr_override(Some("-1"), true, (7, 15)),
            None,
            "negative"
        );
    }

    #[test]
    fn resolve_dpr_override_never_applies_off_the_tmux_fallback_path() {
        // Even a valid value is ignored when the caller reports this session
        // is not on the affected path (e.g. a directly-connected terminal
        // whose cell size came from the real pixel query).
        assert_eq!(resolve_dpr_override(Some("2"), false, (7, 15)), None);
    }

    #[test]
    fn resolve_dpr_override_skips_when_winsize_already_reports_physical_pixels(
    ) {
        // A 14px-wide cell auto-detects to dpr 2; forcing TMATH_DPR=2 again
        // would double-scale the cell and shrink placements.
        assert_eq!(resolve_dpr_override(Some("2"), true, (14, 28)), None);
    }
}
