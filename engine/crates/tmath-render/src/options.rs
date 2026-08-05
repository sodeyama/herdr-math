use std::error::Error;
use std::fmt;

/// Fixed text color for the V3 dark theme.
pub const DARK_THEME_TEXT_COLOR: &str = "#e6edf3";

/// Table border stroke color: a mid-luminance gray derived from
/// [`DARK_THEME_TEXT_COLOR`], mixed 57.5% toward a representative dark
/// terminal background (`#0d1117`, GitHub Dark's background — the natural
/// pairing for a text color that is itself GitHub Dark's foreground) so a
/// stroke reads clearly on real dark terminal backgrounds without being as
/// bright as body text. Rendered PNGs have a transparent background (`#set
/// page(fill: none)`), so this is a fixed design choice, not a color read
/// from the actual terminal — it only needs to work across the dark
/// backgrounds real terminals plausibly use, not be theme-accurate.
///
/// Derivation (`round(text + (bg - text) * 0.575)` per channel):
/// `e6edf3` → `(230, 237, 243)`, `0d1117` → `(13, 17, 23)`,
/// mixed → `(105, 111, 117)` = `#696f75`.
pub const TABLE_STROKE_COLOR: &str = "#696f75";

/// One of the CJK fonts embedded in the binary (D-CONFIG phase 2). Shaped as
/// an enum (rather than a bare `String`) so the valid set is closed and
/// exhaustive-matched everywhere it is consumed — adding a second embedded
/// family later is a new variant plus a new `include_bytes!` asset, not a
/// string-comparison bug waiting to happen. AGENTS.md requires every font to
/// be embedded in the binary (no system font scan, no arbitrary file
/// loading), so this — not a free-form path or family-name string — is the
/// only way `cjk_font` in `config.toml` can select a font.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub enum CjkFont {
    /// M PLUS 2 (`engine/crates/tmath-render/assets/fonts/`), the current
    /// default and, for now, the only embedded CJK family.
    #[default]
    MPlus2,
}

impl CjkFont {
    /// The `config.toml` `cjk_font` value that selects this font (kebab-case
    /// slug, per D-CONFIG's Phase 1 convention for config value spelling).
    pub const fn slug(self) -> &'static str {
        match self {
            Self::MPlus2 => "m-plus-2",
        }
    }

    /// Parses a `config.toml` `cjk_font` value. `None` for anything that is
    /// not exactly one of the known slugs — the caller is expected to warn
    /// and fall back to `CjkFont::default()`, the same fail-closed pattern
    /// `config::load`'s other keys already use.
    pub fn from_slug(slug: &str) -> Option<Self> {
        match slug {
            "m-plus-2" => Some(Self::MPlus2),
            _ => None,
        }
    }

    /// The exact Typst family name this font's embedded OTF/TTF declares in
    /// its own `name` table — what must appear in the `#set text(font:
    /// (...))` fallback list for Typst to actually select it.
    pub const fn typst_family_name(self) -> &'static str {
        match self {
            Self::MPlus2 => "M PLUS 2",
        }
    }
}

/// Layout options that affect rendered output and cache identity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderOptions {
    pub content_width_pt: f64,
    pub font_size_pt: f64,
    pub device_pixel_ratio: u8,
    pub cjk_font: CjkFont,
}

impl RenderOptions {
    /// Creates validated render options and clamps DPR to the supported
    /// range. `cjk_font` defaults to `CjkFont::default()`; use
    /// [`RenderOptions::with_cjk_font`] to select a different embedded
    /// family.
    pub fn new(
        content_width_pt: f64,
        font_size_pt: f64,
        device_pixel_ratio: u8,
    ) -> Result<Self, RenderOptionsError> {
        if !content_width_pt.is_finite() || content_width_pt <= 0.0 {
            return Err(RenderOptionsError::InvalidContentWidth);
        }
        if !font_size_pt.is_finite() || font_size_pt <= 0.0 {
            return Err(RenderOptionsError::InvalidFontSize);
        }

        Ok(Self {
            content_width_pt,
            font_size_pt,
            device_pixel_ratio: device_pixel_ratio.clamp(1, 4),
            cjk_font: CjkFont::default(),
        })
    }

    /// Selects a specific embedded CJK family, overriding the default.
    pub fn with_cjk_font(mut self, cjk_font: CjkFont) -> Self {
        self.cjk_font = cjk_font;
        self
    }
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            content_width_pt: 480.0,
            font_size_pt: 12.0,
            device_pixel_ratio: 1,
            cjk_font: CjkFont::default(),
        }
    }
}

/// Validation failure for render layout options.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RenderOptionsError {
    InvalidContentWidth,
    InvalidFontSize,
}

impl fmt::Display for RenderOptionsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidContentWidth => "content width must be positive and finite",
            Self::InvalidFontSize => "font size must be positive and finite",
        };
        formatter.write_str(message)
    }
}

impl Error for RenderOptionsError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_v3_layout_contract() {
        let options = RenderOptions::default();
        assert_eq!(options.content_width_pt, 480.0);
        assert_eq!(options.font_size_pt, 12.0);
        assert_eq!(options.device_pixel_ratio, 1);
        assert_eq!(DARK_THEME_TEXT_COLOR, "#e6edf3");
    }

    #[test]
    fn constructor_rejects_invalid_dimensions() {
        for width in [0.0, -1.0, f64::INFINITY, f64::NEG_INFINITY, f64::NAN] {
            assert!(RenderOptions::new(width, 12.0, 1).is_err());
        }
        for font_size in [0.0, -1.0, f64::INFINITY, f64::NEG_INFINITY, f64::NAN] {
            assert!(RenderOptions::new(480.0, font_size, 1).is_err());
        }
    }

    #[test]
    fn constructor_clamps_device_pixel_ratio() {
        assert_eq!(
            RenderOptions::new(480.0, 12.0, 0)
                .unwrap()
                .device_pixel_ratio,
            1
        );
        assert_eq!(
            RenderOptions::new(480.0, 12.0, 2)
                .unwrap()
                .device_pixel_ratio,
            2
        );
        assert_eq!(
            RenderOptions::new(480.0, 12.0, u8::MAX)
                .unwrap()
                .device_pixel_ratio,
            4
        );
    }
}
