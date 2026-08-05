//! In-process document rendering through the native V3 renderer.

use std::io::Cursor;

use tmath_core::placement::decode_png;
use tmath_render::{
    parse_blocks_limited, render_block, CjkFont, ErrorCode, Limits, RenderError, RenderOptions,
    RenderedBlock, SafeErrorDetails, SafeErrorRecord, SafeLimitKind,
};

const BLOCK_GAP_PX: u32 = 8;
const MAX_COMPOSITE_PIXELS: u64 = 64 * 1024 * 1024;

pub(crate) struct NativeRenderSuccess {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub formula_errors: usize,
}

/// Re-encodes a rendered block through the same RGBA PNG path used by the
/// one-shot compositor. Stream event byte counts therefore describe the exact
/// PNG a one-block one-shot native render would produce.
pub(crate) fn canonical_block_png(
    rendered: &RenderedBlock,
    max_pixels: u64,
) -> Result<Vec<u8>, RenderError> {
    let (width, height, rgba) = decode_png(&rendered.png, max_pixels)
        .map_err(|_| internal_error("native block PNG could not be decoded"))?;
    if width != rendered.width_px || height != rendered.height_px {
        return Err(internal_error(
            "native block PNG dimensions did not match its metadata",
        ));
    }
    encode_rgba(width, height, &rgba)
}

struct DecodedBlock {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

/// Renders all semantic blocks before composing one image, so a block failure
/// cannot emit a partial placement. Per-block placements arrive in Phase 2/3.
pub(crate) fn render_document_native(
    source: &str,
    content_width: u32,
    font_size: u32,
    device_pixel_ratio: u8,
    cjk_font: CjkFont,
) -> Result<NativeRenderSuccess, RenderError> {
    // The node renderer treats CLI pixels as CSS pixels. Use the same numeric
    // value as Typst points here so both engines receive the same layout input.
    let options = RenderOptions::new(
        f64::from(content_width),
        f64::from(font_size),
        device_pixel_ratio,
    )
    .map_err(|_| internal_error("native render options were invalid"))?
    .with_cjk_font(cjk_font);
    let limits = Limits::default();
    let blocks = parse_blocks_limited(source, &limits)?;
    let mut decoded = Vec::with_capacity(blocks.len());
    let mut formula_errors = 0usize;

    for block in &blocks {
        let rendered = render_block(block, &options)?;
        formula_errors = formula_errors.saturating_add(rendered.formula_errors.len());
        let scaled_limits = limits.scaled(device_pixel_ratio);
        let (width, height, rgba) = decode_png(&rendered.png, scaled_limits.image_pixels)
            .map_err(|_| internal_error("native block PNG could not be decoded"))?;
        if width != rendered.width_px || height != rendered.height_px {
            return Err(internal_error(
                "native block PNG dimensions did not match its metadata",
            ));
        }
        decoded.push(DecodedBlock {
            width,
            height,
            rgba,
        });
    }

    let composite_pixel_limit = limits
        .scaled(device_pixel_ratio)
        .image_pixels
        .min(MAX_COMPOSITE_PIXELS);
    let gap = BLOCK_GAP_PX
        .checked_mul(u32::from(device_pixel_ratio.clamp(1, 4)))
        .ok_or_else(|| image_too_large(None, None, None, composite_pixel_limit))?;
    let (width, height, rgba) = stack_blocks(&decoded, gap, &options, composite_pixel_limit)?;
    let png = encode_rgba(width, height, &rgba)?;

    Ok(NativeRenderSuccess {
        png,
        width,
        height,
        formula_errors,
    })
}

fn stack_blocks(
    blocks: &[DecodedBlock],
    gap: u32,
    options: &RenderOptions,
    pixel_limit: u64,
) -> Result<(u32, u32, Vec<u8>), RenderError> {
    if blocks.is_empty() {
        let width = ((options.content_width_pt * f64::from(options.device_pixel_ratio)).round()
            as u32)
            .max(1);
        if u64::from(width) > pixel_limit {
            return Err(image_too_large(
                Some(width),
                Some(1),
                Some(width.into()),
                pixel_limit,
            ));
        }
        return Ok((width, 1, vec![0; width as usize * 4]));
    }

    let width = blocks.iter().map(|block| block.width).max().unwrap_or(1);
    let gaps = gap
        .checked_mul(u32::try_from(blocks.len().saturating_sub(1)).unwrap_or(u32::MAX))
        .ok_or_else(|| image_too_large(Some(width), None, None, pixel_limit))?;
    let height = blocks.iter().try_fold(gaps, |total, block| {
        total
            .checked_add(block.height)
            .ok_or_else(|| image_too_large(Some(width), None, None, pixel_limit))
    })?;
    let pixels = u64::from(width)
        .checked_mul(u64::from(height))
        .ok_or_else(|| image_too_large(Some(width), Some(height), None, pixel_limit))?;
    if pixels > pixel_limit {
        return Err(image_too_large(
            Some(width),
            Some(height),
            Some(pixels),
            pixel_limit,
        ));
    }
    let byte_len =
        usize::try_from(pixels.checked_mul(4).ok_or_else(|| {
            image_too_large(Some(width), Some(height), Some(pixels), pixel_limit)
        })?)
        .map_err(|_| image_too_large(Some(width), Some(height), Some(pixels), pixel_limit))?;
    let mut composite = vec![0; byte_len];
    let destination_stride = width as usize * 4;
    let mut destination_y = 0u32;

    for block in blocks {
        let source_stride = block.width as usize * 4;
        for row in 0..block.height as usize {
            let source_start = row * source_stride;
            let destination_start = (destination_y as usize + row) * destination_stride;
            composite[destination_start..destination_start + source_stride]
                .copy_from_slice(&block.rgba[source_start..source_start + source_stride]);
        }
        destination_y = destination_y
            .checked_add(block.height)
            .and_then(|value| value.checked_add(gap))
            .unwrap_or(height);
    }

    Ok((width, height, composite))
}

fn encode_rgba(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, RenderError> {
    let mut output = Vec::new();
    {
        let mut encoder = png::Encoder::new(Cursor::new(&mut output), width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|_| internal_error("native composite PNG header encoding failed"))?;
        writer
            .write_image_data(rgba)
            .map_err(|_| internal_error("native composite PNG encoding failed"))?;
    }
    Ok(output)
}

fn internal_error(message: &'static str) -> RenderError {
    RenderError::new(
        SafeErrorRecord {
            code: ErrorCode::RendererFailed,
            retryable: false,
            details: None,
        },
        message,
    )
}

fn image_too_large(
    width: Option<u32>,
    height: Option<u32>,
    pixels: Option<u64>,
    limit: u64,
) -> RenderError {
    RenderError::new(
        SafeErrorRecord {
            code: ErrorCode::ImageTooLarge,
            retryable: false,
            details: Some(SafeErrorDetails {
                limit_kind: Some(SafeLimitKind::ImagePixels),
                limit: Some(limit),
                actual: pixels,
                width: width.map(u64::from),
                height: height.map(u64::from),
                ..SafeErrorDetails::default()
            }),
        },
        "native composite image exceeded its finite limit",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stack_preserves_source_order_and_transparent_gap() {
        let blocks = [
            DecodedBlock {
                width: 1,
                height: 1,
                rgba: vec![1, 2, 3, 4],
            },
            DecodedBlock {
                width: 1,
                height: 1,
                rgba: vec![5, 6, 7, 8],
            },
        ];
        let options = RenderOptions::default();
        let (width, height, rgba) =
            stack_blocks(&blocks, 1, &options, MAX_COMPOSITE_PIXELS).unwrap();
        assert_eq!((width, height), (1, 3));
        assert_eq!(&rgba[0..4], &[1, 2, 3, 4]);
        assert_eq!(&rgba[4..8], &[0, 0, 0, 0]);
        assert_eq!(&rgba[8..12], &[5, 6, 7, 8]);
    }
}
