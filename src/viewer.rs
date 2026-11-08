//! UI glue: owns the application state on the UI thread, turns layout,
//! render results, and search hits into Slint properties, and decides which
//! pages and thumbnails to render or evict.

use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use slint::{ComponentHandle, Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, VecModel};

use crate::keymap::Action;
use crate::layout::{self, Layout, Zoom};
use crate::nav;
use crate::pdf::{Bitmap, NormRect, Request, Worker};
use crate::search::SearchState;
use crate::store::{Recent, Store, Theme as ThemeSetting};
use crate::{AppWindow, Highlight, PageView, RecentItem, Theme, ThumbView};

/// Pages rendered ahead of and behind the visible ones.
const PRELOAD: usize = 2;
/// Rendered pages kept beyond the preload window before being dropped.
const KEEP: usize = 4;
/// Thumbnails rendered around the visible ones, and kept beyond that.
const THUMB_PRELOAD: usize = 6;
const THUMB_KEEP: usize = 20;
/// Must match `Theme.sidebar-width`.
const SIDEBAR_WIDTH: f32 = 180.0;
/// Recent documents listed on the start screen.
const RECENT_SHOWN: usize = 8;

pub struct Viewer {
    worker: Worker,
    store: Store,

    path: Option<PathBuf>,
    /// A document requested but not yet opened by the worker.
    pending: Option<PathBuf>,
    page_sizes: Vec<(f32, f32)>,
    layout: Layout,
    zoom: Zoom,
    current_page: usize,
    pages: Rc<VecModel<PageView>>,
    /// Rendered pages and the pixel width they were rendered at.
    cache: HashMap<usize, (u32, Image)>,
    last_request: Option<(Range<usize>, u32)>,

    thumb_layout: Layout,
    thumbs: Rc<VecModel<ThumbView>>,
    thumb_cache: HashMap<usize, Image>,
    last_thumb_request: Option<Range<usize>>,

    search: Option<SearchState>,
    next_search_id: u64,

    recent: Rc<VecModel<RecentItem>>,
}

impl Viewer {
    pub fn new(window: &AppWindow, worker: Worker, store: Store) -> Self {
        let pages = Rc::new(VecModel::default());
        let thumbs = Rc::new(VecModel::default());
        let recent = Rc::new(VecModel::default());
        window.set_pages(pages.clone().into());
        window.set_thumbs(thumbs.clone().into());
        window.set_recent(recent.clone().into());

        let viewer = Self {
            worker,
            store,
            path: None,
            pending: None,
            page_sizes: Vec::new(),
            layout: Layout::default(),
            zoom: Zoom::FitWidth,
            current_page: 0,
            pages,
            cache: HashMap::new(),
            last_request: None,
            thumb_layout: Layout::default(),
            thumbs,
            thumb_cache: HashMap::new(),
            last_thumb_request: None,
            search: None,
            next_search_id: 0,
            recent,
        };
        viewer.apply_settings(window);
        viewer.update_recent();
        window.set_sidebar_visible(viewer.store.sidebar);
        viewer
    }

    // ----- Documents -------------------------------------------------------

    pub fn open(&mut self, window: &AppWindow, path: PathBuf) {
        if !path.exists() {
            self.store.recent.retain(|r| r.path != path);
            self.save();
            self.update_recent();
            window.set_status(format!("File not found: {}", path.display()).into());
            return;
        }
        window.set_status("".into());
        self.pending = Some(path.clone());
        self.worker.send(Request::Open(path));
    }

    pub fn open_recent(&mut self, window: &AppWindow, index: usize) {
        if let Some(path) = self.store.recent.get(index).map(|r| r.path.clone()) {
            self.open(window, path);
        }
    }

    pub fn opened(&mut self, window: &AppWindow, title: String, page_sizes: Vec<(f32, f32)>) {
        self.remember_position();
        self.path = self.pending.take();
        let restored = self
            .path
            .as_deref()
            .and_then(|path| self.store.recent_for(path))
            .cloned();

        self.page_sizes = page_sizes;
        self.zoom = restored.as_ref().map_or(Zoom::FitWidth, |r| r.zoom);
        self.current_page = 0;
        self.layout = Layout::default();
        self.cache.clear();
        self.last_request = None;
        self.thumb_cache.clear();
        self.last_thumb_request = None;
        self.close_search(window);
        if window.get_overlay() != "settings" {
            self.set_overlay(window, "");
        }

        window.set_document_title(title.into());
        window.set_page_count(ui_int(self.page_sizes.len()));
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
    }

    pub fn open_failed(&mut self, window: &AppWindow, message: String) {
        self.pending = None;
        window.set_status(message.into());
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
    pub fn relayout(&mut self, window: &AppWindow) {
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

        if self.thumb_layout.page_count() != self.page_sizes.len() {
            self.thumb_layout = Layout::new(&self.page_sizes, SIDEBAR_WIDTH, 1.0);
            window.set_thumbs_content_height(self.thumb_layout.content_height());
        }
        self.refresh(window);
    }

    /// Updates visible pages, the page counter, and render requests.
    pub fn refresh(&mut self, window: &AppWindow) {
        let count = self.layout.page_count();
        if count == 0 {
            self.pages.set_vec(Vec::new());
            self.thumbs.set_vec(Vec::new());
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
            self.reveal_thumb(window, current);
            // Saved on every page change so the position survives a crash.
            self.remember_position();
        }
        self.cache.retain(|page, _| keep.contains(page));
        self.pages.set_vec(self.page_views(window, preload.clone()));
        self.request_renders(window, visible, preload);
        self.refresh_thumbs(window);
    }

    pub fn refresh_thumbs(&mut self, window: &AppWindow) {
        let count = self.thumb_layout.page_count();
        if count == 0 || !window.get_sidebar_visible() {
            return;
        }
        let top = -window.get_thumbs_viewport_y();
        let visible = self.thumb_layout.visible(top, window.get_reader_height());
        let preload = layout::with_margin(visible.clone(), THUMB_PRELOAD, count);
        let keep = layout::with_margin(visible.clone(), THUMB_KEEP, count);

        self.thumb_cache.retain(|page, _| keep.contains(page));
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

    // ----- Actions ---------------------------------------------------------

    pub fn handle(&mut self, window: &AppWindow, action: Action) {
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
            Action::Search => self.set_overlay(window, "search"),
            Action::GoToPage if !self.page_sizes.is_empty() => {
                window.set_goto_text(ui_int(self.current_page + 1).to_string().into());
                self.set_overlay(window, "goto");
            }
            Action::Settings => {
                let next = if window.get_overlay() == "settings" {
                    ""
                } else {
                    "settings"
                };
                self.set_overlay(window, next);
            }
            Action::ToggleSidebar => {
                self.store.sidebar = !window.get_sidebar_visible();
                window.set_sidebar_visible(self.store.sidebar);
                self.save();
                self.last_thumb_request = None;
                self.refresh_thumbs(window);
            }
            Action::Fullscreen => {
                let fullscreen = !window.get_fullscreen();
                window.set_fullscreen(fullscreen);
                window.window().set_fullscreen(fullscreen);
            }
            Action::Escape => {
                if window.get_overlay() == "search" {
                    self.close_search(window);
                }
                if window.get_overlay() != "" {
                    self.set_overlay(window, "");
                } else if window.get_fullscreen() {
                    window.set_fullscreen(false);
                    window.window().set_fullscreen(false);
                }
            }
            _ => {
                if let Some(page) =
                    nav::target_page(action, self.current_page, self.layout.page_count())
                {
                    self.go_to_page(window, page);
                }
            }
        }
    }

    pub fn go_to_typed_page(&mut self, window: &AppWindow, text: &str) {
        if let Some(page) = nav::parse_page(text, self.page_sizes.len()) {
            self.go_to_page(window, page);
        }
        self.set_overlay(window, "");
    }

    pub fn go_to_page(&mut self, window: &AppWindow, page: usize) {
        self.scroll_to(window, self.layout.scroll_to(page));
        self.refresh(window);
    }

    fn set_zoom(&mut self, window: &AppWindow, zoom: Zoom) {
        if self.page_sizes.is_empty() || zoom == self.zoom {
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

    fn set_overlay(&self, window: &AppWindow, overlay: &str) {
        window.set_overlay(overlay.into());
        window.invoke_focus_overlay();
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

    fn close_search(&mut self, window: &AppWindow) {
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

    // ----- Settings --------------------------------------------------------

    pub fn toggle_theme(&mut self, window: &AppWindow) {
        self.store.theme = match self.store.theme {
            ThemeSetting::Dark => ThemeSetting::Light,
            ThemeSetting::Light => ThemeSetting::Dark,
        };
        self.apply_settings(window);
        self.save();
    }

    pub fn toggle_sidebar_default(&mut self, window: &AppWindow) {
        self.handle(window, Action::ToggleSidebar);
        self.apply_settings(window);
    }

    fn apply_settings(&self, window: &AppWindow) {
        let light = self.store.theme == ThemeSetting::Light;
        window.global::<Theme>().set_dark(!light);
        window.set_light_theme(light);
        window.set_sidebar_default(self.store.sidebar);
    }

    fn save(&self) {
        // Losing a reading position is not worth interrupting the reader for.
        let _ = self.store.save();
    }

    fn update_recent(&self) {
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

    // ----- Helpers ---------------------------------------------------------

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
        if items.is_empty() {
            ModelRc::default()
        } else {
            Rc::new(VecModel::from(items)).into()
        }
    }

    fn request_renders(
        &mut self,
        window: &AppWindow,
        visible: Range<usize>,
        preload: Range<usize>,
    ) {
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

fn to_image(bitmap: &Bitmap) -> Image {
    let buffer = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
        &bitmap.rgba,
        bitmap.width,
        bitmap.height,
    );
    Image::from_rgba8(buffer)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

pub fn ui_int(value: usize) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}
