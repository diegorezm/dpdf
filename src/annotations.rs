//! Highlights, underlines, strikethroughs, and notes. Stored per document in
//! `%APPDATA%\dpdf\annotations\<hash>.toml`; the PDF itself is never touched.

use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::{data_dir, write_atomic};
use crate::pdf::NormRect;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    Highlight,
    Underline,
    Strikethrough,
}

/// Annotation colors: name and the hex shown on the page.
pub const COLORS: [(&str, &str); 5] = [
    ("yellow", "#f7d44c"),
    ("green", "#7fd18b"),
    ("blue", "#7fb2f0"),
    ("pink", "#f28fb4"),
    ("red", "#ef6b5f"),
];

pub fn color_hex(name: &str) -> &'static str {
    COLORS
        .iter()
        .find(|(n, _)| *n == name)
        .map_or(COLORS[0].1, |(_, hex)| hex)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Annotation {
    pub id: u64,
    /// Zero-based page.
    pub page: usize,
    pub kind: Kind,
    pub color: String,
    pub text: String,
    #[serde(default)]
    pub note: String,
    /// One-based line on the page where it starts; 0 if unknown.
    #[serde(default)]
    pub line: u32,
    /// Unix seconds.
    pub created: u64,
    pub rects: Vec<NormRect>,
}

/// How far the reader got with an outline section: 1 read, 2 reviewed
/// once, 3 reviewed twice. Sections are matched by page and title, which
/// stay the same as long as the PDF does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Section {
    pub page: usize,
    pub title: String,
    pub level: u8,
}

pub const MAX_LEVEL: u8 = 3;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Annotations {
    /// The document these belong to, for humans reading the file.
    #[serde(default)]
    pub document: String,
    #[serde(default, rename = "annotation")]
    pub items: Vec<Annotation>,
    /// Outline sections with a page, so the home screen can show
    /// progress without opening the document.
    #[serde(default)]
    pub outline_sections: usize,
    #[serde(default, rename = "section")]
    pub sections: Vec<Section>,
}

impl Annotations {
    pub fn load(doc: &Path) -> Self {
        file_for(doc)
            .and_then(|path| fs::read_to_string(path).ok())
            .and_then(|text| toml::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, doc: &Path) -> io::Result<()> {
        let path = file_for(doc).ok_or_else(|| io::Error::other("APPDATA is not set"))?;
        if self.items.is_empty() && self.sections.is_empty() {
            return match fs::remove_file(&path) {
                Err(err) if err.kind() != io::ErrorKind::NotFound => Err(err),
                _ => Ok(()),
            };
        }
        let body = toml::to_string(self).map_err(io::Error::other)?;
        write_atomic(&path, &body)
    }

    pub fn add(
        &mut self,
        page: usize,
        kind: Kind,
        color: &str,
        text: String,
        rects: Vec<NormRect>,
        now: u64,
    ) -> u64 {
        let id = self.items.iter().map(|a| a.id).max().map_or(1, |m| m + 1);
        self.items.push(Annotation {
            id,
            page,
            kind,
            color: color.to_owned(),
            text,
            note: String::new(),
            line: 0,
            created: now,
            rects,
        });
        id
    }

    pub fn level(&self, page: usize, title: &str) -> u8 {
        self.sections
            .iter()
            .find(|s| s.page == page && s.title == title)
            .map_or(0, |s| s.level)
    }

    /// Moves a section to the next level, wrapping back to unread.
    pub fn cycle_level(&mut self, page: usize, title: &str) -> u8 {
        let level = (self.level(page, title) + 1) % (MAX_LEVEL + 1);
        self.sections
            .retain(|s| !(s.page == page && s.title == title));
        if level > 0 {
            self.sections.push(Section {
                page,
                title: title.to_owned(),
                level,
            });
        }
        level
    }

    /// How many of `sections` (page, title) are read, and how many there are.
    pub fn progress<'a>(&self, sections: impl Iterator<Item = (usize, &'a str)>) -> (usize, usize) {
        sections.fold((0, 0), |(done, total), (page, title)| {
            (done + usize::from(self.level(page, title) > 0), total + 1)
        })
    }

    pub fn get_mut(&mut self, id: u64) -> Option<&mut Annotation> {
        self.items.iter_mut().find(|a| a.id == id)
    }

    pub fn get(&self, id: u64) -> Option<&Annotation> {
        self.items.iter().find(|a| a.id == id)
    }

    pub fn remove(&mut self, id: u64) {
        self.items.retain(|a| a.id != id);
    }

    pub fn on_page(&self, page: usize) -> impl Iterator<Item = &Annotation> {
        self.items.iter().filter(move |a| a.page == page)
    }

    /// The topmost annotation under a point on a page, in page fractions.
    pub fn at_point(&self, page: usize, x: f32, y: f32) -> Option<u64> {
        self.on_page(page)
            .filter(|a| {
                a.rects
                    .iter()
                    .any(|r| x >= r.x && x <= r.x + r.width && y >= r.y && y <= r.y + r.height)
            })
            .last()
            .map(|a| a.id)
    }

    /// Notes as Markdown for Obsidian, in reading order. `file` is the
    /// PDF's file name: each quote links to its page with
    /// `[[file#page=N]]`, which Obsidian opens at that page when the PDF is
    /// in the vault, and carries a `^dpdf-<id>` block id that stays the
    /// same across exports, so other notes can link to single quotes.
    pub fn to_markdown(&self, title: &str, file: &str, exported: u64) -> String {
        let mut items: Vec<&Annotation> = self.items.iter().collect();
        items.sort_by(|a, b| {
            let top = |x: &Annotation| x.rects.first().map_or(0.0, |r| r.y);
            a.page.cmp(&b.page).then(top(a).total_cmp(&top(b)))
        });

        let mut out = format!(
            "---\ntitle: \"{}\"\nsource: \"[[{}]]\"\nexported: {}\nannotations: {}\ntags:\n  - dpdf\n---\n\n# {title}\n",
            yaml_escape(title),
            yaml_escape(file),
            date(exported),
            items.len(),
        );
        let mut page = None;
        for a in items {
            if page != Some(a.page) {
                page = Some(a.page);
                let _ = write!(out, "\n## Page {}\n", a.page + 1);
            }
            let text = a.text.split_whitespace().collect::<Vec<_>>().join(" ");
            let text = match a.kind {
                Kind::Strikethrough => format!("~~{text}~~"),
                Kind::Highlight | Kind::Underline => text,
            };
            let place = if a.line > 0 {
                format!("p. {}, line {}", a.page + 1, a.line)
            } else {
                format!("p. {}", a.page + 1)
            };
            let _ = write!(
                out,
                "\n> [!quote|{}] [[{file}#page={}|{place}]]\n> {text}\n\n^dpdf-{}\n",
                a.color,
                a.page + 1,
                a.id,
            );
            if !a.note.trim().is_empty() {
                out.push('\n');
                out.push_str(a.note.trim());
                out.push('\n');
            }
        }
        out
    }
}

/// Escapes a string for a double-quoted YAML scalar.
fn yaml_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Undo and redo for annotation changes. Each entry is a snapshot of the
/// annotations before a change; documents have few annotations, so
/// snapshots are cheap and can't drift out of sync the way inverse
/// operations can.
#[derive(Debug, Default)]
pub struct History {
    undo: Vec<Vec<Annotation>>,
    redo: Vec<Vec<Annotation>>,
}

/// Changes remembered per document.
const HISTORY_LIMIT: usize = 200;

impl History {
    /// Call before changing `current`.
    pub fn record(&mut self, current: &[Annotation]) {
        self.undo.push(current.to_vec());
        if self.undo.len() > HISTORY_LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// The state to restore, or `None` if there is nothing to undo.
    pub fn undo(&mut self, current: &[Annotation]) -> Option<Vec<Annotation>> {
        let previous = self.undo.pop()?;
        self.redo.push(current.to_vec());
        Some(previous)
    }

    pub fn redo(&mut self, current: &[Annotation]) -> Option<Vec<Annotation>> {
        let next = self.redo.pop()?;
        self.undo.push(current.to_vec());
        Some(next)
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }
}

fn file_for(doc: &Path) -> Option<PathBuf> {
    let key = doc.to_string_lossy().to_lowercase();
    Some(
        data_dir()?
            .join("annotations")
            .join(format!("{:016x}.toml", fnv1a(key.as_bytes()))),
    )
}

/// FNV-1a: stable across Rust versions, unlike `DefaultHasher`.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &b| {
        (hash ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// `YYYY-MM-DD` (UTC) for Unix seconds.
pub fn date(unix: u64) -> String {
    // Howard Hinnant's civil-from-days algorithm.
    let days = (unix / 86_400) as i64 + 719_468;
    let era = days.div_euclid(146_097);
    let doe = days.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(y: f32) -> NormRect {
        NormRect {
            x: 0.1,
            y,
            width: 0.5,
            height: 0.02,
        }
    }

    #[test]
    fn dates() {
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(date(951_782_400), "2000-02-29");
        assert_eq!(date(1_794_009_600), "2026-11-07");
    }

    #[test]
    fn add_find_remove() {
        let mut a = Annotations::default();
        let first = a.add(
            2,
            Kind::Highlight,
            "green",
            "hello".into(),
            vec![rect(0.5)],
            0,
        );
        let second = a.add(
            2,
            Kind::Underline,
            "blue",
            "world".into(),
            vec![rect(0.5)],
            0,
        );
        assert_ne!(first, second);
        assert_eq!(a.at_point(2, 0.2, 0.51), Some(second));
        assert_eq!(a.at_point(2, 0.9, 0.51), None);
        assert_eq!(a.at_point(3, 0.2, 0.51), None);
        a.remove(second);
        assert_eq!(a.at_point(2, 0.2, 0.51), Some(first));
    }

    #[test]
    fn markdown_in_reading_order() {
        let mut a = Annotations::default();
        let late = a.add(
            4,
            Kind::Highlight,
            "yellow",
            "later".into(),
            vec![rect(0.1)],
            0,
        );
        a.add(
            1,
            Kind::Strikethrough,
            "red",
            "gone\n  text".into(),
            vec![rect(0.9)],
            0,
        );
        a.add(
            1,
            Kind::Highlight,
            "yellow",
            "first".into(),
            vec![rect(0.2)],
            0,
        );
        if let Some(item) = a.get_mut(late) {
            item.note = "My thought.".into();
        }
        if let Some(item) = a.get_mut(late) {
            item.line = 7;
        }
        let md = a.to_markdown("Book \"2\"", "book.pdf", 0);
        assert_eq!(
            md,
            "---\ntitle: \"Book \\\"2\\\"\"\nsource: \"[[book.pdf]]\"\nexported: 1970-01-01\n\
             annotations: 3\ntags:\n  - dpdf\n---\n\n# Book \"2\"\n\
             \n## Page 2\n\
             \n> [!quote|yellow] [[book.pdf#page=2|p. 2]]\n> first\n\n^dpdf-3\n\
             \n> [!quote|red] [[book.pdf#page=2|p. 2]]\n> ~~gone text~~\n\n^dpdf-2\n\
             \n## Page 5\n\
             \n> [!quote|yellow] [[book.pdf#page=5|p. 5, line 7]]\n> later\n\n^dpdf-1\n\
             \nMy thought.\n"
        );
    }

    #[test]
    fn toml_round_trip() {
        let mut a = Annotations {
            document: "C:\\a.pdf".into(),
            ..Default::default()
        };
        a.cycle_level(3, "Intro");
        a.add(
            0,
            Kind::Strikethrough,
            "red",
            "x".into(),
            vec![rect(0.3)],
            5,
        );
        let text = toml::to_string(&a).unwrap();
        let back: Annotations = toml::from_str(&text).unwrap();
        assert_eq!(back.items, a.items);
        assert_eq!(back.sections, a.sections);
    }

    #[test]
    fn section_levels_cycle() {
        let mut a = Annotations::default();
        assert_eq!(a.level(1, "A"), 0);
        assert_eq!(a.cycle_level(1, "A"), 1);
        assert_eq!(a.cycle_level(1, "A"), 2);
        assert_eq!(a.cycle_level(1, "A"), 3);
        assert_eq!(a.cycle_level(1, "A"), 0);
        assert!(a.sections.is_empty());
        a.cycle_level(1, "A");
        let outline = [(1, "A"), (1, "B"), (5, "C")];
        assert_eq!(a.progress(outline.into_iter()), (1, 3));
    }

    #[test]
    fn undo_and_redo() {
        let mut a = Annotations::default();
        let mut history = History::default();

        history.record(&a.items);
        let id = a.add(0, Kind::Highlight, "yellow", "x".into(), vec![rect(0.1)], 0);
        history.record(&a.items);
        if let Some(item) = a.get_mut(id) {
            item.color = "red".into();
        }

        a.items = history.undo(&a.items).unwrap();
        assert_eq!(a.items[0].color, "yellow");
        a.items = history.undo(&a.items).unwrap();
        assert!(a.items.is_empty());
        assert!(history.undo(&a.items).is_none());

        a.items = history.redo(&a.items).unwrap();
        a.items = history.redo(&a.items).unwrap();
        assert_eq!(a.items[0].color, "red");
        assert!(history.redo(&a.items).is_none());

        // A new change drops what could be redone.
        a.items = history.undo(&a.items).unwrap();
        history.record(&a.items);
        a.remove(id);
        assert!(history.redo(&a.items).is_none());
    }

    #[test]
    fn hash_is_stable() {
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
    }
}
