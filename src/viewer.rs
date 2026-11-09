//! UI glue for the open document: turns layout, render results, search
//! hits, the text selection, and annotations into Slint properties, and
//! decides which pages and thumbnails to render or evict.

use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use slint::{Color, ComponentHandle, Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, VecModel};

use crate::annotations::{self, Annotations, History, Kind};
use crate::config::{parse_color, DefaultZoom};
use crate::keymap::Action;
use crate::layout::{self, Layout, Zoom};
use crate::nav;
use crate::pdf::{Bitmap, LinkTarget, NormRect, OutlineItem, Request, Worker};
use crate::search::SearchState;
use crate::store::{Recent, Store};
use crate::{AppWindow, Highlight, Mark, PageView, RecentItem, SideItem, ThumbView};

/// Pages rendered ahead of and behind the visible ones.
const PRELOAD: usize = 2;
/// Rendered pages kept beyond the preload window before being dropped.
const KEEP: usize = 4;
/// Thumbnails rendered around the visible ones, and kept beyond that.
const THUMB_PRELOAD: usize = 6;
const THUMB_KEEP: usize = 20;
/// Recent documents listed on the start screen.
const RECENT_SHOWN: usize = 8;
/// Line scroll step, in logical pixels.
const SCROLL_STEP: f32 = 60.0;
/// Pointer travel before a press becomes a drag.
const DRAG_THRESHOLD: f32 = 4.0;

struct Selection {
    page: usize,
    rects: Vec<NormRect>,
    text: String,
}

struct Drag {
    page: usize,
    from: (f32, f32),
    start: (f32, f32),
    dragging: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Popup {
    None,
    Selection,
    Annotation(u64),
}

/// What the app should do after a popup button.
pub enum PopupOutcome {
    Nothing,
    /// Open the note editor with this quote and existing note.
    EditNote {
        quote: String,
        note: String,
    },
    Copied,
}

/// What the sidebar shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidePanel {
    Thumbs,
    Outline,
    Notes,
}

impl SidePanel {
    pub fn name(self) -> &'static str {
        match self {
            SidePanel::Thumbs => "thumbs",
            SidePanel::Outline => "outline",
            SidePanel::Notes => "notes",
        }
    }

    pub fn from_name(name: &str) -> Self {
        match name {
            "outline" => SidePanel::Outline,
            "notes" => SidePanel::Notes,
            _ => SidePanel::Thumbs,
        }
    }
}

pub struct Viewer {
    worker: Worker,
    pub store: Store,

    path: Option<PathBuf>,
    /// A document requested but not yet opened by the worker.
    pending: Option<PathBuf>,
    title: String,
    page_sizes: Vec<(f32, f32)>,
    layout: Layout,
    zoom: Zoom,
    default_zoom: Zoom,
    current_page: usize,
    pages: Rc<VecModel<PageView>>,
    /// Rendered pages and the pixel width they were rendered at.
    cache: HashMap<usize, (u32, Image)>,
    last_request: Option<(Range<usize>, u32)>,
    /// Pages currently in the UI model, to skip rebuilding it while scrolling.
    shown_pages: Option<Range<usize>>,
    shown_thumbs: Option<Range<usize>>,

    thumb_layout: Layout,
    /// Matches `Theme.sidebar-width`.
    sidebar_width: f32,
    thumbs: Rc<VecModel<ThumbView>>,
    thumb_cache: HashMap<usize, Image>,
    last_thumb_request: Option<Range<usize>>,

    search: Option<SearchState>,
    next_search_id: u64,

    annotations: Annotations,
    history: History,
    selection: Option<Selection>,
    drag: Option<Drag>,
    popup: Popup,
    /// The annotation whose note is being edited.
    note_target: Option<u64>,

    recent: Rc<VecModel<RecentItem>>,

    outline: Vec<OutlineItem>,
    /// Page labels, empty when the document has none.
    labels: Vec<String>,
    side_panel: SidePanel,
    side_items: Rc<VecModel<SideItem>>,
    /// The page each side item jumps to.
    side_pages: Vec<usize>,
    /// Set while the window is being resized: keep showing stretched
    /// pages instead of rendering every intermediate size.
    renders_held: bool,
    /// Passwords typed this run, so switching tabs doesn't ask again.
    /// Never written to disk.
    passwords: HashMap<PathBuf, String>,
}

impl Viewer {
    pub fn new(window: &AppWindow, worker: Worker, store: Store) -> Self {
        let pages = Rc::new(VecModel::default());
        let thumbs = Rc::new(VecModel::default());
        let recent = Rc::new(VecModel::default());
        window.set_pages(pages.clone().into());
        window.set_thumbs(thumbs.clone().into());
        window.set_recent(recent.clone().into());
        let side_items = Rc::new(VecModel::default());
        window.set_side_items(side_items.clone().into());
        let colors: Vec<Color> = annotations::COLORS
            .iter()
            .map(|(_, hex)| to_color(hex))
            .collect();
        window.set_annotation_colors(Rc::new(VecModel::from(colors)).into());

        let viewer = Self {
            worker,
            store,
            path: None,
            pending: None,
            title: String::new(),
            page_sizes: Vec::new(),
            layout: Layout::default(),
            zoom: Zoom::FitWidth,
            default_zoom: Zoom::FitWidth,
            current_page: 0,
            pages,
            cache: HashMap::new(),
            last_request: None,
            shown_pages: None,
            shown_thumbs: None,
            thumb_layout: Layout::default(),
            sidebar_width: 180.0,
            thumbs,
            thumb_cache: HashMap::new(),
            last_thumb_request: None,
            search: None,
            next_search_id: 0,
            annotations: Annotations::default(),
            history: History::default(),
            selection: None,
            drag: None,
            popup: Popup::None,
            note_target: None,
            recent,
            outline: Vec::new(),
            labels: Vec::new(),
            side_panel: SidePanel::Thumbs,
            side_items,
            side_pages: Vec::new(),
            passwords: HashMap::new(),
            renders_held: false,
        };
        viewer.update_recent();
        viewer
    }

    // ----- Documents -------------------------------------------------------

    /// Whether `path` is open or being opened.
    pub fn is_showing(&self, path: &Path) -> bool {
        self.path.as_deref() == Some(path) || self.pending.as_deref() == Some(path)
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    /// Starts opening a document. Returns false if the file is gone.
    pub fn open(&mut self, window: &AppWindow, path: PathBuf) -> bool {
        if !path.exists() {
            self.store.recent.retain(|r| r.path != path);
            self.save();
            self.update_recent();
            window.set_status(format!("File not found: {}", path.display()).into());
            return false;
        }
        window.set_status("".into());
        self.pending = Some(path.clone());
        let password = self.passwords.get(&path).cloned();
        self.worker.send(Request::Open { path, password });
        true
    }

    /// Retries opening an encrypted document with a password.
    pub fn open_with_password(&mut self, path: PathBuf, password: String) {
        self.passwords.insert(path.clone(), password.clone());
        self.pending = Some(path.clone());
        self.worker.send(Request::Open {
            path,
            password: Some(password),
        });
    }

    /// Whether the worker is waiting on this path, e.g. for a password.
    pub fn is_pending(&self, path: &Path) -> bool {
        self.pending.as_deref() == Some(path)
    }

    /// Back to the start screen.
    pub fn close(&mut self, window: &AppWindow) {
        self.remember_position();
        self.path = None;
        self.pending = None;
        self.title.clear();
        self.reset_document_state(window);
        self.page_sizes.clear();
        self.outline.clear();
        self.labels.clear();
        self.update_side_items();
        self.layout = Layout::default();
        self.thumb_layout = Layout::default();
        window.set_page_count(0);
        window.set_page_label("".into());
        window.set_current_page(0);
        window.set_document_title("".into());
        self.refresh(window);
        self.update_recent();
    }

    pub fn opened(
        &mut self,
        window: &AppWindow,
        title: String,
        page_sizes: Vec<(f32, f32)>,
        outline: Vec<OutlineItem>,
    ) {
        self.remember_position();
        self.path = self.pending.take();
        let restored = self
            .path
            .as_deref()
            .and_then(|path| self.store.recent_for(path))
            .cloned();

        self.title = title;
        self.page_sizes = page_sizes;
        self.outline = outline;
        self.labels.clear();
        window.set_page_label("".into());
        self.zoom = restored.as_ref().map_or(self.default_zoom, |r| r.zoom);
        self.current_page = 0;
        self.layout = Layout::default();
        self.thumb_layout = Layout::default();
        self.reset_document_state(window);
        self.annotations = self
            .path
            .as_deref()
            .map(Annotations::load)
            .unwrap_or_default();

        window.set_document_title(self.title.clone().into());
        window.set_page_count(ui_int(self.page_sizes.len()));
        window.set_current_page(0);
        window.set_status("".into());
        window.set_viewport_x(0.0);
        window.set_viewport_y(0.0);
        window.set_thumbs_viewport_y(0.0);
        self.relayout(window);

        if let Some(page) = restored.map(|r| r.page) {
            self.go_to_page(window, page.min(self.page_sizes.len().saturating_sub(1)));
        }
        self.remember_position();
        self.update_recent();
        self.update_side_items();
    }

    fn reset_document_state(&mut self, window: &AppWindow) {
        self.cache.clear();
        self.last_request = None;
        self.thumb_cache.clear();
        self.last_thumb_request = None;
        self.selection = None;
        self.drag = None;
        self.popup = Popup::None;
        self.note_target = None;
        self.annotations = Annotations::default();
        self.history.clear();
        self.close_search(window);
    }

    /// The worker couldn't open a document; returns the path it was asked for.
    pub fn open_failed(&mut self, window: &AppWindow, message: String) -> Option<PathBuf> {
        window.set_status(message.into());
        self.pending.take()
    }

    /// Saves the reading position of the open document.
    pub fn remember_position(&mut self) {
        let Some(path) = self.path.clone() else {
            return;
        };
        self.store.remember(Recent {
            path,
            page: self.current_page,
            zoom: self.zoom,
        });
        self.save();
    }

    pub fn set_default_zoom(&mut self, zoom: DefaultZoom) {
        self.default_zoom = match zoom {
            DefaultZoom::FitWidth => Zoom::FitWidth,
            DefaultZoom::FitPage => Zoom::FitPage,
        };
    }

    /// Thumbnails are laid out for the sidebar width; re-render on change.
    pub fn set_sidebar_width(&mut self, window: &AppWindow, width: f32) {
        if width == self.sidebar_width {
            return;
        }
        self.sidebar_width = width;
        self.thumb_layout = Layout::default();
        self.thumb_cache.clear();
        self.last_thumb_request = None;
        self.relayout(window);
    }

    pub fn set_inverted(&mut self, window: &AppWindow, inverted: bool) {
        self.worker.send(Request::SetInverted(inverted));
        self.cache.clear();
        self.thumb_cache.clear();
        self.last_request = None;
        self.last_thumb_request = None;
        self.refresh(window);
    }

    // ----- Rendering -------------------------------------------------------

    pub fn rendered(&mut self, window: &AppWindow, page: usize, width_px: u32, bitmap: Bitmap) {
        // A render for an outdated width still beats a blank page.
        let stale = width_px != self.target_width_px(window);
        if stale && self.cache.contains_key(&page) {
            return;
        }
        self.cache.insert(page, (width_px, to_image(&bitmap)));
        self.refresh(window);
    }

    pub fn thumbnail(&mut self, window: &AppWindow, page: usize, bitmap: Bitmap) {
        self.thumb_cache.insert(page, to_image(&bitmap));
        self.refresh_thumbs(window);
    }

    /// Recomputes page positions after a resize, zoom, or new document.
    pub fn resized(&mut self, window: &AppWindow) {
        self.renders_held = true;
        self.relayout(window);
    }

    pub fn resize_settled(&mut self, window: &AppWindow) {
        self.renders_held = false;
        self.refresh(window);
    }

    pub fn relayout(&mut self, window: &AppWindow) {
        if self.page_sizes.is_empty() {
            return;
        }
        let old_height = self.layout.content_height();
        let top = -window.get_viewport_y();

        let factor = self.zoom_factor(window);
        self.layout = Layout::new(&self.page_sizes, window.get_reader_width(), factor);
        self.update_zoom_display(window);
        window.set_content_width(self.layout.content_width());
        window.set_content_height(self.layout.content_height());

        // Keep the same relative reading position.
        if old_height > 0.0 {
            let ratio = top / old_height;
            self.scroll_to(window, ratio * self.layout.content_height());
        }
        self.clamp_x(window);

        if self.thumb_layout.page_count() != self.page_sizes.len() {
            self.thumb_layout = Layout::new(&self.page_sizes, self.sidebar_width, 1.0);
            window.set_thumbs_content_height(self.thumb_layout.content_height());
        }
        self.refresh(window);
    }

    /// Updates visible pages, the page counter, and render requests after
    /// anything changed.
    pub fn refresh(&mut self, window: &AppWindow) {
        self.update(window, true);
    }

    /// Like `refresh`, for scrolling: page items are only rebuilt when a
    /// different set of pages comes into range, so frames stay cheap.
    pub fn scrolled(&mut self, window: &AppWindow) {
        self.update(window, false);
    }

    fn update(&mut self, window: &AppWindow, force: bool) {
        let count = self.layout.page_count();
        if count == 0 {
            self.pages.set_vec(Vec::new());
            self.thumbs.set_vec(Vec::new());
            self.shown_pages = None;
            self.shown_thumbs = None;
            window.set_popup("".into());
            return;
        }
        let top = -window.get_viewport_y();
        let height = window.get_reader_height();
        let visible = self.layout.visible(top, height);
        let preload = layout::with_margin(visible.clone(), PRELOAD, count);
        let keep = layout::with_margin(visible.clone(), PRELOAD + KEEP, count);

        let current = self.layout.current_page(top, height);
        if current != self.current_page || window.get_current_page() == 0 {
            self.current_page = current;
            window.set_current_page(ui_int(current + 1));
            self.update_page_label(window);
            if self.side_panel == SidePanel::Outline {
                self.update_side_items();
            }
            self.reveal_thumb(window, current);
            // Saved on every page change so the position survives a crash.
            self.remember_position();
        }
        self.cache.retain(|page, _| keep.contains(page));
        if force || self.shown_pages.as_ref() != Some(&preload) {
            self.pages.set_vec(self.page_views(window, preload.clone()));
            self.shown_pages = Some(preload.clone());
        }
        self.request_renders(window, visible, preload);
        self.update_thumbs(window, force);
        self.update_popup(window);
    }

    pub fn refresh_thumbs(&mut self, window: &AppWindow) {
        self.update_thumbs(window, true);
    }

    pub fn thumbs_scrolled(&mut self, window: &AppWindow) {
        self.update_thumbs(window, false);
    }

    fn update_thumbs(&mut self, window: &AppWindow, force: bool) {
        let count = self.thumb_layout.page_count();
        if count == 0 || !window.get_sidebar_visible() {
            return;
        }
        let top = -window.get_thumbs_viewport_y();
        let visible = self.thumb_layout.visible(top, window.get_reader_height());
        let preload = layout::with_margin(visible.clone(), THUMB_PRELOAD, count);
        let keep = layout::with_margin(visible.clone(), THUMB_KEEP, count);

        self.thumb_cache.retain(|page, _| keep.contains(page));
        if !force && self.shown_thumbs.as_ref() == Some(&preload) {
            return;
        }
        self.shown_thumbs = Some(preload.clone());
        let views: Vec<ThumbView> = preload
            .clone()
            .filter_map(|page| {
                let rect = self.thumb_layout.rect(page)?;
                let image = self.thumb_cache.get(&page).cloned();
                Some(ThumbView {
                    page: ui_int(page),
                    x: rect.x,
                    y: rect.y,
                    width: rect.width,
                    height: rect.height,
                    loaded: image.is_some(),
                    image: image.unwrap_or_default(),
                })
            })
            .collect();
        self.thumbs.set_vec(views);

        if self.last_thumb_request.as_ref() != Some(&preload) {
            let width = self.thumb_layout.rect(0).map_or(1.0, |r| r.width);
            let width_px = (width * window.window().scale_factor()).round().max(1.0) as u32;
            let pages = visible
                .clone()
                .chain(preload.clone().filter(|p| !visible.contains(p)))
                .filter(|p| !self.thumb_cache.contains_key(p))
                .collect();
            self.worker.send(Request::Thumbnails { pages, width_px });
            self.last_thumb_request = Some(preload);
        }
    }

    pub fn sidebar_shown(&mut self, window: &AppWindow) {
        self.last_thumb_request = None;
        self.refresh_thumbs(window);
    }

    // ----- Actions ---------------------------------------------------------

    /// Handles document actions. Returns false for actions it doesn't own.
    pub fn handle(&mut self, window: &AppWindow, action: Action) -> bool {
        if self.page_sizes.is_empty() {
            return false;
        }
        match action {
            Action::ZoomIn => {
                let factor = self.zoom_factor(window);
                self.set_zoom(window, Zoom::zoom_in(factor));
            }
            Action::ZoomOut => {
                let factor = self.zoom_factor(window);
                self.set_zoom(window, Zoom::zoom_out(factor));
            }
            Action::ZoomReset | Action::FitWidth => self.set_zoom(window, Zoom::FitWidth),
            Action::FitPage => self.set_zoom(window, Zoom::FitPage),
            Action::ScrollDown => self.scroll_by(window, 0.0, SCROLL_STEP),
            Action::ScrollUp => self.scroll_by(window, 0.0, -SCROLL_STEP),
            Action::ScrollLeft => self.scroll_by(window, -SCROLL_STEP, 0.0),
            Action::ScrollRight => self.scroll_by(window, SCROLL_STEP, 0.0),
            Action::HalfPageDown => self.scroll_by(window, 0.0, window.get_reader_height() / 2.0),
            Action::HalfPageUp => self.scroll_by(window, 0.0, -window.get_reader_height() / 2.0),
            Action::NextMatch => self.search_step(window, true),
            Action::PreviousMatch => self.search_step(window, false),
            Action::PreviousPage | Action::NextPage | Action::FirstPage | Action::LastPage => {
                if let Some(page) =
                    nav::target_page(action, self.current_page, self.layout.page_count())
                {
                    self.go_to_page(window, page);
                }
            }
            _ => return false,
        }
        true
    }

    pub fn page_count(&self) -> usize {
        self.page_sizes.len()
    }

    pub fn current_page(&self) -> usize {
        self.current_page
    }

    pub fn go_to_typed_page(&mut self, window: &AppWindow, text: &str) {
        let text = text.trim();
        // A label like "iv" or "A-3" wins over a page number.
        let labelled = self
            .labels
            .iter()
            .position(|label| !label.is_empty() && label.eq_ignore_ascii_case(text));
        if let Some(page) = labelled.or_else(|| nav::parse_page(text, self.page_sizes.len())) {
            self.go_to_page(window, page);
        }
    }

    pub fn go_to_page(&mut self, window: &AppWindow, page: usize) {
        self.scroll_to(window, self.layout.scroll_to(page));
        self.refresh(window);
    }

    fn set_zoom(&mut self, window: &AppWindow, zoom: Zoom) {
        if zoom == self.zoom {
            return;
        }
        // Relayout moves the current page, so remember it first.
        let page = self.current_page;
        self.zoom = zoom;
        self.relayout(window);
        if zoom == Zoom::FitPage {
            self.go_to_page(window, page);
        }
        self.remember_position();
    }

    /// Scale relative to fit width for the current zoom mode.
    fn zoom_factor(&self, window: &AppWindow) -> f32 {
        let page = self
            .page_sizes
            .get(self.current_page)
            .or(self.page_sizes.first())
            .copied()
            .unwrap_or((1.0, 1.0));
        let viewport = (window.get_reader_width(), window.get_reader_height());
        self.zoom.factor(page, viewport)
    }

    fn update_zoom_display(&self, window: &AppWindow) {
        let width_px = self.layout.rect(0).map_or(0.0, |r| r.width);
        let width_pt = self.page_sizes.first().map_or(0.0, |s| s.0);
        window.set_zoom_percent(layout::percent(width_px, width_pt));
        window.set_zoom_mode(
            match self.zoom {
                Zoom::FitWidth => "fit-width",
                Zoom::FitPage => "fit-page",
                Zoom::Custom(_) => "custom",
            }
            .into(),
        );
    }

    fn scroll_to(&self, window: &AppWindow, top: f32) {
        let max = (self.layout.content_height() - window.get_reader_height()).max(0.0);
        window.set_viewport_y(-top.clamp(0.0, max));
    }

    fn scroll_by(&mut self, window: &AppWindow, dx: f32, dy: f32) {
        if dy != 0.0 {
            self.scroll_to(window, -window.get_viewport_y() + dy);
        }
        if dx != 0.0 {
            window.set_viewport_x(window.get_viewport_x() - dx);
            self.clamp_x(window);
        }
        self.refresh(window);
    }

    fn clamp_x(&self, window: &AppWindow) {
        let max = (self.layout.content_width() - window.get_reader_width()).max(0.0);
        window.set_viewport_x(-(-window.get_viewport_x()).clamp(0.0, max));
    }

    /// Scrolls the sidebar so the current page's thumbnail is visible.
    fn reveal_thumb(&self, window: &AppWindow, page: usize) {
        let Some(rect) = self.thumb_layout.rect(page) else {
            return;
        };
        let height = window.get_reader_height();
        let top = -window.get_thumbs_viewport_y();
        if rect.y >= top && rect.y + rect.height <= top + height {
            return;
        }
        let max = (self.thumb_layout.content_height() - height).max(0.0);
        let target = rect.y - (height - rect.height) / 2.0;
        window.set_thumbs_viewport_y(-target.clamp(0.0, max));
    }

    // ----- Side panel, labels, links ----------------------------------------

    pub fn side_panel(&self) -> SidePanel {
        self.side_panel
    }

    pub fn set_side_panel(&mut self, window: &AppWindow, panel: SidePanel) {
        self.side_panel = panel;
        window.set_side_panel(panel.name().into());
        self.update_side_items();
        if panel == SidePanel::Thumbs {
            self.shown_thumbs = None;
            self.update_thumbs(window, true);
        }
    }

    fn update_side_items(&mut self) {
        let (items, pages): (Vec<SideItem>, Vec<usize>) = match self.side_panel {
            SidePanel::Thumbs => Default::default(),
            SidePanel::Outline => {
                // The current section starts nearest before the page.
                let current = self
                    .outline
                    .iter()
                    .enumerate()
                    .filter_map(|(i, item)| Some((i, item.page?)))
                    .filter(|(_, page)| *page <= self.current_page)
                    .max_by_key(|(_, page)| *page)
                    .map(|(i, _)| i);
                self.outline
                    .iter()
                    .enumerate()
                    .filter_map(|(i, item)| {
                        let page = item.page?;
                        let row = SideItem {
                            title: item.title.as_str().into(),
                            detail: "".into(),
                            depth: ui_int(item.depth.min(6)),
                            current: current == Some(i),
                            ..Default::default()
                        };
                        Some((row, page))
                    })
                    .unzip()
            }
            SidePanel::Notes => {
                let mut notes: Vec<_> = self.annotations.items.iter().collect();
                notes.sort_by(|a, b| {
                    a.page.cmp(&b.page).then_with(|| {
                        let y = |n: &annotations::Annotation| n.rects.first().map_or(0.0, |r| r.y);
                        y(a).total_cmp(&y(b))
                    })
                });
                notes
                    .into_iter()
                    .map(|a| {
                        let page = self.page_name(a.page);
                        let detail = if a.note.is_empty() {
                            format!("p. {page}")
                        } else {
                            format!("p. {page} · {}", truncate(&a.note, 120))
                        };
                        let row = SideItem {
                            title: truncate(&a.text, 140).into(),
                            detail: detail.into(),
                            depth: 0,
                            color: to_color(annotations::color_hex(&a.color)),
                            has_color: true,
                            current: false,
                        };
                        (row, a.page)
                    })
                    .unzip()
            }
        };
        self.side_items.set_vec(items);
        self.side_pages = pages;
    }

    pub fn side_item_clicked(&mut self, window: &AppWindow, index: usize) {
        if let Some(&page) = self.side_pages.get(index) {
            self.go_to_page(window, page);
        }
    }

    pub fn labels_found(&mut self, window: &AppWindow, labels: Vec<String>) {
        if labels.len() == self.page_sizes.len() {
            self.labels = labels;
            self.update_page_label(window);
            self.update_side_items();
        }
    }

    /// The label of a page if it has one, else its number.
    fn page_name(&self, page: usize) -> String {
        match self.labels.get(page) {
            Some(label) if !label.is_empty() => label.clone(),
            _ => (page + 1).to_string(),
        }
    }

    fn update_page_label(&self, window: &AppWindow) {
        let name = self.page_name(self.current_page);
        let shown = if name == (self.current_page + 1).to_string() {
            String::new()
        } else {
            name
        };
        window.set_page_label(shown.into());
    }

    /// Follows a clicked link. Returns an external URI for the caller to open.
    pub fn link(&mut self, window: &AppWindow, target: Option<LinkTarget>) -> Option<String> {
        match target? {
            LinkTarget::Page(page) if page < self.page_sizes.len() => {
                self.go_to_page(window, page);
                None
            }
            LinkTarget::Page(_) => None,
            LinkTarget::Uri(uri) => Some(uri),
        }
    }

    // ----- Search ----------------------------------------------------------

    pub fn search_edited(&mut self, window: &AppWindow, query: &str) {
        self.next_search_id += 1;
        let id = self.next_search_id;
        let query = query.trim().to_owned();
        self.search = (!query.is_empty()).then(|| SearchState::new(id, self.current_page));
        window.set_search_status("".into());
        self.worker.send(Request::Search { id, query });
        self.refresh(window);
    }

    pub fn search_step(&mut self, window: &AppWindow, forward: bool) {
        let Some(search) = &mut self.search else {
            return;
        };
        if forward {
            search.next();
        } else {
            search.previous();
        }
        self.reveal_match(window);
    }

    pub fn search_hits(
        &mut self,
        window: &AppWindow,
        id: u64,
        page: usize,
        matches: Vec<Vec<NormRect>>,
    ) {
        let Some(search) = self.search.as_mut().filter(|s| s.id == id) else {
            return;
        };
        if search.add(page, matches) {
            self.reveal_match(window);
        } else {
            self.update_search_status(window);
            self.refresh(window);
        }
    }

    pub fn search_done(&mut self, window: &AppWindow, id: u64) {
        let Some(search) = self.search.as_mut().filter(|s| s.id == id) else {
            return;
        };
        if search.finish() {
            self.reveal_match(window);
        } else {
            self.update_search_status(window);
        }
    }

    pub fn close_search(&mut self, window: &AppWindow) {
        if self.search.take().is_some() {
            self.next_search_id += 1;
            self.worker.send(Request::Search {
                id: self.next_search_id,
                query: String::new(),
            });
        }
        window.set_search_text("".into());
        window.set_search_status("".into());
        self.refresh(window);
    }

    /// Scrolls the current match into view if it isn't already.
    fn reveal_match(&mut self, window: &AppWindow) {
        self.update_search_status(window);
        let target = self
            .search
            .as_ref()
            .and_then(|s| s.current())
            .and_then(|m| {
                let page = self.layout.rect(m.page)?;
                let first = m.rects.first()?;
                Some(page.y + first.y * page.height)
            });
        if let Some(y) = target {
            let top = -window.get_viewport_y();
            let height = window.get_reader_height();
            let margin = 48.0;
            if y < top + margin || y > top + height - margin {
                self.scroll_to(window, y - height / 3.0);
            }
        }
        self.refresh(window);
    }

    fn update_search_status(&self, window: &AppWindow) {
        let status = self.search.as_ref().map(|s| s.status()).unwrap_or_default();
        window.set_search_status(status.into());
    }

    // ----- Selection and annotations --------------------------------------

    /// Mouse input on the reading area, in reader coordinates.
    pub fn pointer(&mut self, window: &AppWindow, kind: &str, x: f32, y: f32) {
        match kind {
            "down" => {
                self.drag = self.page_point(window, x, y).map(|(page, from)| Drag {
                    page,
                    from,
                    start: (x, y),
                    dragging: false,
                });
                if self.selection.take().is_some() || self.popup != Popup::None {
                    self.popup = Popup::None;
                    self.refresh(window);
                }
            }
            "move" => {
                let Some(drag) = &mut self.drag else {
                    return;
                };
                let moved = (x - drag.start.0).abs().max((y - drag.start.1).abs());
                if !drag.dragging && moved < DRAG_THRESHOLD {
                    return;
                }
                drag.dragging = true;
                let (page, from) = (drag.page, drag.from);
                let to = self.point_on_page(window, page, x, y);
                self.worker.send(Request::Select { page, from, to });
            }
            "up" => {
                let Some(drag) = self.drag.take() else {
                    return;
                };
                if drag.dragging {
                    let to = self.point_on_page(window, drag.page, x, y);
                    self.worker.send(Request::Select {
                        page: drag.page,
                        from: drag.from,
                        to,
                    });
                    if self.selection.is_some() {
                        self.popup = Popup::Selection;
                    }
                } else if let Some(id) =
                    self.annotations
                        .at_point(drag.page, drag.from.0, drag.from.1)
                {
                    self.popup = Popup::Annotation(id);
                    window.set_annotation_has_note(
                        self.annotations.get(id).is_some_and(|a| !a.note.is_empty()),
                    );
                } else {
                    self.worker.send(Request::LinkAt {
                        page: drag.page,
                        point: drag.from,
                    });
                }
                self.refresh(window);
            }
            _ => {}
        }
    }

    pub fn selected(
        &mut self,
        window: &AppWindow,
        page: usize,
        rects: Vec<NormRect>,
        text: String,
    ) {
        self.selection = (!rects.is_empty()).then_some(Selection { page, rects, text });
        // The final selection of a finished drag shows the toolbar.
        if self.drag.is_none() && self.selection.is_some() {
            self.popup = Popup::Selection;
        }
        self.refresh(window);
    }

    /// Clears the selection or popup. Returns whether there was one.
    pub fn clear_selection(&mut self, window: &AppWindow) -> bool {
        let had = self.selection.take().is_some() || self.popup != Popup::None;
        self.popup = Popup::None;
        if had {
            self.refresh(window);
        }
        had
    }

    /// Runs a selection- or annotation-toolbar button.
    pub fn popup_action(&mut self, window: &AppWindow, action: &str, index: usize) -> PopupOutcome {
        let color = annotations::COLORS
            .get(index)
            .map_or(annotations::COLORS[0].0, |(name, _)| name);
        let outcome = match (self.popup, action) {
            (Popup::Selection, "highlight") => {
                self.annotate(Kind::Highlight, color);
                PopupOutcome::Nothing
            }
            (Popup::Selection, "underline") => {
                self.annotate(Kind::Underline, "blue");
                PopupOutcome::Nothing
            }
            (Popup::Selection, "strike") => {
                self.annotate(Kind::Strikethrough, "red");
                PopupOutcome::Nothing
            }
            (Popup::Selection, "note") => match self.annotate(Kind::Highlight, "yellow") {
                Some(id) => self.edit_note(id),
                None => PopupOutcome::Nothing,
            },
            (Popup::Selection, "copy") => self.copy(),
            (Popup::Annotation(id), _) => self.annotation_action(id, action, color),
            _ => PopupOutcome::Nothing,
        };
        if !matches!(outcome, PopupOutcome::EditNote { .. }) {
            self.popup = Popup::None;
        }
        self.save_annotations(window);
        self.refresh(window);
        outcome
    }

    fn annotation_action(&mut self, id: u64, action: &str, color: &str) -> PopupOutcome {
        match action {
            "delete" => {
                self.history.record(&self.annotations.items);
                self.annotations.remove(id);
            }
            "note" => return self.edit_note(id),
            "copy" => {
                let text = self.annotations.get(id).map(|a| a.text.clone());
                return text.map_or(PopupOutcome::Nothing, |t| copy_text(&t));
            }
            _ => {
                self.history.record(&self.annotations.items);
                if let Some(a) = self.annotations.get_mut(id) {
                    match action {
                        "color" => a.color = color.to_owned(),
                        "underline" => a.kind = Kind::Underline,
                        "strike" => a.kind = Kind::Strikethrough,
                        _ => {}
                    }
                }
            }
        }
        PopupOutcome::Nothing
    }

    /// Keyboard shortcuts for the selection (Copy, Highlight, AddNote).
    pub fn selection_action(&mut self, window: &AppWindow, action: Action) -> PopupOutcome {
        if self.selection.is_none() {
            if let (Popup::Annotation(_), Action::Copy) = (self.popup, action) {
                return self.popup_action(window, "copy", 0);
            }
            return PopupOutcome::Nothing;
        }
        self.popup = Popup::Selection;
        match action {
            Action::Copy => self.popup_action(window, "copy", 0),
            Action::Highlight => self.popup_action(window, "highlight", 0),
            Action::AddNote => self.popup_action(window, "note", 0),
            _ => PopupOutcome::Nothing,
        }
    }

    fn annotate(&mut self, kind: Kind, color: &str) -> Option<u64> {
        let selection = self.selection.take()?;
        self.history.record(&self.annotations.items);
        Some(self.annotations.add(
            selection.page,
            kind,
            color,
            selection.text,
            selection.rects,
            annotations::now(),
        ))
    }

    fn edit_note(&mut self, id: u64) -> PopupOutcome {
        let Some(a) = self.annotations.get(id) else {
            return PopupOutcome::Nothing;
        };
        self.note_target = Some(id);
        self.popup = Popup::None;
        PopupOutcome::EditNote {
            quote: a.text.clone(),
            note: a.note.clone(),
        }
    }

    fn copy(&mut self) -> PopupOutcome {
        let text = self.selection.as_ref().map(|s| s.text.clone());
        self.selection = None;
        text.map_or(PopupOutcome::Nothing, |t| copy_text(&t))
    }

    pub fn note_saved(&mut self, window: &AppWindow, note: &str) {
        let note = note.trim();
        let target = self.note_target.take();
        let changed = target
            .and_then(|id| self.annotations.get(id))
            .is_some_and(|a| a.note != note);
        if changed {
            self.history.record(&self.annotations.items);
            if let Some(a) = target.and_then(|id| self.annotations.get_mut(id)) {
                a.note = note.to_owned();
            }
        }
        self.save_annotations(window);
        self.refresh(window);
    }

    /// Undoes (or redoes) the last annotation change. Returns false if
    /// there was nothing to undo.
    pub fn undo(&mut self, window: &AppWindow, redo: bool) -> bool {
        let restored = if redo {
            self.history.redo(&self.annotations.items)
        } else {
            self.history.undo(&self.annotations.items)
        };
        let Some(items) = restored else {
            return false;
        };
        self.annotations.items = items;
        self.selection = None;
        self.popup = Popup::None;
        self.save_annotations(window);
        self.refresh(window);
        true
    }

    pub fn note_cancelled(&mut self) {
        self.note_target = None;
    }

    fn save_annotations(&mut self, window: &AppWindow) {
        if self.side_panel == SidePanel::Notes {
            self.update_side_items();
        }
        let Some(path) = self.path.clone() else {
            return;
        };
        self.annotations.document = path.display().to_string();
        if let Err(err) = self.annotations.save(&path) {
            window.set_status(format!("Couldn't save notes: {err}").into());
        }
    }

    /// The document's notes as Markdown, if it has any.
    pub fn notes_markdown(&self) -> Option<String> {
        (!self.annotations.items.is_empty()).then(|| {
            self.annotations
                .to_markdown(&self.title, annotations::now())
        })
    }

    /// Converts a reader point to a page and a point in page fractions.
    fn page_point(&self, window: &AppWindow, x: f32, y: f32) -> Option<(usize, (f32, f32))> {
        let cx = x - window.get_viewport_x();
        let cy = y - window.get_viewport_y();
        let page = self.layout.visible(cy, 0.01).start;
        let rect = self.layout.rect(page)?;
        let inside =
            cx >= rect.x && cx <= rect.x + rect.width && cy >= rect.y && cy <= rect.y + rect.height;
        inside.then(|| {
            (
                page,
                ((cx - rect.x) / rect.width, (cy - rect.y) / rect.height),
            )
        })
    }

    /// A reader point in fractions of `page`, clamped to the page.
    fn point_on_page(&self, window: &AppWindow, page: usize, x: f32, y: f32) -> (f32, f32) {
        let Some(rect) = self.layout.rect(page) else {
            return (0.0, 0.0);
        };
        let cx = x - window.get_viewport_x();
        let cy = y - window.get_viewport_y();
        (
            ((cx - rect.x) / rect.width).clamp(0.0, 1.0),
            ((cy - rect.y) / rect.height).clamp(0.0, 1.0),
        )
    }

    /// Positions the toolbar above the selection or annotation.
    fn update_popup(&self, window: &AppWindow) {
        let anchor = match self.popup {
            Popup::None => None,
            Popup::Selection => self
                .selection
                .as_ref()
                .and_then(|s| Some((s.page, *s.rects.first()?))),
            Popup::Annotation(id) => self
                .annotations
                .get(id)
                .and_then(|a| Some((a.page, *a.rects.first()?))),
        };
        let Some((page, r)) = anchor else {
            window.set_popup("".into());
            return;
        };
        let Some(rect) = self.layout.rect(page) else {
            return;
        };
        let x = rect.x + (r.x + r.width / 2.0) * rect.width + window.get_viewport_x();
        let y = rect.y + r.y * rect.height + window.get_viewport_y();
        window.set_popup_x(x);
        window.set_popup_y(y);
        window.set_popup(
            match self.popup {
                Popup::Selection => "selection",
                _ => "annotation",
            }
            .into(),
        );
    }

    // ----- Helpers ---------------------------------------------------------

    fn save(&self) {
        // Losing a reading position is not worth interrupting the reader for.
        let _ = self.store.save();
    }

    pub fn update_recent(&self) {
        let items: Vec<RecentItem> = self
            .store
            .recent
            .iter()
            .take(RECENT_SHOWN)
            .map(|r| RecentItem {
                title: file_name(&r.path).into(),
                folder: r
                    .path
                    .parent()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default()
                    .into(),
            })
            .collect();
        self.recent.set_vec(items);
    }

    /// Physical pixel width pages should be rendered at to look sharp.
    fn target_width_px(&self, window: &AppWindow) -> u32 {
        let page_width = self.layout.rect(0).map_or(0.0, |r| r.width);
        (page_width * window.window().scale_factor())
            .round()
            .max(1.0) as u32
    }

    fn page_views(&self, window: &AppWindow, pages: Range<usize>) -> Vec<PageView> {
        let scale = window.window().scale_factor();
        // Snap to physical pixels so a 1:1 render isn't resampled into blur.
        let snap = |v: f32| (v * scale).round() / scale;
        pages
            .filter_map(|page| {
                let rect = self.layout.rect(page)?;
                let image = self.cache.get(&page).map(|(_, img)| img.clone());
                Some(PageView {
                    x: snap(rect.x),
                    y: snap(rect.y),
                    width: snap(rect.width),
                    height: snap(rect.height),
                    loaded: image.is_some(),
                    image: image.unwrap_or_default(),
                    highlights: self.highlights(page),
                    marks: self.marks(page),
                })
            })
            .collect()
    }

    fn highlights(&self, page: usize) -> ModelRc<Highlight> {
        let Some(search) = &self.search else {
            return ModelRc::default();
        };
        let items: Vec<Highlight> = search
            .on_page(page)
            .map(|(r, current)| Highlight {
                x: r.x,
                y: r.y,
                width: r.width,
                height: r.height,
                current,
            })
            .collect();
        model_or_default(items)
    }

    fn marks(&self, page: usize) -> ModelRc<Mark> {
        let mut items: Vec<Mark> = Vec::new();
        for a in self.annotations.on_page(page) {
            let color = to_color(annotations::color_hex(&a.color));
            let kind = match a.kind {
                Kind::Highlight => 0,
                Kind::Underline => 1,
                Kind::Strikethrough => 2,
            };
            let last = a.rects.len().saturating_sub(1);
            items.extend(a.rects.iter().enumerate().map(|(i, r)| Mark {
                x: r.x,
                y: r.y,
                width: r.width,
                height: r.height,
                kind,
                color,
                note: i == last && !a.note.is_empty(),
            }));
        }
        if let Some(s) = self.selection.as_ref().filter(|s| s.page == page) {
            items.extend(s.rects.iter().map(|r| Mark {
                x: r.x,
                y: r.y,
                width: r.width,
                height: r.height,
                kind: 3,
                color: Color::default(),
                note: false,
            }));
        }
        model_or_default(items)
    }

    fn request_renders(
        &mut self,
        window: &AppWindow,
        visible: Range<usize>,
        preload: Range<usize>,
    ) {
        if self.renders_held {
            return;
        }
        let width_px = self.target_width_px(window);
        let key = (preload.clone(), width_px);
        if self.last_request.as_ref() == Some(&key) {
            return;
        }
        // Visible pages first, then the ones just outside the viewport.
        let pages: Vec<usize> = visible
            .clone()
            .chain(preload.filter(|p| !visible.contains(p)))
            .filter(|p| self.cache.get(p).map(|(w, _)| *w) != Some(width_px))
            .collect();
        self.last_request = Some(key);
        self.worker.send(Request::Render { pages, width_px });
    }
}

fn copy_text(text: &str) -> PopupOutcome {
    match clipboard_win::set_clipboard_string(text) {
        Ok(()) => PopupOutcome::Copied,
        Err(_) => PopupOutcome::Nothing,
    }
}

fn model_or_default<T: Clone + 'static>(items: Vec<T>) -> ModelRc<T> {
    if items.is_empty() {
        ModelRc::default()
    } else {
        Rc::new(VecModel::from(items)).into()
    }
}

/// Shortens text to `max` characters, on a char boundary, with an ellipsis.
fn truncate(text: &str, max: usize) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match text.char_indices().nth(max) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text,
    }
}

pub fn to_color(hex: &str) -> Color {
    parse_color(hex).map_or(Color::default(), |[r, g, b, a]| {
        Color::from_argb_u8(a, r, g, b)
    })
}

fn to_image(bitmap: &Bitmap) -> Image {
    let buffer = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
        &bitmap.rgba,
        bitmap.width,
        bitmap.height,
    );
    Image::from_rgba8(buffer)
}

pub fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

pub fn ui_int(value: usize) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}
