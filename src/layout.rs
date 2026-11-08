//! Geometry of the continuous page strip, in logical pixels. Pure and UI-free.

use std::ops::Range;

/// Vertical gap between pages and around the strip.
pub const GAP: f32 = 16.0;
/// Horizontal margin on each side of the pages at fit-width.
pub const SIDE_MARGIN: f32 = 24.0;

pub const MIN_ZOOM: f32 = 0.25;
pub const MAX_ZOOM: f32 = 5.0;
pub const ZOOM_STEP: f32 = 1.2;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Page positions for a document laid out top to bottom, every page scaled
/// to the same width.
#[derive(Debug, Default)]
pub struct Layout {
    rects: Vec<PageRect>,
    content_width: f32,
    content_height: f32,
}

impl Layout {
    /// `page_sizes` are page dimensions in points; `zoom` 1.0 means fit width.
    pub fn new(page_sizes: &[(f32, f32)], viewport_width: f32, zoom: f32) -> Self {
        let fit_width = (viewport_width - 2.0 * SIDE_MARGIN).max(1.0);
        let page_width = (fit_width * zoom).max(1.0);
        let content_width = viewport_width.max(page_width + 2.0 * SIDE_MARGIN);
        let x = (content_width - page_width) / 2.0;

        let mut y = GAP;
        let rects = page_sizes
            .iter()
            .map(|&(w, h)| {
                let aspect = if w > 0.0 && h > 0.0 { h / w } else { 1.0 };
                let rect = PageRect {
                    x,
                    y,
                    width: page_width,
                    height: page_width * aspect,
                };
                y += rect.height + GAP;
                rect
            })
            .collect();

        Self {
            rects,
            content_width,
            content_height: y,
        }
    }

    pub fn rect(&self, page: usize) -> Option<PageRect> {
        self.rects.get(page).copied()
    }

    pub fn page_count(&self) -> usize {
        self.rects.len()
    }

    pub fn content_width(&self) -> f32 {
        self.content_width
    }

    pub fn content_height(&self) -> f32 {
        self.content_height
    }

    /// Pages intersecting `[top, top + height)`.
    pub fn visible(&self, top: f32, height: f32) -> Range<usize> {
        let bottom = top + height;
        let start = self.rects.partition_point(|r| r.y + r.height <= top);
        let end = self.rects.partition_point(|r| r.y < bottom);
        start..end.max(start)
    }

    /// The page shown at the vertical middle of the viewport.
    pub fn current_page(&self, top: f32, height: f32) -> usize {
        let middle = top + height / 2.0;
        let index = self
            .rects
            .partition_point(|r| r.y + r.height + GAP <= middle);
        index.min(self.rects.len().saturating_sub(1))
    }

    /// Scroll offset that puts `page` at the top of the viewport.
    pub fn scroll_to(&self, page: usize) -> f32 {
        self.rect(page).map_or(0.0, |r| r.y - GAP)
    }
}

/// Expands `range` by `margin` pages on each side, clamped to `count`.
pub fn with_margin(range: Range<usize>, margin: usize, count: usize) -> Range<usize> {
    range.start.saturating_sub(margin)..(range.end + margin).min(count)
}

pub fn zoom_in(zoom: f32) -> f32 {
    (zoom * ZOOM_STEP).min(MAX_ZOOM)
}

pub fn zoom_out(zoom: f32) -> f32 {
    (zoom / ZOOM_STEP).max(MIN_ZOOM)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Three US Letter pages in a viewport whose fit width is 612.
    fn letter() -> Layout {
        Layout::new(&[(612.0, 792.0); 3], 612.0 + 2.0 * SIDE_MARGIN, 1.0)
    }

    #[test]
    fn stacks_pages_with_gaps() {
        let layout = letter();
        assert_eq!(layout.rect(0).map(|r| (r.y, r.height)), Some((GAP, 792.0)));
        assert_eq!(layout.rect(1).map(|r| r.y), Some(GAP * 2.0 + 792.0));
        assert_eq!(layout.content_height(), GAP * 4.0 + 792.0 * 3.0);
        assert_eq!(layout.rect(0).map(|r| r.x), Some(SIDE_MARGIN));
    }

    #[test]
    fn visible_range() {
        let layout = letter();
        assert_eq!(layout.visible(0.0, 500.0), 0..1);
        assert_eq!(layout.visible(700.0, 300.0), 0..2);
        assert_eq!(layout.visible(10_000.0, 500.0), 3..3);
    }

    #[test]
    fn current_page_follows_middle() {
        let layout = letter();
        assert_eq!(layout.current_page(0.0, 600.0), 0);
        assert_eq!(layout.current_page(layout.scroll_to(1), 600.0), 1);
        assert_eq!(layout.current_page(1e9, 600.0), 2);
        assert_eq!(Layout::default().current_page(0.0, 600.0), 0);
    }

    #[test]
    fn zoom_widens_content() {
        let layout = Layout::new(&[(100.0, 100.0)], 248.0, 2.0);
        assert_eq!(layout.rect(0).map(|r| r.width), Some(400.0));
        assert_eq!(layout.content_width(), 448.0);
        assert_eq!(zoom_in(MAX_ZOOM), MAX_ZOOM);
        assert_eq!(zoom_out(MIN_ZOOM), MIN_ZOOM);
    }

    #[test]
    fn margin_is_clamped() {
        assert_eq!(with_margin(0..2, 2, 3), 0..3);
        assert_eq!(with_margin(5..6, 2, 100), 3..8);
    }
}
