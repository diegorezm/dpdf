//! Reading history and the last session, saved as a small text file in
//! `%APPDATA%\dpdf\state.txt`. Preferences live in `config.toml` instead.
//!
//! Format, one entry per line:
//! ```text
//! recent=<page>\t<zoom>\t<path>   (zoom: fit-width, fit-page, or a factor)
//! tab=<path>
//! active-tab=<index>
//! ```
//! Unknown or malformed lines are ignored, so the file never blocks startup.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::config::{data_dir, write_atomic};
use crate::layout::Zoom;

pub const MAX_RECENT: usize = 50;

#[derive(Debug, Clone, PartialEq)]
pub struct Recent {
    pub path: PathBuf,
    /// Zero-based page the reader was on.
    pub page: usize,
    pub zoom: Zoom,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Store {
    /// Most recently opened first.
    pub recent: Vec<Recent>,
    /// Tabs open when the app last ran, in order.
    pub session: Vec<PathBuf>,
    pub active_tab: usize,
}

impl Store {
    pub fn parse(text: &str) -> Self {
        let mut store = Self::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            match key.trim() {
                "recent" => store.recent.extend(parse_recent(value)),
                "tab" if !value.is_empty() => store.session.push(PathBuf::from(value)),
                "active-tab" => store.active_tab = value.trim().parse().unwrap_or(0),
                _ => {}
            }
        }
        store.recent.truncate(MAX_RECENT);
        store
    }

    pub fn serialize(&self) -> String {
        let mut out = String::new();
        for r in &self.recent {
            out.push_str(&format!(
                "recent={}\t{}\t{}\n",
                r.page,
                r.zoom.to_token(),
                r.path.display()
            ));
        }
        for tab in &self.session {
            out.push_str(&format!("tab={}\n", tab.display()));
        }
        out.push_str(&format!("active-tab={}\n", self.active_tab));
        out
    }

    pub fn recent_for(&self, path: &Path) -> Option<&Recent> {
        self.recent.iter().find(|r| r.path == path)
    }

    /// Records a document as the most recent one.
    pub fn remember(&mut self, entry: Recent) {
        self.recent.retain(|r| r.path != entry.path);
        self.recent.insert(0, entry);
        self.recent.truncate(MAX_RECENT);
    }

    /// Loads from disk; a missing or unreadable file gives defaults.
    pub fn load() -> Self {
        file_path()
            .and_then(|path| fs::read_to_string(path).ok())
            .map(|text| Self::parse(&text))
            .unwrap_or_default()
    }

    pub fn save(&self) -> io::Result<()> {
        let path = file_path().ok_or_else(|| io::Error::other("APPDATA is not set"))?;
        write_atomic(&path, &self.serialize())
    }
}

fn parse_recent(value: &str) -> Option<Recent> {
    let mut parts = value.splitn(3, '\t');
    let page = parts.next()?.parse().ok()?;
    let zoom = parts.next()?;
    let path = parts.next().filter(|p| !p.is_empty())?;
    Some(Recent {
        path: PathBuf::from(path),
        page,
        zoom: Zoom::from_token(zoom).unwrap_or(Zoom::FitWidth),
    })
}

fn file_path() -> Option<PathBuf> {
    Some(data_dir()?.join("state.txt"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recent(path: &str, page: usize) -> Recent {
        Recent {
            path: PathBuf::from(path),
            page,
            zoom: Zoom::Custom(1.5),
        }
    }

    #[test]
    fn round_trip() {
        let store = Store {
            recent: vec![recent("C:\\docs\\a b.pdf", 12), recent("D:\\x.pdf", 0)],
            session: vec![
                PathBuf::from("C:\\docs\\a b.pdf"),
                PathBuf::from("E:\\c.pdf"),
            ],
            active_tab: 1,
        };
        assert_eq!(Store::parse(&store.serialize()), store);
    }

    #[test]
    fn defaults_and_garbage() {
        assert_eq!(Store::parse(""), Store::default());
        let store = Store::parse(
            "nonsense\ntheme=dark\nrecent=x\t1\tC:\\a.pdf\nrecent=3\tbogus\tC:\\b.pdf\ntab=\nactive-tab=z\n",
        );
        assert_eq!(
            store.recent,
            vec![Recent {
                path: "C:\\b.pdf".into(),
                page: 3,
                zoom: Zoom::FitWidth
            }]
        );
        assert!(store.session.is_empty());
        assert_eq!(store.active_tab, 0);
    }

    #[test]
    fn remember_moves_to_front_and_caps() {
        let mut store = Store::default();
        for i in 0..MAX_RECENT + 5 {
            store.remember(recent(&format!("{i}.pdf"), i));
        }
        assert_eq!(store.recent.len(), MAX_RECENT);
        store.remember(recent("10.pdf", 99));
        assert_eq!(store.recent[0].page, 99);
        assert_eq!(
            store
                .recent
                .iter()
                .filter(|r| r.path == Path::new("10.pdf"))
                .count(),
            1
        );
        assert_eq!(
            store.recent_for(Path::new("10.pdf")).map(|r| r.page),
            Some(99)
        );
    }
}
