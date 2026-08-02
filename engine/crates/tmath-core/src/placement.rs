//! Scrollback-anchored image placement.
//!
//! A rendered document block is decoded from PNG to RGBA and transmitted as one
//! virtual Kitty placement (`U=1,c,r`) glued to real main-buffer cells through a
//! placeholder grid, so the image scrolls with the shell scrollback. The
//! [`PlacementTracker`] owns image ids, replacement/delete of stale blocks, and
//! the concurrent-placement and total-pixel limits.

use std::io;

use crate::kitty::{self, Placement, MAX_PLACEHOLDER_CELLS};

/// Measured pixel size of one terminal cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellSize {
    pub width: u32,
    pub height: u32,
}

/// A block placed in the main buffer and tracked for later replacement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlacedBlock {
    pub image_id: u32,
    pub cols: u32,
    pub rows: u32,
    pub pixels: u64,
}

/// Bounds applied before any placement is emitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlacementLimits {
    pub max_concurrent_placements: usize,
    pub max_total_pixels: u64,
}

impl Default for PlacementLimits {
    fn default() -> Self {
        Self {
            max_concurrent_placements: 64,
            max_total_pixels: 64 * 1024 * 1024,
        }
    }
}

/// Why a placement was rejected before any bytes were emitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlacementError {
    TooManyPlacements { limit: usize, actual: usize },
    TooManyPixels { limit: u64, actual: u64 },
    InvalidImage { width: u32, height: u32 },
}

impl std::fmt::Display for PlacementError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlacementError::TooManyPlacements { limit, actual } => {
                write!(f, "placement count {actual} exceeds limit {limit}")
            }
            PlacementError::TooManyPixels { limit, actual } => {
                write!(f, "placement pixels {actual} exceed limit {limit}")
            }
            PlacementError::InvalidImage { width, height } => {
                write!(f, "invalid image dimensions {width}x{height}")
            }
        }
    }
}

/// Computes the cell grid a virtual placement occupies for a pixel image.
pub fn grid_for(width_px: u32, height_px: u32, cell: CellSize) -> (u32, u32) {
    let cols = width_px
        .div_ceil(cell.width.max(1))
        .clamp(1, MAX_PLACEHOLDER_CELLS);
    let rows = height_px
        .div_ceil(cell.height.max(1))
        .clamp(1, MAX_PLACEHOLDER_CELLS);
    (cols, rows)
}

/// Decodes a transparent PNG into RGBA bytes, rejecting empty or out-of-bounds
/// images before any transmission.
pub fn decode_png(
    png_bytes: &[u8],
    max_pixels: u64,
) -> Result<(u32, u32, Vec<u8>), PlacementError> {
    let mut decoder = png::Decoder::new(io::Cursor::new(png_bytes));
    // Expand palette/16-bit input to 8-bit RGBA so the payload is always the
    // `f=32` RGBA format regardless of how the renderer optimized its PNG.
    decoder.set_transformations(
        png::Transformations::normalize_to_color8() | png::Transformations::ALPHA,
    );
    let mut reader = decoder
        .read_info()
        .map_err(|_| PlacementError::InvalidImage {
            width: 0,
            height: 0,
        })?;
    let buffer_size = reader
        .output_buffer_size()
        .ok_or(PlacementError::InvalidImage {
            width: 0,
            height: 0,
        })?;
    let mut buffer = vec![0u8; buffer_size];
    let info = reader
        .next_frame(&mut buffer)
        .map_err(|_| PlacementError::InvalidImage {
            width: 0,
            height: 0,
        })?;
    let width = info.width;
    let height = info.height;
    if width == 0 || height == 0 || (width as u64).saturating_mul(height as u64) > max_pixels {
        return Err(PlacementError::InvalidImage { width, height });
    }
    let (color, depth) = reader.output_color_type();
    if color != png::ColorType::Rgba || depth != png::BitDepth::Eight {
        return Err(PlacementError::InvalidImage { width, height });
    }
    Ok((width, height, buffer))
}

/// Tracks placed blocks and owns image ids and the placement limits.
#[derive(Debug)]
pub struct PlacementTracker {
    next_image_id: u32,
    active: Vec<PlacedBlock>,
    limits: PlacementLimits,
}

impl PlacementTracker {
    pub fn new(limits: PlacementLimits) -> Self {
        Self {
            next_image_id: 1,
            active: Vec::new(),
            limits,
        }
    }

    /// Reserves an image id for a block of the given pixel size, enforcing the
    /// concurrency and total-pixel limits before any emission.
    pub fn reserve(
        &mut self,
        width_px: u32,
        height_px: u32,
        cell: CellSize,
    ) -> Result<PlacedBlock, PlacementError> {
        if (width_px as u64).saturating_mul(height_px as u64) == 0 {
            return Err(PlacementError::InvalidImage {
                width: width_px,
                height: height_px,
            });
        }
        if self.active.len() >= self.limits.max_concurrent_placements {
            return Err(PlacementError::TooManyPlacements {
                limit: self.limits.max_concurrent_placements,
                actual: self.active.len(),
            });
        }
        let pixels = (width_px as u64).saturating_mul(height_px as u64);
        let running: u64 = self.active.iter().map(|block| block.pixels).sum();
        if running.saturating_add(pixels) > self.limits.max_total_pixels {
            return Err(PlacementError::TooManyPixels {
                limit: self.limits.max_total_pixels,
                actual: running.saturating_add(pixels),
            });
        }
        let (cols, rows) = grid_for(width_px, height_px, cell);
        let block = PlacedBlock {
            image_id: self.next_image_id,
            cols,
            rows,
            pixels,
        };
        self.next_image_id = self.next_image_id.wrapping_add(1);
        // The pixel budget is claimed now; the caller replaces/removes on failure.
        self.active.push(block);
        Ok(block)
    }

    /// Removes a placed block and returns its image id for deletion.
    pub fn remove(&mut self, image_id: u32) -> Option<PlacedBlock> {
        let index = self
            .active
            .iter()
            .position(|block| block.image_id == image_id)?;
        Some(self.active.remove(index))
    }

    /// Replaces a placed block's pixel dimensions, returning the block with the
    /// same image id, or `None` when the id is unknown.
    pub fn replace(
        &mut self,
        image_id: u32,
        width_px: u32,
        height_px: u32,
        cell: CellSize,
    ) -> Result<PlacedBlock, PlacementError> {
        let index = self
            .active
            .iter()
            .position(|block| block.image_id == image_id)
            .ok_or(PlacementError::InvalidImage {
                width: 0,
                height: 0,
            })?;
        let (cols, rows) = grid_for(width_px, height_px, cell);
        let pixels = (width_px as u64).saturating_mul(height_px as u64);
        let other_pixels: u64 = self
            .active
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != index)
            .map(|(_, block)| block.pixels)
            .sum();
        if other_pixels.saturating_add(pixels) > self.limits.max_total_pixels {
            return Err(PlacementError::TooManyPixels {
                limit: self.limits.max_total_pixels,
                actual: other_pixels.saturating_add(pixels),
            });
        }
        let block = PlacedBlock {
            image_id,
            cols,
            rows,
            pixels,
        };
        self.active[index] = block;
        Ok(block)
    }

    pub fn active(&self) -> &[PlacedBlock] {
        &self.active
    }

    /// The row at which a newly placed block begins, computed as the sum of the
    /// rows of every earlier block plus one.
    pub fn home_row_for_next(&self) -> u32 {
        self.active
            .iter()
            .map(|block| block.rows)
            .fold(0u32, u32::saturating_add)
            + 1
    }

    /// The home row of an existing placed block, or `None` when the id is
    /// unknown.
    pub fn home_row_of(&self, image_id: u32) -> Option<u32> {
        let mut row = 1u32;
        for block in &self.active {
            if block.image_id == image_id {
                return Some(row);
            }
            row = row.saturating_add(block.rows);
        }
        None
    }

    pub fn limits(&self) -> &PlacementLimits {
        &self.limits
    }
}

/// Builds the byte sequence that places one block at a home row in the main
/// buffer: move the cursor home, transmit the virtual placement, then write the
/// placeholder grid that glues the image to real scrollback cells.
pub fn emit_placed_block(
    image_id: u32,
    width_px: u32,
    height_px: u32,
    rgba: &[u8],
    cols: u32,
    rows: u32,
    home_row: u32,
) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(format!("\x1b[{home_row};1H").as_bytes());
    out.extend_from_slice(&kitty::kitty_transmit_placed(
        image_id,
        width_px,
        height_px,
        rgba,
        Placement::Cells { cols, rows },
    ));
    out.extend_from_slice(&placeholder_grid_at_cursor(image_id, cols, rows));
    out
}

/// Builds the byte sequence that replaces a block: delete the stale image by id
/// first, then place the new image at the same home row.
pub fn emit_replaced_block(
    image_id: u32,
    width_px: u32,
    height_px: u32,
    rgba: &[u8],
    cols: u32,
    rows: u32,
    home_row: u32,
) -> Vec<u8> {
    let mut out = kitty::kitty_delete_id(image_id);
    out.extend_from_slice(&emit_placed_block(
        image_id, width_px, height_px, rgba, cols, rows, home_row,
    ));
    out
}

/// Builds the byte sequence that removes a placed block, deleting its image and
/// leaving the surrounding cells unchanged.
pub fn emit_remove_block(image_id: u32) -> Vec<u8> {
    kitty::kitty_delete_id(image_id)
}

/// Writes a placeholder grid relative to the current cursor position instead of
/// absolute rows, so a block can be placed anywhere in the main buffer.
fn placeholder_grid_at_cursor(image_id: u32, cols: u32, rows: u32) -> Vec<u8> {
    let cols = cols.min(MAX_PLACEHOLDER_CELLS) as usize;
    let rows = rows.min(MAX_PLACEHOLDER_CELLS) as usize;
    let (r, g, b) = (image_id >> 16 & 0xff, image_id >> 8 & 0xff, image_id & 0xff);
    let mut out = format!("\x1b[38;2;{r};{g};{b}m").into_bytes();
    for row in 0..rows {
        let row_di = kitty::ROW_COLUMN_DIACRITICS[row];
        for col in 0..cols {
            let col_di = kitty::ROW_COLUMN_DIACRITICS[col];
            push_char(&mut out, kitty::PLACEHOLDER);
            push_char(&mut out, row_di);
            push_char(&mut out, col_di);
        }
        if row + 1 < rows {
            out.extend_from_slice(b"\r\n");
        }
    }
    out.extend_from_slice(b"\x1b[39m");
    out
}

fn push_char(out: &mut Vec<u8>, ch: char) {
    let mut buf = [0u8; 4];
    out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_for_rounds_up_cells() {
        let cell = CellSize {
            width: 10,
            height: 20,
        };
        assert_eq!(grid_for(0, 0, cell), (1, 1));
        assert_eq!(grid_for(10, 20, cell), (1, 1));
        assert_eq!(grid_for(11, 21, cell), (2, 2));
        assert_eq!(grid_for(100, 200, cell), (10, 10));
    }

    #[test]
    fn grid_for_clamps_to_addressable_cells() {
        let cell = CellSize {
            width: 1,
            height: 1,
        };
        let (cols, rows) = grid_for(u32::MAX, u32::MAX, cell);
        assert_eq!(cols, MAX_PLACEHOLDER_CELLS);
        assert_eq!(rows, MAX_PLACEHOLDER_CELLS);
    }

    #[test]
    fn reserve_enforces_concurrency_and_pixels() {
        let tracker = PlacementTracker::new(PlacementLimits {
            max_concurrent_placements: 2,
            max_total_pixels: 1000,
        });
        let mut tracker = tracker;
        let cell = CellSize {
            width: 10,
            height: 10,
        };
        let first = tracker.reserve(10, 10, cell).unwrap();
        assert_eq!(first.image_id, 1);
        let second = tracker.reserve(10, 10, cell).unwrap();
        assert_eq!(second.image_id, 2);
        assert!(matches!(
            tracker.reserve(10, 10, cell),
            Err(PlacementError::TooManyPlacements {
                limit: 2,
                actual: 2
            })
        ));
        tracker.remove(second.image_id);
        assert!(matches!(
            tracker.reserve(50, 50, cell),
            Err(PlacementError::TooManyPixels {
                limit: 1000,
                actual: 2600
            })
        ));
    }

    #[test]
    fn remove_returns_the_image_id_for_deletion() {
        let mut tracker = PlacementTracker::new(PlacementLimits::default());
        let block = tracker
            .reserve(
                10,
                10,
                CellSize {
                    width: 10,
                    height: 10,
                },
            )
            .unwrap();
        let removed = tracker.remove(block.image_id).unwrap();
        assert_eq!(removed.image_id, block.image_id);
        assert!(
            tracker.remove(block.image_id).is_none(),
            "unknown id is None"
        );
    }

    #[test]
    fn replace_keeps_the_image_id() {
        let mut tracker = PlacementTracker::new(PlacementLimits::default());
        let block = tracker
            .reserve(
                10,
                10,
                CellSize {
                    width: 10,
                    height: 10,
                },
            )
            .unwrap();
        let replaced = tracker
            .replace(
                block.image_id,
                30,
                30,
                CellSize {
                    width: 10,
                    height: 10,
                },
            )
            .unwrap();
        assert_eq!(replaced.image_id, block.image_id);
        assert_eq!((replaced.cols, replaced.rows), (3, 3));
        assert!(tracker.active().len() == 1);
    }

    #[test]
    fn emit_places_at_home_row_with_grid() {
        let out = emit_placed_block(7, 10, 20, &[0xff; 10 * 20 * 4], 1, 1, 3);
        let text = String::from_utf8_lossy(&out);
        assert!(text.starts_with("\x1b[3;1H"), "home row move first");
        assert!(text.contains("i=7,U=1,c=1,r=1,q=2"));
        assert!(
            text.contains("\x1b[38;2;0;0;7m"),
            "image id encoded as color"
        );
        assert!(text.ends_with("\x1b[39m"), "color reset at the end");
    }

    #[test]
    fn home_rows_stack_blocks_in_source_order() {
        let mut tracker = PlacementTracker::new(PlacementLimits::default());
        let cell = CellSize {
            width: 10,
            height: 20,
        };
        let a = tracker.reserve(10, 20, cell).unwrap();
        assert_eq!((a.rows, tracker.home_row_for_next()), (1, 2));
        let b = tracker.reserve(30, 40, cell).unwrap();
        assert_eq!((b.rows, tracker.home_row_for_next()), (2, 4));
        assert_eq!(tracker.home_row_of(a.image_id), Some(1));
        assert_eq!(tracker.home_row_of(b.image_id), Some(2));
        assert_eq!(tracker.home_row_of(999), None);
    }

    #[test]
    fn replace_emits_a_scoped_delete_before_replacing() {
        let out = emit_replaced_block(5, 10, 20, &[0xff; 10 * 20 * 4], 1, 1, 2);
        let text = String::from_utf8_lossy(&out);
        assert!(
            text.starts_with("\x1b_Ga=d,d=I,i=5,q=2\x1b\\"),
            "scoped delete first"
        );
        assert!(text.contains("i=5,U=1,c=1,r=1,q=2"), "same image id reused");
    }

    #[test]
    fn decode_png_expands_palette_to_rgba8() {
        let mut encoder_bytes = Vec::new();
        {
            let width = 1;
            let height = 1;
            let mut encoder = png::Encoder::new(io::Cursor::new(&mut encoder_bytes), width, height);
            encoder.set_color(png::ColorType::Indexed);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.set_palette(vec![0x2a, 0x2a, 0x2a]);
            encoder.set_trns(vec![0x00]);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&[0]).unwrap();
        }
        let (width, height, rgba) = decode_png(&encoder_bytes, 100).unwrap();
        assert_eq!((width, height), (1, 1));
        assert_eq!(rgba, vec![42, 42, 42, 0]);
    }

    #[test]
    fn decode_png_rejects_invalid_input() {
        assert!(matches!(
            decode_png(b"not a png", 100),
            Err(PlacementError::InvalidImage { .. })
        ));
    }

    #[test]
    fn fail_closed_emits_nothing_when_a_block_is_rejected() {
        let mut tracker = PlacementTracker::new(PlacementLimits {
            max_concurrent_placements: 1,
            max_total_pixels: 100,
        });
        let cell = CellSize {
            width: 10,
            height: 10,
        };
        let block = tracker.reserve(10, 10, cell).unwrap();
        assert!(matches!(
            tracker.reserve(10, 10, cell),
            Err(PlacementError::TooManyPlacements { .. })
        ));
        // A rejected reserve must not have mutated the tracker.
        assert_eq!(tracker.active().len(), 1);
        assert_eq!(tracker.home_row_for_next(), block.rows + 1);
    }
}
