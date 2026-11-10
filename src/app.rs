//! Application state above the open document: tabs and the session,
//! keybindings, the command palette, settings, themes, and overlays.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;

use slint::{ComponentHandle, Model, VecModel};

use crate::config::{
    self, built_in_themes, load_themes, Config, Corners, DefaultZoom, Density, PageColors,
    ThemeDef, ThumbnailSize,
};
use crate::export::Format;
use crate::keymap::{Action, Chord, KeyResult, Keymap};
use crate::palette::{self, Entry, Target};
use crate::pdf::{Event, Worker};
use crate::store::{unique_name, Session, Store, DEFAULT_SESSION};
use crate::study::{self, Clock, StudyLog};
use crate::titlebar;
use crate::update;
use crate::viewer::PageBuffer;
use crate::viewer::{file_name, to_color, ui_int, PopupOutcome, SidePanel, Viewer};
use crate::{
    AppWindow, KeyRow, PaletteItem, RecentItem, SettingsWindow, SubjectItem, TabItem, Theme,
    ThemeCard,
};

const PALETTE_LIMIT: usize = 12;
/// Characters a session search needs before it starts.
const MIN_SESSION_QUERY: usize = 3;
const PALETTE_PLACEHOLDER: &str = "Search commands, tabs, files, themes, or type a page number…";
/// Lines of the export shown in the export sheet.
const EXPORT_PREVIEW_LINES: usize = 14;

#[derive(Clone)]
enum Prompt {
    NewSession,
    RenameSession,
    /// The password of an encrypted document.
    Password(PathBuf),
}

pub struct App {
    viewer: Viewer,
    settings: SettingsWindow,
    /// Title bar caption, text, and border colors, and whether it's dark.
    title_colors: Option<(slint::Color, slint::Color, slint::Color, bool)>,
    config: Config,
    themes: Vec<ThemeDef>,
    keymap: Keymap,
    /// Problems with config.toml, themes, or keybindings, shown in Settings.
    problems: Vec<String>,

    tabs: Vec<PathBuf>,
    active: Option<usize>,
    tab_items: Rc<VecModel<TabItem>>,
    session_items: Rc<VecModel<RecentItem>>,
    /// What the name prompt is asking for.
    prompt: Option<Prompt>,
    /// A newer release, while its prompt is open or it downloads.
    update: Option<update::Release>,

    study: StudyLog,
    clock: Clock,
    /// Saves study time a little after it changes.
    study_save: slint::Timer,
    /// The study time shown in the header, to skip redundant updates.
    study_shown: String,

    export_format: Format,
    /// Group Markdown exports by outline section.
    export_by_section: bool,

    /// The palette is searching the session's documents, with this id.
    session_search: Option<u64>,
    next_session_search: u64,
    /// A page to show once its document opens, and the query to highlight.
    open_at: Option<(PathBuf, usize, String)>,

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
        let session_items = Rc::new(VecModel::default());
        window.set_tabs(tab_items.clone().into());
        window.set_sessions(session_items.clone().into());
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
            title_colors: None,
            config: Config::default(),
            themes: built_in_themes(),
            keymap: Keymap::default(),
            problems: Vec::new(),
            tabs: Vec::new(),
            active: None,
            tab_items,
            session_items,
            prompt: None,
            update: None,
            study: StudyLog::load(),
            clock: Clock::default(),
            study_save: slint::Timer::default(),
            study_shown: String::new(),
            export_format: Format::Markdown,
            export_by_section: true,
            session_search: None,
            next_session_search: 0,
            open_at: None,
            palette_entries: Vec::new(),
            palette_shown: Vec::new(),
            palette_selected: 0,
            palette_items,
        };
        app.reload_config(window);
        window.set_sidebar_visible(app.config.sidebar);

        // Attach to the last session, like `tmux attach`.
        app.load_session(window, app.config.restore_session);
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
        window.set_sidebar_button(self.config.sidebar_button);
        window.set_study_timer(self.config.study_timer);
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
        let metrics = self.config.metrics();
        for t in [window.global::<Theme>(), self.settings.global::<Theme>()] {
            t.set_font_family(self.config.font.as_str().into());
            t.set_font_size(self.config.font_size());
            t.set_spacing(metrics.spacing);
            t.set_radius(metrics.radius);
            t.set_header_height(metrics.header_height);
            t.set_sidebar_width(metrics.sidebar_width);
        }
        self.viewer.set_sidebar_width(window, metrics.sidebar_width);
        let mut fonts: Vec<slint::SharedString> =
            config::FONTS.iter().map(|f| (*f).into()).collect();
        if !config::FONTS.contains(&self.config.font.as_str()) {
            fonts.insert(0, self.config.font.as_str().into());
        }
        let s = &self.settings;
        s.set_font_options(Rc::new(VecModel::from(fonts)).into());
        s.set_font_name(self.config.font.as_str().into());
        s.set_font_size(self.config.font_size().round() as i32);
        s.set_density(label(self.config.density).into());
        s.set_corners(label(self.config.corners).into());
        s.set_thumbnail_size(label(self.config.thumbnail_size).into());
        self.title_colors = theme.as_ref().map(|t| {
            (
                to_color(&t.background),
                to_color(&t.text),
                to_color(&t.border),
                t.dark,
            )
        });
        paint_title_bars_soon(window.as_weak());
        if let Some(theme) = &theme {
            apply_theme(&window.global::<Theme>(), theme);
            apply_theme(&self.settings.global::<Theme>(), theme);
        } else {
            problems.push(format!("Theme \"{}\" not found", self.config.theme));
        }

        let built_in: Vec<String> = built_in_themes().into_iter().map(|t| t.name).collect();
        let cards: Vec<ThemeCard> = self
            .themes
            .iter()
            .map(|t| ThemeCard {
                name: t.name.as_str().into(),
                background: to_color(&t.background),
                text: to_color(&t.text),
                border: to_color(&t.border),
                dot1: opaque(to_color(&t.accent)),
                dot2: opaque(to_color(&t.selection)),
                dot3: opaque(to_color(&t.match_current)),
                custom: !built_in.contains(&t.name),
            })
            .collect();
        self.settings
            .set_theme_cards(Rc::new(VecModel::from(cards)).into());
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

        self.update_key_rows();

        let names = crate::annotations::COLORS
            .iter()
            .map(|(color, _)| self.config.color_name(color))
            .collect();
        self.viewer.set_color_names(names);
        self.viewer.set_default_zoom(self.config.default_zoom);
        self.viewer
            .set_inverted(window, self.config.page_colors == PageColors::Inverted);
    }

    fn update_key_rows(&self) {
        let query = self.settings.get_query();
        let rows: Vec<KeyRow> = Action::ALL
            .iter()
            .filter(|a| {
                query.trim().is_empty()
                    || palette::score(&query, a.title()).is_some()
                    || palette::score(&query, a.name()).is_some()
            })
            .map(|a| KeyRow {
                title: a.title().into(),
                name: a.name().into(),
                keys: self.keymap.keys_for(*a).join(",  ").into(),
            })
            .collect();
        self.settings
            .set_key_rows(Rc::new(VecModel::from(rows)).into());
    }

    /// Filters the settings sections by the search box and jumps to the
    /// first match.
    pub fn settings_searched(&mut self, query: &str) {
        let [general, reading, theme, appearance, keys, files] = matching_sections(query);
        let s = &self.settings;
        s.set_show_general(general);
        s.set_show_reading(reading);
        s.set_show_theme(theme);
        s.set_show_appearance(appearance);
        s.set_show_keys(keys);
        s.set_show_files(files);
        let first = [
            (general, "general"),
            (reading, "reading"),
            (theme, "theme"),
            (appearance, "appearance"),
            (keys, "keys"),
            (files, "files"),
        ]
        .into_iter()
        .find(|(shown, _)| *shown);
        if let Some((_, section)) = first {
            if !query.trim().is_empty() {
                s.set_section(section.into());
            }
        }
        self.update_key_rows();
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
            "font" => self.config.font = value.to_owned(),
            "font-size" => {
                let step = if value == "-" { -1.0 } else { 1.0 };
                self.config.font_size = (self.config.font_size() + step)
                    .clamp(config::MIN_FONT_SIZE, config::MAX_FONT_SIZE);
            }
            "density" => {
                self.config.density = match value {
                    "Compact" => Density::Compact,
                    "Spacious" => Density::Spacious,
                    _ => Density::Comfortable,
                }
            }
            "corners" => {
                self.config.corners = match value {
                    "Sharp" => Corners::Sharp,
                    "Round" => Corners::Round,
                    _ => Corners::Rounded,
                }
            }
            "thumbnail-size" => {
                self.config.thumbnail_size = match value {
                    "Small" => ThumbnailSize::Small,
                    "Large" => ThumbnailSize::Large,
                    _ => ThumbnailSize::Medium,
                }
            }
            "sidebar" => {
                self.config.sidebar = on;
                window.set_sidebar_visible(on);
                self.viewer.sidebar_shown(window);
            }
            "vim" => self.config.vim = on,
            "reset" => {
                // Custom key bindings are hand-written in config.toml;
                // restoring visual defaults shouldn't wipe them.
                self.config = Config {
                    keys: std::mem::take(&mut self.config.keys),
                    vim: self.config.vim,
                    ..Config::default()
                };
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
            Action::SearchSession => self.open_session_search(window),
            Action::GoToPage if self.viewer.page_count() > 0 => {
                window.set_goto_text(ui_int(self.viewer.current_page() + 1).to_string().into());
                self.set_overlay(window, "goto");
            }
            Action::Settings => self.toggle_settings(window),
            Action::ToggleSidebar => {
                window.set_sidebar_visible(!window.get_sidebar_visible());
                self.viewer.sidebar_shown(window);
                // Closing from the sidebar's own button removes the focused
                // element; take focus back so Ctrl+B still reaches us.
                window.invoke_focus_overlay();
            }
            Action::ShowThumbnails | Action::ShowOutline | Action::ShowNotes => {
                let panel = match action {
                    Action::ShowOutline => SidePanel::Outline,
                    Action::ShowNotes => SidePanel::Notes,
                    _ => SidePanel::Thumbs,
                };
                self.show_side_panel(window, panel, true);
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
            Action::Home => self.go_home(window),
            Action::SwitchSession => self.open_session_picker(window),
            Action::NewSession => self.ask_name(window, Prompt::NewSession, ""),
            Action::RenameSession => {
                let name = self.session_name().to_owned();
                self.ask_name(window, Prompt::RenameSession, &name);
            }
            Action::DeleteSession => self.delete_session(window),
            Action::Copy | Action::Highlight | Action::AddNote => {
                let outcome = self.viewer.selection_action(window, action);
                self.popup_outcome(window, outcome);
            }
            Action::Color1 | Action::Color2 | Action::Color3 | Action::Color4 | Action::Color5 => {
                let index = action.color_index().unwrap_or(0);
                let outcome = self.viewer.color_key(window, index);
                self.popup_outcome(window, outcome);
            }
            Action::ToggleStudyTimer => {
                self.clock.paused = !self.clock.paused;
                self.clock.reset();
                window.set_study_paused(self.clock.paused);
                let state = if self.clock.paused {
                    "paused"
                } else {
                    "running"
                };
                self.flash(window, &format!("Study timer {state}"));
            }
            Action::MarkSection if self.viewer.page_count() > 0 => {
                if !self.viewer.mark_current_section(window) {
                    self.flash(window, "This document has no outline");
                }
            }
            Action::Undo | Action::Redo => {
                if !self.viewer.undo(window, action == Action::Redo) {
                    let what = if action == Action::Redo {
                        "redo"
                    } else {
                        "undo"
                    };
                    self.flash(window, &format!("Nothing to {what}"));
                }
            }
            Action::ExportNotes => self.export_notes(window),
            Action::CheckForUpdates => {
                window.set_status("Checking for updates…".into());
                crate::check_for_update(window, true);
            }
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
    pub fn toggle_settings(&self, window: &AppWindow) {
        let settings = self.settings.window();
        let result = if settings.is_visible() {
            settings.hide()
        } else {
            self.settings.show()
        };
        // Failing to open settings isn't worth crashing the reader over.
        let _ = result;
        paint_title_bars_soon(window.as_weak());
    }

    /// Colors both windows' title bars with the theme. Returns false if a
    /// visible window isn't ready yet.
    pub fn paint_title_bars(&self, window: &AppWindow) -> bool {
        let Some((caption, text, border, dark)) = self.title_colors else {
            return true;
        };
        let main = titlebar::paint(window.window(), caption, text, border, dark);
        let settings = !self.settings.window().is_visible()
            || titlebar::paint(self.settings.window(), caption, text, border, dark);
        main && settings
    }

    pub fn close_settings(&self) {
        let _ = self.settings.hide();
    }

    pub fn set_overlay(&mut self, window: &AppWindow, overlay: &str) {
        if overlay != "palette" {
            self.end_session_search(window);
        }
        // Dismissing the password prompt gives up on that document.
        if overlay != "prompt" {
            if let Some(Prompt::Password(path)) = self.prompt.take() {
                if let Some(index) = self.tabs.iter().position(|t| *t == path) {
                    self.close_tab(window, index);
                }
            }
        }
        window.set_overlay(overlay.into());
        window.invoke_focus_overlay();
    }

    /// Shows a sidebar panel. With `toggle`, asking for the panel already
    /// shown hides the sidebar.
    pub fn show_side_panel(&mut self, window: &AppWindow, panel: SidePanel, toggle: bool) {
        let visible = window.get_sidebar_visible();
        if toggle && visible && self.viewer.side_panel() == panel {
            window.set_sidebar_visible(false);
            return;
        }
        if !visible {
            window.set_sidebar_visible(true);
        }
        self.viewer.set_side_panel(window, panel);
        self.viewer.sidebar_shown(window);
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

    /// Back to the home screen; the tabs stay open.
    pub fn go_home(&mut self, window: &AppWindow) {
        self.viewer.close(window);
        self.active = None;
        self.tabs_changed(window);
    }

    // ----- Sessions --------------------------------------------------------

    fn session_name(&self) -> &str {
        let store = &self.viewer.store;
        store
            .sessions
            .get(store.current_session)
            .map_or(DEFAULT_SESSION, |s| s.name.as_str())
    }

    /// Shows the current session's tabs, reopening its active one if `open`.
    fn load_session(&mut self, window: &AppWindow, open: bool) {
        let store = &self.viewer.store;
        let session = store
            .sessions
            .get(store.current_session)
            .cloned()
            .unwrap_or_else(|| Session::new(DEFAULT_SESSION));
        self.tabs = session.tabs.into_iter().filter(|p| p.exists()).collect();
        self.active = None;
        match session.active.filter(|&a| open && a < self.tabs.len()) {
            Some(active) => self.activate(window, active),
            None => {
                self.viewer.close(window);
                self.tabs_changed(window);
            }
        }
    }

    pub fn switch_session(&mut self, window: &AppWindow, index: usize) {
        let store = &self.viewer.store;
        if index == store.current_session || index >= store.sessions.len() {
            return;
        }
        self.tabs_changed(window);
        self.viewer.store.current_session = index;
        self.load_session(window, true);
        let message = format!("Session: {}", self.session_name());
        self.flash(window, &message);
    }

    fn new_session(&mut self, window: &AppWindow, name: &str) {
        self.tabs_changed(window);
        let store = &mut self.viewer.store;
        let name = unique_name(&store.sessions, name);
        store.sessions.push(Session::new(&name));
        store.current_session = store.sessions.len() - 1;
        self.load_session(window, false);
    }

    fn rename_session(&mut self, window: &AppWindow, name: &str) {
        let store = &mut self.viewer.store;
        let current = store.current_session;
        let others: Vec<Session> = store
            .sessions
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != current)
            .map(|(_, s)| s.clone())
            .collect();
        if let Some(session) = store.sessions.get_mut(current) {
            session.name = unique_name(&others, name);
        }
        self.tabs_changed(window);
    }

    fn delete_session(&mut self, window: &AppWindow) {
        let store = &mut self.viewer.store;
        if store.sessions.len() <= 1 {
            self.flash(window, "The only session can't be deleted");
            return;
        }
        let removed = store.sessions.remove(store.current_session);
        store.current_session = store.current_session.min(store.sessions.len() - 1);
        self.load_session(window, true);
        self.flash(window, &format!("Deleted session {}", removed.name));
    }

    fn ask_name(&mut self, window: &AppWindow, prompt: Prompt, initial: &str) {
        window.set_prompt_label(
            match prompt {
                Prompt::NewSession => "New session",
                Prompt::RenameSession => "Rename session",
                Prompt::Password(_) => "Password",
            }
            .into(),
        );
        window.set_prompt_secret(matches!(prompt, Prompt::Password(_)));
        self.prompt = Some(prompt);
        window.set_prompt_text(initial.into());
        self.set_overlay(window, "prompt");
    }

    pub fn prompt_accepted(&mut self, window: &AppWindow, text: &str) {
        // Taken first: closing the overlay with a password prompt pending
        // would give up on the document.
        let prompt = self.prompt.take();
        self.set_overlay(window, "");
        match prompt {
            Some(Prompt::NewSession) => self.new_session(window, text),
            Some(Prompt::RenameSession) => self.rename_session(window, text),
            Some(Prompt::Password(path)) if self.viewer.is_pending(&path) => {
                self.viewer.open_with_password(path, text.to_owned());
            }
            Some(Prompt::Password(_)) => {}
            None => {}
        }
    }

    fn open_session_picker(&mut self, window: &AppWindow) {
        self.end_session_search(window);
        self.palette_entries = self.session_entries();
        self.palette_entries.push(Entry {
            target: Target::Command(Action::NewSession),
            title: "New session…".to_owned(),
            detail: String::new(),
            keys: self
                .keymap
                .keys_for(Action::NewSession)
                .into_iter()
                .next()
                .unwrap_or_default(),
        });
        window.set_palette_query("".into());
        self.palette_edited(window, "");
        // Start on the session after the current one, like the tmux chooser.
        let current = self.viewer.store.current_session;
        self.palette_selected = (current + 1) % self.viewer.store.sessions.len().max(1);
        self.update_palette(window);
        self.set_overlay(window, "palette");
    }

    fn session_entries(&self) -> Vec<Entry> {
        let store = &self.viewer.store;
        store
            .sessions
            .iter()
            .enumerate()
            .map(|(i, s)| Entry {
                target: Target::Session(i),
                title: format!("Session: {}", s.name),
                detail: session_detail(s, i == store.current_session),
                keys: String::new(),
            })
            .collect()
    }

    fn flash(&self, window: &AppWindow, message: &str) {
        window.set_status(message.into());
        clear_status_later(window);
    }

    fn tabs_changed(&mut self, window: &AppWindow) {
        let items: Vec<TabItem> = self
            .tabs
            .iter()
            .enumerate()
            .map(|(i, path)| TabItem {
                title: file_name(path).into(),
                active: Some(i) == self.active,
            })
            .collect();
        self.tab_items.set_vec(items);

        let store = &mut self.viewer.store;
        let current = store.current_session;
        if let Some(session) = store.sessions.get_mut(current) {
            session.tabs = self.tabs.clone();
            session.active = self.active;
        }
        let _ = store.save();

        let sessions: Vec<RecentItem> = store
            .sessions
            .iter()
            .enumerate()
            .map(|(i, s)| RecentItem {
                title: s.name.as_str().into(),
                folder: session_detail(s, i == current).into(),
            })
            .collect();
        self.session_items.set_vec(sessions);
        window.set_session_name(self.session_name().into());
        window.set_home(self.active.is_none());
        if self.active.is_none() {
            self.update_subjects(window);
        }
        self.clock.reset();
        self.show_study_time(window);
    }

    // ----- Study time -------------------------------------------------------

    /// Counts the time since the last activity toward the open document.
    pub fn activity(&mut self, window: &AppWindow) {
        if !self.config.study_timer {
            return;
        }
        let Some(path) = self.viewer.path().map(Path::to_path_buf) else {
            self.clock.reset();
            return;
        };
        let seconds = self.clock.tick(crate::annotations::now());
        if seconds == 0 {
            return;
        }
        self.study.add(study::today(), &path, seconds);
        self.show_study_time(window);
        if !self.study_save.running() {
            self.study_save.start(
                slint::TimerMode::SingleShot,
                std::time::Duration::from_secs(30),
                || crate::with_app(|app| app.save_study()),
            );
        }
    }

    fn save_study(&self) {
        // Losing a few seconds of study time isn't worth bothering about.
        let _ = self.study.save();
    }

    fn show_study_time(&mut self, window: &AppWindow) {
        let today = study::today();
        let text = study::format_duration(self.study.total(today, today));
        if text != self.study_shown {
            window.set_study_time(text.as_str().into());
            self.study_shown = text;
        }
    }

    /// The home screen's list of the session's documents, with progress,
    /// notes, and time.
    fn update_subjects(&self, window: &AppWindow) {
        let today = study::today();
        let subjects: Vec<SubjectItem> = self
            .tabs
            .iter()
            .enumerate()
            .map(|(i, path)| {
                let notes = crate::annotations::Annotations::load(path);
                let (done, total) = (
                    notes.sections.len().min(notes.outline_sections),
                    notes.outline_sections,
                );
                let mut detail = Vec::new();
                if total > 0 {
                    detail.push(format!("{done} of {total} sections"));
                }
                detail.push(match notes.items.len() {
                    1 => "1 note".to_owned(),
                    n => format!("{n} notes"),
                });
                let time = self.study.for_path(path);
                if time >= 60 {
                    detail.push(study::format_duration(time));
                }
                SubjectItem {
                    title: file_name(path).into(),
                    detail: detail.join(" · ").into(),
                    progress: if total == 0 {
                        0.0
                    } else {
                        done as f32 / total as f32
                    },
                    has_progress: total > 0,
                    active: Some(i) == self.active,
                }
            })
            .collect();
        window.set_subjects(Rc::new(VecModel::from(subjects)).into());
        let mut summary = Vec::new();
        if self.config.study_timer {
            summary.push(format!(
                "today {}",
                study::format_duration(self.study.total(today, today))
            ));
            summary.push(format!(
                "this week {}",
                study::format_duration(self.study.total(today - 6, today))
            ));
        }
        window.set_study_summary(summary.join(" · ").into());
    }

    // ----- Worker events ---------------------------------------------------

    pub fn event(&mut self, window: &AppWindow, event: Event<PageBuffer>) {
        match event {
            // A document opened after a newer one was requested is stale.
            Event::Opened { ref path, .. } if !self.viewer.is_pending(path) => {}
            Event::Opened {
                title,
                page_sizes,
                outline,
                ..
            } => {
                self.viewer.opened(window, title, page_sizes, outline);
                self.tabs_changed(window);
                if let Some((path, page, query)) = self.open_at.take() {
                    if self.viewer.is_showing(&path) {
                        self.show_hit(window, page, &query);
                    }
                }
            }
            Event::PasswordNeeded { path, retry } => {
                if self.viewer.is_pending(&path) {
                    self.ask_name(window, Prompt::Password(path), "");
                    if retry {
                        window.set_prompt_label("Wrong password".into());
                    }
                }
            }
            Event::Labels(labels) => self.viewer.labels_found(window, labels),
            Event::Link(target) => {
                if let Some(uri) = self.viewer.link(window, target) {
                    // Only http(s) and mailto links get here (see pdf::safe_uri).
                    let _ = Command::new("explorer.exe").arg(&uri).spawn();
                    self.flash(window, "Opened link in browser");
                }
            }
            Event::Rendered {
                page,
                width_px,
                bitmap,
            } => self.viewer.rendered(window, page, width_px, bitmap),
            Event::Thumbnail { page, bitmap } => self.viewer.thumbnail(page, bitmap),
            Event::SearchHits { id, page, matches } => {
                self.viewer.search_hits(window, id, page, matches)
            }
            Event::SearchDone { id } => self.viewer.search_done(window, id),
            Event::Selection(parts) => self.viewer.selected(window, parts),
            Event::FileHit {
                id,
                path,
                page,
                snippet,
                count,
            } => self.file_hit(window, id, path, page, snippet, count),
            Event::FilesDone { .. } => {}
            Event::Article { number, page } => self.article_found(window, number, page),
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
        self.set_overlay(window, "");
        // "art. 37" jumps to where that article of a law starts.
        match crate::nav::parse_article(text) {
            Some(number) => {
                self.viewer.find_article(number);
                window.set_status(format!("Looking for art. {number}…").into());
            }
            None => self.viewer.go_to_typed_page(window, text),
        }
    }

    fn article_found(&mut self, window: &AppWindow, number: u32, page: Option<usize>) {
        match page {
            Some(page) => {
                window.set_status("".into());
                self.viewer.go_to_page(window, page);
            }
            None => self.flash(window, &format!("Art. {number} not found")),
        }
    }

    // ----- Session search ----------------------------------------------------

    /// Back to the command palette's own search; stops the worker.
    fn end_session_search(&mut self, window: &AppWindow) {
        if self.session_search.take().is_some() {
            self.viewer.search_files(0, "", Vec::new());
            window.set_palette_placeholder(PALETTE_PLACEHOLDER.into());
        }
    }

    fn open_session_search(&mut self, window: &AppWindow) {
        if self.tabs.is_empty() {
            self.flash(window, "No tabs in this session");
            return;
        }
        self.palette_entries.clear();
        self.session_search = Some(self.next_session_search);
        window.set_palette_placeholder(
            format!(
                "Search the text of {} in this session…",
                match self.tabs.len() {
                    1 => "the tab".to_owned(),
                    n => format!("all {n} tabs"),
                }
            )
            .into(),
        );
        window.set_palette_query("".into());
        self.palette_edited(window, "");
        self.set_overlay(window, "palette");
    }

    fn session_query_edited(&mut self, window: &AppWindow, query: &str) {
        self.next_session_search += 1;
        let id = self.next_session_search;
        self.session_search = Some(id);
        self.palette_shown.clear();
        self.palette_selected = 0;
        let query = query.trim();
        let paths = if query.chars().count() >= MIN_SESSION_QUERY {
            self.tabs.clone()
        } else {
            Vec::new()
        };
        let searching = !paths.is_empty();
        self.viewer
            .search_files(id, if searching { query } else { "" }, paths);
        self.update_palette(window);
    }

    fn file_hit(
        &mut self,
        window: &AppWindow,
        id: u64,
        path: PathBuf,
        page: usize,
        snippet: String,
        count: usize,
    ) {
        if self.session_search != Some(id) || window.get_overlay() != "palette" {
            return;
        }
        let matches = match count {
            1 => String::new(),
            n => format!(" · {n} matches"),
        };
        self.palette_shown.push(Entry {
            target: Target::Hit(path.clone(), page),
            title: snippet,
            detail: format!("{} · p. {}{matches}", file_name(&path), page + 1),
            keys: String::new(),
        });
        self.update_palette(window);
    }

    /// Shows a page found by the session search, opening its tab first.
    fn open_hit(&mut self, window: &AppWindow, path: PathBuf, page: usize) {
        let query = window.get_palette_query().trim().to_owned();
        if self.viewer.is_showing(&path) && self.viewer.page_count() > 0 {
            self.show_hit(window, page, &query);
        } else {
            self.open_at = Some((path.clone(), page, query));
            self.open_path(window, path);
        }
    }

    fn show_hit(&mut self, window: &AppWindow, page: usize, query: &str) {
        self.viewer.go_to_page(window, page);
        // Highlight the matches, as if searched in the document.
        window.set_search_text(query.into());
        self.viewer.search_edited(window, query);
        self.set_overlay(window, "search");
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

    /// Opens the export sheet for the current document.
    fn export_notes(&mut self, window: &AppWindow) {
        if self.viewer.page_count() == 0 {
            return;
        }
        if !self.viewer.has_notes() {
            self.flash(window, "No notes in this document yet");
            return;
        }
        self.update_export(window);
        self.set_overlay(window, "export");
    }

    fn update_export(&self, window: &AppWindow) {
        let (text, count) = self
            .viewer
            .export(self.export_format, self.export_by_section);
        let preview: Vec<&str> = text.lines().take(EXPORT_PREVIEW_LINES).collect();
        let mut preview = preview.join("\n");
        if text.lines().count() > EXPORT_PREVIEW_LINES {
            preview.push_str("\n…");
        }
        window.set_export_format(self.export_format.name().into());
        window.set_export_by_section(self.export_by_section);
        window.set_export_preview(preview.into());
        window.set_export_count(
            match count {
                1 => "1 note".to_owned(),
                n => format!("{n} notes"),
            }
            .into(),
        );
        window.set_export_ready(count > 0);
    }

    /// A choice in the export sheet: "format" or "by-section".
    pub fn export_option(&mut self, window: &AppWindow, key: &str, value: &str) {
        match key {
            "format" => {
                if let Some(format) = Format::from_name(value) {
                    self.export_format = format;
                }
            }
            "by-section" => self.export_by_section = value == "true",
            _ => return,
        }
        self.update_export(window);
    }

    /// A color toggled in the notes filter, which the export follows.
    pub fn note_filter_clicked(&mut self, window: &AppWindow, index: usize) {
        self.viewer.note_filter_clicked(index);
        if window.get_overlay() == "export" {
            self.update_export(window);
        }
    }

    pub fn export_accepted(&mut self, window: &AppWindow) {
        let format = self.export_format;
        let (text, count) = self.viewer.export(format, self.export_by_section);
        if count == 0 {
            return;
        }
        let name = format!(
            "{}.{}",
            safe_file_name(self.viewer.title()),
            format.extension()
        );
        // A configured folder (an Obsidian vault) is written without
        // asking; re-exporting replaces the file with the current notes.
        let folder = self.config.notes_folder.trim();
        let target = if folder.is_empty() {
            rfd::FileDialog::new()
                .set_title("Export notes")
                .set_file_name(&name)
                .add_filter(format.label(), &[format.extension()])
                .save_file()
        } else {
            Some(std::path::Path::new(folder).join(&name))
        };
        let Some(path) = target else {
            return;
        };
        self.set_overlay(window, "");
        let message = match std::fs::write(&path, text) {
            Ok(()) => format!("Notes exported to {}", file_name(&path)),
            Err(err) => format!("Couldn't export notes: {err}"),
        };
        self.flash(window, &message);
    }

    // ----- Command palette -------------------------------------------------

    fn open_palette(&mut self, window: &AppWindow) {
        self.end_session_search(window);
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
        entries.extend(self.session_entries());
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
        if self.session_search.is_some() {
            return self.session_query_edited(window, query);
        }
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
            Target::Session(i) => self.switch_session(window, i),
            Target::Hit(path, page) => self.open_hit(window, path, page),
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

    // ----- Updates ------------------------------------------------------------

    pub fn checks_updates(&self) -> bool {
        self.config.check_updates
    }

    pub fn update_checked(
        &mut self,
        window: &AppWindow,
        result: Result<Option<update::Release>, String>,
        manual: bool,
    ) {
        match result {
            Ok(Some(release)) => {
                let skipped = update::skipped().as_deref() == Some(release.version.as_str());
                // Don't cover something the user is typing into.
                let busy = !window.get_overlay().is_empty();
                if manual || (!skipped && !busy) {
                    window.set_status("".into());
                    window.set_update_version(release.version.as_str().into());
                    window.set_update_installs(update::installed());
                    self.update = Some(release);
                    self.set_overlay(window, "update");
                }
            }
            Ok(None) if manual => {
                window.set_status("dpdf is up to date".into());
                clear_status_later(window);
            }
            Err(err) if manual => {
                window.set_status(format!("Couldn't check for updates: {err}").into());
                clear_status_later(window);
            }
            _ => {}
        }
    }

    /// "install", "skip", or "later", from the update prompt.
    pub fn update_action(&mut self, window: &AppWindow, action: &str) {
        self.set_overlay(window, "");
        let Some(release) = self.update.take() else {
            return;
        };
        match action {
            "install" if update::installed() => {
                window.set_status(format!("Downloading dpdf {}…", release.version).into());
                crate::download_update(window, release);
            }
            // A portable copy: let the user fetch the new zip.
            "install" => update::open_page(&release),
            "skip" => {
                if let Err(err) = update::skip(&release) {
                    window.set_status(format!("Couldn't save that: {err}").into());
                    clear_status_later(window);
                }
            }
            _ => {}
        }
    }

    pub fn update_downloaded(&mut self, window: &AppWindow, result: Result<PathBuf, String>) {
        let result = result.and_then(|path| update::install(&path).map_err(|e| e.to_string()));
        match result {
            // The installer replaces the running exe; get out of its way.
            Ok(()) => {
                let _ = slint::quit_event_loop();
            }
            Err(err) => {
                window.set_status(format!("Update failed: {err}").into());
                clear_status_later(window);
            }
        }
    }

    /// Saves positions and the session on exit.
    pub fn quit(&mut self) {
        self.viewer.remember_position();
        self.save_study();
    }
}

/// Words each settings section answers to, for the settings search box:
/// general, reading, theme, appearance, keybindings, files.
const SECTION_WORDS: [&str; 6] = [
    "general session sessions restore startup tabs attach",
    "reading zoom fit width page default sidebar thumbnails vim keyboard",
    "theme themes colors dark light custom",
    "appearance interface font text size density spacing compact corners rounded thumbnails page colors inverted night dark",
    "keybindings keys shortcuts bindings vim",
    "configuration config files toml data folder reload",
];

fn matching_sections(query: &str) -> [bool; 6] {
    let query = query.trim();
    // Every action is listed under Keybindings, so match those too.
    let action_match = Action::ALL
        .iter()
        .any(|a| palette::score(query, a.title()).is_some());
    let mut shown = SECTION_WORDS.map(|words| {
        query.is_empty() || words.split(' ').any(|w| palette::score(query, w).is_some())
    });
    shown[4] |= action_match;
    shown
}

/// The Settings label for a config choice, e.g. `Density::Compact` -> "Compact".
fn label(value: impl std::fmt::Debug) -> String {
    format!("{value:?}")
}

fn session_detail(session: &Session, current: bool) -> String {
    let tabs = match session.tabs.len() {
        1 => "1 tab".to_owned(),
        n => format!("{n} tabs"),
    };
    if current {
        format!("{tabs} · current")
    } else {
        tabs
    }
}

fn opaque(color: slint::Color) -> slint::Color {
    slint::Color::from_argb_u8(255, color.red(), color.green(), color.blue())
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

/// Paints the title bars once the native windows exist, retrying briefly
/// because they appear a moment after `show()`.
pub fn paint_title_bars_soon(window: slint::Weak<AppWindow>) {
    fn attempt(window: slint::Weak<AppWindow>, tries_left: u32) {
        slint::Timer::single_shot(std::time::Duration::from_millis(50), move || {
            let mut done = true;
            if let Some(win) = window.upgrade() {
                crate::with_app(|app| done = app.paint_title_bars(&win));
            }
            if !done && tries_left > 0 {
                attempt(window, tries_left - 1);
            }
        });
    }
    attempt(window, 40);
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

/// `title` with the characters Windows forbids in file names replaced.
fn safe_file_name(title: &str) -> String {
    let name: String = title
        .chars()
        .map(|c| {
            if r#"<>:"/\|?*"#.contains(c) || c.is_control() {
                '-'
            } else {
                c
            }
        })
        .collect();
    let name = name.trim().trim_end_matches('.').to_owned();
    if name.is_empty() {
        "notes".to_owned()
    } else {
        name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_file_names_are_safe() {
        assert_eq!(safe_file_name("a: b/c?"), "a- b-c-");
        assert_eq!(safe_file_name(" ... "), "notes");
    }

    #[test]
    fn settings_search() {
        assert_eq!(matching_sections(""), [true; 6]);
        assert_eq!(
            matching_sections("vim"),
            [false, true, false, false, true, false]
        );
        let theme = matching_sections("dark");
        assert!(theme[2] && theme[3] && !theme[0]);
        // Action titles land in Keybindings.
        assert!(matching_sections("export notes")[4]);
        assert_eq!(matching_sections("zzzqqq"), [false; 6]);
    }
}
