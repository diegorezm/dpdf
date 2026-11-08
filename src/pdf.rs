//! PDF access on a dedicated worker thread. PDFium never runs on the UI thread.

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
    Error(String),
}

pub struct Bitmap {
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
    let mut queue: VecDeque<usize> = VecDeque::new();
    let mut width_px = 0;

    loop {
        // Handle every pending request before rendering, so a newer render
        // request replaces stale work instead of waiting behind it.
        let request = if queue.is_empty() {
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
                queue.clear();
                match open(&pdfium, &path) {
                    Ok((doc, page_sizes)) => {
                        document = Some(doc);
                        on_event(Event::Opened {
                            title: file_title(&path),
                            page_sizes,
                        });
                    }
                    Err(err) => {
                        document = None;
                        on_event(Event::Error(format!("Couldn't open file: {err}")));
                    }
                }
            }
            Some(Request::Render { pages, width_px: w }) => {
                queue = pages.into();
                width_px = w;
            }
            None => {
                let (Some(page), Some(doc)) = (queue.pop_front(), &document) else {
                    continue;
                };
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

fn render(doc: &PdfDocument, page: usize, width_px: u32) -> Result<Bitmap, PdfiumError> {
    let index = PdfPageIndex::try_from(page).map_err(|_| PdfiumError::PageIndexOutOfBounds)?;
    let pdf_page = doc.pages().get(index)?;
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
    }
}
