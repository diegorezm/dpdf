//! Reading history and sessions, saved as a small text file in
//! `%APPDATA%\leme\state.txt`. Preferences live in `config.toml` instead.
//!
//! Format, one entry per line:
//! ```text
//! recent=<page>\t<zoom>\t<path>   (zoom: fit-width, fit-page, or a factor)
//! session=<name>                  (starts a session; tab lines follow)
//! tab=<path>
//! active-tab=<index>
//! current-session=<name>
//! ```
//! Tabs before any `session=` line belong to a session named "main", which
//! is how files from before sessions existed are read. Unknown or malformed
//! lines are ignored, so the file never blocks startup.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::config::{data_dir, write_atomic};
use crate::layout::Zoom;

pub const MAX_RECENT: usize = 50;
pub const DEFAULT_SESSION: &str = "main";

#[derive(Debug, Clone, PartialEq)]
pub struct Recent {
    pub path: PathBuf,
    /// Zero-based page the reader was on.
    pub page: usize,
    pub zoom: Zoom,
}

/// A named set of tabs, like a tmux session.
#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    pub name: String,
    pub tabs: Vec<PathBuf>,
    /// The tab in front, or `None` when the session shows the home screen.
    pub active: Option<usize>,
}

impl Session {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            tabs: Vec::new(),
            active: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Store {
    /// Most recently opened first.
    pub recent: Vec<Recent>,
    /// Never empty.
    pub sessions: Vec<Session>,
    pub current_session: usize,
}

impl Default for Store {
    fn default() -> Self {
        Self {
            recent: Vec::new(),
            sessions: vec![Session::new(DEFAULT_SESSION)],
            current_session: 0,
        }
    }
}

impl Store {
    pub fn parse(text: &str) -> Self {
        let mut recent = Vec::new();
        let mut sessions: Vec<Session> = Vec::new();
        let mut current = String::new();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let session = |sessions: &mut Vec<Session>| -> usize {
                if sessions.is_empty() {
                    sessions.push(Session::new(DEFAULT_SESSION));
                }
                sessions.len() - 1
            };
            match key.trim() {
                "recent" => recent.extend(parse_recent(value)),
                "session" if !value.trim().is_empty() => {
                    let name = unique_name(&sessions, value.trim());
                    sessions.push(Session::new(&name));
                }
                "tab" if !value.is_empty() => {
                    let i = session(&mut sessions);
                    sessions[i].tabs.push(PathBuf::from(value));
                }
                "active-tab" => {
                    let i = session(&mut sessions);
                    sessions[i].active = value.trim().parse().ok();
                }
                "current-session" => current = value.trim().to_owned(),
                _ => {}
            }
        }
        recent.truncate(MAX_RECENT);
        for s in &mut sessions {
            s.active = s.active.filter(|&a| a < s.tabs.len());
        }
        if sessions.is_empty() {
            sessions.push(Session::new(DEFAULT_SESSION));
        }
        let current_session = sessions.iter().position(|s| s.name == current).unwrap_or(0);
        Self {
            recent,
            sessions,
            current_session,
        }
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
        for s in &self.sessions {
            out.push_str(&format!("session={}\n", s.name));
            for tab in &s.tabs {
                out.push_str(&format!("tab={}\n", tab.display()));
            }
            if let Some(active) = s.active {
                out.push_str(&format!("active-tab={active}\n"));
            }
        }
        if let Some(s) = self.sessions.get(self.current_session) {
            out.push_str(&format!("current-session={}\n", s.name));
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
        write_atomic(&path, &self.serialize())
    }
}

/// `name`, or `name 2`, `name 3`… if it's taken. Names never contain
/// newlines, so they survive the line-based file.
pub fn unique_name(sessions: &[Session], name: &str) -> String {
    let base: String = name.lines().next().unwrap_or("").trim().to_owned();
    let base = if base.is_empty() {
        "session".to_owned()
    } else {
        base
    };
    let taken = |n: &str| sessions.iter().any(|s| s.name.eq_ignore_ascii_case(n));
    if !taken(&base) {
        return base;
    }
    (2..)
        .map(|i| format!("{base} {i}"))
        .find(|n| !taken(n))
        .unwrap_or(base)
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
            sessions: vec![
                Session {
                    name: "main".into(),
                    tabs: vec![PathBuf::from("C:\\docs\\a b.pdf")],
                    active: None,
                },
                Session {
                    name: "thesis".into(),
                    tabs: vec![PathBuf::from("E:\\c.pdf"), PathBuf::from("E:\\d.pdf")],
                    active: Some(1),
                },
            ],
            current_session: 1,
        };
        assert_eq!(Store::parse(&store.serialize()), store);
    }

    #[test]
    fn reads_files_from_before_sessions() {
        let store = Store::parse("tab=C:\\a.pdf\ntab=C:\\b.pdf\nactive-tab=1\n");
        assert_eq!(store.sessions.len(), 1);
        assert_eq!(store.sessions[0].name, DEFAULT_SESSION);
        assert_eq!(store.sessions[0].tabs.len(), 2);
        assert_eq!(store.sessions[0].active, Some(1));
    }

    #[test]
    fn defaults_and_garbage() {
        assert_eq!(Store::parse(""), Store::default());
        let store = Store::parse(
            "nonsense\ntheme=dark\nrecent=x\t1\tC:\\a.pdf\nrecent=3\tbogus\tC:\\b.pdf\ntab=\nactive-tab=9\ncurrent-session=nope\n",
        );
        assert_eq!(
            store.recent,
            vec![Recent {
                path: "C:\\b.pdf".into(),
                page: 3,
                zoom: Zoom::FitWidth
            }]
        );
        assert_eq!(store.sessions[0].active, None);
        assert_eq!(store.current_session, 0);
    }

    #[test]
    fn unique_session_names() {
        let sessions = vec![Session::new("main"), Session::new("main 2")];
        assert_eq!(unique_name(&sessions, "Main"), "Main 3");
        assert_eq!(unique_name(&sessions, "study"), "study");
        assert_eq!(unique_name(&sessions, "  \n"), "session");
        assert_eq!(unique_name(&sessions, "a\nb"), "a");
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
            store.recent_for(Path::new("10.pdf")).map(|r| r.page),
            Some(99)
        );
    }
}
