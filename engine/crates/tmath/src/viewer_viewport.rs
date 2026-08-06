//! Pure viewport state machine for `tmath agent-viewer` (AT-3-502).
//!
//! The viewer keeps a visibility window over an ordered list of block
//! heights (rows). [`Viewport`] tracks that window's top offset (rows from
//! the start of the content), the pane's visible height, and whether
//! `follow` is engaged. It has no terminal or I/O dependency, so it is
//! exercised entirely with unit tests; [`crate::agent_viewer`] is the only
//! caller.
//!
//! State transitions:
//! - [`Viewport::scroll_by`] moves the offset by a row delta, clamps it to
//!   `[0, max_offset]`, and sets follow to whether the RESULT landed at the
//!   top (`new_offset == 0`) — any manual scroll input maps here through
//!   [`tmath_core::scroll_driver::scroll_delta`]. A scroll step that starts
//!   and ends at the top (e.g. a wheel-up notch while already following at
//!   offset `0`) never disengages follow, and a scroll step that lands back
//!   on the top re-engages it.
//! - [`Viewport::jump_to_bottom`] is `End`/`F`: it disengages follow and
//!   pins the offset to the bottom so the newest content is visible.
//! - [`Viewport::jump_to_top_and_follow`] is `Home`: it re-engages follow
//!   and pins the offset to the top.
//! - [`Viewport::set_block_heights`] applies a new block-height list after an
//!   append/replace. While follow is engaged the window stays pinned to the
//!   top (content grows downward from the start); while disengaged the offset
//!   is left unchanged (clamped only if content shrank under it), so a
//!   scrolled reading position survives new output arriving below.

/// A block's height in terminal rows, as placed by the emitter.
pub type Rows = u32;

/// Visibility window over a list of block heights.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Viewport {
    heights: Vec<Rows>,
    pane_rows: Rows,
    offset: Rows,
    follow: bool,
}

impl Viewport {
    /// Creates a viewport with follow engaged and no content yet.
    pub fn new(pane_rows: Rows) -> Self {
        Self {
            heights: Vec::new(),
            pane_rows: pane_rows.max(1),
            offset: 0,
            follow: true,
        }
    }

    /// Whether follow is currently engaged.
    pub fn following(&self) -> bool {
        self.follow
    }

    /// Current top-of-window offset, in rows from the start of the content.
    pub fn offset(&self) -> Rows {
        self.offset
    }

    /// Total content height across all blocks.
    pub fn total_rows(&self) -> Rows {
        self.heights.iter().copied().fold(0, Rows::saturating_add)
    }

    /// The largest offset that still leaves the pane full of content (0 when
    /// content is shorter than the pane).
    pub fn max_offset(&self) -> Rows {
        self.total_rows().saturating_sub(self.pane_rows)
    }

    /// Updates the pane's visible height (e.g. on terminal resize), reclamping
    /// the offset when follow is engaged or the window now overshoots.
    ///
    /// Not called from `agent_viewer` yet: resize detection is not part of
    /// this task (T3-302 covers scroll/follow only). Kept as a tested public
    /// entry point for whichever task wires up `SIGWINCH`/resize handling.
    #[allow(dead_code)]
    pub fn set_pane_rows(&mut self, pane_rows: Rows) {
        self.pane_rows = pane_rows.max(1);
        self.reclamp();
    }

    /// Applies a manual scroll delta in rows (positive scrolls down/forward,
    /// negative scrolls up/backward) and clamps the result. Follow tracks
    /// the RESULTING position, not the act of scrolling: it disengages the
    /// instant the window leaves the top (`new_offset != 0`) and re-engages
    /// the instant a scroll lands it back on the top. A wheel-up notch while
    /// already at offset `0` (delta clamped back to `0`) never disengages
    /// follow. When content fits the pane (`max_offset() == 0`), the offset
    /// is always `0`, so follow stays permanently engaged and this function
    /// is inert — correct, since there is nothing to scroll. Returns whether
    /// the offset actually changed.
    pub fn scroll_by(&mut self, delta_rows: f32) -> bool {
        let before = self.offset;
        let max = self.max_offset();
        let target = (self.offset as f32 + delta_rows).clamp(0.0, max as f32);
        self.offset = target.round().clamp(0.0, max as f32) as Rows;
        self.follow = self.offset == 0;
        self.offset != before
    }

    /// `End`/`F`: disengages follow and pins the window to the bottom.
    pub fn jump_to_bottom(&mut self) {
        self.follow = false;
        self.offset = self.max_offset();
    }

    /// `Home`: re-engages follow and pins the window to the top.
    pub fn jump_to_top_and_follow(&mut self) {
        self.follow = true;
        self.offset = 0;
    }

    /// Applies a new block-height list after an append/replace/remove. While
    /// follow is engaged the window is re-pinned to the top; while disengaged
    /// the offset is preserved (clamped down only if the content is now
    /// shorter than the previous offset).
    pub fn set_block_heights(&mut self, heights: Vec<Rows>) {
        self.heights = heights;
        self.reclamp();
        if self.follow {
            self.offset = 0;
        }
    }

    /// The block-local visible window, as an inclusive-exclusive `(first,
    /// last_exclusive)` index range into the block-height list, plus the row
    /// offset within the first visible block to start drawing from. Blocks
    /// entirely above or below the window are excluded.
    pub fn visible_blocks(&self) -> VisibleRange {
        if self.heights.is_empty() {
            return VisibleRange {
                first: 0,
                last_exclusive: 0,
                skip_rows_in_first: 0,
            };
        }
        let window_start = self.offset;
        let window_end = self.offset.saturating_add(self.pane_rows);

        let mut first = None;
        let mut last_exclusive = 0;
        let mut skip_rows_in_first = 0;
        let mut cursor: Rows = 0;
        for (index, &height) in self.heights.iter().enumerate() {
            let block_start = cursor;
            let block_end = cursor.saturating_add(height);
            if block_end > window_start && block_start < window_end {
                if first.is_none() {
                    first = Some(index);
                    skip_rows_in_first = window_start.saturating_sub(block_start);
                }
                last_exclusive = index + 1;
            }
            cursor = block_end;
        }

        match first {
            Some(first) => VisibleRange {
                first,
                last_exclusive,
                skip_rows_in_first,
            },
            None => VisibleRange {
                first: 0,
                last_exclusive: 0,
                skip_rows_in_first: 0,
            },
        }
    }

    fn reclamp(&mut self) {
        let max = self.max_offset();
        if self.offset > max {
            self.offset = max;
        }
    }

    /// Stage 2's transient scrollbar thumb: the region-relative row range
    /// (0-indexed within the pane's `pane_rows` content rows, e.g. `2..5`
    /// meaning "draw the thumb glyph on the 3rd, 4th, and 5th content
    /// rows") the thumb should occupy for the CURRENT offset, or `None`
    /// when there is nothing to scroll (`total_rows <= pane_rows` — the
    /// whole document already fits, so a scrollbar would be meaningless
    /// chrome). Pure position math only; drawing the thumb glyph in the
    /// pane's last column and deciding WHEN it should be visible
    /// (`agent_viewer`'s auto-hide timer) are the caller's job.
    ///
    /// Thumb height is proportional to the fraction of content visible
    /// (`pane_rows * pane_rows / total_rows`, rounded, clamped to at least
    /// 1 row so it never vanishes to nothing on very long documents), and
    /// thumb position is proportional to scroll progress
    /// (`offset / max_offset`) within the remaining track
    /// (`pane_rows - thumb_height`), matching the conventional
    /// proportional-scrollbar contract most terminal/GUI scrollbars use:
    /// offset `0` puts the thumb at the top, `max_offset` puts it flush at
    /// the bottom, with linear interpolation in between.
    pub fn scrollbar_thumb_rows(&self) -> Option<std::ops::Range<Rows>> {
        let total = self.total_rows();
        if total <= self.pane_rows {
            return None;
        }
        let pane = self.pane_rows;
        let thumb_height = ((u64::from(pane) * u64::from(pane)) / u64::from(total))
            .clamp(1, u64::from(pane)) as Rows;
        let track = pane.saturating_sub(thumb_height);
        let max_offset = self.max_offset();
        let thumb_start = if max_offset == 0 {
            0
        } else {
            ((u64::from(self.offset) * u64::from(track)) / u64::from(max_offset)) as Rows
        }
        .min(track);
        Some(thumb_start..thumb_start.saturating_add(thumb_height))
    }
}

/// The block index range currently intersecting the visible window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VisibleRange {
    pub first: usize,
    pub last_exclusive: usize,
    /// Rows of the first visible block that are scrolled above the window
    /// top. `agent_viewer::sync_visible_window` passes this straight through
    /// to `TerminalSink::sync_window`, which crops the first drawn block's
    /// placeholder rows to `skip_rows_in_first..rows`
    /// (`native_stream::sync_window_operations`'s `skip_rows_in_first`
    /// parameter) — a genuine protocol-native crop (see
    /// `tmath_core::placement::emit_placed_block_row_range_cursor`'s doc
    /// comment), not a re-render. This closes the scroll-region viewer's
    /// reach-the-beginning defect: before this was wired up, a block only
    /// partially scrolled into view was always drawn in FULL, pushing its
    /// top rows above the pane's actual content area — indistinguishable
    /// from "scrolling stopped working" even though `Viewport::offset()`
    /// had genuinely reached `0`, since the visible content never actually
    /// showed block 0's true top.
    pub skip_rows_in_first: Rows,
}

impl VisibleRange {
    /// Used by `sync_visible_window` (T3-303 passes an empty range straight
    /// through to `sync_window`, which correctly deletes a previously
    /// non-empty window's placements rather than skip the call) and by
    /// stage 2's `agent_viewer::finish_momentum_step`, which must not
    /// attempt an incremental scroll-back step into an empty window.
    pub fn is_empty(&self) -> bool {
        self.first >= self.last_exclusive
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_viewport_follows_with_no_content() {
        let viewport = Viewport::new(10);
        assert!(viewport.following());
        assert_eq!(viewport.offset(), 0);
        assert_eq!(viewport.max_offset(), 0);
    }

    #[test]
    fn scroll_by_disengages_follow_and_clamps_to_bounds() {
        let mut viewport = Viewport::new(5);
        viewport.set_block_heights(vec![10, 10, 10]); // total 30, max_offset 25
        assert!(
            viewport.following(),
            "append pinned to top while following"
        );
        assert_eq!(viewport.offset(), 0);

        assert!(viewport.scroll_by(10.0));
        assert!(!viewport.following(), "manual scroll disengages follow");
        assert_eq!(viewport.offset(), 10);

        assert!(viewport.scroll_by(100.0));
        assert_eq!(viewport.offset(), 25, "clamped at the bottom");
        assert!(
            !viewport.following(),
            "landing on the bottom does not re-engage follow"
        );

        assert!(viewport.scroll_by(-100.0));
        assert_eq!(viewport.offset(), 0, "clamped at the top");
        assert!(
            viewport.following(),
            "landing back on the top re-engages follow"
        );

        assert!(viewport.scroll_by(3.0));
        assert_eq!(viewport.offset(), 3);
        assert!(
            !viewport.following(),
            "moving off the top again disengages follow"
        );
    }

    /// A wheel-up notch while already following at the top must NOT
    /// disengage follow — the offset never actually leaves zero.
    #[test]
    fn scroll_by_at_the_top_that_stays_at_the_top_never_disengages_follow() {
        let mut viewport = Viewport::new(5);
        viewport.set_block_heights(vec![10, 10, 10]); // max_offset 25
        assert!(viewport.following());
        assert_eq!(viewport.offset(), 0);

        assert!(!viewport.scroll_by(-3.0), "no actual movement at the clamp");
        assert!(
            viewport.following(),
            "an up-notch that cannot move past the top must not disengage follow"
        );

        assert!(!viewport.scroll_by(0.0));
        assert!(viewport.following());
    }

    /// Scrolling down (disengaging) and then all the way back up re-engages
    /// follow, without needing Home.
    #[test]
    fn scrolling_back_up_to_the_top_reengages_follow() {
        let mut viewport = Viewport::new(5);
        viewport.set_block_heights(vec![10, 10, 10]); // max_offset 25
        assert!(viewport.scroll_by(10.0)); // offset -> 10, disengages
        assert!(!viewport.following());

        assert!(viewport.scroll_by(10.0)); // offset -> 20, still short of 0
        assert!(!viewport.following(), "not back at the top yet");

        assert!(viewport.scroll_by(-20.0)); // offset -> 0, exactly the top
        assert!(
            viewport.following(),
            "landing exactly on offset 0 re-engages follow"
        );
    }

    /// Content-fits case named in the coordinator's ruling: `max_offset() ==
    /// 0` means offset is always `0 == max`, so follow stays permanently
    /// engaged and scroll input is inert.
    #[test]
    fn scroll_by_when_content_fits_the_pane_keeps_follow_engaged() {
        let mut viewport = Viewport::new(10);
        viewport.set_block_heights(vec![3]); // total 3 <= pane 10, max_offset 0
        assert!(viewport.following());
        assert!(!viewport.scroll_by(-5.0));
        assert!(
            viewport.following(),
            "nothing to scroll: follow stays engaged"
        );
        assert!(!viewport.scroll_by(5.0));
        assert!(viewport.following());
    }

    #[test]
    fn scroll_by_reports_no_change_when_already_clamped() {
        let mut viewport = Viewport::new(5);
        viewport.set_block_heights(vec![3]); // total 3 < pane 5, max_offset 0
        assert!(!viewport.scroll_by(-5.0), "already at the top");
        assert!(!viewport.scroll_by(5.0), "content fits, nothing to scroll");
    }

    #[test]
    fn end_or_f_jumps_to_bottom_and_disengages_follow() {
        let mut viewport = Viewport::new(5);
        viewport.set_block_heights(vec![10, 10, 10]);
        assert!(viewport.following());
        assert_eq!(viewport.offset(), 0);

        viewport.jump_to_bottom();
        assert!(!viewport.following());
        assert_eq!(viewport.offset(), 25);
    }

    #[test]
    fn home_reengages_follow_and_jumps_to_top() {
        let mut viewport = Viewport::new(5);
        viewport.set_block_heights(vec![10, 10, 10]);
        viewport.jump_to_bottom();
        assert!(!viewport.following());
        assert_eq!(viewport.offset(), 25);

        viewport.jump_to_top_and_follow();
        assert!(viewport.following());
        assert_eq!(viewport.offset(), 0);
    }

    #[test]
    fn append_while_following_keeps_the_viewport_pinned_to_the_top() {
        let mut viewport = Viewport::new(5);
        viewport.set_block_heights(vec![10]);
        assert_eq!(
            viewport.offset(),
            0,
            "follow keeps the window pinned to the top"
        );

        viewport.set_block_heights(vec![10, 8]); // total 18, max_offset 13
        assert!(viewport.following());
        assert_eq!(
            viewport.offset(),
            0,
            "follow keeps pinning to the top as blocks are appended"
        );
    }

    #[test]
    fn append_while_disengaged_keeps_the_offset_stable() {
        let mut viewport = Viewport::new(5);
        viewport.set_block_heights(vec![10, 10]); // total 20, max_offset 15
        viewport.scroll_by(10.0); // offset -> 10, follow disengaged
        assert_eq!(viewport.offset(), 10);

        viewport.set_block_heights(vec![10, 10, 10]); // total 30, max_offset 25
        assert!(!viewport.following(), "append does not re-engage follow");
        assert_eq!(
            viewport.offset(),
            10,
            "offset measured from the top stays stable across an append"
        );
    }

    #[test]
    fn disengaged_offset_is_clamped_down_if_content_shrinks_under_it() {
        let mut viewport = Viewport::new(5);
        viewport.set_block_heights(vec![10, 10, 10]); // max_offset 25
        viewport.scroll_by(10.0); // offset -> 10
        assert_eq!(viewport.offset(), 10);

        viewport.set_block_heights(vec![10]); // total 10, max_offset 5
        assert!(!viewport.following());
        assert_eq!(
            viewport.offset(),
            5,
            "offset clamps down when content shrinks below the previous offset"
        );
    }

    #[test]
    fn set_pane_rows_reclamps_offset() {
        let mut viewport = Viewport::new(5);
        viewport.set_block_heights(vec![10, 10, 10]);
        viewport.scroll_by(10.0); // offset -> 10, max_offset 25
        assert_eq!(viewport.offset(), 10);

        viewport.set_pane_rows(28); // max_offset becomes 2
        assert_eq!(viewport.offset(), 2, "resize clamps an out-of-range offset");
    }

    #[test]
    fn visible_blocks_reports_the_intersecting_index_range() {
        let mut viewport = Viewport::new(5);
        viewport.set_block_heights(vec![3, 4, 6]); // total 13, max_offset 8
        viewport.jump_to_bottom();
        assert_eq!(viewport.offset(), 8);

        let visible = viewport.visible_blocks();
        // Window is rows [8, 13). Block 0: [0,3) excluded. Block 1: [3,7)
        // intersects [8,13)? 7 > 8 is false, so block 1 is excluded too.
        // Block 2: [7,13) intersects [8,13) -> included.
        assert_eq!(visible.first, 2);
        assert_eq!(visible.last_exclusive, 3);
        assert!(!visible.is_empty());
    }

    #[test]
    fn visible_blocks_empty_when_no_content() {
        let viewport = Viewport::new(5);
        assert!(viewport.visible_blocks().is_empty());
    }

    #[test]
    fn visible_blocks_spans_multiple_blocks_within_the_window() {
        let mut viewport = Viewport::new(10);
        viewport.set_block_heights(vec![3, 4, 6]); // total 13, pane 10, max_offset 3
        viewport.jump_to_bottom();
        assert_eq!(viewport.offset(), 3);
        // Window [3, 13): block0 [0,3) excluded (block_end 3 > window_start 3
        // is false), block1 [3,7) included, block2 [7,13) included.
        let visible = viewport.visible_blocks();
        assert_eq!(visible.first, 1);
        assert_eq!(visible.last_exclusive, 3);
    }

    #[test]
    fn scrollbar_thumb_rows_is_none_when_content_fits_the_pane() {
        let mut viewport = Viewport::new(10);
        viewport.set_block_heights(vec![3, 4]); // total 7 <= pane 10
        assert_eq!(viewport.scrollbar_thumb_rows(), None);

        // Exactly equal is still "fits" — no scrollbar needed.
        let mut viewport = Viewport::new(10);
        viewport.set_block_heights(vec![10]);
        assert_eq!(viewport.scrollbar_thumb_rows(), None);
    }

    #[test]
    fn scrollbar_thumb_rows_at_the_top_starts_the_thumb_at_row_zero() {
        let mut viewport = Viewport::new(10);
        viewport.set_block_heights(vec![100]); // total 100, pane 10, max_offset 90
        viewport.scroll_by(f32::MIN); // Home: offset -> 0
        let thumb = viewport.scrollbar_thumb_rows().expect("scrollbar needed");
        assert_eq!(
            thumb.start, 0,
            "offset 0 must put the thumb at the very top"
        );
    }

    #[test]
    fn scrollbar_thumb_rows_at_the_bottom_ends_the_thumb_flush_with_the_pane() {
        let mut viewport = Viewport::new(10);
        viewport.set_block_heights(vec![100]); // total 100, pane 10, max_offset 90
        viewport.jump_to_bottom(); // offset -> max_offset (90)
        let thumb = viewport.scrollbar_thumb_rows().expect("scrollbar needed");
        assert_eq!(
            thumb.end, 10,
            "offset at max_offset must put the thumb flush against the pane's bottom row"
        );
    }

    #[test]
    fn scrollbar_thumb_rows_height_is_proportional_to_visible_fraction() {
        // pane 10 out of total 100 -> visible fraction 1/10 -> thumb height
        // ~= 10 * (10/100) = 1 row.
        let mut viewport = Viewport::new(10);
        viewport.set_block_heights(vec![100]);
        let thumb = viewport.scrollbar_thumb_rows().expect("scrollbar needed");
        assert_eq!(thumb.end - thumb.start, 1);

        // pane 10 out of total 20 -> visible fraction 1/2 -> thumb height
        // ~= 10 * (10/20) = 5 rows, clearly taller than the 1-row case above.
        let mut tall_viewport = Viewport::new(10);
        tall_viewport.set_block_heights(vec![20]);
        let taller_thumb = tall_viewport
            .scrollbar_thumb_rows()
            .expect("scrollbar needed");
        assert_eq!(taller_thumb.end - taller_thumb.start, 5);
    }

    #[test]
    fn scrollbar_thumb_rows_never_exceeds_the_pane_even_on_a_huge_document() {
        let mut viewport = Viewport::new(3);
        viewport.set_block_heights(vec![1_000_000]);
        let thumb = viewport.scrollbar_thumb_rows().expect("scrollbar needed");
        assert!(
            thumb.end <= 3,
            "thumb must never extend past the pane: {thumb:?}"
        );
        assert!(
            thumb.end - thumb.start >= 1,
            "thumb must stay at least 1 row even on a huge document: {thumb:?}"
        );
    }

    #[test]
    fn scrollbar_thumb_rows_moves_proportionally_at_the_midpoint() {
        let mut viewport = Viewport::new(10);
        viewport.set_block_heights(vec![100]); // max_offset 90
        viewport.scroll_by(f32::MIN); // Home: offset -> 0 first
        viewport.scroll_by(45.0); // then halfway
        assert_eq!(viewport.offset(), 45);
        let thumb = viewport.scrollbar_thumb_rows().expect("scrollbar needed");
        // Thumb height 1, track = pane(10) - thumb(1) = 9; at offset 45/90
        // (exactly halfway), thumb_start = 45 * 9 / 90 = 4.5 -> 4 (integer
        // division), landing roughly in the middle of the pane, not at
        // either edge.
        assert!(
            thumb.start > 0 && thumb.end < 10,
            "a midpoint offset must land the thumb strictly between the \
             pane's edges: {thumb:?}"
        );
    }
}
