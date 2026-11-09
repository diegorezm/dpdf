//! Application state above the open document: tabs and the session,
//! keybindings, the command palette, settings, themes, and overlays.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;

use slint::{ComponentHandle, Model, SharedString, VecModel};

use crate::config::{
    self, built_in_themes, load_themes, Config, DefaultZoom, PageColors, ThemeDef,
};
use crate::keymap::{Action, Chord, KeyResult, Keymap};
use crate::palette::{self, Entry, Target};
use crate::pdf::{Event, Worker};
use crate::store::Store;
use crate::viewer::{file_name, to_color, ui_int, PopupOutcome, Viewer};
use crate::{AppWindow, KeyRow, PaletteItem, SettingsWindow, TabItem, Theme};

const PALETTE_LIMIT: usize = 12;

pub struct App {
    viewer: Viewer,
    settings: SettingsWindow,
    config: Config,
    themes: Vec<ThemeDef>,
    keymap: Keymap,
    /// Problems with config.toml, themes, or keybindings, shown in Settings.
    problems: Vec<String>,

    tabs: Vec<PathBuf>,
    active: Option<usize>,
    tab_items: Rc<VecModel<TabItem>>,

    palette_entries: Vec<Entry>,
    palette_shown: Vec<Entry>,
    palette_selected: usize,
    palette_items: Rc<VecModel<PaletteItem>>,
}

impl App {
    pub fn new(window: &AppWindow, settings: SettingsWindow, worker: Worker) -> Self {
        let store = Store::load();
        let tab_items = Rc::new(VecModel::default());
        let palette_items = Rc::new(VecModel::default());
        window.set_tabs(tab_items.clone().into());
        window.set_palette_items(palette_items.clone().into());
        settings.set_version(env!("CARGO_PKG_VERSION").into());
        settings.set_data_dir(
            config::data_dir()
                .map(|d| d.display().to_string())
                .unwrap_or_default()
                .into(),
        );

        let mut app = Self {
            viewer: Viewer::new(window, worker, store),
            settings,
            config: Config::default(),
            themes: built_in_themes(),
            keymap: Keymap::default(),
            problems: Vec::new(),
            tabs: Vec::new(),
            active: None,
            tab_items,
            palette_entries: Vec::new(),
            palette_shown: Vec::new(),
            palette_selected: 0,
            palette_items,
        };
        app.reload_config(window);
        window.set_sidebar_visible(app.config.sidebar);
        window.set_session_tabs(ui_int(app.viewer.store.session.len()));
        if app.config.restore_session {
            app.restore_session(window);
        }
        app
    }

    // ----- Config and themes ----------------------------------------------

    pub fn reload_config(&mut self, window: &AppWindow) {
        let (config, problem) = Config::load();
        let (themes, theme_problems) = load_themes();
        self.config = config;
        self.themes = themes;
        self.problems = problem.into_iter().chain(theme_problems).collect();
        self.apply_config(window);
    }

    fn apply_config(&mut self, window: &AppWindow) {
        let (keymap, key_problems) = Keymap::new(self.config.vim, &self.config.keys);
        self.keymap = keymap;
        let mut problems = self.problems.clone();
        problems.extend(key_problems);

        let theme = self
            .themes
            .iter()
            .find(|t| t.name.eq_ignore_ascii_case(&self.config.theme))
            .or(self.themes.first())
            .cloned();
        if let Some(theme) = &theme {
            apply_theme(&window.global::<Theme>(), theme);
            apply_theme(&self.settings.global::<Theme>(), theme);
        } else {
            problems.push(format!("Theme \"{}\" not found", self.config.theme));
        }

        let names: Vec<SharedString> = self.themes.iter().map(|t| t.name.as_str().into()).collect();
        self.settings
            .set_theme_names(Rc::new(VecModel::from(names)).into());
        self.settings
            .set_theme_name(theme.map(|t| t.name).unwrap_or_default().into());
        self.settings.set_page_colors(
            match self.config.page_colors {
                PageColors::Original => "Original",
                PageColors::Inverted => "Inverted",
            }
            .into(),
        );
        self.settings.set_default_zoom(
            match self.config.default_zoom {
                DefaultZoom::FitWidth => "Fit width",
                DefaultZoom::FitPage => "Fit page",
            }
            .into(),
        );
        self.settings
            .set_restore_session(self.config.restore_session);
        self.settings.set_sidebar_default(self.config.sidebar);
        self.settings.set_vim(self.config.vim);
        self.settings.set_problems(problems.join("\n").into());

        let rows: Vec<KeyRow> = Action::ALL
            .iter()
            .map(|a| KeyRow {
                title: a.title().into(),
                name: a.name().into(),
                keys: self.keymap.keys_for(*a).join(",  ").into(),
            })
            .collect();
        self.settings
            .set_key_rows(Rc::new(VecModel::from(rows)).into());

        self.viewer.set_default_zoom(self.config.default_zoom);
        self.viewer
            .set_inverted(window, self.config.page_colors == PageColors::Inverted);
    }

    fn save_config(&self, window: &AppWindow) {
        if let Err(err) = self.config.save() {
            window.set_status(format!("Couldn't save config: {err}").into());
        }
    }

    /// A change from the settings screen.
    pub fn setting(&mut self, window: &AppWindow, key: &str, value: &str) {
        let on = value == "true";
        match key {
            "theme" => self.config.theme = value.to_owned(),
            "page-colors" => {
                self.config.page_colors = if value == "Inverted" {
                    PageColors::Inverted
                } else {
                    PageColors::Original
                }
            }
            "default-zoom" => {
                self.config.default_zoom = if value == "Fit page" {
                    DefaultZoom::FitPage
                } else {
                    DefaultZoom::FitWidth
                }
            }
            "restore-session" => self.config.restore_session = on,
            "sidebar" => {
                self.config.sidebar = on;
                window.set_sidebar_visible(on);
                self.viewer.sidebar_shown(window);
            }
            "vim" => self.config.vim = on,
            "reset" => {
                self.config = Config::default();
                window.set_sidebar_visible(self.config.sidebar);
            }
            "reload" => return self.reload_config(window),
            "open-config" => return self.run(window, Action::OpenConfig),
            "open-themes" => return self.run(window, Action::OpenThemesFolder),
            "open-data-dir" => {
                if let Some(dir) = config::data_dir() {
                    let _ = std::fs::create_dir_all(&dir);
                    reveal(&dir);
                }
                return;
            }
            "restore-session-now" => {
                self.set_overlay(window, "");
                return self.restore_session(window);
            }
            _ => return,
        }
        self.save_config(window);
        self.apply_config(window);
    }

    // ----- Keys ------------------------------------------------------------

    pub fn key_pressed(
        &mut self,
        window: &AppWindow,
        text: &str,
        ctrl: bool,
        shift: bool,
        alt: bool,
    ) -> bool {
        let Some(chord) = Chord::from_event(text, ctrl, shift, alt) else {
            return false;
        };
        match self.keymap.press(chord) {
            KeyResult::Action(action) => {
                self.run(window, action);
                true
            }
            KeyResult::Pending => true,
            KeyResult::Unbound => false,
        }
    }

    pub fn run(&mut self, window: &AppWindow, action: Action) {
        match action {
            Action::Open => self.open_dialog(window),
            Action::CommandPalette => self.open_palette(window),
            Action::Search if self.viewer.page_count() > 0 => self.set_overlay(window, "search"),
            Action::GoToPage if self.viewer.page_count() > 0 => {
                window.set_goto_text(ui_int(self.viewer.current_page() + 1).to_string().into());
                self.set_overlay(window, "goto");
            }
            Action::Settings => self.toggle_settings(),
            Action::ToggleSidebar => {
                window.set_sidebar_visible(!window.get_sidebar_visible());
                self.viewer.sidebar_shown(window);
            }
            Action::FocusMode => window.set_focus_mode(!window.get_focus_mode()),
            Action::Fullscreen => {
                let fullscreen = !window.get_fullscreen();
                window.set_fullscreen(fullscreen);
                window.window().set_fullscreen(fullscreen);
            }
            Action::NextTab | Action::PreviousTab => {
                let (Some(active), len) = (self.active, self.tabs.len()) else {
                    return;
                };
                let next = if action == Action::NextTab {
                    (active + 1) % len
                } else {
                    (active + len - 1) % len
                };
                self.activate(window, next);
            }
            Action::CloseTab => {
                if let Some(active) = self.active {
                    self.close_tab(window, active);
                }
            }
            Action::RestoreSession => self.restore_session(window),
            Action::Copy | Action::Highlight | Action::AddNote => {
                let outcome = self.viewer.selection_action(window, action);
                self.popup_outcome(window, outcome);
            }
            Action::ExportNotes => self.export_notes(window),
            Action::ReloadConfig => {
                self.reload_config(window);
                window.set_status("Config reloaded".into());
            }
            Action::OpenConfig => match self.config.ensure_file() {
                Some(path) => reveal(&path),
                None => window.set_status("Couldn't create config.toml".into()),
            },
            Action::OpenThemesFolder => {
                if let Some(dir) = config::themes_dir() {
                    ensure_theme_template(&dir);
                    reveal(&dir);
                }
            }
            Action::Escape => self.escape(window),
            _ => {
                self.viewer.handle(window, action);
            }
        }
    }

    fn escape(&mut self, window: &AppWindow) {
        let overlay = window.get_overlay();
        if overlay == "search" {
            self.viewer.close_search(window);
        }
        if overlay == "note" {
            self.viewer.note_cancelled();
        }
        if !overlay.is_empty() {
            self.set_overlay(window, "");
            return;
        }
        if self.viewer.clear_selection(window) {
            return;
        }
        if window.get_fullscreen() {
            window.set_fullscreen(false);
            window.window().set_fullscreen(false);
        } else if window.get_focus_mode() {
            window.set_focus_mode(false);
        }
    }

    /// Shows the settings window, or hides it if it's already in front.
    pub fn toggle_settings(&self) {
        let window = self.settings.window();
        let result = if window.is_visible() {
            window.hide()
        } else {
            self.settings.show()
        };
        // Failing to open settings isn't worth crashing the reader over.
        let _ = result;
    }

    pub fn close_settings(&self) {
        let _ = self.settings.hide();
    }

    pub fn set_overlay(&self, window: &AppWindow, overlay: &str) {
        window.set_overlay(overlay.into());
        window.invoke_focus_overlay();
    }

    // ----- Tabs and session ------------------------------------------------

    fn open_dialog(&mut self, window: &AppWindow) {
        // Modal native dialog; returns once the user picks or cancels.
        let picked = rfd::FileDialog::new()
            .set_title("Open PDF")
            .add_filter("PDF", &["pdf"])
            .pick_files();
        for path in picked.unwrap_or_default() {
            self.open_path(window, path);
        }
    }

    /// Opens a document in a new tab, or switches to it if already open.
    pub fn open_path(&mut self, window: &AppWindow, path: PathBuf) {
        let index = match self.tabs.iter().position(|t| same_file(t, &path)) {
            Some(index) => index,
            None => {
                self.tabs.push(path);
                self.tabs.len() - 1
            }
        };
        self.activate(window, index);
    }

    pub fn open_tab(&mut self, window: &AppWindow, index: usize) {
        self.activate(window, index);
    }

    fn activate(&mut self, window: &AppWindow, index: usize) {
        let Some(path) = self.tabs.get(index).cloned() else {
            return;
        };
        let already = self.active == Some(index) && self.viewer.is_showing(&path);
        self.active = Some(index);
        if !already {
            self.viewer.remember_position();
            self.viewer.clear_selection(window);
            if !self.viewer.open(window, path) {
                self.tabs.remove(index);
                self.active = None;
                if self.tabs.is_empty() {
                    self.viewer.close(window);
                } else {
                    self.activate(window, index.min(self.tabs.len() - 1));
                    return;
                }
            }
        }
        self.tabs_changed(window);
    }

    pub fn close_tab(&mut self, window: &AppWindow, index: usize) {
        if index >= self.tabs.len() {
            return;
        }
        self.tabs.remove(index);
        match self.active {
            _ if self.tabs.is_empty() => {
                self.active = None;
                self.viewer.close(window);
            }
            Some(active) if active == index => {
                self.active = None;
                self.activate(window, index.min(self.tabs.len() - 1));
            }
            Some(active) if active > index => self.active = Some(active - 1),
            _ => {}
        }
        self.tabs_changed(window);
    }

    pub fn restore_session(&mut self, window: &AppWindow) {
        let session: Vec<PathBuf> = self
            .viewer
            .store
            .session
            .iter()
            .filter(|p| p.exists())
            .cloned()
            .collect();
        let active = self.viewer.store.active_tab;
        let first_new = self.tabs.len();
        for path in session {
            if !self.tabs.iter().any(|t| same_file(t, &path)) {
                self.tabs.push(path);
            }
        }
        if self.tabs.is_empty() {
            return;
        }
        let target = (first_new + active).min(self.tabs.len() - 1);
        self.activate(window, target);
    }

    fn tabs_changed(&mut self, window: &AppWindow) {
        let items: Vec<TabItem> = self
            .tabs
            .iter()
            .enumerate()
            .map(|(i, path)| TabItem {
                title: path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default()
                    .into(),
                active: Some(i) == self.active,
            })
            .collect();
        self.tab_items.set_vec(items);

        let store = &mut self.viewer.store;
        // An empty tab list keeps the last session around to restore later.
        if !self.tabs.is_empty() {
            store.session = self.tabs.clone();
            store.active_tab = self.active.unwrap_or(0);
            let _ = store.save();
        }
        window.set_session_tabs(if self.tabs.is_empty() {
            ui_int(self.viewer.store.session.len())
        } else {
            0
        });
    }

    // ----- Worker events ---------------------------------------------------

    pub fn event(&mut self, window: &AppWindow, event: Event) {
        match event {
            Event::Opened { title, page_sizes } => {
                self.viewer.opened(window, title, page_sizes);
                self.tabs_changed(window);
            }
            Event::Rendered {
                page,
                width_px,
                bitmap,
            } => self.viewer.rendered(window, page, width_px, bitmap),
            Event::Thumbnail { page, bitmap } => self.viewer.thumbnail(window, page, bitmap),
            Event::SearchHits { id, page, matches } => {
                self.viewer.search_hits(window, id, page, matches)
            }
            Event::SearchDone { id } => self.viewer.search_done(window, id),
            Event::Selection { page, rects, text } => {
                self.viewer.selected(window, page, rects, text)
            }
            Event::Error(message) => {
                if let Some(path) = self.viewer.open_failed(window, message) {
                    if let Some(index) = self.tabs.iter().position(|t| *t == path) {
                        self.close_tab(window, index);
                    }
                }
            }
        }
    }

    // ----- Viewer passthrough ---------------------------------------------

    pub fn viewer(&mut self) -> &mut Viewer {
        &mut self.viewer
    }

    pub fn open_recent(&mut self, window: &AppWindow, index: usize) {
        if let Some(path) = self.viewer.store.recent.get(index).map(|r| r.path.clone()) {
            self.open_path(window, path);
        }
    }

    pub fn go_to_typed_page(&mut self, window: &AppWindow, text: &str) {
        self.viewer.go_to_typed_page(window, text);
        self.set_overlay(window, "");
    }

    pub fn popup_action(&mut self, window: &AppWindow, action: &str, index: usize) {
        let outcome = self.viewer.popup_action(window, action, index);
        self.popup_outcome(window, outcome);
    }

    fn popup_outcome(&mut self, window: &AppWindow, outcome: PopupOutcome) {
        match outcome {
            PopupOutcome::EditNote { quote, note } => {
                window.set_note_quote(quote.into());
                window.set_note_text(note.into());
                self.set_overlay(window, "note");
            }
            PopupOutcome::Copied => {
                window.set_status("Copied".into());
                clear_status_later(window);
            }
            PopupOutcome::Nothing => {}
        }
    }

    pub fn note_saved(&mut self, window: &AppWindow) {
        let text = window.get_note_text();
        self.viewer.note_saved(window, &text);
        self.set_overlay(window, "");
    }

    fn export_notes(&mut self, window: &AppWindow) {
        let Some(markdown) = self.viewer.notes_markdown() else {
            window.set_status("No notes in this document yet".into());
            clear_status_later(window);
            return;
        };
        let name = format!("{} - notes.md", self.viewer.title());
        let target = rfd::FileDialog::new()
            .set_title("Export notes")
            .set_file_name(&name)
            .add_filter("Markdown", &["md"])
            .save_file();
        if let Some(path) = target {
            let message = match std::fs::write(&path, markdown) {
                Ok(()) => format!("Notes exported to {}", file_name(&path)),
                Err(err) => format!("Couldn't export notes: {err}"),
            };
            window.set_status(message.into());
            clear_status_later(window);
        }
    }

    // ----- Command palette -------------------------------------------------

    fn open_palette(&mut self, window: &AppWindow) {
        let mut entries: Vec<Entry> = Vec::new();
        for (i, path) in self.tabs.iter().enumerate() {
            entries.push(Entry {
                target: Target::Tab(i),
                title: file_name(path),
                detail: "Open tab".to_owned(),
                keys: String::new(),
            });
        }
        for action in Action::ALL {
            if *action == Action::CommandPalette || *action == Action::Escape {
                continue;
            }
            entries.push(Entry {
                target: Target::Command(*action),
                title: action.title().to_owned(),
                detail: String::new(),
                keys: self
                    .keymap
                    .keys_for(*action)
                    .into_iter()
                    .next()
                    .unwrap_or_default(),
            });
        }
        for theme in &self.themes {
            entries.push(Entry {
                target: Target::Theme(theme.name.clone()),
                title: format!("Theme: {}", theme.name),
                detail: if theme.dark { "Dark" } else { "Light" }.to_owned(),
                keys: String::new(),
            });
        }
        for recent in &self.viewer.store.recent {
            if self.tabs.iter().any(|t| same_file(t, &recent.path)) {
                continue;
            }
            entries.push(Entry {
                target: Target::Recent(recent.path.clone()),
                title: file_name(&recent.path),
                detail: recent
                    .path
                    .parent()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
                keys: String::new(),
            });
        }
        self.palette_entries = entries;
        window.set_palette_query("".into());
        self.palette_edited(window, "");
        self.set_overlay(window, "palette");
    }

    pub fn palette_edited(&mut self, window: &AppWindow, query: &str) {
        let mut shown = palette::filter(&self.palette_entries, query, PALETTE_LIMIT);
        if let Some(page) = palette::page_query(query) {
            if page <= self.viewer.page_count() {
                shown.insert(
                    0,
                    Entry {
                        target: Target::GoToPage(page - 1),
                        title: format!("Go to page {page}"),
                        detail: String::new(),
                        keys: String::new(),
                    },
                );
            }
        }
        self.palette_shown = shown;
        self.palette_selected = 0;
        self.update_palette(window);
    }

    pub fn palette_moved(&mut self, window: &AppWindow, delta: i32) {
        let len = self.palette_shown.len();
        if len == 0 {
            return;
        }
        let next = (self.palette_selected as i64 + i64::from(delta)).rem_euclid(len as i64);
        self.palette_selected = next as usize;
        self.update_palette(window);
    }

    pub fn palette_accept(&mut self, window: &AppWindow, index: Option<usize>) {
        let index = index.unwrap_or(self.palette_selected);
        let Some(entry) = self.palette_shown.get(index).cloned() else {
            return;
        };
        self.set_overlay(window, "");
        match entry.target {
            Target::Command(action) => self.run(window, action),
            Target::Tab(i) => self.activate(window, i),
            Target::Recent(path) => self.open_path(window, path),
            Target::GoToPage(page) => self.viewer.go_to_page(window, page),
            Target::Theme(name) => self.setting(window, "theme", &name),
        }
    }

    fn update_palette(&self, window: &AppWindow) {
        let items: Vec<PaletteItem> = self
            .palette_shown
            .iter()
            .map(|e| PaletteItem {
                title: e.title.as_str().into(),
                detail: e.detail.as_str().into(),
                keys: e.keys.as_str().into(),
            })
            .collect();
        self.palette_items.set_vec(items);
        window.set_palette_selected(ui_int(self.palette_selected));
        debug_assert!(self.palette_items.row_count() == self.palette_shown.len());
    }

    /// Saves positions and the session on exit.
    pub fn quit(&mut self) {
        self.viewer.remember_position();
    }
}

fn apply_theme(t: &Theme<'_>, theme: &ThemeDef) {
    t.set_background(to_color(&theme.background));
    t.set_surface(to_color(&theme.surface));
    t.set_raised(to_color(&theme.raised));
    t.set_border(to_color(&theme.border));
    t.set_text(to_color(&theme.text));
    t.set_text_muted(to_color(&theme.text_muted));
    t.set_accent(to_color(&theme.accent));
    t.set_hover(to_color(&theme.hover));
    t.set_selection(to_color(&theme.selection));
    t.set_match(to_color(&theme.match_));
    t.set_match_current(to_color(&theme.match_current));
}

/// Opens a file with its default app, or a folder in Explorer.
fn reveal(path: &Path) {
    // Explorer returns a non-zero code even on success; nothing to check.
    let _ = Command::new("explorer.exe").arg(path).spawn();
}

/// Gives users a starting point when they first open the themes folder.
fn ensure_theme_template(dir: &Path) {
    let _ = std::fs::create_dir_all(dir);
    let empty = std::fs::read_dir(dir).map_or(true, |mut d| d.next().is_none());
    if empty {
        let template = include_str!("../themes/graphite.toml")
            .replace("name = \"Graphite\"", "name = \"My Theme\"");
        let _ = std::fs::write(dir.join("my-theme.toml"), template);
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    a == b
        || a.to_string_lossy()
            .eq_ignore_ascii_case(&b.to_string_lossy())
}

/// Clears transient status messages ("Copied") after a moment.
fn clear_status_later(window: &AppWindow) {
    let weak = window.as_weak();
    slint::Timer::single_shot(std::time::Duration::from_secs(2), move || {
        if let Some(window) = weak.upgrade() {
            window.set_status("".into());
        }
    });
}
