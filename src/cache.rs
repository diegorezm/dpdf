//! Rendered pages, bounded by a memory budget. Pure, so it is testable
//! without a window: the value type is whatever the UI stores.

use std::collections::HashMap;
use std::ops::Range;

#[derive(Clone)]
struct Entry<T> {
    width_px: u32,
    bytes: usize,
    value: T,
}

/// Page renders keyed by page, each remembering the width it was rendered
/// at. Inserting past the budget evicts the pages farthest from the ones
/// the reader is looking at.
#[derive(Clone)]
pub struct PageCache<T> {
    entries: HashMap<usize, Entry<T>>,
    bytes: usize,
    budget: usize,
}

impl<T: Clone> PageCache<T> {
    pub fn new(budget: usize) -> Self {
        Self {
            entries: HashMap::new(),
            bytes: 0,
            budget,
        }
    }

    pub fn get(&self, page: usize) -> Option<&T> {
        self.entries.get(&page).map(|e| &e.value)
    }

    pub fn width(&self, page: usize) -> Option<u32> {
        self.entries.get(&page).map(|e| e.width_px)
    }

    pub fn contains(&self, page: usize) -> bool {
        self.entries.contains_key(&page)
    }

    #[cfg(test)]
    fn bytes(&self) -> usize {
        self.bytes
    }

    /// Adds a render, then evicts pages outside `visible`, farthest first,
    /// until the cache fits its budget. Visible pages always stay.
    pub fn insert(
        &mut self,
        page: usize,
        width_px: u32,
        bytes: usize,
        value: T,
        visible: &Range<usize>,
    ) {
        let entry = Entry {
            width_px,
            bytes,
            value,
        };
        if let Some(old) = self.entries.insert(page, entry) {
            self.bytes -= old.bytes;
        }
        self.bytes += bytes;
        self.evict(visible);
    }

    /// Keeps only the pages in `range`.
    pub fn retain(&mut self, range: &Range<usize>) {
        let bytes = &mut self.bytes;
        self.entries.retain(|page, e| {
            let keep = range.contains(page);
            if !keep {
                *bytes -= e.bytes;
            }
            keep
        });
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }

    fn evict(&mut self, visible: &Range<usize>) {
        if self.bytes <= self.budget {
            return;
        }
        let distance = |page: usize| {
            if page < visible.start {
                visible.start - page
            } else {
                page.saturating_sub(visible.end.saturating_sub(1))
            }
        };
        let mut candidates: Vec<usize> = self
            .entries
            .keys()
            .copied()
            .filter(|p| !visible.contains(p))
            .collect();
        candidates.sort_by_key(|&p| std::cmp::Reverse(distance(p)));
        for page in candidates {
            if self.bytes <= self.budget {
                break;
            }
            if let Some(e) = self.entries.remove(&page) {
                self.bytes -= e.bytes;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evicts_farthest_pages_over_budget() {
        let mut cache = PageCache::new(30);
        for page in 0..3 {
            cache.insert(page, 100, 10, page, &(1..2));
        }
        assert_eq!(cache.bytes(), 30);
        // Page 5 pushes it over and is the farthest from page 1.
        cache.insert(5, 100, 10, 5, &(1..2));
        assert!(!cache.contains(5));
        assert_eq!(cache.bytes(), 30);
        // Looking at page 5 now: page 0 goes first.
        cache.insert(5, 100, 10, 5, &(5..6));
        assert!(cache.contains(5) && !cache.contains(0));
        assert_eq!(cache.bytes(), 30);
    }

    #[test]
    fn visible_pages_stay_even_over_budget() {
        let mut cache = PageCache::new(5);
        cache.insert(0, 100, 10, (), &(0..2));
        cache.insert(1, 100, 10, (), &(0..2));
        assert!(cache.contains(0) && cache.contains(1));
        assert_eq!(cache.bytes(), 20);
    }

    #[test]
    fn replacing_and_retaining_track_bytes() {
        let mut cache = PageCache::new(100);
        cache.insert(3, 100, 10, (), &(0..1));
        cache.insert(3, 200, 40, (), &(0..1));
        assert_eq!((cache.bytes(), cache.width(3)), (40, Some(200)));
        cache.insert(7, 100, 10, (), &(0..1));
        cache.retain(&(5..10));
        assert_eq!(cache.bytes(), 10);
        assert!(!cache.contains(3));
        cache.clear();
        assert_eq!(cache.bytes(), 0);
    }
}
