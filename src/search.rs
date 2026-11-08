//! Search results and match navigation. Pure and UI-free.

use crate::pdf::NormRect;

pub struct Match {
    pub page: usize,
    pub rects: Vec<NormRect>,
}

/// Results of one search, filled in page order as the worker reports them.
pub struct SearchState {
    pub id: u64,
    matches: Vec<Match>,
    current: Option<usize>,
    /// The page the reader was on; the first match at or after it is selected.
    start_page: usize,
    done: bool,
}

impl SearchState {
    pub fn new(id: u64, start_page: usize) -> Self {
        Self {
            id,
            matches: Vec::new(),
            current: None,
            start_page,
            done: false,
        }
    }

    /// Adds a page's matches. Returns true if the selection changed.
    pub fn add(&mut self, page: usize, rects: Vec<Vec<NormRect>>) -> bool {
        let first_new = self.matches.len();
        self.matches
            .extend(rects.into_iter().map(|rects| Match { page, rects }));
        if self.current.is_none() && page >= self.start_page && first_new < self.matches.len() {
            self.current = Some(first_new);
            return true;
        }
        false
    }

    /// Marks the search complete. Returns true if the selection changed.
    pub fn finish(&mut self) -> bool {
        self.done = true;
        if self.current.is_none() && !self.matches.is_empty() {
            // Nothing after the start page: wrap to the first match.
            self.current = Some(0);
            return true;
        }
        false
    }

    pub fn next(&mut self) {
        let len = self.matches.len();
        if len > 0 {
            self.current = Some(self.current.map_or(0, |i| (i + 1) % len));
        }
    }

    pub fn previous(&mut self) {
        let len = self.matches.len();
        if len > 0 {
            self.current = Some(self.current.map_or(len - 1, |i| (i + len - 1) % len));
        }
    }

    pub fn current(&self) -> Option<&Match> {
        self.current.and_then(|i| self.matches.get(i))
    }

    /// Matches on `page`, each flagged if it is the current one.
    pub fn on_page(&self, page: usize) -> impl Iterator<Item = (&NormRect, bool)> {
        let start = self.matches.partition_point(|m| m.page < page);
        self.matches[start..]
            .iter()
            .enumerate()
            .take_while(move |(_, m)| m.page == page)
            .flat_map(move |(offset, m)| {
                let is_current = self.current == Some(start + offset);
                m.rects.iter().map(move |r| (r, is_current))
            })
    }

    /// Status like "3 / 27", "0 results", or "searching…".
    pub fn status(&self) -> String {
        match (self.current, self.matches.len(), self.done) {
            (_, 0, true) => "no results".to_owned(),
            (_, 0, false) => "searching…".to_owned(),
            (Some(i), total, done) => {
                format!("{} / {}{}", i + 1, total, if done { "" } else { "+" })
            }
            (None, total, _) => format!("{total}+"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect() -> NormRect {
        NormRect {
            x: 0.0,
            y: 0.0,
            width: 0.1,
            height: 0.1,
        }
    }

    #[test]
    fn selects_first_match_after_start_page() {
        let mut s = SearchState::new(1, 3);
        assert!(!s.add(1, vec![vec![rect()]]));
        assert!(s.add(4, vec![vec![rect()], vec![rect()]]));
        assert_eq!(s.current().map(|m| m.page), Some(4));
        assert_eq!(s.status(), "2 / 3+");
        assert!(!s.finish());
        assert_eq!(s.status(), "2 / 3");
    }

    #[test]
    fn wraps_to_start_when_nothing_after() {
        let mut s = SearchState::new(1, 9);
        s.add(2, vec![vec![rect()]]);
        assert!(s.current().is_none());
        assert!(s.finish());
        assert_eq!(s.current().map(|m| m.page), Some(2));
    }

    #[test]
    fn next_and_previous_wrap() {
        let mut s = SearchState::new(1, 0);
        s.add(0, vec![vec![rect()]]);
        s.add(5, vec![vec![rect()]]);
        s.next();
        assert_eq!(s.current().map(|m| m.page), Some(5));
        s.next();
        assert_eq!(s.current().map(|m| m.page), Some(0));
        s.previous();
        assert_eq!(s.current().map(|m| m.page), Some(5));
    }

    #[test]
    fn on_page_flags_current() {
        let mut s = SearchState::new(1, 0);
        s.add(0, vec![vec![rect()]]);
        s.add(2, vec![vec![rect(), rect()], vec![rect()]]);
        s.next();
        let flags: Vec<bool> = s.on_page(2).map(|(_, c)| c).collect();
        assert_eq!(flags, vec![true, true, false]);
        assert_eq!(s.on_page(1).count(), 0);
    }

    #[test]
    fn empty_status() {
        let mut s = SearchState::new(1, 0);
        assert_eq!(s.status(), "searching…");
        s.finish();
        assert_eq!(s.status(), "no results");
        s.next();
        assert!(s.current().is_none());
    }
}
