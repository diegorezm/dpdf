//! Command palette entries and fuzzy matching. Pure and UI-free.

use std::path::PathBuf;

use crate::keymap::Action;

#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    Command(Action),
    Tab(usize),
    Recent(PathBuf),
    GoToPage(usize),
    Theme(String),
    Session(usize),
    /// A page of a document found by searching the session.
    Hit(PathBuf, usize),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub target: Target,
    pub title: String,
    /// Muted text after the title, e.g. a folder or "Tab".
    pub detail: String,
    /// Shortcut shown on the right.
    pub keys: String,
}

/// Fuzzy subsequence score: higher is better, `None` if `query` doesn't
/// match. Rewards consecutive letters, word starts, and early matches.
pub fn score(query: &str, text: &str) -> Option<i32> {
    let query: Vec<char> = query
        .to_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    if query.is_empty() {
        return Some(0);
    }
    let text: Vec<char> = text.to_lowercase().chars().collect();
    let mut score = 0;
    let mut qi = 0;
    let mut previous: Option<usize> = None;
    for (ti, &c) in text.iter().enumerate() {
        if qi == query.len() {
            break;
        }
        if c != query[qi] {
            continue;
        }
        let word_start = ti == 0 || !text[ti - 1].is_alphanumeric();
        score += 1;
        if word_start {
            score += 8;
        }
        if previous == Some(ti.wrapping_sub(1)) {
            score += 5;
        }
        if ti < 3 {
            score += 3;
        }
        previous = Some(ti);
        qi += 1;
    }
    (qi == query.len()).then(|| score - text.len() as i32 / 8)
}

/// Entries matching `query`, best first. Ties keep the original order,
/// so callers list what matters most first.
pub fn filter(entries: &[Entry], query: &str, limit: usize) -> Vec<Entry> {
    let mut scored: Vec<(i32, usize)> = entries
        .iter()
        .enumerate()
        .filter_map(|(i, e)| {
            let title = score(query, &e.title);
            let detail = score(query, &e.detail).map(|s| s / 2);
            title.max(detail).map(|s| (s, i))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored
        .into_iter()
        .take(limit)
        .map(|(_, i)| entries[i].clone())
        .collect()
}

/// "42" or ":42" means "go to page 42".
pub fn page_query(query: &str) -> Option<usize> {
    let q = query.trim().trim_start_matches(':');
    q.parse::<usize>().ok().filter(|n| *n > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(title: &str) -> Entry {
        Entry {
            target: Target::Command(Action::Open),
            title: title.to_owned(),
            detail: String::new(),
            keys: String::new(),
        }
    }

    #[test]
    fn subsequence_matching() {
        assert!(score("zin", "Zoom in").is_some());
        assert!(score("zi", "Fit width").is_none());
        assert_eq!(score("", "anything"), Some(0));
        assert!(score("ZOOM", "zoom out").is_some());
    }

    #[test]
    fn ranks_word_starts_and_prefixes() {
        let entries = [
            entry("Toggle focus mode"),
            entry("Fit page"),
            entry("Go to page…"),
        ];
        let ranked = filter(&entries, "fp", 10);
        assert_eq!(ranked[0].title, "Fit page");
        let ranked = filter(&entries, "page", 10);
        assert_eq!(ranked.len(), 2);
        assert_eq!(filter(&entries, "", 2).len(), 2);
    }

    #[test]
    fn page_queries() {
        assert_eq!(page_query("42"), Some(42));
        assert_eq!(page_query(":7"), Some(7));
        assert_eq!(page_query("0"), None);
        assert_eq!(page_query("open"), None);
    }
}
