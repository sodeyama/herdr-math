use std::error::Error;
use std::fmt;

/// Fixed text color for the V3 dark theme.
pub const DARK_THEME_TEXT_COLOR: &str = "#e6edf3";

/// Layout options that affect rendered output and cache identity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderOptions {
    pub content_width_pt: f64,
    pub font_size_pt: f64,
    pub device_pixel_ratio: u8,
}

impl RenderOptions {
    /// Creates validated render options and clamps DPR to the supported range.
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
        })
    }
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            content_width_pt: 480.0,
            font_size_pt: 12.0,
            device_pixel_ratio: 1,
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
