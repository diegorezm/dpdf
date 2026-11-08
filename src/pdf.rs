//! PDF access on a dedicated worker thread. PDFium never runs on the UI thread.
//!
//! The worker does one small unit of work at a time (one page render, one
//! page searched, one thumbnail) and checks for new requests in between, so
//! newer requests replace stale work instead of waiting behind it.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;

use pdfium_render::prelude::*;

pub enum Request {
    Open(PathBuf),
    /// Replaces the render queue: render `pages`, in order, `width_px` wide.
    Render {
        pages: Vec<usize>,
        width_px: u32,
    },
    /// Replaces the thumbnail queue. Thumbnails run after renders and search.
    Thumbnails {
        pages: Vec<usize>,
        width_px: u32,
    },
    /// Starts a search over all pages, replacing any running one. An empty
    /// query just cancels.
    Search {
        id: u64,
        query: String,
    },
}

pub enum Event {
    /// `page_sizes` are in points, one per page.
    Opened {
        title: String,
        page_sizes: Vec<(f32, f32)>,
    },
    Rendered {
        page: usize,
        width_px: u32,
        bitmap: Bitmap,
    },
    Thumbnail {
        page: usize,
        bitmap: Bitmap,
    },
    /// Matches on one page; sent only for pages with matches, in page order.
    SearchHits {
        id: u64,
        page: usize,
        matches: Vec<Vec<NormRect>>,
    },
    SearchDone {
        id: u64,
    },
    Error(String),
}

pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    /// Tightly packed RGBA8 pixels.
    pub rgba: Vec<u8>,
}

/// A rectangle as fractions of the page size, origin at the top left.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NormRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
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

#[derive(Default)]
struct Queue {
    pages: VecDeque<usize>,
    width_px: u32,
}

struct SearchJob {
    id: u64,
    query: String,
    next_page: usize,
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
    let mut renders = Queue::default();
    let mut thumbs = Queue::default();
    let mut search: Option<SearchJob> = None;

    loop {
        let idle = document.is_none()
            || (renders.pages.is_empty() && thumbs.pages.is_empty() && search.is_none());
        let request = if idle {
            match rx.recv() {
                Ok(request) => Some(request),
                Err(_) => return,
            }
        } else {
            match rx.try_recv() {
                Ok(request) => Some(request),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => return,
            }
        };

        match request {
            Some(Request::Open(path)) => {
                renders = Queue::default();
                thumbs = Queue::default();
                search = None;
                match open(&pdfium, &path) {
                    Ok((doc, page_sizes)) => {
                        document = Some(doc);
                        on_event(Event::Opened {
                            title: file_title(&path),
                            page_sizes,
                        });
                    }
                    Err(err) => on_event(Event::Error(format!("Couldn't open file: {err}"))),
                }
                continue;
            }
            Some(Request::Render { pages, width_px }) => {
                renders = Queue {
                    pages: pages.into(),
                    width_px,
                };
                continue;
            }
            Some(Request::Thumbnails { pages, width_px }) => {
                thumbs = Queue {
                    pages: pages.into(),
                    width_px,
                };
                continue;
            }
            Some(Request::Search { id, query }) => {
                search = (!query.is_empty()).then_some(SearchJob {
                    id,
                    query,
                    next_page: 0,
                });
                continue;
            }
            None => {}
        }

        let Some(doc) = &document else { continue };

        if let Some(page) = renders.pages.pop_front() {
            let width_px = renders.width_px;
            match render(doc, page, width_px) {
                Ok(bitmap) => on_event(Event::Rendered {
                    page,
                    width_px,
                    bitmap,
                }),
                Err(err) => on_event(Event::Error(format!(
                    "Couldn't render page {}: {err}",
                    page + 1
                ))),
            }
        } else if let Some(job) = &mut search {
            let page = job.next_page;
            job.next_page += 1;
            if let Ok(matches) = search_page(doc, page, &job.query) {
                if !matches.is_empty() {
                    on_event(Event::SearchHits {
                        id: job.id,
                        page,
                        matches,
                    });
                }
            }
            if job.next_page >= page_count(doc) {
                on_event(Event::SearchDone { id: job.id });
                search = None;
            }
        } else if let Some(page) = thumbs.pages.pop_front() {
            let width_px = thumbs.width_px;
            // A missing thumbnail isn't worth interrupting the reader for.
            if let Ok(bitmap) = render(doc, page, width_px) {
                on_event(Event::Thumbnail { page, bitmap });
            }
        }
    }
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

fn open<'a>(
    pdfium: &'a Pdfium,
    path: &Path,
) -> Result<(PdfDocument<'a>, Vec<(f32, f32)>), PdfiumError> {
    let doc = pdfium.load_pdf_from_file(path, None)?;
    // Reads sizes without loading each page, so large documents open fast.
    let sizes = doc
        .pages()
        .page_sizes()?
        .iter()
        .map(|rect| (rect.width().value, rect.height().value))
        .collect();
    Ok((doc, sizes))
}

fn page_count(doc: &PdfDocument) -> usize {
    usize::try_from(doc.pages().len()).unwrap_or(0)
}

fn load_page<'a>(doc: &'a PdfDocument, page: usize) -> Result<PdfPage<'a>, PdfiumError> {
    let index = PdfPageIndex::try_from(page).map_err(|_| PdfiumError::PageIndexOutOfBounds)?;
    doc.pages().get(index)
}

fn render(doc: &PdfDocument, page: usize, width_px: u32) -> Result<Bitmap, PdfiumError> {
    let pdf_page = load_page(doc, page)?;
    let width = Pixels::try_from(width_px.max(1)).unwrap_or(Pixels::MAX);
    let config = PdfRenderConfig::new()
        .set_target_width(width)
        .render_form_data(true);
    let bitmap = pdf_page.render_with_config(&config)?;
    Ok(Bitmap {
        width: u32::try_from(bitmap.width()).unwrap_or(0),
        height: u32::try_from(bitmap.height()).unwrap_or(0),
        rgba: bitmap.as_rgba_bytes(),
    })
}

/// Case-insensitive search of one page. Each match is one or more rects,
/// since a match can wrap across lines.
fn search_page(
    doc: &PdfDocument,
    page: usize,
    query: &str,
) -> Result<Vec<Vec<NormRect>>, PdfiumError> {
    let pdf_page = load_page(doc, page)?;
    let size = (pdf_page.width().value, pdf_page.height().value);
    let text = pdf_page.text()?;
    let search = text.search(query, &PdfSearchOptions::new())?;
    let matches = search
        .iter(PdfSearchDirection::SearchForward)
        .map(|segments| {
            segments
                .iter()
                .map(|segment| normalize(segment.bounds(), size))
                .collect()
        })
        .collect();
    Ok(matches)
}

/// Converts a PDF rect (points, origin bottom left) to page fractions.
fn normalize(rect: PdfRect, (page_w, page_h): (f32, f32)) -> NormRect {
    if page_w <= 0.0 || page_h <= 0.0 {
        return NormRect {
            x: 0.0,
            y: 0.0,
            width: 0.0,
            height: 0.0,
        };
    }
    NormRect {
        x: rect.left().value / page_w,
        y: (page_h - rect.top().value) / page_h,
        width: rect.width().value / page_w,
        height: rect.height().value / page_h,
    }
}

fn file_title(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Untitled".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_from_path() {
        assert_eq!(file_title(Path::new("C:/docs/paper.pdf")), "paper");
    }

    #[test]
    fn normalize_flips_y() {
        let rect = PdfRect::new_from_values(700.0, 100.0, 720.0, 200.0);
        let n = normalize(rect, (500.0, 800.0));
        assert_eq!(n.x, 0.2);
        assert_eq!(n.y, 0.1);
        assert_eq!(n.width, 0.2);
        assert_eq!(n.height, 0.025);
    }

    /// Needs PDFium; skipped when the library can't be loaded.
    #[test]
    fn opens_and_renders_fixture() {
        let Ok(pdfium) = bind() else {
            eprintln!("PDFium unavailable, skipping");
            return;
        };
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/two-pages.pdf");
        let (doc, sizes) = open(&pdfium, &path).unwrap();
        assert_eq!(sizes, vec![(200.0, 300.0); 2]);
        let bitmap = render(&doc, 1, 100).unwrap();
        assert_eq!((bitmap.width, bitmap.height), (100, 150));
        assert_eq!(bitmap.rgba.len(), 100 * 150 * 4);
        assert!(render(&doc, 5, 100).is_err());
        assert_eq!(
            search_page(&doc, 0, "anything").unwrap(),
            Vec::<Vec<_>>::new()
        );
    }
}
