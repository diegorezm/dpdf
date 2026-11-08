//! UI glue for the continuous page view: turns layout and render results
//! into Slint properties, and decides which pages to render or evict.

use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;

use slint::{ComponentHandle, Image, Rgba8Pixel, SharedPixelBuffer, VecModel};

use crate::keymap::Action;
use crate::layout::{self, Layout};
use crate::nav;
use crate::pdf::{Bitmap, Request, Worker};
use crate::{AppWindow, PageView};

/// Pages rendered ahead of and behind the visible ones.
const PRELOAD: usize = 2;
/// Rendered pages kept beyond the preload window before being dropped.
const KEEP: usize = 4;

pub struct Viewer {
    worker: Worker,
    pages: Rc<VecModel<PageView>>,
    page_sizes: Vec<(f32, f32)>,
    layout: Layout,
    zoom: f32,
    /// Rendered pages and the pixel width they were rendered at.
    cache: HashMap<usize, (u32, Image)>,
    last_request: Option<(Range<usize>, u32)>,
}

impl Viewer {
    pub fn new(window: &AppWindow, worker: Worker) -> Self {
        let pages = Rc::new(VecModel::default());
        window.set_pages(pages.clone().into());
        Self {
            worker,
            pages,
            page_sizes: Vec::new(),
            layout: Layout::default(),
            zoom: 1.0,
            cache: HashMap::new(),
            last_request: None,
        }
    }

    pub fn open(&mut self, path: std::path::PathBuf) {
        self.worker.send(Request::Open(path));
    }

    pub fn opened(&mut self, window: &AppWindow, title: String, page_sizes: Vec<(f32, f32)>) {
        self.page_sizes = page_sizes;
        self.zoom = 1.0;
        self.cache.clear();
        self.last_request = None;
        window.set_document_title(title.into());
        window.set_page_count(ui_int(self.page_sizes.len()));
        window.set_status("".into());
        window.set_viewport_x(0.0);
        window.set_viewport_y(0.0);
        self.relayout(window);
    }

    pub fn rendered(&mut self, window: &AppWindow, page: usize, width_px: u32, bitmap: Bitmap) {
        // A render for an outdated width still beats a blank page.
        let stale = width_px != self.target_width_px(window);
        if stale && self.cache.contains_key(&page) {
            return;
        }
        let buffer = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
            &bitmap.rgba,
            bitmap.width,
            bitmap.height,
        );
        self.cache
            .insert(page, (width_px, Image::from_rgba8(buffer)));
        self.refresh(window);
    }

    /// Recomputes page positions after a resize, zoom, or new document.
    pub fn relayout(&mut self, window: &AppWindow) {
        let old_height = self.layout.content_height();
        let top = -window.get_viewport_y();

        self.layout = Layout::new(&self.page_sizes, window.get_reader_width(), self.zoom);
        window.set_content_width(self.layout.content_width());
        window.set_content_height(self.layout.content_height());

        // Keep the same relative reading position.
        if old_height > 0.0 {
            let ratio = top / old_height;
            self.scroll_to(window, ratio * self.layout.content_height());
        }
        self.refresh(window);
    }

    /// Updates visible pages, the page counter, and render requests.
    pub fn refresh(&mut self, window: &AppWindow) {
        let count = self.layout.page_count();
        if count == 0 {
            self.pages.set_vec(Vec::new());
            return;
        }
        let top = -window.get_viewport_y();
        let height = window.get_reader_height();
        let visible = self.layout.visible(top, height);
        let preload = layout::with_margin(visible.clone(), PRELOAD, count);
        let keep = layout::with_margin(visible.clone(), PRELOAD + KEEP, count);

        window.set_current_page(ui_int(self.layout.current_page(top, height) + 1));
        self.cache.retain(|page, _| keep.contains(page));
        self.pages.set_vec(self.page_views(window, preload.clone()));
        self.request_renders(window, visible, preload);
    }

    pub fn handle(&mut self, window: &AppWindow, action: Action) {
        match action {
            Action::ZoomIn => self.set_zoom(window, layout::zoom_in(self.zoom)),
            Action::ZoomOut => self.set_zoom(window, layout::zoom_out(self.zoom)),
            Action::ZoomReset => self.set_zoom(window, 1.0),
            _ => {
                let current = usize::try_from(window.get_current_page() - 1).unwrap_or(0);
                if let Some(page) = nav::target_page(action, current, self.layout.page_count()) {
                    self.scroll_to(window, self.layout.scroll_to(page));
                    self.refresh(window);
                }
            }
        }
    }

    fn set_zoom(&mut self, window: &AppWindow, zoom: f32) {
        if self.page_sizes.is_empty() || zoom == self.zoom {
            return;
        }
        self.zoom = zoom;
        self.relayout(window);
    }

    fn scroll_to(&self, window: &AppWindow, top: f32) {
        let max = (self.layout.content_height() - window.get_reader_height()).max(0.0);
        window.set_viewport_y(-top.clamp(0.0, max));
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
                })
            })
            .collect()
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

pub fn ui_int(value: usize) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}
