//! Interface language. English is the source language; Brazilian
//! Portuguese comes from `translations/pt-BR/LC_MESSAGES/leme.po`, the same
//! file Slint bundles for the `.slint` markup, so each text is translated
//! once for both.
//!
//! Text built in Rust goes through `tr` (or `trf` for text with `{}`
//! placeholders, like Slint's `@tr`). Settings sends back the translated
//! labels of its choices; `untr` turns them into the English ones the
//! config code matches on.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

const PT_BR_PO: &str = include_str!("../translations/pt-BR/LC_MESSAGES/leme.po");

/// Languages offered in Settings: config value and the name shown.
pub const LANGUAGES: [(&str, &str); 2] = [("pt-BR", "Português (Brasil)"), ("en", "English")];

static PORTUGUESE: AtomicBool = AtomicBool::new(false);

/// Picks the language for `tr` and for the Slint markup. Unknown codes
/// mean English.
pub fn set_language(code: &str) {
    let portuguese = is_portuguese(code);
    PORTUGUESE.store(portuguese, Ordering::Relaxed);
    // Fails only before a window exists, or without bundled translations;
    // the interface then stays in English, which is still usable.
    let _ = slint::select_bundled_translation(if portuguese { "pt-BR" } else { "en" });
}

fn is_portuguese(code: &str) -> bool {
    code.trim().to_ascii_lowercase().starts_with("pt")
}

fn table() -> &'static HashMap<String, String> {
    static TABLE: OnceLock<HashMap<String, String>> = OnceLock::new();
    TABLE.get_or_init(|| parse_po(PT_BR_PO))
}

/// `text` in the current language.
pub fn tr(text: &str) -> String {
    if !PORTUGUESE.load(Ordering::Relaxed) {
        return text.to_owned();
    }
    table()
        .get(text)
        .filter(|t| !t.is_empty())
        .cloned()
        .unwrap_or_else(|| text.to_owned())
}

/// `tr` with each `{}` replaced by the next argument.
pub fn trf(text: &str, args: &[&dyn std::fmt::Display]) -> String {
    fill(&tr(text), args)
}

fn fill(template: &str, args: &[&dyn std::fmt::Display]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut args = args.iter();
    let mut rest = template;
    while let Some(i) = rest.find("{}") {
        out.push_str(&rest[..i]);
        match args.next() {
            Some(arg) => out.push_str(&arg.to_string()),
            None => out.push_str("{}"),
        }
        rest = &rest[i + 2..];
    }
    out.push_str(rest);
    out
}

/// The English text whose translation is `text`, or `text` itself.
pub fn untr(text: &str) -> String {
    table()
        .iter()
        .find(|(_, translated)| *translated == text)
        .map_or_else(|| text.to_owned(), |(english, _)| english.clone())
}

/// Reads `msgid` / `msgstr` pairs, including strings continued on
/// following lines. Plural forms and contexts aren't used by leme.
fn parse_po(text: &str) -> HashMap<String, String> {
    #[derive(PartialEq)]
    enum Field {
        None,
        Id,
        Str,
    }
    let mut table = HashMap::new();
    let (mut id, mut value, mut field) = (String::new(), String::new(), Field::None);
    let mut finish = |id: &mut String, value: &mut String| {
        if !id.is_empty() && !value.is_empty() {
            table.insert(std::mem::take(id), std::mem::take(value));
        }
        id.clear();
        value.clear();
    };
    for line in text.lines().map(str::trim) {
        if let Some(rest) = line.strip_prefix("msgid ") {
            finish(&mut id, &mut value);
            id = unquote(rest);
            field = Field::Id;
        } else if let Some(rest) = line.strip_prefix("msgstr ") {
            value = unquote(rest);
            field = Field::Str;
        } else if line.starts_with('"') {
            match field {
                Field::Id => id.push_str(&unquote(line)),
                Field::Str => value.push_str(&unquote(line)),
                Field::None => {}
            }
        } else if line.is_empty() || line.starts_with('#') {
            field = Field::None;
        }
    }
    finish(&mut id, &mut value);
    table
}

fn unquote(text: &str) -> String {
    let inner = text
        .trim()
        .strip_prefix('"')
        .and_then(|t| t.strip_suffix('"'))
        .unwrap_or_default();
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_po() {
        let po = "# comment\nmsgid \"\"\nmsgstr \"\"\n\"Content-Type: text/plain\\n\"\n\n\
                  msgid \"Open\"\nmsgstr \"Abrir\"\n\n\
                  msgid \"Say \\\"hi\\\"\"\nmsgstr \"\"\n\"Diga \"\n\"\\\"oi\\\"\"\n\n\
                  msgid \"Untranslated\"\nmsgstr \"\"\n";
        let table = parse_po(po);
        assert_eq!(table.get("Open").map(String::as_str), Some("Abrir"));
        assert_eq!(
            table.get("Say \"hi\"").map(String::as_str),
            Some("Diga \"oi\"")
        );
        assert!(!table.contains_key("Untranslated"));
        assert!(!table.contains_key(""));
    }

    #[test]
    fn fills_placeholders() {
        assert_eq!(fill("{} of {}", &[&3, &"9"]), "3 of 9");
        assert_eq!(fill("{} and {}", &[&1]), "1 and {}");
        assert_eq!(fill("none", &[&1]), "none");
    }

    #[test]
    fn bundled_translation_is_complete() {
        // Every entry has a translation and keeps its placeholders.
        let table = table();
        assert!(table.len() > 100);
        for (english, portuguese) in table {
            assert_eq!(
                english.matches("{}").count(),
                portuguese.matches("{}").count(),
                "{english}"
            );
        }
        assert_eq!(table.get("Open").map(String::as_str), Some("Abrir"));
    }

    #[test]
    fn language_codes() {
        assert!(is_portuguese("pt-BR"));
        assert!(is_portuguese("PT"));
        assert!(!is_portuguese("en"));
        assert!(!is_portuguese(""));
    }
}
