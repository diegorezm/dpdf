//! Notes exported for other tools: Markdown for notes apps, a TSV file
//! Anki imports as cards, and CSV for anything else. Leme only writes the
//! file; studying the cards happens in the app that reads it.

use std::fmt::Write as _;

use crate::annotations::{Annotation, Annotations, Kind, COLORS};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Markdown,
    Anki,
    Csv,
}

impl Format {
    pub const ALL: [Format; 3] = [Format::Markdown, Format::Anki, Format::Csv];

    pub fn name(self) -> &'static str {
        match self {
            Format::Markdown => "markdown",
            Format::Anki => "anki",
            Format::Csv => "csv",
        }
    }

    pub fn from_name(name: &str) -> Option<Format> {
        Format::ALL.into_iter().find(|f| f.name() == name)
    }

    pub fn extension(self) -> &'static str {
        match self {
            Format::Markdown => "md",
            Format::Anki => "txt",
            Format::Csv => "csv",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Format::Markdown => "Markdown",
            Format::Anki => "Anki",
            Format::Csv => "CSV",
        }
    }
}

pub struct Options<'a> {
    pub title: &'a str,
    /// The PDF's file name, for Markdown links.
    pub file: &'a str,
    pub exported: u64,
    /// Colors to include.
    pub colors: &'a [String],
    /// The name of each color, in `COLORS` order.
    pub names: &'a [String],
    /// Outline sections with a page, in outline order.
    pub sections: &'a [(usize, String)],
    /// Group Markdown by outline section and color instead of by page.
    pub by_section: bool,
}

impl Options<'_> {
    fn color_name<'s>(&'s self, color: &'s str) -> &'s str {
        crate::annotations::color_index(color)
            .and_then(|i| self.names.get(i))
            .map_or(color, String::as_str)
    }

    /// The section an annotation falls in: the last one starting at or
    /// before its page.
    fn section_of(&self, page: usize) -> Option<&str> {
        self.sections
            .iter()
            .filter(|(start, _)| *start <= page)
            .max_by_key(|(start, _)| *start)
            .map(|(_, title)| title.as_str())
    }
}

/// The annotations to export, in reading order. Empty if none match.
pub fn selected<'a>(annotations: &'a Annotations, opts: &Options) -> Vec<&'a Annotation> {
    let mut items: Vec<&Annotation> = annotations
        .items
        .iter()
        .filter(|a| opts.colors.contains(&a.color))
        .collect();
    items.sort_by(|a, b| {
        let top = |x: &Annotation| x.rects.first().map_or(0.0, |r| r.y);
        a.page.cmp(&b.page).then(top(a).total_cmp(&top(b)))
    });
    items
}

pub fn export(annotations: &Annotations, format: Format, opts: &Options) -> String {
    let items = selected(annotations, opts);
    match format {
        Format::Markdown if opts.by_section && !opts.sections.is_empty() => {
            markdown_by_section(&items, opts)
        }
        Format::Markdown => {
            let only = Annotations {
                items: items.into_iter().cloned().collect(),
                ..Default::default()
            };
            only.to_markdown(opts.title, opts.file, opts.exported)
        }
        Format::Anki => anki(&items, opts),
        Format::Csv => csv(&items, opts),
    }
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A study summary: each section, then each color in it, then the quotes.
fn markdown_by_section(items: &[&Annotation], opts: &Options) -> String {
    let mut out = format!("# {}\n", opts.title);
    let mut groups: Vec<(Option<&str>, Vec<&Annotation>)> = Vec::new();
    for a in items {
        let section = opts.section_of(a.page);
        match groups.last_mut() {
            Some((s, list)) if *s == section => list.push(a),
            _ => groups.push((section, vec![a])),
        }
    }
    for (section, list) in groups {
        let _ = write!(
            out,
            "\n## {}\n",
            section.map_or_else(
                || crate::i18n::tr("Before the first section"),
                str::to_owned
            )
        );
        for (color, _) in COLORS {
            let in_color: Vec<_> = list.iter().filter(|a| a.color == color).collect();
            if in_color.is_empty() {
                continue;
            }
            let _ = write!(out, "\n### {}\n\n", opts.color_name(color));
            for a in in_color {
                let text = one_line(&a.text);
                let text = match a.kind {
                    Kind::Strikethrough => format!("~~{text}~~"),
                    Kind::Highlight | Kind::Underline => text,
                };
                let _ = writeln!(out, "- \"{text}\" (p. {})", a.page + 1);
                for line in a.note.trim().lines().filter(|l| !l.trim().is_empty()) {
                    let _ = writeln!(out, "  > {}", line.trim());
                }
            }
        }
    }
    out
}

/// Anki's text import: the note (or where the quote is from) on the
/// front, the quote on the back, and the color and document as tags. The
/// header lines tell Anki the separator and which column holds the tags.
fn anki(items: &[&Annotation], opts: &Options) -> String {
    let mut out = String::from("#separator:tab\n#html:true\n#tags column:3\n");
    let doc_tag = tag(opts.title);
    for a in items {
        let place = match opts.section_of(a.page) {
            Some(section) => format!("{} · {section} · p. {}", opts.title, a.page + 1),
            None => format!("{} · p. {}", opts.title, a.page + 1),
        };
        let front = if a.note.trim().is_empty() {
            html(&place)
        } else {
            format!("{}<br><small>{}</small>", html(a.note.trim()), html(&place))
        };
        let tags = format!("leme {doc_tag} {}", tag(opts.color_name(&a.color)));
        let _ = writeln!(out, "{front}\t{}\t{tags}", html(&one_line(&a.text)));
    }
    out
}

fn csv(items: &[&Annotation], opts: &Options) -> String {
    let mut out = String::from("page,section,color,kind,text,note\n");
    for a in items {
        let kind = match a.kind {
            Kind::Highlight => "highlight",
            Kind::Underline => "underline",
            Kind::Strikethrough => "strikethrough",
        };
        let fields = [
            (a.page + 1).to_string(),
            opts.section_of(a.page).unwrap_or_default().to_owned(),
            opts.color_name(&a.color).to_owned(),
            kind.to_owned(),
            one_line(&a.text),
            a.note.trim().to_owned(),
        ];
        let row: Vec<String> = fields.iter().map(|f| csv_field(f)).collect();
        let _ = writeln!(out, "{}", row.join(","));
    }
    out
}

fn csv_field(text: &str) -> String {
    if text.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_owned()
    }
}

/// Text for an HTML field: escaped, line breaks as `<br>`, no tabs.
fn html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\t', " ")
        .replace("\r\n", "<br>")
        .replace('\n', "<br>")
}

/// An Anki tag: no spaces, which separate tags.
fn tag(text: &str) -> String {
    let tag: String = text
        .trim()
        .chars()
        .map(|c| if c.is_whitespace() { '_' } else { c })
        .collect();
    if tag.is_empty() {
        "untitled".to_owned()
    } else {
        tag
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdf::NormRect;

    fn sample() -> Annotations {
        let rect = |y| NormRect {
            x: 0.1,
            y,
            width: 0.5,
            height: 0.02,
        };
        let mut a = Annotations::default();
        a.add(
            11,
            Kind::Highlight,
            "red",
            "aos estrangeiros,\n na forma da lei".into(),
            vec![rect(0.5)],
            0,
        );
        a.add(
            11,
            Kind::Highlight,
            "yellow",
            "até dois anos".into(),
            vec![rect(0.7)],
            0,
        );
        a.add(
            2,
            Kind::Highlight,
            "blue",
            "ignored".into(),
            vec![rect(0.1)],
            0,
        );
        a.add(
            30,
            Kind::Highlight,
            "red",
            "ex tunc".into(),
            vec![rect(0.1)],
            0,
        );
        if let Some(item) = a.get_mut(1) {
            item.note = "banca troca <isso>".into();
        }
        a
    }

    fn options<'a>(
        colors: &'a [String],
        names: &'a [String],
        sections: &'a [(usize, String)],
    ) -> Options<'a> {
        Options {
            title: "Aula 03",
            file: "aula.pdf",
            exported: 0,
            colors,
            names,
            sections,
            by_section: true,
        }
    }

    fn names() -> Vec<String> {
        ["Important", "Case law", "Later", "Definition", "Trap"]
            .map(String::from)
            .to_vec()
    }

    #[test]
    fn markdown_groups_by_section_and_color() {
        let colors = vec!["red".to_owned(), "yellow".to_owned()];
        let names = names();
        let sections = vec![(10, "Princípios".to_owned()), (25, "Atos".to_owned())];
        let md = export(
            &sample(),
            Format::Markdown,
            &options(&colors, &names, &sections),
        );
        assert_eq!(
            md,
            "# Aula 03\n\
             \n## Princípios\n\
             \n### Important\n\n- \"até dois anos\" (p. 12)\n\
             \n### Trap\n\n- \"aos estrangeiros, na forma da lei\" (p. 12)\n  > banca troca <isso>\n\
             \n## Atos\n\
             \n### Trap\n\n- \"ex tunc\" (p. 31)\n"
        );
    }

    #[test]
    fn anki_cards() {
        let colors = vec!["red".to_owned()];
        let names = names();
        let sections = vec![(10, "Princípios".to_owned())];
        let tsv = export(
            &sample(),
            Format::Anki,
            &options(&colors, &names, &sections),
        );
        let lines: Vec<&str> = tsv.lines().collect();
        assert_eq!(
            lines[..3],
            ["#separator:tab", "#html:true", "#tags column:3"]
        );
        assert_eq!(
            lines[3],
            "banca troca &lt;isso&gt;<br><small>Aula 03 · Princípios · p. 12</small>\taos estrangeiros, na forma da lei\tleme Aula_03 Trap"
        );
        assert_eq!(
            lines[4],
            "Aula 03 · Princípios · p. 31\tex tunc\tleme Aula_03 Trap"
        );
        assert_eq!(lines.len(), 5);
    }

    #[test]
    fn csv_quotes_fields() {
        let colors = vec!["red".to_owned()];
        let names = names();
        let csv = export(&sample(), Format::Csv, &options(&colors, &names, &[]));
        assert_eq!(
            csv,
            "page,section,color,kind,text,note\n\
             12,,Trap,highlight,\"aos estrangeiros, na forma da lei\",banca troca <isso>\n\
             31,,Trap,highlight,ex tunc,\n"
        );
    }

    #[test]
    fn formats_by_name() {
        for f in Format::ALL {
            assert_eq!(Format::from_name(f.name()), Some(f));
        }
        assert_eq!(Format::from_name("pdf"), None);
    }
}
