//! Pure DECSTBM scroll-region operation builders for the window-managed
//! agent-viewer (stage 1 of the scroll-region viewer).
//!
//! From startup, the agent-viewer owns its pane as a fixed window: row 1 is
//! the live status bar (see the `status_bar` module doc in
//! `native_stream.rs`), and a DECSTBM scroll region
//! (`tmath_core::terminal::decstbm_set`) covers rows `2..=pane_rows`. Every
//! content mutation happens INSIDE that region:
//!
//! - An append while following at the document top (`region_append_top_operations`,
//!   used when follow is engaged) homes to `region_top + rows_already_placed`
//!   and draws the new block downward with no region scroll until the pane
//!   fills; appends entirely below the visible window are a no-op.
//! - An append while scrolled away from the top (legacy tail-follow path,
//!   `region_append_operations`) scrolls the region up and draws at the bottom.
//!   old flowing-append path (`TerminalSink::append`'s bare cursor-relative
//!   write), which let the terminal's own natural scrollback growth push
//!   row 1 out of view, corrupting the fixed status bar. Region-scroll keeps
//!   row 1 untouched by construction: DECSTBM confines the terminal's own
//!   scroll behavior to the region, so nothing above it moves.
//! - A scroll-back step scrolls the region down (`region_scroll_down`,
//!   `CSI {n} T`) and draws the entering block(s) at the region's TOP edge
//!   from retained PNGs (`region_scroll_back_operations`).
//! - A block only partially visible at either edge draws only its visible
//!   placeholder rows (`tmath_core::placement::emit_placed_block_row_range_cursor`),
//!   a genuine protocol-native crop (see that function's doc comment) rather
//!   than an all-or-nothing pop-in.
//!
//! Every function here is pure (`TerminalOp` lists in, no terminal I/O), so
//! this module is exercised entirely with unit tests — consistent with
//! `native_stream.rs`'s existing op-builder functions
//! (`divergence_rewrite_operations`, `sync_window_operations`, etc.), which
//! this module deliberately mirrors in shape rather than reusing directly:
//! `native_stream::PlacedState` is private to that module, and this module's
//! [`RegionBlock`] is a narrower, decoupled view (just what a region-scroll
//! redraw needs) rather than a dependency on that module's internal
//! bookkeeping type.
//!
//! **Planned: a transient scrollbar** (not implemented in stage 1) will live
//! in the region's rightmost column, showing scroll position only while
//! scrolling. Two stage-1 facts it depends on, established now so the
//! addition is a pure stage-2 append rather than a stage-1 rework:
//! - Column safety: no op this module builds ever writes a placeholder cell
//!   in the pane's absolute last column — see `layout::PANE_MARGIN_COLS`'s
//!   doc comment for the guarantee (every rendered block's cell grid stays
//!   at least 2 columns narrower than the pane).
//! - Full-row-clear interaction: `native_stream::clear_rows` (used by
//!   `divergence_rewrite_operations`/`sync_window_operations` and, in the
//!   emit-batch path this module's region operations do not replace,
//!   `tail_replace_operations`) writes `\x1b[2K`, which clears an ENTIRE
//!   line, including the scrollbar column — so any row a scrollbar thumb
//!   occupies must be repainted after such a clear runs on it, or the
//!   thumb silently vanishes until the next scroll tick redraws it. The
//!   region-scroll operations THIS module builds do not use `clear_rows` at
//!   all (a scroll-up/down plus a bounded placeholder redraw never needs a
//!   full-line erase the way a stale-tail rewrite does), so they do not
//!   independently threaten the scrollbar column; the risk is specifically
//!   from `clear_rows`-based paths that may still run inside the same
//!   region (e.g. a `NeedsWindowSync` fallback). Stage 2's tick loop should
//!   redraw the scrollbar thumb unconditionally after any op batch that
//!   could have cleared its row, rather than trying to track which batches
//!   are "safe."

use tmath_core::placement::{
    decode_png, emit_placed_block_row_range_cursor, grid_for, CellSize, RowRangePlacement,
    TerminalOp,
};

/// One block's data as a region-scroll operation needs it: enough to decode,
/// grid, and (optionally) crop its placeholder rows. Deliberately narrower
/// than `native_stream::PlacedState` (see the module doc) — callers adapt
/// their own bookkeeping into this at the call site.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RegionBlock<'a> {
    pub id: u64,
    pub png: &'a [u8],
}

/// A byte-sequence build error: an id does not fit `u32` (the Kitty image-id
/// space), or a PNG fails to decode. Mirrors `native_stream`'s
/// `stream_error()` shape at the boundary — this module has no `RenderError`
/// dependency of its own, so callers map this to their own error type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RegionOpError;

/// Builds `CSI {n} S` (Scroll Up), which — while a DECSTBM region is active —
/// moves the region's content up by `n` rows and reveals `n` blank rows at
/// the region's bottom edge, without disturbing anything above the region
/// (the fixed status bar at row 1). `n == 0` builds nothing (a genuine no-op,
/// not a zero-parameter `CSI S` — `CSI 0 S` is equivalent to `CSI 1 S` per
/// the ECMA-48 default-parameter rule, which would incorrectly scroll by one
/// row when the caller means "nothing to scroll").
pub(crate) fn region_scroll_up(rows: u32) -> Vec<TerminalOp> {
    if rows == 0 {
        return Vec::new();
    }
    vec![TerminalOp::Local(format!("\x1b[{rows}S").into_bytes())]
}

/// Builds `CSI {n} T` (Scroll Down) — the mirror of [`region_scroll_up`] for
/// scroll-back: moves the region's content down by `n` rows and reveals `n`
/// blank rows at the region's TOP edge. Same `n == 0` no-op rule as
/// `region_scroll_up`, for the same reason.
pub(crate) fn region_scroll_down(rows: u32) -> Vec<TerminalOp> {
    if rows == 0 {
        return Vec::new();
    }
    vec![TerminalOp::Local(format!("\x1b[{rows}T").into_bytes())]
}

/// Builds the operations for an append while following (AT-3-502): scrolls
/// the region up by however many rows are about to be drawn (the new
/// block's own row count, or the region's own height if the block is
/// taller than the whole region), homes the cursor so the LAST drawn row
/// lands exactly on the region's bottom row, and draws the block there —
/// its full image if it fits entirely within the region, or only its
/// bottom-aligned visible rows if the region is shorter than the block
/// itself (a block taller than the whole pane; the partial-edge crop kicks
/// in symmetrically here too, not just on scroll-back). Homing above the
/// bottom row rather than AT it is deliberate: `emit_placed_block_row_range_cursor`
/// writes top-to-bottom from the cursor's starting row, so homing directly
/// to `region_bottom` would push every row past the first one off the
/// bottom of the region — an out-of-bounds write the Kitty graphics
/// protocol leaves "undefined, up to implementations" for a placement that
/// crosses a scroll-region edge.
///
/// `region_bottom` is the DECSTBM region's own last row (1-indexed,
/// matching `decstbm_set`'s `bottom` parameter) — the same value the caller
/// passed to `Terminal::set_scroll_region`.
pub(crate) fn region_append_operations(
    block: RegionBlock<'_>,
    cell: CellSize,
    max_image_pixels: u64,
    region_bottom: u32,
) -> Result<Vec<TerminalOp>, RegionOpError> {
    let (width, height, rgba) =
        decode_png(block.png, max_image_pixels).map_err(|_| RegionOpError)?;
    let (cols, rows) = grid_for(width, height, cell);
    let image_id = u32::try_from(block.id).map_err(|_| RegionOpError)?;

    let visible_rows = rows.saturating_sub(region_bottom)..rows;
    let drawn_rows = visible_rows.end - visible_rows.start;
    let mut operations = region_scroll_up(drawn_rows);
    // The drawn rows must END at the region's bottom row, not start there:
    // `emit_placed_block_row_range_cursor` writes top-to-bottom from the
    // cursor's starting position via `\r\n` between rows, so homing to
    // `region_bottom` itself (the LAST row) and then writing `drawn_rows`
    // downward would push every row past `drawn_rows == 1` off the bottom
    // of the region — exactly the "undefined, up to implementations"
    // placement-crosses-region-edge behavior the Kitty graphics protocol
    // warns about. Homing `drawn_rows - 1` rows above the bottom instead
    // means the LAST row written lands exactly on `region_bottom`.
    let home_row = region_bottom.saturating_sub(drawn_rows.saturating_sub(1));
    let home = format!("\x1b[{home_row};1H");
    operations.push(TerminalOp::Local(home.into_bytes()));
    operations.extend(emit_placed_block_row_range_cursor(
        RowRangePlacement {
            image_id,
            width_px: width,
            height_px: height,
            rgba: &rgba,
            cols,
            rows,
        },
        visible_rows,
        true,
    ));
    Ok(operations)
}

/// Builds the operations for an append while follow is pinned to the top
/// (AT-3-502 top-down streaming): homes to `region_top + rows_before` and
/// draws the block downward. No region scroll — content grows from the top
/// until the pane is full; once the append starts at or below the region's
/// bottom edge (`rows_before >= region_rows`), nothing is drawn (the caller's
/// viewport keeps offset `0`, so the new block is off-screen below).
pub(crate) fn region_append_top_operations(
    block: RegionBlock<'_>,
    cell: CellSize,
    max_image_pixels: u64,
    region_top: u32,
    region_bottom: u32,
    rows_before: u32,
) -> Result<Vec<TerminalOp>, RegionOpError> {
    let region_rows = region_bottom.saturating_sub(region_top).saturating_add(1);
    if rows_before >= region_rows {
        return Ok(Vec::new());
    }

    let (width, height, rgba) =
        decode_png(block.png, max_image_pixels).map_err(|_| RegionOpError)?;
    let (cols, rows) = grid_for(width, height, cell);
    let image_id = u32::try_from(block.id).map_err(|_| RegionOpError)?;

    let space = region_rows.saturating_sub(rows_before);
    let visible_rows = 0..rows.min(space);
    if visible_rows.is_empty() {
        return Ok(Vec::new());
    }

    let home_row = region_top.saturating_add(rows_before);
    let mut operations = vec![TerminalOp::Local(format!("\x1b[{home_row};1H").into_bytes())];
    operations.extend(emit_placed_block_row_range_cursor(
        RowRangePlacement {
            image_id,
            width_px: width,
            height_px: height,
            rgba: &rgba,
            cols,
            rows,
        },
        visible_rows,
        true,
    ));
    Ok(operations)
}

/// Top-down in-place tail growth while follow is pinned to the top: replaces
/// the last block at `region_top + rows_before_tail` without scrolling the
/// region. Returns a no-op when the tail starts at or below the visible window.
pub(crate) fn region_tail_replace_top_operations(
    old_image_id: u64,
    old_rows: u32,
    new_block: RegionBlock<'_>,
    cell: CellSize,
    max_image_pixels: u64,
    region_top: u32,
    region_bottom: u32,
    rows_before_tail: u32,
) -> Result<Vec<TerminalOp>, RegionOpError> {
    let region_rows = region_bottom.saturating_sub(region_top).saturating_add(1);
    if rows_before_tail >= region_rows {
        return Ok(Vec::new());
    }

    let (width, height, rgba) =
        decode_png(new_block.png, max_image_pixels).map_err(|_| RegionOpError)?;
    let (cols, rows) = grid_for(width, height, cell);
    let new_image_id = u32::try_from(new_block.id).map_err(|_| RegionOpError)?;
    let old_image_id = u32::try_from(old_image_id).map_err(|_| RegionOpError)?;

    let space = region_rows.saturating_sub(rows_before_tail);
    let visible_rows = 0..rows.min(space);
    if visible_rows.is_empty() {
        return Ok(Vec::new());
    }

    let mut operations = vec![TerminalOp::Graphics(tmath_core::kitty::kitty_delete_id(
        old_image_id,
    ))];

    let home_row = region_top.saturating_add(rows_before_tail);
    operations.push(TerminalOp::Local(
        format!("\x1b[{home_row};1H").into_bytes(),
    ));
    let clear_rows = old_rows.min(space);
    if clear_rows > 0 {
        operations.push(TerminalOp::Local(crate::native_stream::clear_rows(
            clear_rows,
        )));
    }
    operations.extend(emit_placed_block_row_range_cursor(
        RowRangePlacement {
            image_id: new_image_id,
            width_px: width,
            height_px: height,
            rgba: &rgba,
            cols,
            rows,
        },
        visible_rows,
        true,
    ));
    Ok(operations)
}

/// Builds the operations for one scroll-back step (AT-3-502/503 extended to
/// the region-managed viewer): scrolls the region down by `entering`'s total
/// row count, homes to the region's TOP row (`region_top`, matching
/// `decstbm_set`'s `top` parameter — `2` when the status bar reserves row
/// 1), and draws each entering block in order from its retained PNG,
/// top-aligned. A block taller than the remaining region space at the time
/// it is drawn is cropped to its TOP visible rows (the mirror of
/// `region_append_operations`' bottom-aligned crop) rather than pushed past
/// the region into the status bar.
///
/// `entering` is already in top-to-bottom document order (the blocks now
/// revealed at the window's new top edge, closest-to-top first) — the
/// caller (`agent_viewer`) is responsible for that ordering, since it is the
/// one with the viewport's block list.
///
/// Called from `native_stream::TerminalSink::try_scroll_window_incrementally`
/// (stage 2), which handles the narrow "one or more blocks entering at the
/// top edge, nothing else about the window shape changed" case; anything
/// broader (a jump, a `skip_rows_in_first` change, a forward step) still
/// goes through `sync_window`'s full resync, which remains correct for
/// every shape this function does not attempt.
pub(crate) fn region_scroll_back_operations(
    entering: &[RegionBlock<'_>],
    cell: CellSize,
    max_image_pixels: u64,
    region_top: u32,
    region_bottom: u32,
) -> Result<Vec<TerminalOp>, RegionOpError> {
    let region_rows = region_bottom.saturating_sub(region_top).saturating_add(1);
    let mut decoded = Vec::with_capacity(entering.len());
    let mut total_rows: u32 = 0;
    for block in entering {
        let (width, height, rgba) =
            decode_png(block.png, max_image_pixels).map_err(|_| RegionOpError)?;
        let (cols, rows) = grid_for(width, height, cell);
        let image_id = u32::try_from(block.id).map_err(|_| RegionOpError)?;
        total_rows = total_rows.saturating_add(rows);
        decoded.push((image_id, width, height, rgba, cols, rows));
    }

    if decoded.is_empty() {
        // No entering blocks: a true no-op. Matches
        // `region_scroll_up`/`region_scroll_down`'s own zero-rows contract
        // — no scroll, and (unlike a nonempty call) no home-move either,
        // since there is nothing to draw at that home position. Emitting an
        // unconditional home here regardless of `entering` was the bug this
        // guards: an empty scroll-back step must write nothing at all, not
        // a cursor move to a position nothing then uses.
        return Ok(Vec::new());
    }

    let mut operations = region_scroll_down(total_rows.min(region_rows));
    let home = format!("\x1b[{region_top};1H");
    operations.push(TerminalOp::Local(home.into_bytes()));

    let mut remaining_region_rows = region_rows;
    for (image_id, width, height, rgba, cols, rows) in decoded {
        if remaining_region_rows == 0 {
            break;
        }
        let visible_rows = 0..rows.min(remaining_region_rows);
        let drawn_rows = visible_rows.end - visible_rows.start;
        operations.extend(emit_placed_block_row_range_cursor(
            RowRangePlacement {
                image_id,
                width_px: width,
                height_px: height,
                rgba: &rgba,
                cols,
                rows,
            },
            visible_rows,
            true,
        ));
        remaining_region_rows = remaining_region_rows.saturating_sub(drawn_rows);
    }
    Ok(operations)
}

/// Builds the operations for replacing the region's CURRENT LAST block in
/// place (a growing streamed tail — e.g. display math lengthening character
/// by character within the same block, the common per-tick case while
/// following) with a region-scroll-aware equivalent of
/// `native_stream::tail_replace_operations`: scrolls the region up by
/// however many NET NEW rows the replacement adds beyond the old block's
/// rows (`0` if the replacement is the same height or shorter — nothing
/// above needs to move for that), deletes the old image, clears the old
/// block's full row span (`clear_rows` — a full-line clear, not the
/// placeholder-bounded crop `region_append_operations` uses, since nothing
/// of the old image survives here), and draws the new block bottom-aligned
/// to the region exactly like `region_append_operations` (so a replacement
/// taller than the whole region still crops to its bottom-aligned visible
/// rows rather than overflowing).
///
/// Unlike the per-op `native_stream::TerminalSink::replace`'s
/// `top_is_reachable` check (a live `CSI 6n` cursor-position query, only
/// meaningful when growth can silently scroll the raw pane out from under
/// it), this function needs no such check: the DECSTBM region confines all
/// scrolling to `region_top..=region_bottom` by construction, so the tail
/// is always reachable within the region regardless of how much history
/// exists above it — that is the whole point of the region-managed
/// architecture. Callers therefore call this unconditionally whenever the
/// region is active and a plan replaces the current last block, without any
/// live cursor query.
pub(crate) fn region_tail_replace_operations(
    old_image_id: u64,
    old_rows: u32,
    new_block: RegionBlock<'_>,
    cell: CellSize,
    max_image_pixels: u64,
    region_bottom: u32,
) -> Result<Vec<TerminalOp>, RegionOpError> {
    let (width, height, rgba) =
        decode_png(new_block.png, max_image_pixels).map_err(|_| RegionOpError)?;
    let (cols, rows) = grid_for(width, height, cell);
    let new_image_id = u32::try_from(new_block.id).map_err(|_| RegionOpError)?;
    let old_image_id = u32::try_from(old_image_id).map_err(|_| RegionOpError)?;

    let visible_rows = rows.saturating_sub(region_bottom)..rows;
    let drawn_rows = visible_rows.end - visible_rows.start;
    let growth = drawn_rows.saturating_sub(old_rows);
    let mut operations = region_scroll_up(growth);

    operations.push(TerminalOp::Graphics(tmath_core::kitty::kitty_delete_id(
        old_image_id,
    )));

    // Home so the LAST drawn row lands on the region's bottom (same
    // reasoning as `region_append_operations`'s doc comment), then clear
    // the OLD block's full row span from there upward before drawing —
    // `clear_rows` moves the cursor back to where it started once done, so
    // the subsequent draw still starts at the same home row.
    let home_row = region_bottom.saturating_sub(drawn_rows.saturating_sub(1));
    operations.push(TerminalOp::Local(
        format!("\x1b[{home_row};1H").into_bytes(),
    ));
    if old_rows > 0 {
        operations.push(TerminalOp::Local(crate::native_stream::clear_rows(
            old_rows,
        )));
    }
    operations.extend(emit_placed_block_row_range_cursor(
        RowRangePlacement {
            image_id: new_image_id,
            width_px: width,
            height_px: height,
            rgba: &rgba,
            cols,
            rows,
        },
        visible_rows,
        true,
    ));
    Ok(operations)
}

/// The scrollbar thumb glyph (a solid block), drawn in the pane's absolute
/// last column while scrolling. Distinct from any placeholder cell content —
/// see `layout::PANE_MARGIN_COLS`'s doc comment for why no block's
/// placeholder grid can ever reach this column.
const SCROLLBAR_THUMB_GLYPH: char = '█';
/// The scrollbar track glyph, drawn in every region row NOT covered by the
/// thumb while the scrollbar is visible, so the thumb reads as a moving
/// element against a visible rail rather than floating in blank space.
const SCROLLBAR_TRACK_GLYPH: char = '│';
/// SGR for the thumb: a plain, moderately bright color, distinguishable
/// from ordinary block content and from the dim track.
const SCROLLBAR_THUMB_SGR: &str = "\x1b[38;5;250m";
/// SGR for the track: dim, so it reads as background chrome, not content.
const SCROLLBAR_TRACK_SGR: &str = "\x1b[38;5;238m";
const SGR_RESET: &str = "\x1b[0m";

/// Builds the operations that draw stage 2's transient scrollbar: a thumb
/// glyph at `thumb_rows` (region-relative, 0-indexed — the same convention
/// `viewer_viewport::Viewport::scrollbar_thumb_rows` returns) and a track
/// glyph at every other row from `0..region_rows`, all in the pane's
/// absolute LAST column (`pane_cols`).
///
/// Wrapped in DECSC/DECRC (`\x1b7`...`\x1b8`, the same save/restore pair
/// `native_stream`'s status bar uses — chosen there and reused here for the
/// same tmux-passthrough reliability reason) so drawing the scrollbar never
/// disturbs the cursor position content operations rely on. Each row is
/// addressed absolutely (`CSI {row};{pane_cols} H`) rather than
/// cursor-relative, since the scrollbar's column has nothing to do with
/// wherever content operations last left the cursor.
///
/// `region_top` is the DECSTBM region's first row (1-indexed, matching
/// `decstbm_set`'s `top` parameter — `2` when the status bar reserves row
/// 1); `thumb_rows` and the implied `0..region_rows` track range are both
/// 0-indexed offsets INTO the region, so row 0 here means the region's own
/// first row (`region_top`), not the pane's absolute row 1.
///
/// Must be called again, unconditionally, after any operation batch that
/// could have cleared the scrollbar's column with a full-line clear (see
/// the module doc's "Full-row-clear interaction" note) — this function does
/// not itself track whether a redraw is needed; the caller's tick loop
/// decides that.
pub(crate) fn scrollbar_operations(
    thumb_rows: Option<std::ops::Range<u32>>,
    region_rows: u32,
    region_top: u32,
    pane_cols: u32,
) -> Vec<TerminalOp> {
    if region_rows == 0 {
        return Vec::new();
    }
    let mut line = String::from("\x1b7");
    for row in 0..region_rows {
        let absolute_row = region_top + row;
        line.push_str(&format!("\x1b[{absolute_row};{pane_cols}H"));
        let in_thumb = thumb_rows
            .as_ref()
            .is_some_and(|thumb| thumb.contains(&row));
        if in_thumb {
            line.push_str(SCROLLBAR_THUMB_SGR);
            line.push(SCROLLBAR_THUMB_GLYPH);
        } else {
            line.push_str(SCROLLBAR_TRACK_SGR);
            line.push(SCROLLBAR_TRACK_GLYPH);
        }
        line.push_str(SGR_RESET);
    }
    line.push_str("\x1b8");
    vec![TerminalOp::Local(line.into_bytes())]
}

/// Builds the operations that CLEAR the scrollbar's column back to blank —
/// used when the auto-hide timer expires (the scrollbar is transient: shown
/// while scrolling, hidden ~1s after motion stops, per the coordinator's
/// spec) or when there is nothing to scroll
/// (`Viewport::scrollbar_thumb_rows` returned `None`). Writes a plain space
/// at each region row's last column rather than any glyph, wrapped in the
/// same DECSC/DECRC save/restore as `scrollbar_operations`.
pub(crate) fn scrollbar_clear_operations(
    region_rows: u32,
    region_top: u32,
    pane_cols: u32,
) -> Vec<TerminalOp> {
    if region_rows == 0 {
        return Vec::new();
    }
    let mut line = String::from("\x1b7");
    for row in 0..region_rows {
        let absolute_row = region_top + row;
        line.push_str(&format!("\x1b[{absolute_row};{pane_cols}H "));
    }
    line.push_str("\x1b8");
    vec![TerminalOp::Local(line.into_bytes())]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgba8_png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut encoder = png::Encoder::new(std::io::Cursor::new(&mut bytes), width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer
            .write_image_data(&vec![0xffu8; (width * height * 4) as usize])
            .unwrap();
        drop(writer);
        bytes
    }

    fn direct_text(operations: &[TerminalOp]) -> String {
        let mut bytes = Vec::new();
        tmath_core::placement::write_terminal_ops(&mut bytes, operations, false).unwrap();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    fn row_diacritic(index: u32) -> char {
        tmath_core::kitty::row_column_diacritic(index).expect("row index in range")
    }

    #[test]
    fn region_scroll_up_zero_rows_is_a_true_no_op() {
        assert!(region_scroll_up(0).is_empty());
    }

    #[test]
    fn region_scroll_up_builds_csi_s() {
        let ops = region_scroll_up(5);
        assert_eq!(direct_text(&ops), "\x1b[5S");
    }

    #[test]
    fn region_scroll_down_zero_rows_is_a_true_no_op() {
        assert!(region_scroll_down(0).is_empty());
    }

    #[test]
    fn region_scroll_down_builds_csi_t() {
        let ops = region_scroll_down(3);
        assert_eq!(direct_text(&ops), "\x1b[3T");
    }

    #[test]
    fn append_top_operations_draw_at_region_top_when_rows_before_is_zero() {
        let cell = CellSize {
            width: 10,
            height: 10,
        };
        let png = rgba8_png(10, 20); // 2 rows
        let block = RegionBlock { id: 7, png: &png };
        let ops = region_append_top_operations(block, cell, u64::MAX, 2, 24, 0).unwrap();
        let text = direct_text(&ops);
        assert!(
            !text.contains('S'),
            "top-down append must not scroll the region: {text:?}"
        );
        assert!(
            text.contains("\x1b[2;1H"),
            "first block homes to the region top row 2: {text:?}"
        );
        assert!(text.contains("i=7,U=1,c=1,r=2,q=2"));
    }

    #[test]
    fn append_top_operations_draw_after_prior_rows_without_scrolling() {
        let cell = CellSize {
            width: 10,
            height: 10,
        };
        let png = rgba8_png(10, 10); // 1 row
        let block = RegionBlock { id: 8, png: &png };
        let ops = region_append_top_operations(block, cell, u64::MAX, 2, 10, 3).unwrap();
        let text = direct_text(&ops);
        assert!(
            text.contains("\x1b[5;1H"),
            "second block homes 3 rows below region top row 2: {text:?}"
        );
        assert!(!text.contains('S'));
    }

    #[test]
    fn append_top_operations_is_a_no_op_when_the_append_starts_below_the_region() {
        let cell = CellSize {
            width: 10,
            height: 10,
        };
        let png = rgba8_png(10, 10);
        let block = RegionBlock { id: 9, png: &png };
        let ops = region_append_top_operations(block, cell, u64::MAX, 2, 10, 9).unwrap();
        assert!(ops.is_empty());
    }

    #[test]
    fn append_operations_scroll_up_by_the_blocks_own_rows_then_draw_at_the_bottom() {
        let cell = CellSize {
            width: 10,
            height: 10,
        };
        let png = rgba8_png(10, 20); // 2 rows tall at this cell size
        let block = RegionBlock { id: 7, png: &png };
        let ops = region_append_operations(block, cell, u64::MAX, 24).unwrap();
        let text = direct_text(&ops);
        assert!(
            text.starts_with("\x1b[2S"),
            "scroll up by the block's own 2 rows: {text:?}"
        );
        assert!(
            text.contains("\x1b[23;1H"),
            "home 1 row above the bottom so the LAST of the 2 drawn rows \
             lands exactly on the region's bottom row 24: {text:?}"
        );
        assert!(text.contains("i=7,U=1,c=1,r=2,q=2"));
    }

    #[test]
    fn append_operations_crop_a_block_taller_than_the_whole_region_to_its_bottom_rows() {
        let cell = CellSize {
            width: 10,
            height: 10,
        };
        // 5 rows tall, 2 cols wide (so row/col diacritic 0 never collide —
        // see the `placement.rs` row-range test for why), but the region is
        // only 3 rows (region_bottom=3, region_top implied 1 for this
        // narrow unit) — the block must crop to its BOTTOM 3 rows
        // (visible_rows 2..5), never scroll by more than the region itself
        // has.
        let png = rgba8_png(20, 50);
        let block = RegionBlock { id: 9, png: &png };
        let ops = region_append_operations(block, cell, u64::MAX, 3).unwrap();
        let text = direct_text(&ops);
        assert!(
            text.starts_with("\x1b[3S"),
            "scroll clamps to the region's own 3 rows, not the block's 5: {text:?}"
        );
        assert!(
            text.contains("\x1b[1;1H"),
            "3 drawn rows home 2 rows above the region's bottom (row 3), \
             i.e. row 1, so the last drawn row still lands on row 3: {text:?}"
        );
        let placeholder_count = text
            .chars()
            .filter(|&c| c == tmath_core::kitty::PLACEHOLDER)
            .count();
        assert_eq!(placeholder_count, 6, "3 rows x 2 cols drawn: {text:?}");
        let cell_at = |row: u32, col: u32| {
            format!(
                "{}{}{}",
                tmath_core::kitty::PLACEHOLDER,
                row_diacritic(row),
                row_diacritic(col)
            )
        };
        assert!(
            text.contains(&cell_at(2, 0)),
            "the crop keeps the block's ORIGINAL row 2 (0-indexed, i.e. the \
             3rd row from the top of the 5-row image) as its first drawn row \
             — a bottom-aligned crop, not a top-aligned one: {text:?}"
        );
        assert!(text.contains(&cell_at(3, 0)));
        assert!(text.contains(&cell_at(4, 0)));
        assert!(!text.contains(&cell_at(0, 0)));
        assert!(!text.contains(&cell_at(1, 0)));
    }

    #[test]
    fn scroll_back_operations_scroll_down_by_total_entering_rows_then_draw_at_the_top() {
        let cell = CellSize {
            width: 10,
            height: 10,
        };
        let png_a = rgba8_png(10, 10); // 1 row
        let png_b = rgba8_png(10, 20); // 2 rows
        let entering = [
            RegionBlock { id: 1, png: &png_a },
            RegionBlock { id: 2, png: &png_b },
        ];
        let ops = region_scroll_back_operations(&entering, cell, u64::MAX, 2, 24).unwrap();
        let text = direct_text(&ops);
        assert!(
            text.starts_with("\x1b[3T"),
            "scroll down by the entering blocks' total 3 rows: {text:?}"
        );
        assert!(
            text.contains("\x1b[2;1H"),
            "home to the region's top row: {text:?}"
        );
        assert!(
            text.contains("i=1,U=1,c=1,r=1,q=2"),
            "block 1 drawn first (top): {text:?}"
        );
        assert!(
            text.contains("i=2,U=1,c=1,r=2,q=2"),
            "block 2 drawn after it: {text:?}"
        );
        assert!(
            text.find("i=1,").unwrap() < text.find("i=2,").unwrap(),
            "top-to-bottom document order is preserved: {text:?}"
        );
    }

    #[test]
    fn scroll_back_operations_crop_the_last_entering_block_to_the_remaining_region_space() {
        let cell = CellSize {
            width: 10,
            height: 10,
        };
        let png_a = rgba8_png(10, 10); // 1 row
        let png_b = rgba8_png(10, 30); // 3 rows, but only 2 rows remain in a 3-row region
        let entering = [
            RegionBlock { id: 1, png: &png_a },
            RegionBlock { id: 2, png: &png_b },
        ];
        // region_top=1, region_bottom=3 -> region_rows=3; block 1 takes 1,
        // leaving 2 rows for block 2's own 3.
        let ops = region_scroll_back_operations(&entering, cell, u64::MAX, 1, 3).unwrap();
        let text = direct_text(&ops);
        let placeholder_count = |id_marker: &str, s: &str| {
            // Count placeholder cells appearing after this block's own
            // placement command, up to the next placement command (or end).
            let start = s.find(id_marker).unwrap();
            let rest = &s[start..];
            let end = rest[1..]
                .find("i=")
                .map(|offset| offset + 1)
                .unwrap_or(rest.len());
            rest[..end]
                .chars()
                .filter(|&c| c == tmath_core::kitty::PLACEHOLDER)
                .count()
        };
        assert_eq!(placeholder_count("i=1,", &text), 1);
        assert_eq!(
            placeholder_count("i=2,", &text),
            2,
            "block 2 crops to the 2 rows still remaining in the region: {text:?}"
        );
        assert!(
            text.contains(row_diacritic(0)),
            "block 2's crop is TOP-aligned (keeps its own rows 0 and 1): {text:?}"
        );
    }

    #[test]
    fn scroll_back_operations_with_no_entering_blocks_is_a_true_no_op() {
        let cell = CellSize {
            width: 10,
            height: 10,
        };
        let ops = region_scroll_back_operations(&[], cell, u64::MAX, 2, 24).unwrap();
        assert!(ops.is_empty());
    }

    #[test]
    fn an_id_over_u32_max_is_rejected() {
        let cell = CellSize {
            width: 10,
            height: 10,
        };
        let png = rgba8_png(10, 10);
        let block = RegionBlock {
            id: u64::from(u32::MAX) + 1,
            png: &png,
        };
        assert_eq!(
            region_append_operations(block, cell, u64::MAX, 24),
            Err(RegionOpError)
        );
    }

    #[test]
    fn tail_replace_growing_scrolls_up_by_only_the_net_new_rows() {
        let cell = CellSize {
            width: 10,
            height: 10,
        };
        // Old block: 2 rows. New block: 5 rows. Net growth: 3 rows.
        let new_png = rgba8_png(10, 50);
        let new_block = RegionBlock {
            id: 8,
            png: &new_png,
        };
        let ops = region_tail_replace_operations(7, 2, new_block, cell, u64::MAX, 24).unwrap();
        let text = direct_text(&ops);
        assert!(
            text.starts_with("\x1b[3S"),
            "scrolls up by the NET growth (5 new - 2 old = 3), not the new \
             block's full 5 rows: {text:?}"
        );
        assert!(
            text.contains("a=d,d=I,i=7"),
            "the old image is deleted: {text:?}"
        );
        assert!(
            text.contains("\x1b[2K"),
            "the old block's full row span is cleared with a full-line \
             clear, not a placeholder-bounded crop: {text:?}"
        );
        assert!(
            text.contains("i=8,U=1,c=1,r=5,q=2"),
            "the new block is drawn: {text:?}"
        );
        assert!(
            text.contains("\x1b[20;1H"),
            "5 drawn rows home 4 rows above the region's bottom (24), i.e. \
             row 20, so the last drawn row lands on row 24: {text:?}"
        );
    }

    #[test]
    fn tail_replace_shrinking_or_same_height_never_scrolls() {
        let cell = CellSize {
            width: 10,
            height: 10,
        };
        // Old block: 5 rows. New block: 2 rows (shrinking) — nothing above
        // needs room made for it, so no scroll at all.
        let new_png = rgba8_png(10, 20);
        let new_block = RegionBlock {
            id: 8,
            png: &new_png,
        };
        let ops = region_tail_replace_operations(7, 5, new_block, cell, u64::MAX, 24).unwrap();
        let text = direct_text(&ops);
        assert!(
            !text.contains('S'),
            "a shrinking replace must never scroll the region: {text:?}"
        );
        assert!(
            text.contains("\x1b[2K"),
            "the old 5-row span is still cleared: {text:?}"
        );
        assert!(text.contains("i=8,U=1,c=1,r=2,q=2"));
    }

    #[test]
    fn tail_replace_crops_a_replacement_taller_than_the_whole_region() {
        let cell = CellSize {
            width: 10,
            height: 10,
        };
        // New block is 5 rows, but the region is only 3 rows tall — must
        // crop to the bottom 3 rows, mirroring `region_append_operations`'s
        // own crop behavior for the same shape.
        let new_png = rgba8_png(20, 50);
        let new_block = RegionBlock {
            id: 8,
            png: &new_png,
        };
        let ops = region_tail_replace_operations(7, 1, new_block, cell, u64::MAX, 3).unwrap();
        let text = direct_text(&ops);
        let placeholder_count = text
            .chars()
            .filter(|&c| c == tmath_core::kitty::PLACEHOLDER)
            .count();
        assert_eq!(placeholder_count, 6, "3 rows x 2 cols cropped: {text:?}");
        let cell_at = |row: u32, col: u32| {
            format!(
                "{}{}{}",
                tmath_core::kitty::PLACEHOLDER,
                row_diacritic(row),
                row_diacritic(col)
            )
        };
        assert!(
            text.contains(&cell_at(2, 0)),
            "bottom-aligned crop: {text:?}"
        );
        assert!(!text.contains(&cell_at(0, 0)));
    }

    #[test]
    fn tail_replace_rejects_an_id_over_u32_max() {
        let cell = CellSize {
            width: 10,
            height: 10,
        };
        let new_png = rgba8_png(10, 10);
        let new_block = RegionBlock {
            id: u64::from(u32::MAX) + 1,
            png: &new_png,
        };
        assert_eq!(
            region_tail_replace_operations(7, 1, new_block, cell, u64::MAX, 24),
            Err(RegionOpError)
        );

        let ok_png = rgba8_png(10, 10);
        let ok_block = RegionBlock {
            id: 8,
            png: &ok_png,
        };
        assert_eq!(
            region_tail_replace_operations(
                u64::from(u32::MAX) + 1,
                1,
                ok_block,
                cell,
                u64::MAX,
                24
            ),
            Err(RegionOpError),
            "an out-of-range OLD id must also be rejected"
        );
    }

    #[test]
    fn scrollbar_operations_with_zero_region_rows_is_a_true_no_op() {
        assert!(scrollbar_operations(Some(0..2), 0, 2, 80).is_empty());
    }

    #[test]
    fn scrollbar_operations_draws_the_thumb_at_the_right_rows_and_column() {
        // A 5-row region (top=2, so absolute rows 2..7), thumb at
        // region-relative rows 1..3 (absolute rows 3..5), pane 80 cols wide.
        let ops = scrollbar_operations(Some(1..3), 5, 2, 80);
        let text = direct_text(&ops);
        assert!(text.starts_with("\x1b7"), "wrapped in DECSC: {text:?}");
        assert!(text.ends_with("\x1b8"), "wrapped in DECRC: {text:?}");
        // Every row addresses column 80 (the pane's last column).
        for absolute_row in 2..7 {
            assert!(
                text.contains(&format!("\x1b[{absolute_row};80H")),
                "row {absolute_row} must be addressed at column 80: {text:?}"
            );
        }
        // Thumb rows (region-relative 1, 2 -> absolute 3, 4) get the thumb
        // glyph; every other row gets the track glyph.
        let thumb_count = text.matches(SCROLLBAR_THUMB_GLYPH).count();
        let track_count = text.matches(SCROLLBAR_TRACK_GLYPH).count();
        assert_eq!(
            thumb_count, 2,
            "thumb glyph drawn exactly at the 2 thumb rows: {text:?}"
        );
        assert_eq!(
            track_count, 3,
            "track glyph drawn at the other 3 rows: {text:?}"
        );
    }

    #[test]
    fn scrollbar_operations_with_no_thumb_draws_only_track() {
        // `None` (content fits, no scrollbar needed per
        // `Viewport::scrollbar_thumb_rows`) still draws the track everywhere
        // — the caller decides whether to call this at all; when it does,
        // every row is track.
        let ops = scrollbar_operations(None, 5, 2, 80);
        let text = direct_text(&ops);
        assert_eq!(text.matches(SCROLLBAR_THUMB_GLYPH).count(), 0);
        assert_eq!(text.matches(SCROLLBAR_TRACK_GLYPH).count(), 5);
    }

    #[test]
    fn scrollbar_clear_operations_with_zero_region_rows_is_a_true_no_op() {
        assert!(scrollbar_clear_operations(0, 2, 80).is_empty());
    }

    #[test]
    fn scrollbar_clear_operations_writes_a_blank_at_every_row_and_column() {
        let ops = scrollbar_clear_operations(3, 2, 80);
        let text = direct_text(&ops);
        assert!(text.starts_with("\x1b7"));
        assert!(text.ends_with("\x1b8"));
        for absolute_row in 2..5 {
            assert!(
                text.contains(&format!("\x1b[{absolute_row};80H ")),
                "row {absolute_row} must be cleared to a blank at column 80: {text:?}"
            );
        }
        assert!(
            !text.contains(SCROLLBAR_THUMB_GLYPH) && !text.contains(SCROLLBAR_TRACK_GLYPH),
            "no scrollbar glyphs must remain after a clear: {text:?}"
        );
    }
}
