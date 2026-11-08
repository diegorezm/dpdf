//! Settings and reading history, saved as a small text file in
//! `%APPDATA%\dpdf\state.txt`. Nothing leaves the machine.
//!
//! Format, one entry per line:
//! ```text
//! theme=dark
//! sidebar=true
//! recent=<page>\t<zoom>\t<path>
//! ```
//! Unknown or malformed lines are ignored, so the file never blocks startup.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub const MAX_RECENT: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    Dark,
    Light,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Recent {
    pub path: PathBuf,
    /// Zero-based page the reader was on.
    pub page: usize,
    pub zoom: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Store {
    pub theme: Theme,
    pub sidebar: bool,
    /// Most recently opened first.
    pub recent: Vec<Recent>,
}

impl Default for Store {
    fn default() -> Self {
        Self {
            theme: Theme::Dark,
            sidebar: true,
            recent: Vec::new(),
        }
    }
}

impl Store {
    pub fn parse(text: &str) -> Self {
        let mut store = Self::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            match key.trim() {
                "theme" => {
                    store.theme = match value.trim() {
                        "light" => Theme::Light,
                        _ => Theme::Dark,
                    }
                }
                "sidebar" => store.sidebar = value.trim() != "false",
                "recent" => store.recent.extend(parse_recent(value)),
                _ => {}
            }
        }
        store.recent.truncate(MAX_RECENT);
        store
    }

    pub fn serialize(&self) -> String {
        let theme = match self.theme {
            Theme::Dark => "dark",
            Theme::Light => "light",
        };
        let mut out = format!("theme={theme}\nsidebar={}\n", self.sidebar);
        for r in &self.recent {
            out.push_str(&format!(
                "recent={}\t{}\t{}\n",
                r.page,
                r.zoom,
                r.path.display()
            ));
        }
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
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        // Write then rename, so a crash never leaves a half-written file.
        let tmp = path.with_extension("tmp");
        fs::write(&tmp, self.serialize())?;
        fs::rename(tmp, path)
    }
}

fn parse_recent(value: &str) -> Option<Recent> {
    let mut parts = value.splitn(3, '\t');
    let page = parts.next()?.parse().ok()?;
    let zoom: f32 = parts.next()?.parse().ok()?;
    let path = parts.next().filter(|p| !p.is_empty())?;
    Some(Recent {
        path: PathBuf::from(path),
        page,
        zoom: if zoom.is_finite() && zoom > 0.0 {
            zoom
        } else {
            1.0
        },
    })
}

fn file_path() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(base).join("dpdf").join("state.txt"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recent(path: &str, page: usize) -> Recent {
        Recent {
            path: PathBuf::from(path),
            page,
            zoom: 1.5,
        }
    }

    #[test]
    fn round_trip() {
        let store = Store {
            theme: Theme::Light,
            sidebar: false,
            recent: vec![recent("C:\\docs\\a b.pdf", 12), recent("D:\\x.pdf", 0)],
        };
        assert_eq!(Store::parse(&store.serialize()), store);
    }

    #[test]
    fn defaults_and_garbage() {
        assert_eq!(Store::parse(""), Store::default());
        let store = Store::parse(
            "nonsense\ntheme=purple\nrecent=x\t1\tC:\\a.pdf\nrecent=3\t-1\tC:\\b.pdf\n",
        );
        assert_eq!(store.theme, Theme::Dark);
        assert_eq!(
            store.recent,
            vec![Recent {
                path: "C:\\b.pdf".into(),
                page: 3,
                zoom: 1.0
            }]
        );
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
