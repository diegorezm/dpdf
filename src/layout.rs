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

/// Zoom (relative to fit width) at which a page of `page_size` points fits
/// entirely in the viewport.
pub fn fit_page_zoom(page_size: (f32, f32), viewport: (f32, f32)) -> f32 {
    let (page_w, page_h) = page_size;
    let fit_width = (viewport.0 - 2.0 * SIDE_MARGIN).max(1.0);
    if page_w <= 0.0 || page_h <= 0.0 {
        return 1.0;
    }
    let height_at_fit_width = fit_width * page_h / page_w;
    let available = (viewport.1 - 2.0 * GAP).max(1.0);
    (available / height_at_fit_width).clamp(MIN_ZOOM, 1.0)
}

/// How pages are scaled. The fit modes are recomputed on every resize.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Zoom {
    FitWidth,
    FitPage,
    /// A factor relative to fit width.
    Custom(f32),
}

impl Zoom {
    /// The scale factor relative to fit width, for the page being read.
    pub fn factor(self, page_size: (f32, f32), viewport: (f32, f32)) -> f32 {
        match self {
            Zoom::FitWidth => 1.0,
            Zoom::FitPage => fit_page_zoom(page_size, viewport),
            Zoom::Custom(factor) => factor.clamp(MIN_ZOOM, MAX_ZOOM),
        }
    }

    pub fn zoom_in(factor: f32) -> Self {
        Zoom::Custom((factor * ZOOM_STEP).min(MAX_ZOOM))
    }

    pub fn zoom_out(factor: f32) -> Self {
        Zoom::Custom((factor / ZOOM_STEP).max(MIN_ZOOM))
    }

    pub fn to_token(self) -> String {
        match self {
            Zoom::FitWidth => "fit-width".to_owned(),
            Zoom::FitPage => "fit-page".to_owned(),
            Zoom::Custom(factor) => factor.to_string(),
        }
    }

    pub fn from_token(token: &str) -> Option<Self> {
        match token {
            "fit-width" => Some(Zoom::FitWidth),
            "fit-page" => Some(Zoom::FitPage),
            _ => {
                let factor: f32 = token.parse().ok()?;
                (factor.is_finite() && factor > 0.0).then_some(Zoom::Custom(factor))
            }
        }
    }
}

/// Zoom as a percentage of the page's real size (1pt = 1/72in, 96 px/in).
pub fn percent(page_width_px: f32, page_width_pt: f32) -> i32 {
    if page_width_pt <= 0.0 {
        return 100;
    }
    (page_width_px / (page_width_pt * 96.0 / 72.0) * 100.0).round() as i32
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
        assert_eq!(Zoom::zoom_in(MAX_ZOOM), Zoom::Custom(MAX_ZOOM));
        assert_eq!(Zoom::zoom_out(MIN_ZOOM), Zoom::Custom(MIN_ZOOM));
    }

    #[test]
    fn fit_page() {
        // Fit width 612 makes the page 792 tall; 400 available => ~0.505.
        let zoom = fit_page_zoom(
            (612.0, 792.0),
            (612.0 + 2.0 * SIDE_MARGIN, 400.0 + 2.0 * GAP),
        );
        assert!((zoom - 400.0 / 792.0).abs() < 1e-4);
        // A wide page already fits at fit width.
        assert_eq!(fit_page_zoom((800.0, 100.0), (500.0, 800.0)), 1.0);
    }

    #[test]
    fn zoom_modes() {
        let viewport = (612.0 + 2.0 * SIDE_MARGIN, 400.0 + 2.0 * GAP);
        assert_eq!(Zoom::FitWidth.factor((612.0, 792.0), viewport), 1.0);
        assert!(Zoom::FitPage.factor((612.0, 792.0), viewport) < 1.0);
        assert_eq!(Zoom::Custom(99.0).factor((1.0, 1.0), viewport), MAX_ZOOM);
        for zoom in [Zoom::FitWidth, Zoom::FitPage, Zoom::Custom(1.5)] {
            assert_eq!(Zoom::from_token(&zoom.to_token()), Some(zoom));
        }
        assert_eq!(Zoom::from_token("-1"), None);
        assert_eq!(Zoom::from_token("abc"), None);
    }

    #[test]
    fn percent_of_real_size() {
        assert_eq!(percent(816.0, 612.0), 100);
        assert_eq!(percent(408.0, 612.0), 50);
        assert_eq!(percent(10.0, 0.0), 100);
    }

    #[test]
    fn margin_is_clamped() {
        assert_eq!(with_margin(0..2, 2, 3), 0..3);
        assert_eq!(with_margin(5..6, 2, 100), 3..8);
    }
}
