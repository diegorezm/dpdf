//! User configuration and themes, as TOML files in `%APPDATA%\dpdf\`:
//!
//! - `config.toml`: preferences and keybindings.
//! - `themes\*.toml`: extra or overriding themes, same format as the
//!   built-ins in the repository's `themes/` folder.
//!
//! Bad or missing files never block startup; defaults are used instead and
//! the problem is reported once.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum PageColors {
    #[default]
    Original,
    /// Inverted lightness, for reading at night.
    Inverted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum DefaultZoom {
    #[default]
    FitWidth,
    FitPage,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Theme name, matched case-insensitively.
    pub theme: String,
    pub page_colors: PageColors,
    pub default_zoom: DefaultZoom,
    /// Reopen the tabs from the last session on startup.
    pub restore_session: bool,
    pub sidebar: bool,
    /// Interface font family, e.g. "Segoe UI Variable Text" or "Inter".
    pub font: String,
    /// Interface font size in logical pixels.
    pub font_size: f32,
    /// Adds vim-style keys (j/k, gg/G, ctrl+d/u, /, n/N, :).
    pub vim: bool,
    /// Overrides: action name -> key sequences, e.g. `open = ["ctrl+o"]`.
    /// An empty list unbinds the action.
    pub keys: BTreeMap<String, Vec<String>>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: "Graphite".to_owned(),
            page_colors: PageColors::Original,
            default_zoom: DefaultZoom::FitWidth,
            restore_session: true,
            sidebar: true,
            font: "Segoe UI Variable Text".to_owned(),
            font_size: 14.0,
            vim: false,
            keys: BTreeMap::new(),
        }
    }
}

impl Config {
    /// Loads the config. A parse error yields defaults plus the message.
    pub fn load() -> (Self, Option<String>) {
        let Some(path) = data_dir().map(|d| d.join("config.toml")) else {
            return (Self::default(), None);
        };
        match fs::read_to_string(&path) {
            Ok(text) => match toml::from_str(&text) {
                Ok(config) => (config, None),
                Err(err) => (
                    Self::default(),
                    Some(format!("config.toml: {}", err.message())),
                ),
            },
            Err(_) => (Self::default(), None),
        }
    }

    pub fn save(&self) -> io::Result<()> {
        let path = data_dir()
            .ok_or_else(|| io::Error::other("APPDATA is not set"))?
            .join("config.toml");
        let body = toml::to_string_pretty(self).map_err(io::Error::other)?;
        write_atomic(&path, &format!("{CONFIG_HEADER}{body}"))
    }

    /// Writes the config if it doesn't exist yet, so users have a file to edit.
    pub fn ensure_file(&self) -> Option<PathBuf> {
        let path = data_dir()?.join("config.toml");
        if !path.exists() {
            self.save().ok()?;
        }
        Some(path)
    }
}

const CONFIG_HEADER: &str = "\
# dpdf configuration. Changes made in Settings are written here too.
# Reload with the \"Reload config\" command (Ctrl+K).
#
# Keybindings: [keys] maps an action to key sequences, e.g.
#   [keys]
#   open = [\"ctrl+o\"]
#   first-page = [\"home\", \"g g\"]
# Modifiers: ctrl, shift, alt. Separate sequence steps with a space.
# See Settings > Keybindings for every action name.

";

/// Colors for the whole interface. Hex strings, `#rrggbb` or `#rrggbbaa`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThemeDef {
    pub name: String,
    pub dark: bool,
    pub background: String,
    pub surface: String,
    pub raised: String,
    pub border: String,
    pub text: String,
    pub text_muted: String,
    pub accent: String,
    pub hover: String,
    pub selection: String,
    #[serde(rename = "match")]
    pub match_: String,
    pub match_current: String,
}

/// A user theme file: every color optional, filled from a built-in.
#[derive(Debug, Deserialize)]
struct PartialTheme {
    name: String,
    #[serde(default = "default_true")]
    dark: bool,
    background: Option<String>,
    surface: Option<String>,
    raised: Option<String>,
    border: Option<String>,
    text: Option<String>,
    text_muted: Option<String>,
    accent: Option<String>,
    hover: Option<String>,
    selection: Option<String>,
    #[serde(rename = "match")]
    match_: Option<String>,
    match_current: Option<String>,
}

fn default_true() -> bool {
    true
}

const BUILT_IN: [&str; 10] = [
    include_str!("../themes/graphite.toml"),
    include_str!("../themes/catppuccin-mocha.toml"),
    include_str!("../themes/gruvbox.toml"),
    include_str!("../themes/mono.toml"),
    include_str!("../themes/nord.toml"),
    include_str!("../themes/rose-pine.toml"),
    include_str!("../themes/tokyo-night.toml"),
    include_str!("../themes/catppuccin-latte.toml"),
    include_str!("../themes/paper.toml"),
    include_str!("../themes/sepia.toml"),
];

pub fn built_in_themes() -> Vec<ThemeDef> {
    // The built-ins are complete and tested, so a failure here is a bug in
    // the repository's theme files; skip rather than crash.
    BUILT_IN
        .iter()
        .filter_map(|text| toml::from_str(text).ok())
        .collect()
}

/// Built-in themes plus user themes; a user theme with the same name
/// replaces the built-in. Returns problems found in user files.
pub fn load_themes() -> (Vec<ThemeDef>, Vec<String>) {
    let mut themes = built_in_themes();
    let mut problems = Vec::new();
    let Some(dir) = themes_dir() else {
        return (themes, problems);
    };
    let Ok(entries) = fs::read_dir(&dir) else {
        return (themes, problems);
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|ext| ext == "toml"))
        .collect();
    paths.sort();
    for path in paths {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
        let parsed = fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|text| parse_user_theme(&text, &themes));
        match parsed {
            Ok(theme) => {
                themes.retain(|t| !t.name.eq_ignore_ascii_case(&theme.name));
                themes.push(theme);
            }
            Err(err) => problems.push(format!("{}: {err}", name.unwrap_or_default())),
        }
    }
    (themes, problems)
}

fn parse_user_theme(text: &str, built_in: &[ThemeDef]) -> Result<ThemeDef, String> {
    let p: PartialTheme = toml::from_str(text).map_err(|e| e.message().to_owned())?;
    let base = built_in
        .iter()
        .find(|t| t.dark == p.dark)
        .cloned()
        .ok_or("no built-in theme to fall back on")?;
    let theme = ThemeDef {
        name: p.name,
        dark: p.dark,
        background: p.background.unwrap_or(base.background),
        surface: p.surface.unwrap_or(base.surface),
        raised: p.raised.unwrap_or(base.raised),
        border: p.border.unwrap_or(base.border),
        text: p.text.unwrap_or(base.text),
        text_muted: p.text_muted.unwrap_or(base.text_muted),
        accent: p.accent.unwrap_or(base.accent),
        hover: p.hover.unwrap_or(base.hover),
        selection: p.selection.unwrap_or(base.selection),
        match_: p.match_.unwrap_or(base.match_),
        match_current: p.match_current.unwrap_or(base.match_current),
    };
    for color in theme.colors() {
        parse_color(color).ok_or_else(|| format!("invalid color {color:?}"))?;
    }
    Ok(theme)
}

impl ThemeDef {
    pub fn colors(&self) -> [&str; 11] {
        [
            &self.background,
            &self.surface,
            &self.raised,
            &self.border,
            &self.text,
            &self.text_muted,
            &self.accent,
            &self.hover,
            &self.selection,
            &self.match_,
            &self.match_current,
        ]
    }
}

/// Parses `#rgb`, `#rrggbb`, or `#rrggbbaa` into RGBA.
pub fn parse_color(text: &str) -> Option<[u8; 4]> {
    let hex = text.trim().strip_prefix('#')?;
    let byte = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
    match hex.len() {
        3 => {
            let nib = |i: usize| {
                u8::from_str_radix(hex.get(i..i + 1)?, 16)
                    .ok()
                    .map(|v| v * 17)
            };
            Some([nib(0)?, nib(1)?, nib(2)?, 255])
        }
        6 => Some([byte(0)?, byte(2)?, byte(4)?, 255]),
        8 => Some([byte(0)?, byte(2)?, byte(4)?, byte(6)?]),
        _ => None,
    }
}

pub fn data_dir() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("APPDATA")?).join("dpdf"))
}

pub fn themes_dir() -> Option<PathBuf> {
    Some(data_dir()?.join("themes"))
}

/// Write then rename, so a crash never leaves a half-written file.
pub fn write_atomic(path: &std::path::Path, contents: &str) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, contents)?;
    fs::rename(tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_ins_are_complete_and_valid() {
        let themes = built_in_themes();
        assert_eq!(themes.len(), BUILT_IN.len());
        for theme in &themes {
            for color in theme.colors() {
                assert!(parse_color(color).is_some(), "{}: {color}", theme.name);
            }
        }
    }

    #[test]
    fn user_theme_fills_gaps_from_built_in() {
        let base = built_in_themes();
        let theme =
            parse_user_theme("name = \"Mine\"\ndark = false\naccent = \"#ff0000\"", &base).unwrap();
        assert_eq!(theme.accent, "#ff0000");
        let paper = base.iter().find(|t| !t.dark).unwrap();
        assert_eq!(theme.background, paper.background);
    }

    #[test]
    fn user_theme_rejects_bad_colors() {
        let base = built_in_themes();
        assert!(parse_user_theme("name = \"X\"\ntext = \"red\"", &base).is_err());
        assert!(parse_user_theme("dark = true", &base).is_err());
    }

    #[test]
    fn colors() {
        assert_eq!(parse_color("#fff"), Some([255, 255, 255, 255]));
        assert_eq!(parse_color("#102030"), Some([16, 32, 48, 255]));
        assert_eq!(parse_color("#10203040"), Some([16, 32, 48, 64]));
        assert_eq!(parse_color("102030"), None);
        assert_eq!(parse_color("#12345"), None);
    }

    #[test]
    fn config_round_trip_and_defaults() {
        let mut config = Config {
            vim: true,
            page_colors: PageColors::Inverted,
            ..Config::default()
        };
        config.keys.insert("open".into(), vec!["ctrl+o".into()]);
        let text = toml::to_string_pretty(&config).unwrap();
        assert_eq!(toml::from_str::<Config>(&text).unwrap(), config);

        let partial: Config = toml::from_str("vim = true").unwrap();
        assert!(partial.vim);
        assert_eq!(partial.theme, "Graphite");
    }
}
