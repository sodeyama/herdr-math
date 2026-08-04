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
//!   `[0, max_offset]`, and disengages follow — any manual scroll input maps
//!   here through [`tmath_core::scroll_driver::scroll_delta`].
//! - [`Viewport::jump_to_bottom_and_follow`] is `End`/`F`: it re-engages
//!   follow and pins the offset to the bottom in one step.
//! - [`Viewport::set_block_heights`] applies a new block-height list after an
//!   append/replace. While follow is engaged the window stays pinned to the
//!   bottom (the newest block is visible); while disengaged the offset is
//!   left unchanged (clamped only if content shrank under it), so scrolled-up
//!   reading position survives new output arriving below.

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
    /// negative scrolls up/backward), clamps the result, and disengages
    /// follow. Returns whether the offset actually changed.
    pub fn scroll_by(&mut self, delta_rows: f32) -> bool {
        self.follow = false;
        let before = self.offset;
        let max = self.max_offset();
        let target = (self.offset as f32 + delta_rows).clamp(0.0, max as f32);
        self.offset = target.round().clamp(0.0, max as f32) as Rows;
        self.offset != before
    }

    /// `End`/`F`: re-engages follow and pins the window to the bottom.
    pub fn jump_to_bottom_and_follow(&mut self) {
        self.follow = true;
        self.offset = self.max_offset();
    }

    /// Applies a new block-height list after an append/replace/remove. While
    /// follow is engaged the window is re-pinned to the bottom; while
    /// disengaged the offset is preserved (clamped down only if the content
    /// is now shorter than the previous offset).
    pub fn set_block_heights(&mut self, heights: Vec<Rows>) {
        self.heights = heights;
        self.reclamp();
        if self.follow {
            self.offset = self.max_offset();
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
}

/// The block index range currently intersecting the visible window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VisibleRange {
    pub first: usize,
    pub last_exclusive: usize,
    /// Rows of the first visible block that are scrolled above the window
    /// top (informational; full-window redraw does not crop mid-block).
    pub skip_rows_in_first: Rows,
}

impl VisibleRange {
    /// Not called from `agent_viewer` (since T3-303, `sync_visible_window`
    /// passes an empty range straight through to `sync_window`, which
    /// correctly deletes a previously non-empty window's placements rather
    /// than skip the call). Kept as a tested public predicate for the empty
    /// case.
    #[allow(dead_code)]
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
            "append pinned to bottom while following"
        );
        assert_eq!(viewport.offset(), 25);

        assert!(viewport.scroll_by(-100.0));
        assert!(!viewport.following(), "manual scroll disengages follow");
        assert_eq!(viewport.offset(), 0, "clamped at the top");

        assert!(viewport.scroll_by(100.0));
        assert_eq!(viewport.offset(), 25, "clamped at the bottom");

        assert!(viewport.scroll_by(-3.0));
        assert_eq!(viewport.offset(), 22);
    }

    #[test]
    fn scroll_by_reports_no_change_when_already_clamped() {
        let mut viewport = Viewport::new(5);
        viewport.set_block_heights(vec![3]); // total 3 < pane 5, max_offset 0
        viewport.follow = false;
        assert!(!viewport.scroll_by(-5.0), "already at the top");
        assert!(!viewport.scroll_by(5.0), "content fits, nothing to scroll");
    }

    #[test]
    fn end_or_f_reengages_follow_and_jumps_to_bottom() {
        let mut viewport = Viewport::new(5);
        viewport.set_block_heights(vec![10, 10, 10]);
        viewport.scroll_by(-100.0);
        assert!(!viewport.following());
        assert_eq!(viewport.offset(), 0);

        viewport.jump_to_bottom_and_follow();
        assert!(viewport.following());
        assert_eq!(viewport.offset(), 25);
    }

    #[test]
    fn append_while_following_pins_the_new_tail_visible() {
        let mut viewport = Viewport::new(5);
        viewport.set_block_heights(vec![10]);
        assert_eq!(
            viewport.offset(),
            5,
            "max_offset for a 10-row block in a 5-row pane"
        );

        viewport.set_block_heights(vec![10, 8]); // total 18, max_offset 13
        assert!(viewport.following());
        assert_eq!(
            viewport.offset(),
            13,
            "follow keeps pinning to the bottom as blocks are appended"
        );
    }

    #[test]
    fn append_while_disengaged_keeps_the_offset_stable() {
        let mut viewport = Viewport::new(5);
        viewport.set_block_heights(vec![10, 10]); // total 20, max_offset 15
        viewport.scroll_by(-15.0); // offset -> 0, follow disengaged
        assert_eq!(viewport.offset(), 0);

        viewport.set_block_heights(vec![10, 10, 10]); // total 30, max_offset 25
        assert!(!viewport.following(), "append does not re-engage follow");
        assert_eq!(
            viewport.offset(),
            0,
            "offset measured from the top stays stable across an append"
        );
    }

    #[test]
    fn disengaged_offset_is_clamped_down_if_content_shrinks_under_it() {
        let mut viewport = Viewport::new(5);
        viewport.set_block_heights(vec![10, 10, 10]); // max_offset 25
        viewport.scroll_by(-10.0); // offset -> 15
        assert_eq!(viewport.offset(), 15);

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
        viewport.scroll_by(-10.0); // offset -> 15, max_offset 25
        assert_eq!(viewport.offset(), 15);

        viewport.set_pane_rows(28); // max_offset becomes 2
        assert_eq!(viewport.offset(), 2, "resize clamps an out-of-range offset");
    }

    #[test]
    fn visible_blocks_reports_the_intersecting_index_range() {
        let mut viewport = Viewport::new(5);
        viewport.set_block_heights(vec![3, 4, 6]); // total 13, max_offset 8
        viewport.jump_to_bottom_and_follow();
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
        viewport.jump_to_bottom_and_follow();
        assert_eq!(viewport.offset(), 3);
        // Window [3, 13): block0 [0,3) excluded (block_end 3 > window_start 3
        // is false), block1 [3,7) included, block2 [7,13) included.
        let visible = viewport.visible_blocks();
        assert_eq!(visible.first, 1);
        assert_eq!(visible.last_exclusive, 3);
    }
}
