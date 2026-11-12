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

/// How much room the interface takes: spacing and header height.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Density {
    Compact,
    #[default]
    Comfortable,
    Spacious,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Corners {
    Sharp,
    #[default]
    Rounded,
    Round,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ThumbnailSize {
    Small,
    #[default]
    Medium,
    Large,
}

/// Interface fonts offered in Settings. Any installed font also works
/// when typed into config.toml.
pub const FONTS: [&str; 10] = [
    "Segoe UI Variable Text",
    "Segoe UI",
    "Bahnschrift",
    "Calibri",
    "Arial",
    "Verdana",
    "Tahoma",
    "Georgia",
    "Cascadia Code",
    "Consolas",
];

pub const MIN_FONT_SIZE: f32 = 10.0;
pub const MAX_FONT_SIZE: f32 = 22.0;

/// Pixel values for the layout choices.
pub struct UiMetrics {
    pub spacing: f32,
    pub header_height: f32,
    pub radius: f32,
    pub sidebar_width: f32,
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
    /// Header button that reopens the closed sidebar.
    pub sidebar_button: bool,
    /// Ask GitHub for a newer release on startup.
    pub check_updates: bool,
    /// Interface language: "pt-BR" or "en".
    pub language: String,
    /// Interface font family, e.g. "Segoe UI Variable Text" or "Inter".
    pub font: String,
    /// Interface font size in logical pixels.
    pub font_size: f32,
    pub density: Density,
    pub corners: Corners,
    pub thumbnail_size: ThumbnailSize,
    /// Adds vim-style keys (j/k, gg/G, ctrl+d/u, /, n/N, :).
    pub vim: bool,
    /// Folder that "Export notes" writes into without asking, e.g. a
    /// folder in an Obsidian vault. Empty asks where to save each time.
    pub notes_folder: String,
    /// Shows study time in the header and records it per day.
    pub study_timer: bool,
    /// What each highlight color means, e.g. `red = "Trap"`. Shown in the
    /// notes filter and used in exports.
    pub color_names: BTreeMap<String, String>,
    /// Replaces a highlight color's shade, e.g. `yellow = "#ffd23f"`.
    /// Highlights keep their color key, so changing a shade recolors them.
    pub highlight_colors: BTreeMap<String, String>,
    /// Overrides: action name -> key sequences, e.g. `open = ["ctrl+o"]`.
    /// An empty list unbinds the action.
    pub keys: BTreeMap<String, Vec<String>>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: "Noite".to_owned(),
            language: "pt-BR".to_owned(),
            page_colors: PageColors::Original,
            default_zoom: DefaultZoom::FitWidth,
            restore_session: true,
            sidebar: true,
            sidebar_button: true,
            check_updates: true,
            font: "Segoe UI Variable Text".to_owned(),
            font_size: 14.0,
            density: Density::Comfortable,
            corners: Corners::Rounded,
            thumbnail_size: ThumbnailSize::Medium,
            vim: false,
            notes_folder: String::new(),
            study_timer: true,
            color_names: BTreeMap::new(),
            highlight_colors: BTreeMap::new(),
            keys: BTreeMap::new(),
        }
    }
}

/// Meanings that fit studying for exams, in English; shown translated
/// unless renamed.
const DEFAULT_COLOR_NAMES: [(&str, &str); 5] = [
    ("yellow", "Important"),
    ("green", "Case law"),
    ("blue", "Later"),
    ("pink", "Definition"),
    ("red", "Trap"),
];

impl Config {
    /// The name the user gave a highlight color, or the color itself.
    pub fn color_name(&self, color: &str) -> String {
        match self.color_names.get(color) {
            Some(name) if !name.trim().is_empty() => name.trim().to_owned(),
            _ => DEFAULT_COLOR_NAMES
                .iter()
                .find(|(c, _)| *c == color)
                .map_or_else(|| color.to_owned(), |(_, name)| crate::i18n::tr(name)),
        }
    }

    /// The shade of a highlight color: the user's, if valid, or the default.
    pub fn color_hex(&self, color: &str) -> String {
        match self.highlight_colors.get(color) {
            Some(hex) if parse_color(hex).is_some() => hex.trim().to_owned(),
            _ => crate::annotations::color_hex(color).to_owned(),
        }
    }

    pub fn font_size(&self) -> f32 {
        if self.font_size.is_finite() {
            self.font_size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE)
        } else {
            14.0
        }
    }

    pub fn metrics(&self) -> UiMetrics {
        let (spacing, header_height) = match self.density {
            Density::Compact => (9.0, 34.0),
            Density::Comfortable => (12.0, 40.0),
            Density::Spacious => (16.0, 48.0),
        };
        UiMetrics {
            spacing,
            // The header grows with the text so large fonts still fit.
            header_height: header_height + (self.font_size() - 14.0).max(0.0) * 2.0,
            radius: match self.corners {
                Corners::Sharp => 0.0,
                Corners::Rounded => 6.0,
                Corners::Round => 11.0,
            },
            sidebar_width: match self.thumbnail_size {
                ThumbnailSize::Small => 140.0,
                ThumbnailSize::Medium => 180.0,
                ThumbnailSize::Large => 240.0,
            },
        }
    }

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
#
# Highlight colors: [color_names] names what each color means, e.g.
#   [color_names]
#   red = \"Trap\"
# and [highlight_colors] their shades, e.g. red = \"#ef6b5f\".

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

const BUILT_IN: [&str; 12] = [
    include_str!("../themes/noite.toml"),
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
    include_str!("../themes/velino.toml"),
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
    fn metrics_and_font_size() {
        let mut config = Config {
            density: Density::Compact,
            corners: Corners::Sharp,
            thumbnail_size: ThumbnailSize::Large,
            font_size: 99.0,
            ..Config::default()
        };
        assert_eq!(config.font_size(), MAX_FONT_SIZE);
        let m = config.metrics();
        assert_eq!((m.spacing, m.radius, m.sidebar_width), (9.0, 0.0, 240.0));
        assert!(m.header_height > 34.0);
        config.font_size = f32::NAN;
        assert_eq!(config.font_size(), 14.0);
        let parsed: Config = toml::from_str("density = \"spacious\"\ncorners = \"round\"").unwrap();
        assert_eq!(parsed.density, Density::Spacious);
        assert_eq!(parsed.corners, Corners::Round);
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
        assert_eq!(partial.theme, "Noite");
        assert!(partial.study_timer);
    }

    #[test]
    fn color_names_fall_back_to_defaults() {
        let config: Config = toml::from_str(
            "[color_names]
red = \"Pegadinha\"
green = \" \"",
        )
        .unwrap();
        assert_eq!(config.color_name("red"), "Pegadinha");
        assert_eq!(config.color_name("green"), "Case law");
        assert_eq!(config.color_name("yellow"), "Important");
        assert_eq!(config.color_name("teal"), "teal");

        let shades: Config =
            toml::from_str("[highlight_colors]\nred = \"#112233\"\nblue = \"nope\"").unwrap();
        assert_eq!(shades.color_hex("red"), "#112233");
        assert_eq!(
            shades.color_hex("blue"),
            crate::annotations::color_hex("blue")
        );
    }
}
