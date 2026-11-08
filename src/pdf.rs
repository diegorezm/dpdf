//! PDF access on a dedicated worker thread. PDFium never runs on the UI thread.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use pdfium_render::prelude::*;

pub enum Request {
    Open(PathBuf),
    /// Render a zero-based page to fit within `max_width` x `max_height` pixels.
    Render {
        page: usize,
        max_width: u32,
        max_height: u32,
    },
}

pub enum Event {
    Opened { title: String, page_count: usize },
    Rendered(RenderedPage),
    Error(String),
}

pub struct RenderedPage {
    pub width: u32,
    pub height: u32,
    /// Tightly packed RGBA8 pixels.
    pub rgba: Vec<u8>,
}

pub struct Worker {
    tx: Sender<Request>,
}

impl Worker {
    /// Spawns the worker. `on_event` is called from the worker thread.
    pub fn spawn(on_event: impl Fn(Event) + Send + 'static) -> Self {
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || run(rx, on_event));
        Self { tx }
    }

    pub fn send(&self, request: Request) {
        // The worker only stops if PDFium failed to load, which was already reported.
        let _ = self.tx.send(request);
    }
}

fn run(rx: Receiver<Request>, on_event: impl Fn(Event)) {
    let pdfium = match bind() {
        Ok(pdfium) => pdfium,
        Err(err) => {
            on_event(Event::Error(format!("PDFium could not be loaded: {err}")));
            return;
        }
    };
    let mut document: Option<PdfDocument> = None;

    while let Ok(first) = rx.recv() {
        for request in coalesce(first, &rx) {
            match request {
                Request::Open(path) => match pdfium.load_pdf_from_file(&path, None) {
                    Ok(doc) => {
                        let page_count = usize::try_from(doc.pages().len()).unwrap_or(0);
                        document = Some(doc);
                        on_event(Event::Opened {
                            title: file_title(&path),
                            page_count,
                        });
                    }
                    Err(err) => {
                        document = None;
                        on_event(Event::Error(format!("Couldn't open file: {err}")));
                    }
                },
                Request::Render {
                    page,
                    max_width,
                    max_height,
                } => {
                    let Some(doc) = &document else { continue };
                    match render(doc, page, max_width, max_height) {
                        Ok(rendered) => on_event(Event::Rendered(rendered)),
                        Err(err) => on_event(Event::Error(format!("Couldn't render page: {err}"))),
                    }
                }
            }
        }
    }
}

/// Drains queued requests, collapsing consecutive renders to the newest one
/// so fast key repeats skip stale pages.
fn coalesce(first: Request, rx: &Receiver<Request>) -> Vec<Request> {
    let mut out: Vec<Request> = Vec::new();
    for request in std::iter::once(first).chain(rx.try_iter()) {
        if matches!(request, Request::Render { .. })
            && matches!(out.last(), Some(Request::Render { .. }))
        {
            out.pop();
        }
        out.push(request);
    }
    out
}

/// Prefers the pdfium.dll next to the executable, then the system library.
fn bind() -> Result<Pdfium, PdfiumError> {
    let local = std::env::current_exe()
        .ok()
        .and_then(|exe| {
            exe.parent()
                .map(Pdfium::pdfium_platform_library_name_at_path)
        })
        .map(|path| Pdfium::bind_to_library(path));
    let bindings = match local {
        Some(Ok(bindings)) => bindings,
        _ => Pdfium::bind_to_system_library()?,
    };
    Ok(Pdfium::new(bindings))
}

fn render(
    doc: &PdfDocument,
    page: usize,
    max_width: u32,
    max_height: u32,
) -> Result<RenderedPage, PdfiumError> {
    let index = PdfPageIndex::try_from(page).map_err(|_| PdfiumError::PageIndexOutOfBounds)?;
    let pdf_page = doc.pages().get(index)?;
    let (width, height) = fit_size(
        (pdf_page.width().value, pdf_page.height().value),
        (max_width, max_height),
    );
    let config = PdfRenderConfig::new()
        .set_target_size(to_pixels(width), to_pixels(height))
        .render_form_data(true);
    let bitmap = pdf_page.render_with_config(&config)?;
    Ok(RenderedPage {
        width: u32::try_from(bitmap.width()).unwrap_or(0),
        height: u32::try_from(bitmap.height()).unwrap_or(0),
        rgba: bitmap.as_rgba_bytes(),
    })
}

/// Largest pixel size with the page's aspect ratio that fits in `bounds`.
fn fit_size((page_w, page_h): (f32, f32), (max_w, max_h): (u32, u32)) -> (u32, u32) {
    if page_w <= 0.0 || page_h <= 0.0 {
        return (max_w.max(1), max_h.max(1));
    }
    let scale = (max_w as f32 / page_w).min(max_h as f32 / page_h);
    let px = |v: f32| (v * scale).round().max(1.0) as u32;
    (px(page_w), px(page_h))
}

fn to_pixels(value: u32) -> Pixels {
    Pixels::try_from(value.max(1)).unwrap_or(Pixels::MAX)
}

fn file_title(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Untitled".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render_req(page: usize) -> Request {
        Request::Render {
            page,
            max_width: 1,
            max_height: 1,
        }
    }

    fn pages(batch: &[Request]) -> Vec<Option<usize>> {
        batch
            .iter()
            .map(|r| match r {
                Request::Render { page, .. } => Some(*page),
                Request::Open(_) => None,
            })
            .collect()
    }

    #[test]
    fn coalesce_keeps_latest_render() {
        let (tx, rx) = mpsc::channel();
        for page in 1..4 {
            tx.send(render_req(page)).unwrap();
        }
        assert_eq!(pages(&coalesce(render_req(0), &rx)), vec![Some(3)]);
    }

    #[test]
    fn coalesce_keeps_opens_in_order() {
        let (tx, rx) = mpsc::channel();
        tx.send(Request::Open(PathBuf::from("a.pdf"))).unwrap();
        tx.send(render_req(0)).unwrap();
        tx.send(render_req(2)).unwrap();
        assert_eq!(
            pages(&coalesce(render_req(5), &rx)),
            vec![Some(5), None, Some(2)]
        );
    }

    #[test]
    fn title_from_path() {
        assert_eq!(file_title(Path::new("C:/docs/paper.pdf")), "paper");
    }

    #[test]
    fn fit_size_keeps_aspect_ratio() {
        assert_eq!(fit_size((200.0, 300.0), (100, 100)), (67, 100));
        assert_eq!(fit_size((300.0, 200.0), (100, 100)), (100, 67));
        assert_eq!(fit_size((612.0, 792.0), (1224, 2000)), (1224, 1584));
        assert_eq!(fit_size((0.0, 100.0), (50, 60)), (50, 60));
    }

    /// Needs PDFium; skipped when the library can't be loaded.
    #[test]
    fn opens_and_renders_fixture() {
        let Ok(pdfium) = bind() else {
            eprintln!("PDFium unavailable, skipping");
            return;
        };
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/two-pages.pdf");
        let doc = pdfium.load_pdf_from_file(&path, None).unwrap();
        assert_eq!(doc.pages().len(), 2);
        let page = render(&doc, 1, 100, 100).unwrap();
        // 200x300 page fit into 100x100 keeps its aspect ratio.
        assert_eq!(page.height, 100);
        assert_eq!(page.width, 67);
        assert_eq!(page.rgba.len(), (page.width * page.height * 4) as usize);
        assert!(render(&doc, 5, 100, 100).is_err());
    }
}
