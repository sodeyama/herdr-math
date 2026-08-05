//! Bounded least-recently-used cache for rendered blocks.
//!
//! [`crate::render_block`] is byte-deterministic for a block and its render
//! options, so returning a cached image is observationally identical to
//! rendering it again. Content hashes are used only as `HashMap` keys here;
//! they are never authorization or boundary-enforcement inputs.
//!
//! Render failures are not cached because later call sites may use different
//! limits, and a cached failure would poison streaming retries. A successful
//! entry whose pixel cost exceeds the whole cache budget is returned to the
//! caller but is not cached.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use crate::{content_hash, render_block, Block, RenderError, RenderOptions, RenderedBlock};

/// Finite bounds for the render cache.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CacheBudget {
    pub max_entries: usize,
    pub max_pixels: u64,
}

/// Cumulative cache counters plus the current retained size.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    pub entries: usize,
    pub pixels: u64,
}

struct CacheEntry {
    rendered: Arc<RenderedBlock>,
    pixels: u64,
}

/// Bounded LRU keyed by the complete render-content hash.
pub struct RenderCache {
    budget: CacheBudget,
    entries: HashMap<[u8; 32], CacheEntry>,
    order: VecDeque<[u8; 32]>,
    hits: u64,
    misses: u64,
    evictions: u64,
    pixels: u64,
}

impl RenderCache {
    /// Creates a cache with positive entry and pixel budgets.
    ///
    /// # Panics
    ///
    /// Panics if either budget axis is zero.
    pub fn new(budget: CacheBudget) -> Self {
        assert!(
            budget.max_entries > 0,
            "max_entries must be greater than zero"
        );
        assert!(
            budget.max_pixels > 0,
            "max_pixels must be greater than zero"
        );
        Self {
            budget,
            entries: HashMap::new(),
            order: VecDeque::new(),
            hits: 0,
            misses: 0,
            evictions: 0,
            pixels: 0,
        }
    }

    /// Returns a cached block or renders and conditionally caches a new one.
    pub fn render(
        &mut self,
        block: &Block,
        options: &RenderOptions,
    ) -> Result<Arc<RenderedBlock>, RenderError> {
        let key = content_hash(block, options);
        // The content hash is identity for this map lookup only. It does not
        // authorize rendering, placement, access, or any boundary decision.
        if let Some(rendered) = self
            .entries
            .get(&key)
            .map(|entry| Arc::clone(&entry.rendered))
        {
            self.hits = self.hits.saturating_add(1);
            self.promote(key);
            return Ok(rendered);
        }

        self.misses = self.misses.saturating_add(1);
        let rendered = Arc::new(render_block(block, options)?);
        let pixels = u64::from(rendered.width_px) * u64::from(rendered.height_px);
        if pixels > self.budget.max_pixels {
            return Ok(rendered);
        }

        self.evict_for(pixels);
        self.pixels += pixels;
        self.entries.insert(
            key,
            CacheEntry {
                rendered: Arc::clone(&rendered),
                pixels,
            },
        );
        self.order.push_back(key);
        Ok(rendered)
    }

    /// Returns cumulative counters and the current cache footprint.
    pub fn stats(&self) -> CacheStats {
        CacheStats {
            hits: self.hits,
            misses: self.misses,
            evictions: self.evictions,
            entries: self.entries.len(),
            pixels: self.pixels,
        }
    }

    fn promote(&mut self, key: [u8; 32]) {
        let position = self
            .order
            .iter()
            .position(|candidate| *candidate == key)
            .expect("cached key must exist in LRU order");
        self.order
            .remove(position)
            .expect("known LRU position must be removable");
        self.order.push_back(key);
    }

    fn evict_for(&mut self, new_pixels: u64) {
        while self.entries.len() >= self.budget.max_entries
            || self.pixels > self.budget.max_pixels - new_pixels
        {
            let key = self
                .order
                .pop_front()
                .expect("cache needing room must have an LRU entry");
            let entry = self
                .entries
                .remove(&key)
                .expect("LRU key must have a cache entry");
            self.pixels = self
                .pixels
                .checked_sub(entry.pixels)
                .expect("entry pixels must be included in total");
            self.evictions = self.evictions.saturating_add(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::{render_block, BlockKind, ErrorCode};

    fn block(kind: BlockKind, source: &str) -> Block {
        Block {
            index: 0,
            kind,
            source: source.to_owned(),
        }
    }

    fn pixels(rendered: &RenderedBlock) -> u64 {
        u64::from(rendered.width_px) * u64::from(rendered.height_px)
    }

    fn cache(max_entries: usize, max_pixels: u64) -> RenderCache {
        RenderCache::new(CacheBudget {
            max_entries,
            max_pixels,
        })
    }

    #[test]
    fn hit_reuses_the_arc_and_matches_a_fresh_deterministic_render() {
        let block = block(BlockKind::Paragraph, "Cache me with $a+b$.");
        let options = RenderOptions::default();
        let fresh = render_block(&block, &options).unwrap();
        let mut cache = cache(4, u64::MAX);

        let first = cache.render(&block, &options).unwrap();
        let second = cache.render(&block, &options).unwrap();

        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(second.png, fresh.png);
        assert_eq!(
            cache.stats(),
            CacheStats {
                hits: 1,
                misses: 1,
                evictions: 0,
                entries: 1,
                pixels: pixels(&first),
            }
        );
    }

    #[test]
    fn every_render_input_is_part_of_the_cache_key() {
        let original = block(BlockKind::Paragraph, "same");
        let options = RenderOptions::default();
        let mut cache = cache(16, u64::MAX);

        cache.render(&original, &options).unwrap();
        cache
            .render(&block(BlockKind::Paragraph, "different"), &options)
            .unwrap();
        cache
            .render(&block(BlockKind::Heading, "same"), &options)
            .unwrap();
        cache
            .render(&original, &RenderOptions::new(481.0, 12.0, 1).unwrap())
            .unwrap();
        cache
            .render(&original, &RenderOptions::new(480.0, 13.0, 1).unwrap())
            .unwrap();
        cache
            .render(&original, &RenderOptions::new(480.0, 12.0, 2).unwrap())
            .unwrap();

        assert_eq!(cache.stats().hits, 0);
        assert_eq!(cache.stats().misses, 6);
        assert_eq!(cache.stats().entries, 6);
    }

    #[test]
    fn pixel_budget_evicts_the_least_recent_entry_and_allows_rerender() {
        let options = RenderOptions::default();
        let blocks = [
            block(BlockKind::Paragraph, "Alpha"),
            block(BlockKind::Paragraph, "Bravo"),
            block(BlockKind::Paragraph, "Gamma"),
        ];
        let rendered = blocks
            .iter()
            .map(|block| render_block(block, &options).unwrap())
            .collect::<Vec<_>>();
        let mut costs = rendered.iter().map(pixels).collect::<Vec<_>>();
        costs.sort_unstable();
        let budget = costs[1] + costs[2];
        let mut cache = cache(3, budget);

        for block in &blocks {
            cache.render(block, &options).unwrap();
        }

        let after_three = cache.stats();
        assert_eq!(after_three.entries, 2);
        assert!(after_three.pixels <= budget);
        assert_eq!(after_three.evictions, 1);

        let rerendered = cache.render(&blocks[0], &options).unwrap();
        assert_eq!(cache.stats().misses, 4);
        assert_eq!(rerendered.png, rendered[0].png);
        assert!(cache.stats().pixels <= budget);
    }

    #[test]
    fn entry_budget_evicts_the_least_recent_entry() {
        let options = RenderOptions::default();
        let blocks = [
            block(BlockKind::Paragraph, "One"),
            block(BlockKind::Paragraph, "Two"),
            block(BlockKind::Paragraph, "Three"),
        ];
        let mut cache = cache(2, u64::MAX);

        for block in &blocks {
            cache.render(block, &options).unwrap();
        }
        assert_eq!(cache.stats().entries, 2);
        assert_eq!(cache.stats().evictions, 1);

        cache.render(&blocks[0], &options).unwrap();
        assert_eq!(cache.stats().misses, 4);
    }

    #[test]
    fn oversized_entry_is_returned_without_being_cached() {
        let block = block(BlockKind::Paragraph, "Too large for this cache");
        let options = RenderOptions::default();
        let fresh = render_block(&block, &options).unwrap();
        let mut cache = cache(2, pixels(&fresh) - 1);

        let rendered = cache.render(&block, &options).unwrap();

        assert_eq!(rendered.png, fresh.png);
        assert_eq!(
            cache.stats(),
            CacheStats {
                hits: 0,
                misses: 1,
                evictions: 0,
                entries: 0,
                pixels: 0,
            }
        );
    }

    #[test]
    fn render_errors_are_misses_but_are_not_cached() {
        // A display-math block with an unterminated `$$` never resolves to
        // one complete formula at all (`FormulaNotFound`), so it stays a
        // genuine run-level `Err` regardless of AT-3-103's invalid-LaTeX
        // badge fix — `$$\frac{a$$` (this test's original input) no longer
        // works here since that string DOES scan as one complete formula
        // and now renders an `[invalid latex]` badge (`Ok`) instead of
        // erroring, per that fix; see `lib.rs::render_tests` for the
        // badge-path coverage.
        let invalid = block(BlockKind::DisplayMath, "$$incomplete");
        let valid = block(BlockKind::Paragraph, "Valid after an error");
        let options = RenderOptions::default();
        let mut cache = cache(2, u64::MAX);

        let error = cache.render(&invalid, &options).unwrap_err();
        assert_eq!(error.safe_record().code, ErrorCode::FormulaNotFound);
        assert_eq!(cache.stats().misses, 1);
        assert_eq!(cache.stats().entries, 0);

        let rendered = cache.render(&valid, &options).unwrap();
        assert!(!rendered.png.is_empty());
        assert_eq!(cache.stats().misses, 2);
        assert_eq!(cache.stats().entries, 1);
    }

    #[test]
    fn touching_an_entry_promotes_it_before_eviction() {
        let options = RenderOptions::default();
        let a = block(BlockKind::Paragraph, "A");
        let b = block(BlockKind::Paragraph, "B");
        let c = block(BlockKind::Paragraph, "C");
        let mut cache = cache(2, u64::MAX);

        cache.render(&a, &options).unwrap();
        cache.render(&b, &options).unwrap();
        cache.render(&a, &options).unwrap();
        cache.render(&c, &options).unwrap();
        assert_eq!(cache.stats().hits, 1);
        assert_eq!(cache.stats().misses, 3);

        cache.render(&a, &options).unwrap();
        assert_eq!(cache.stats().hits, 2);
        cache.render(&b, &options).unwrap();
        assert_eq!(cache.stats().misses, 4);
    }

    #[test]
    fn constructor_rejects_zero_budgets() {
        assert!(std::panic::catch_unwind(|| cache(0, 1)).is_err());
        assert!(std::panic::catch_unwind(|| cache(1, 0)).is_err());
    }
}
