//! PDF access on a dedicated worker thread. PDFium never runs on the UI thread.
//!
//! The worker does one small unit of work at a time (one page render, one
//! page searched, one thumbnail) and checks for new requests in between, so
//! newer requests replace stale work instead of waiting behind it.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;

use pdfium_render::prelude::*;

use crate::recover::{self, Glyph, Recovery};
use serde::{Deserialize, Serialize};

pub enum Request {
    Open {
        path: PathBuf,
        password: Option<String>,
    },
    /// Replaces the render queue: render `previews` quickly at a quarter
    /// of `width_px`, then `pages`, in order, `width_px` wide.
    Render {
        previews: Vec<usize>,
        pages: Vec<usize>,
        width_px: u32,
    },
    /// Replaces the thumbnail queue. Thumbnails run after renders and search.
    Thumbnails { pages: Vec<usize>, width_px: u32 },
    /// Starts a search over all pages, replacing any running one. An empty
    /// query just cancels.
    Search { id: u64, query: String },
    /// Selects the text between two points, each a page and a point in
    /// page fractions. The points may be on different pages.
    Select {
        from: (usize, (f32, f32)),
        to: (usize, (f32, f32)),
    },
    /// Selects the whole line of text nearest a point (double click).
    SelectLine { page: usize, point: (f32, f32) },
    /// Invert page colors in future renders (dark reading).
    SetInverted(bool),
    /// Follows the link at a point on a page (page fractions), if any.
    LinkAt { page: usize, point: (f32, f32) },
    /// Searches the text of other documents, one page at a time, replacing
    /// any running file search. An empty query just cancels.
    SearchFiles {
        id: u64,
        query: String,
        paths: Vec<PathBuf>,
    },
    /// Finds the page where article `number` of a law starts ("Art. 37").
    FindArticle { number: u32 },
}

/// One outline (table of contents) entry, flattened in reading order.
#[derive(Debug, Clone, PartialEq)]
pub struct OutlineItem {
    pub title: String,
    pub depth: usize,
    pub page: Option<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LinkTarget {
    Page(usize),
    /// An external http(s) or mailto link.
    Uri(String),
}

/// `B` is the bitmap type: the worker sends `Bitmap`s, which the receiver
/// may convert (with `map_bitmap`) before handing them to the UI thread.
pub enum Event<B = Bitmap> {
    /// `page_sizes` are in points, one per page.
    Opened {
        path: PathBuf,
        title: String,
        page_sizes: Vec<(f32, f32)>,
        outline: Vec<OutlineItem>,
    },
    /// The document is encrypted; `retry` is true if a password was wrong.
    PasswordNeeded {
        path: PathBuf,
        retry: bool,
    },
    /// Page labels (e.g. "iv"), one per page, empty when a page has none.
    /// Sent once, after the rest of the opening work.
    Labels(Vec<String>),
    Link(Option<LinkTarget>),
    Rendered {
        page: usize,
        width_px: u32,
        bitmap: B,
    },
    Thumbnail {
        page: usize,
        bitmap: B,
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
    /// Result of a `Select`, one part per page; empty when nothing was
    /// selected.
    Selection(Vec<SelectionPart>),
    /// A page of another document that has the query, with the text around
    /// the first match.
    FileHit {
        id: u64,
        path: PathBuf,
        page: usize,
        snippet: String,
        count: usize,
    },
    FilesDone {
        id: u64,
    },
    /// Where an article starts, if found.
    Article {
        number: u32,
        page: Option<usize>,
    },
    Error(String),
}

impl<B> Event<B> {
    /// Converts the bitmap of a render or thumbnail, if any.
    pub fn map_bitmap<C>(self, f: impl FnOnce(B) -> C) -> Event<C> {
        match self {
            Event::Rendered {
                page,
                width_px,
                bitmap,
            } => Event::Rendered {
                page,
                width_px,
                bitmap: f(bitmap),
            },
            Event::Thumbnail { page, bitmap } => Event::Thumbnail {
                page,
                bitmap: f(bitmap),
            },
            Event::Opened {
                path,
                title,
                page_sizes,
                outline,
            } => Event::Opened {
                path,
                title,
                page_sizes,
                outline,
            },
            Event::PasswordNeeded { path, retry } => Event::PasswordNeeded { path, retry },
            Event::Labels(labels) => Event::Labels(labels),
            Event::Link(target) => Event::Link(target),
            Event::SearchHits { id, page, matches } => Event::SearchHits { id, page, matches },
            Event::SearchDone { id } => Event::SearchDone { id },
            Event::Selection(parts) => Event::Selection(parts),
            Event::FileHit {
                id,
                path,
                page,
                snippet,
                count,
            } => Event::FileHit {
                id,
                path,
                page,
                snippet,
                count,
            },
            Event::FilesDone { id } => Event::FilesDone { id },
            Event::Article { number, page } => Event::Article { number, page },
            Event::Error(message) => Event::Error(message),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SelectionPart {
    pub page: usize,
    /// One rect per line.
    pub rects: Vec<NormRect>,
    pub text: String,
    /// One-based line on the page where the selection starts.
    pub line: u32,
}

pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    /// Tightly packed RGBA8 pixels.
    pub rgba: Vec<u8>,
}

/// A rectangle as fractions of the page size, origin at the top left.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
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
    /// Pages to render rough and small first, so they show at once.
    previews: VecDeque<usize>,
    pages: VecDeque<usize>,
    width_px: u32,
}

/// Previews are this many times narrower than the full render.
const PREVIEW_DIVISOR: u32 = 4;

/// A pending selection: drag start and end, as pages and points.
type SelectJob = ((usize, (f32, f32)), (usize, (f32, f32)));

/// Documents kept open, so switching between tabs doesn't reparse them.
/// PDFium loads pages lazily, so an idle open document is cheap.
const OPEN_DOCUMENTS: usize = 4;

/// An open document and what's needed to hand it out again.
struct OpenDoc<'a> {
    path: PathBuf,
    /// To notice the file changing on disk.
    modified: Option<std::time::SystemTime>,
    doc: PdfDocument<'a>,
    page_sizes: Vec<(f32, f32)>,
    outline: Vec<OutlineItem>,
    /// Kept in memory only, to read page labels.
    password: Option<String>,
    /// Page labels, once read.
    labels: Option<Vec<String>>,
    /// For text the PDF draws but maps to nothing; loaded on first need.
    recovery: Recovering,
    /// Each page's text, squashed (see `squash`), once a search has read
    /// it. Later searches skip pages that can't match without asking
    /// PDFium. A few KB per page.
    texts: Vec<Option<String>>,
}

fn modified(path: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Pages a selection may span; bounds the work of a wild drag.
const MAX_SELECT_PAGES: usize = 50;
/// Pages whose character boxes are kept while dragging.
const CHAR_CACHE_PAGES: usize = 8;

/// A search in progress, or the last one, kept to narrow the next.
struct SearchJob {
    id: u64,
    query: String,
    /// Pages to search, ascending.
    pages: Vec<usize>,
    /// Index into `pages` of the next page to search.
    next: usize,
    /// Pages searched so far that had matches.
    hits: Vec<usize>,
}

impl SearchJob {
    /// A search of all `page_count` pages, or, when `query` refines the
    /// `previous` one (typing more letters), only of the pages that query
    /// matched plus those it had not reached yet.
    fn new(id: u64, query: String, page_count: usize, previous: Option<SearchJob>) -> Self {
        let pages = match previous {
            Some(prev) if refines(&prev.query, &query) => {
                let mut pages = prev.hits;
                pages.extend_from_slice(prev.pages.get(prev.next..).unwrap_or_default());
                pages
            }
            _ => (0..page_count).collect(),
        };
        Self {
            id,
            query,
            pages,
            next: 0,
            hits: Vec::new(),
        }
    }

    fn next_page(&mut self) -> Option<usize> {
        let page = self.pages.get(self.next).copied();
        self.next += 1;
        page
    }

    fn is_done(&self) -> bool {
        self.next >= self.pages.len()
    }
}

/// Lowercase letters and digits only. If PDFium finds a query in a page,
/// the squashed query is in the squashed page text, whatever spaces,
/// hyphens, or line breaks PDFium matched across.
fn squash(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Whether every match of `new` contains a match of `old`, so pages
/// without `old` can't have `new` either. Search is case-insensitive.
fn refines(old: &str, new: &str) -> bool {
    !old.is_empty() && new.to_lowercase().contains(&old.to_lowercase())
}

/// A search through other documents' text.
struct FilesJob<'a> {
    id: u64,
    query: Vec<char>,
    paths: VecDeque<PathBuf>,
    /// The document being searched, the next page in it, and the document
    /// itself once a page's text isn't cached.
    current: Option<(PathBuf, usize, Option<PdfDocument<'a>>)>,
    hits: usize,
}

/// Page texts of documents searched before, so searching again while
/// typing reads no PDF. Bounded by document count; a page's text is a few
/// KB.
#[derive(Default)]
struct FileTexts {
    /// Least recently searched first.
    docs: VecDeque<(PathBuf, Option<std::time::SystemTime>, Vec<Option<String>>)>,
}

const CACHED_FILE_TEXTS: usize = 12;

impl FileTexts {
    /// The cached pages of `path`, made current; `None` if not cached or
    /// the file changed.
    fn pages(&mut self, path: &Path) -> Option<&mut Vec<Option<String>>> {
        let i = self.docs.iter().position(|(p, _, _)| p == path)?;
        let entry = self.docs.remove(i)?;
        if entry.1 != modified(path) {
            return None;
        }
        self.docs.push_back(entry);
        self.docs.back_mut().map(|(_, _, pages)| pages)
    }

    fn insert(&mut self, path: &Path, page_count: usize) {
        self.docs.retain(|(p, _, _)| p != path);
        if self.docs.len() >= CACHED_FILE_TEXTS {
            self.docs.pop_front();
        }
        self.docs
            .push_back((path.to_owned(), modified(path), vec![None; page_count]));
    }
}

/// Results a file search stops at.
const MAX_FILE_HITS: usize = 200;
/// Characters of context on each side of a file search match.
const SNIPPET_CONTEXT: usize = 50;

struct ArticleJob {
    number: u32,
    next: usize,
}

fn run(rx: Receiver<Request>, on_event: impl Fn(Event)) {
    let pdfium = match bind() {
        Ok(pdfium) => pdfium,
        Err(err) => {
            on_event(Event::Error(format!("PDFium could not be loaded: {err}")));
            return;
        }
    };
    // Most recently opened first; the front one is the live document.
    let mut documents: VecDeque<OpenDoc> = VecDeque::new();
    let mut renders = Queue::default();
    let mut thumbs = Queue::default();
    let mut search: Option<SearchJob> = None;
    let mut last_search: Option<SearchJob> = None;
    let mut select: Option<SelectJob> = None;
    // Character boxes of recently selected pages, reused while dragging.
    let mut char_cache: VecDeque<(usize, Chars)> = VecDeque::new();
    let mut inverted = false;
    // Whether the live document's page labels are still to be sent.
    let mut labels_due = false;
    let mut link: Option<(usize, (f32, f32))> = None;
    let mut select_line: Option<(usize, (f32, f32))> = None;
    let mut files: Option<FilesJob> = None;
    let mut file_texts = FileTexts::default();
    let mut article: Option<ArticleJob> = None;

    loop {
        // Work on the live document the reader is waiting for, and work
        // that can wait for it.
        let urgent = !documents.is_empty()
            && (!renders.pages.is_empty()
                || !renders.previews.is_empty()
                || search.is_some()
                || select.is_some()
                || select_line.is_some()
                || link.is_some()
                || article.is_some());
        let background =
            files.is_some() || (!documents.is_empty() && (!thumbs.pages.is_empty() || labels_due));
        let idle = !urgent && !background;
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
            Some(Request::Open { path, password }) => {
                renders = Queue::default();
                thumbs = Queue::default();
                search = None;
                last_search = None;
                select = None;
                link = None;
                article = None;
                char_cache.clear();
                labels_due = false;
                let stamp = modified(&path);
                if let Some(i) = documents.iter().position(|d| d.path == path) {
                    match documents.remove(i) {
                        Some(open) if open.modified == stamp => documents.push_front(open),
                        _ => {}
                    }
                }
                if documents.front().is_some_and(|d| d.path == path) {
                    if let Some(open) = documents.front() {
                        labels_due = true;
                        on_event(Event::Opened {
                            title: file_title(&path),
                            path,
                            page_sizes: open.page_sizes.clone(),
                            outline: open.outline.clone(),
                        });
                    }
                    continue;
                }
                match open(&pdfium, &path, password.as_deref()) {
                    Ok((doc, page_sizes)) => {
                        let outline = outline(&doc);
                        labels_due = true;
                        documents.push_front(OpenDoc {
                            path: path.clone(),
                            password: password.clone(),
                            labels: None,
                            recovery: Recovering::Unloaded,
                            modified: stamp,
                            doc,
                            texts: vec![None; page_sizes.len()],
                            page_sizes: page_sizes.clone(),
                            outline: outline.clone(),
                        });
                        documents.truncate(OPEN_DOCUMENTS);
                        on_event(Event::Opened {
                            title: file_title(&path),
                            path,
                            page_sizes,
                            outline,
                        });
                    }
                    Err(PdfiumError::PdfiumLibraryInternalError(
                        PdfiumInternalError::PasswordError,
                    )) => on_event(Event::PasswordNeeded {
                        path,
                        retry: password.is_some(),
                    }),
                    Err(err) => on_event(Event::Error(format!("Couldn't open file: {err}"))),
                }
                continue;
            }
            Some(Request::Render {
                previews,
                pages,
                width_px,
            }) => {
                renders = Queue {
                    previews: previews.into(),
                    pages: pages.into(),
                    width_px,
                };
                continue;
            }
            Some(Request::Thumbnails { pages, width_px }) => {
                thumbs = Queue {
                    previews: VecDeque::new(),
                    pages: pages.into(),
                    width_px,
                };
                continue;
            }
            Some(Request::Select { from, to }) => {
                select_line = None;
                select = Some((from, to));
                continue;
            }
            Some(Request::SelectLine { page, point }) => {
                select = None;
                select_line = Some((page, point));
                continue;
            }
            Some(Request::LinkAt { page, point }) => {
                link = Some((page, point));
                continue;
            }
            Some(Request::SetInverted(value)) => {
                inverted = value;
                continue;
            }
            Some(Request::SearchFiles { id, query, paths }) => {
                let query: Vec<char> = query.trim().chars().map(fold).collect();
                files = (!query.is_empty()).then(|| FilesJob {
                    id,
                    query,
                    paths: paths.into(),
                    current: None,
                    hits: 0,
                });
                continue;
            }
            Some(Request::FindArticle { number }) => {
                article = Some(ArticleJob { number, next: 0 });
                continue;
            }
            Some(Request::Search { id, query }) => {
                let previous = search.take().or(last_search.take());
                let count = documents.front().map_or(0, |d| d.page_sizes.len());
                search = (!query.is_empty()).then(|| SearchJob::new(id, query, count, previous));
                continue;
            }
            None => {}
        }

        // Other documents are searched only when the live one has nothing
        // urgent to do, so reading never waits for them.
        if !urgent {
            if let Some(job) = &mut files {
                if search_files_step(&pdfium, job, &mut file_texts, &on_event) {
                    on_event(Event::FilesDone { id: job.id });
                    files = None;
                }
                continue;
            }
        }

        let Some(OpenDoc {
            doc,
            texts,
            labels,
            path,
            password,
            page_sizes,
            recovery,
            ..
        }) = documents.front_mut()
        else {
            continue;
        };
        let doc = &*doc;
        let mut recover = |page: usize| recovery.glyphs(path, password.as_deref(), page);

        if let Some((page, point)) = link.take() {
            let target = link_at(doc, page, point);
            on_event(Event::Link(target));
        } else if let Some((page, point)) = select_line.take() {
            let boxes = cached_chars(doc, &mut char_cache, page, &mut recover);
            let parts = nearest_char(&boxes, point)
                .and_then(|i| {
                    let (a, b) = line_range(&boxes, i);
                    select_range(&boxes, a, b)
                })
                .map(|(rects, text)| SelectionPart {
                    page,
                    rects,
                    text,
                    line: 0,
                })
                .into_iter()
                .collect();
            on_event(Event::Selection(number_lines(parts, |page| {
                cached_chars(doc, &mut char_cache, page, &mut recover)
            })));
        } else if let Some((from, to)) = select.take() {
            let parts = select_pages(from, to, |page| {
                cached_chars(doc, &mut char_cache, page, &mut recover)
            });
            on_event(Event::Selection(number_lines(parts, |page| {
                cached_chars(doc, &mut char_cache, page, &mut recover)
            })));
        } else if let Some(job) = &mut article {
            let page = job.next;
            job.next += 1;
            let found = load_page(doc, page)
                .ok()
                .and_then(|p| p.text().ok().map(|t| t.all()))
                .is_some_and(|text| crate::nav::starts_article(&text, job.number));
            if found || job.next >= page_sizes.len() {
                on_event(Event::Article {
                    number: job.number,
                    page: found.then_some(page),
                });
                article = None;
            }
        } else if let Some(page) = renders.previews.pop_front() {
            let width_px = (renders.width_px / PREVIEW_DIVISOR).max(1);
            // The full render follows; a failed preview needs no message.
            if let Ok(bitmap) = render(doc, page, width_px, inverted, false) {
                on_event(Event::Rendered {
                    page,
                    width_px,
                    bitmap,
                });
            }
        } else if let Some(page) = renders.pages.pop_front() {
            let width_px = renders.width_px;
            match render(doc, page, width_px, inverted, true) {
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
            if let Some(page) = job.next_page() {
                let text = texts.get_mut(page);
                if let Ok(matches) = search_page(doc, page, &job.query, text) {
                    if !matches.is_empty() {
                        job.hits.push(page);
                        on_event(Event::SearchHits {
                            id: job.id,
                            page,
                            matches,
                        });
                    }
                }
            }
            if job.is_done() {
                on_event(Event::SearchDone { id: job.id });
                last_search = search.take();
            }
        } else if let Some(page) = thumbs.pages.pop_front() {
            let width_px = thumbs.width_px;
            // A missing thumbnail isn't worth interrupting the reader for.
            if let Ok(bitmap) = render(doc, page, width_px, inverted, false) {
                on_event(Event::Thumbnail { page, bitmap });
            }
        } else if labels_due {
            labels_due = false;
            let found = labels.get_or_insert_with(|| {
                page_labels(&pdfium, path, password.as_deref(), page_sizes.len())
            });
            // Most documents have no labels; don't bother the UI then.
            if found.iter().any(|s| !s.is_empty()) {
                on_event(Event::Labels(found.clone()));
            }
        }
    }
}

/// Lowercase without accents, one char for one char, so positions in the
/// folded text match the original: "Licitação" and "licitacao" match.
fn fold(c: char) -> char {
    let c = c.to_lowercase().next().unwrap_or(c);
    match c {
        'á' | 'à' | 'â' | 'ã' | 'ä' => 'a',
        'é' | 'è' | 'ê' | 'ë' => 'e',
        'í' | 'ì' | 'î' | 'ï' => 'i',
        'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o',
        'ú' | 'ù' | 'û' | 'ü' => 'u',
        'ç' => 'c',
        'ñ' => 'n',
        c if c.is_whitespace() => ' ',
        c => c,
    }
}

/// Matches of `query` (folded) in `text`, and the text around the first.
fn find_in_text(text: &str, query: &[char]) -> Option<(usize, String)> {
    let chars: Vec<char> = text.chars().collect();
    let folded: Vec<char> = chars.iter().map(|&c| fold(c)).collect();
    if query.is_empty() || folded.len() < query.len() {
        return None;
    }
    let starts: Vec<usize> = (0..=folded.len() - query.len())
        .filter(|&i| folded[i..i + query.len()] == *query)
        .collect();
    let first = *starts.first()?;
    let from = first.saturating_sub(SNIPPET_CONTEXT);
    let to = (first + query.len() + SNIPPET_CONTEXT).min(chars.len());
    let snippet: String = chars[from..to].iter().collect();
    let mut snippet = snippet.split_whitespace().collect::<Vec<_>>().join(" ");
    if from > 0 {
        snippet.insert(0, '…');
    }
    if to < chars.len() {
        snippet.push('…');
    }
    Some((starts.len(), snippet))
}

/// Searches one page of a file search, or opens its next document.
/// Returns true when the search is finished.
fn search_files_step<'a>(
    pdfium: &'a Pdfium,
    job: &mut FilesJob<'a>,
    texts: &mut FileTexts,
    on_event: &impl Fn(Event),
) -> bool {
    if job.hits >= MAX_FILE_HITS {
        return true;
    }
    let Some((path, page, doc)) = &mut job.current else {
        let Some(path) = job.paths.pop_front() else {
            return true;
        };
        if texts.pages(&path).is_none() {
            // Encrypted or broken documents are skipped.
            let Ok((doc, sizes)) = open(pdfium, &path, None) else {
                return false;
            };
            texts.insert(&path, sizes.len());
            job.current = Some((path, 0, Some(doc)));
        } else {
            job.current = Some((path, 0, None));
        }
        return false;
    };
    let Some(pages) = texts.pages(path) else {
        job.current = None;
        return false;
    };
    let Some(slot) = pages.get_mut(*page) else {
        job.current = None;
        return false;
    };
    if slot.is_none() {
        if doc.is_none() {
            *doc = open(pdfium, path, None).ok().map(|(d, _)| d);
        }
        *slot = doc.as_ref().and_then(|d| {
            load_page(d, *page)
                .ok()
                .and_then(|p| p.text().ok().map(|t| t.all()))
        });
    }
    if let Some((matches, snippet)) = slot.as_deref().and_then(|t| find_in_text(t, &job.query)) {
        job.hits += 1;
        on_event(Event::FileHit {
            id: job.id,
            path: path.clone(),
            page: *page,
            snippet,
            count: matches,
        });
    }
    *page += 1;
    false
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
    password: Option<&str>,
) -> Result<(PdfDocument<'a>, Vec<(f32, f32)>), PdfiumError> {
    let doc = pdfium.load_pdf_from_file(path, password)?;
    // Reads sizes without loading each page, so large documents open fast.
    let sizes = doc
        .pages()
        .page_sizes()?
        .iter()
        .map(|rect| (rect.width().value, rect.height().value))
        .collect();
    Ok((doc, sizes))
}

/// Huge or cyclic outlines are cut off rather than walked forever.
const MAX_OUTLINE_ITEMS: usize = 5000;
const MAX_OUTLINE_DEPTH: usize = 16;

fn outline(doc: &PdfDocument) -> Vec<OutlineItem> {
    let mut items = Vec::new();
    walk_outline(doc.bookmarks().root(), 0, &mut items);
    items
}

fn walk_outline(mut node: Option<PdfBookmark>, depth: usize, items: &mut Vec<OutlineItem>) {
    while let Some(bookmark) = node {
        if items.len() >= MAX_OUTLINE_ITEMS || depth > MAX_OUTLINE_DEPTH {
            return;
        }
        let page = target_page(bookmark.destination(), bookmark.action());
        items.push(OutlineItem {
            title: bookmark.title().unwrap_or_default().trim().to_owned(),
            depth,
            page,
        });
        walk_outline(bookmark.first_child(), depth + 1, items);
        node = bookmark.next_sibling();
    }
}

/// The page a bookmark or link points to, through a destination or a
/// go-to action.
fn target_page(destination: Option<PdfDestination>, action: Option<PdfAction>) -> Option<usize> {
    let page = match destination {
        Some(d) => d.page_index().ok()?,
        None => action?
            .as_local_destination_action()?
            .destination()
            .ok()?
            .page_index()
            .ok()?,
    };
    usize::try_from(page).ok()
}

fn link_at(doc: &PdfDocument, page: usize, (x, y): (f32, f32)) -> Option<LinkTarget> {
    let pdf_page = load_page(doc, page).ok()?;
    let (w, h) = (pdf_page.width().value, pdf_page.height().value);
    let links = pdf_page.links();
    let link = links.link_at_point(PdfPoints::new(x * w), PdfPoints::new(h - y * h))?;
    if let Some(page) = target_page(link.destination(), link.action()) {
        return Some(LinkTarget::Page(page));
    }
    let uri = link.action()?.as_uri_action()?.uri().ok()?;
    safe_uri(&uri).then_some(LinkTarget::Uri(uri))
}

/// Only web and mail links are opened; a PDF must not be able to launch
/// programs or local files.
fn safe_uri(uri: &str) -> bool {
    let lower = uri.to_ascii_lowercase();
    ["http://", "https://", "mailto:"]
        .iter()
        .any(|scheme| lower.starts_with(scheme))
        && !uri.chars().any(|c| c.is_control() || c == '"')
}

/// Page labels (e.g. "iv"), one per page, empty when a page has none.
/// Read from a second handle on the document through the raw bindings:
/// `PdfPage::label` would load and parse every page (300 ms for 1000
/// pages), while the label tree lookup alone takes about a millisecond.
fn page_labels(pdfium: &Pdfium, path: &Path, password: Option<&str>, count: usize) -> Vec<String> {
    let Some(path) = path.to_str() else {
        return Vec::new();
    };
    let bindings = pdfium.bindings();
    // SAFETY: the handle is checked for null, used only within this block,
    // and closed exactly once. Each buffer passed to FPDF_GetPageLabel is
    // as long as the length PDFium just reported for that label.
    unsafe {
        let handle = bindings.FPDF_LoadDocument(path, password);
        if handle.is_null() {
            return Vec::new();
        }
        let labels = (0..count)
            .map(|page| {
                let Ok(index) = i32::try_from(page) else {
                    return String::new();
                };
                let len = bindings.FPDF_GetPageLabel(handle, index, std::ptr::null_mut(), 0);
                let Ok(size) = usize::try_from(len) else {
                    return String::new();
                };
                if size <= 2 {
                    return String::new();
                }
                let mut buffer = vec![0u8; size];
                bindings.FPDF_GetPageLabel(handle, index, buffer.as_mut_ptr().cast(), len);
                utf16le(&buffer)
            })
            .collect();
        bindings.FPDF_CloseDocument(handle);
        labels
    }
}

/// Decodes NUL-terminated UTF-16LE.
fn utf16le(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&pair| u16::from_le_bytes(pair))
        .take_while(|&unit| unit != 0)
        .collect();
    String::from_utf16_lossy(&units)
}

fn load_page<'a>(doc: &'a PdfDocument, page: usize) -> Result<PdfPage<'a>, PdfiumError> {
    let index = PdfPageIndex::try_from(page).map_err(|_| PdfiumError::PageIndexOutOfBounds)?;
    doc.pages().get(index)
}

/// `forms` draws form fields; thumbnails skip them, too small to matter.
fn render(
    doc: &PdfDocument,
    page: usize,
    width_px: u32,
    inverted: bool,
    forms: bool,
) -> Result<Bitmap, PdfiumError> {
    let pdf_page = load_page(doc, page)?;
    let size = (pdf_page.width().value, pdf_page.height().value);
    let width_px = capped_width(width_px, size);
    let width = Pixels::try_from(width_px).unwrap_or(Pixels::MAX);
    let config = PdfRenderConfig::new()
        .set_target_width(width)
        .render_form_data(forms);
    let bitmap = pdf_page.render_with_config(&config)?;
    let mut rgba = bitmap.as_rgba_bytes();
    if inverted {
        invert(&mut rgba);
    }
    Ok(Bitmap {
        width: u32::try_from(bitmap.width()).unwrap_or(0),
        height: u32::try_from(bitmap.height()).unwrap_or(0),
        rgba,
    })
}

/// Most pixels one render may have: 24 M is 72 MB as RGB, an A4 page
/// about 4100 px wide. Deeper zoom stretches the render instead.
const MAX_RENDER_PIXELS: f32 = 24_000_000.0;
/// Longest side of a render; larger textures fail on many GPUs.
const MAX_RENDER_SIDE: f32 = 16_384.0;

/// `width_px`, shrunk so a page of `size` (points) stays within
/// `MAX_RENDER_PIXELS` and `MAX_RENDER_SIDE`. Never below one pixel.
fn capped_width(width_px: u32, (w, h): (f32, f32)) -> u32 {
    let width_px = width_px.max(1);
    if !(w > 0.0 && h > 0.0) {
        return width_px;
    }
    let max = (MAX_RENDER_PIXELS * w / h)
        .sqrt()
        .min(MAX_RENDER_SIDE)
        .min(MAX_RENDER_SIDE * w / h)
        .floor()
        .max(1.0) as u32;
    width_px.min(max)
}

/// Inverts RGB, leaving alpha, then softens pure black so a page reads as
/// a dark-grey surface rather than a black hole.
fn invert(rgba: &mut [u8]) {
    // A lookup table: one load per channel instead of a multiply and divide.
    const LUT: [u8; 256] = {
        let mut lut = [0; 256];
        let mut i = 0;
        while i < 256 {
            lut[i] = 24 + ((255 - i) * 206 / 255) as u8;
            i += 1;
        }
        lut
    };
    for px in rgba.as_chunks_mut::<4>().0 {
        for c in &mut px[..3] {
            *c = LUT[*c as usize];
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct CharBox {
    c: char,
    rect: NormRect,
}

/// A page's characters, and whether it also draws text PDFium reads as
/// blank (see `has_unreadable_text`).
fn char_boxes(doc: &PdfDocument, page: usize) -> Result<(Vec<CharBox>, bool), PdfiumError> {
    let pdf_page = load_page(doc, page)?;
    let size = (pdf_page.width().value, pdf_page.height().value);
    let text = pdf_page.text()?;
    let boxes = text
        .chars()
        .iter()
        .map(|ch| CharBox {
            c: ch.unicode_char().unwrap_or(' '),
            rect: ch
                .loose_bounds()
                .map(|r| normalize(r, size))
                .unwrap_or(NormRect {
                    x: 0.0,
                    y: 0.0,
                    width: 0.0,
                    height: 0.0,
                }),
        })
        .collect::<Vec<_>>();
    let unreadable = has_unreadable_text(&pdf_page, &text, &boxes);
    Ok((boxes, unreadable))
}

/// A page's character boxes, shared so handing them out copies nothing.
type Chars = Rc<[CharBox]>;

/// A page's character boxes, from the cache of recently selected pages.
/// `recover` gives the glyphs of a page whose text PDFium can't read;
/// it's only called for pages that draw such text.
fn cached_chars(
    doc: &PdfDocument,
    cache: &mut VecDeque<(usize, Chars)>,
    page: usize,
    recover: &mut impl FnMut(usize) -> Vec<Glyph>,
) -> Chars {
    if !cache.iter().any(|(p, _)| *p == page) {
        let (mut boxes, unreadable) = char_boxes(doc, page).unwrap_or_default();
        if unreadable {
            boxes = merge_recovered(boxes, recover(page));
        }
        cache.push_front((page, boxes.into()));
        cache.truncate(CHAR_CACHE_PAGES);
    }
    cache
        .iter()
        .find(|(p, _)| *p == page)
        .map(|(_, boxes)| Rc::clone(boxes))
        .unwrap_or_else(|| Rc::new([]))
}

/// The document's recovery of unreadable text: not needed yet, loaded,
/// or not possible.
enum Recovering {
    Unloaded,
    Ready(Box<Recovery>),
    Failed,
}

impl Recovering {
    fn glyphs(&mut self, path: &Path, password: Option<&str>, page: usize) -> Vec<Glyph> {
        if let Recovering::Unloaded = self {
            *self = Recovery::load(path, password)
                .map_or(Recovering::Failed, |r| Recovering::Ready(Box::new(r)));
        }
        match self {
            Recovering::Ready(recovery) => recovery.glyphs(page),
            _ => Vec::new(),
        }
    }
}

/// Whether a page draws text PDFium reads as blank: a text object with
/// visible bounds, no character inside, and no text, as with fonts that
/// map glyphs to spaces. Asking PDFium for an object's text is slow
/// (about 20 µs), so only objects with no character inside are asked.
fn has_unreadable_text(pdf_page: &PdfPage, page_text: &PdfPageText, boxes: &[CharBox]) -> bool {
    let (w, h) = (pdf_page.width().value, pdf_page.height().value);
    if !(w > 0.0 && h > 0.0) {
        return false;
    }
    // Centers of readable characters, sorted by height for range lookups.
    let mut centers: Vec<(f32, f32)> = boxes
        .iter()
        .filter(|b| !b.c.is_whitespace() && b.rect.width > 0.0)
        .map(|b| {
            (
                b.rect.y + b.rect.height / 2.0,
                b.rect.x + b.rect.width / 2.0,
            )
        })
        .collect();
    centers.sort_by(|a, b| a.0.total_cmp(&b.0));
    pdf_page.objects().iter().any(|object| {
        let Some(text) = object.as_text_object() else {
            return false;
        };
        let Ok(bounds) = object.bounds() else {
            return false;
        };
        if bounds.width().value <= 0.5 || bounds.height().value <= 0.5 {
            return false;
        }
        let r = normalize(bounds.to_rect(), (w, h));
        let from = centers.partition_point(|c| c.0 < r.y);
        let has_char = centers[from..]
            .iter()
            .take_while(|c| c.0 <= r.y + r.height)
            .any(|c| c.1 >= r.x && c.1 <= r.x + r.width);
        !has_char && page_text.for_object(text).trim().is_empty()
    })
}

fn is_break(c: char) -> bool {
    matches!(c, '\r' | '\n')
}

/// Horizontal space, as a page fraction, wider than letter spacing.
const WORD_GAP: f32 = 0.003;

/// Adds recovered glyphs to PDFium's characters. Glyphs PDFium already
/// has text for are dropped; the rest are grouped into runs (a formula),
/// which replace the blanks PDFium put there and go into reading order
/// after the text to their left on the same line.
fn merge_recovered(mut boxes: Vec<CharBox>, glyphs: Vec<Glyph>) -> Vec<CharBox> {
    let center = |r: &NormRect| (r.x + r.width / 2.0, r.y + r.height / 2.0);
    let inside = |r: &NormRect, (x, y): (f32, f32)| {
        x >= r.x && x <= r.x + r.width && y >= r.y && y <= r.y + r.height
    };
    let missing: Vec<Glyph> = glyphs
        .into_iter()
        .filter(|g| !g.c.is_whitespace())
        .filter(|g| {
            let c = center(&g.rect);
            !boxes
                .iter()
                .any(|b| !b.c.is_whitespace() && inside(&b.rect, c))
        })
        .collect();

    // Runs: glyphs next to each other on one line, in content order.
    let mut runs: Vec<Vec<Glyph>> = Vec::new();
    for glyph in missing {
        let joins = runs.last().and_then(|run| run.last()).is_some_and(|last| {
            let gap = glyph.rect.x - (last.rect.x + last.rect.width);
            let (_, cy) = center(&glyph.rect);
            let overlap = cy >= last.rect.y - last.rect.height / 2.0
                && cy <= last.rect.y + last.rect.height * 1.5;
            overlap && (-0.01..0.04).contains(&gap)
        });
        match runs.last_mut() {
            Some(run) if joins => run.push(glyph),
            _ => runs.push(vec![glyph]),
        }
    }

    for run in runs {
        let x0 = run.iter().map(|g| g.rect.x).fold(f32::MAX, f32::min);
        let x1 = run
            .iter()
            .map(|g| g.rect.x + g.rect.width)
            .fold(f32::MIN, f32::max);
        let y0 = run.iter().map(|g| g.rect.y).fold(f32::MAX, f32::min);
        let y1 = run
            .iter()
            .map(|g| g.rect.y + g.rect.height)
            .fold(f32::MIN, f32::max);
        let on_line = |b: &CharBox| {
            let (_, cy) = center(&b.rect);
            cy >= y0 && cy <= y1
        };
        // The blanks and line breaks PDFium made of the run. A line break
        // there is kept when it really ends the line: nothing on this line
        // follows it.
        let blank: Vec<bool> = (0..boxes.len())
            .map(|i| {
                let b = &boxes[i];
                let (cx, _) = center(&b.rect);
                let inside =
                    b.c.is_whitespace() && on_line(b) && cx > x0 + WORD_GAP && cx < x1 - WORD_GAP;
                let ends_line = is_break(b.c)
                    && !boxes[i + 1..]
                        .iter()
                        .find(|n| !is_break(n.c))
                        .is_some_and(on_line);
                inside && !ends_line
            })
            .collect();
        let mut index = 0;
        boxes.retain(|_| {
            index += 1;
            !blank[index - 1]
        });
        let at = boxes
            .iter()
            .rposition(|b| on_line(b) && !is_break(b.c) && center(&b.rect).0 <= x0)
            .map(|i| i + 1)
            .or_else(|| boxes.iter().position(on_line))
            .unwrap_or(boxes.len());
        let mut added: Vec<CharBox> = run
            .iter()
            .zip(recover::run_text(&run))
            .flat_map(|(g, text)| {
                let rect = g.rect;
                text.chars()
                    .map(move |c| CharBox { c, rect })
                    .collect::<Vec<_>>()
            })
            .collect();
        // Spaces where the run stands apart from the words around it.
        let spaced = |b: Option<&CharBox>, gap: f32| {
            b.is_some_and(|b| !b.c.is_whitespace() && on_line(b)) && gap > WORD_GAP
        };
        let before = at.checked_sub(1).and_then(|i| boxes.get(i));
        if spaced(
            before,
            before.map_or(0.0, |b| x0 - (b.rect.x + b.rect.width)),
        ) {
            let rect = NormRect {
                width: 0.0,
                ..run[0].rect
            };
            added.insert(0, CharBox { c: ' ', rect });
        }
        let after = boxes.get(at);
        if spaced(after, after.map_or(0.0, |b| b.rect.x - x1)) {
            let rect = NormRect {
                x: x1,
                width: 0.0,
                ..run[run.len() - 1].rect
            };
            added.push(CharBox { c: ' ', rect });
        }
        boxes.splice(at..at, added);
    }
    boxes
}

/// The first and last character of the line holding `chars[index]`: the
/// run of characters around it whose vertical centers overlap its own,
/// stopping at line breaks.
fn line_range(chars: &[CharBox], index: usize) -> (usize, usize) {
    let Some(anchor) = chars.get(index).map(|ch| ch.rect) else {
        return (index, index);
    };
    let center = anchor.y + anchor.height / 2.0;
    let same_line = |ch: &CharBox| {
        if ch.c == '\n' || ch.c == '\r' {
            return Some(false);
        }
        // Spaces often have no box; they don't end the line.
        if ch.rect.width <= 0.0 || ch.rect.height <= 0.0 {
            return None;
        }
        let other = ch.rect.y + ch.rect.height / 2.0;
        Some((center - other).abs() < anchor.height.max(ch.rect.height) / 2.0)
    };
    let mut a = index;
    while a > 0 && same_line(&chars[a - 1]) != Some(false) {
        a -= 1;
    }
    let mut b = index;
    while b + 1 < chars.len() && same_line(&chars[b + 1]) != Some(false) {
        b += 1;
    }
    (a, b)
}

/// Fills in the line each part starts on.
fn number_lines(
    mut parts: Vec<SelectionPart>,
    mut chars: impl FnMut(usize) -> Chars,
) -> Vec<SelectionPart> {
    for part in &mut parts {
        if let Some(first) = part.rects.first() {
            part.line = line_at(&chars(part.page), first.y + first.height / 2.0);
        }
    }
    parts
}

/// The one-based visual line at height `y` (page fraction): the number
/// of distinct text lines whose centers are at or above it. Lines side by
/// side, as in two columns, count once, so the number matches what a
/// reader counts down the page.
fn line_at(chars: &[CharBox], y: f32) -> u32 {
    let mut lines: Vec<(f32, f32)> = chars
        .iter()
        .filter(|ch| ch.rect.width > 0.0 && ch.rect.height > 0.0 && !ch.c.is_whitespace())
        .map(|ch| (ch.rect.y + ch.rect.height / 2.0, ch.rect.height))
        .collect();
    lines.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut count = 0;
    let mut last: Option<(f32, f32)> = None;
    for (center, height) in lines {
        if center > y + height / 4.0 {
            break;
        }
        match last {
            Some((c, h)) if center - c < h.max(height) / 2.0 => {}
            _ => {
                count += 1;
                last = Some((center, height));
            }
        }
    }
    count.max(1)
}

/// Selects text from one page point to another, possibly pages apart.
/// `chars` gives a page's character boxes.
fn select_pages(
    from: (usize, (f32, f32)),
    to: (usize, (f32, f32)),
    mut chars: impl FnMut(usize) -> Chars,
) -> Vec<SelectionPart> {
    if from.0 == to.0 {
        let boxes = chars(from.0);
        return select_text(&boxes, from.1, to.1)
            .map(|(rects, text)| SelectionPart {
                page: from.0,
                rects,
                text,
                line: 0,
            })
            .into_iter()
            .collect();
    }
    let (start, end) = if from.0 < to.0 {
        (from, to)
    } else {
        (to, from)
    };
    let last = end.0.min(start.0 + MAX_SELECT_PAGES - 1);
    (start.0..=last)
        .filter_map(|page| {
            let boxes = chars(page);
            let a = if page == start.0 {
                nearest_char(&boxes, start.1)?
            } else {
                0
            };
            let b = if page == end.0 {
                nearest_char(&boxes, end.1)?
            } else {
                boxes.len().checked_sub(1)?
            };
            let (rects, text) = select_range(&boxes, a, b)?;
            Some(SelectionPart {
                page,
                rects,
                text,
                line: 0,
            })
        })
        .collect()
}

/// Selects the characters between the ones nearest `from` and `to`.
/// Returns one rect per line and the selected text.
fn select_text(
    chars: &[CharBox],
    from: (f32, f32),
    to: (f32, f32),
) -> Option<(Vec<NormRect>, String)> {
    let a = nearest_char(chars, from)?;
    let b = nearest_char(chars, to)?;
    select_range(chars, a, b)
}

fn select_range(chars: &[CharBox], a: usize, b: usize) -> Option<(Vec<NormRect>, String)> {
    let range = chars.get(a.min(b)..=a.max(b))?;

    let mut rects: Vec<NormRect> = Vec::new();
    for ch in range
        .iter()
        .filter(|ch| !ch.c.is_whitespace() && ch.rect.width > 0.0)
    {
        let r = ch.rect;
        match rects.last_mut() {
            // Same line: vertical centers overlap, and the char is to the right.
            Some(line)
                if (line.y + line.height / 2.0 - (r.y + r.height / 2.0)).abs()
                    < line.height.max(r.height) / 2.0
                    && r.x >= line.x =>
            {
                let right = (line.x + line.width).max(r.x + r.width);
                let top = line.y.min(r.y);
                let bottom = (line.y + line.height).max(r.y + r.height);
                line.width = right - line.x;
                line.y = top;
                line.height = bottom - top;
            }
            _ => rects.push(r),
        }
    }
    let text: String = range.iter().map(|ch| ch.c).collect();
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let text = text.trim().to_owned();
    (!rects.is_empty()).then_some((rects, text))
}

fn nearest_char(chars: &[CharBox], (x, y): (f32, f32)) -> Option<usize> {
    let distance = |r: &NormRect| {
        let dx = (r.x - x).max(x - (r.x + r.width)).max(0.0);
        let dy = (r.y - y).max(y - (r.y + r.height)).max(0.0);
        // Favor the same line: vertical distance weighs more.
        dx + dy * 4.0
    };
    chars
        .iter()
        .enumerate()
        .filter(|(_, ch)| ch.rect.width > 0.0)
        .min_by(|(_, a), (_, b)| distance(&a.rect).total_cmp(&distance(&b.rect)))
        .map(|(i, _)| i)
}

/// Case-insensitive search of one page. Each match is one or more rects,
/// since a match can wrap across lines.
/// `cached` is the page's squashed text: when known, pages without the
/// query are skipped without loading them; when unknown, it is filled in.
fn search_page(
    doc: &PdfDocument,
    page: usize,
    query: &str,
    cached: Option<&mut Option<String>>,
) -> Result<Vec<Vec<NormRect>>, PdfiumError> {
    if let Some(Some(text)) = cached.as_deref() {
        if !text.contains(&squash(query)) {
            return Ok(Vec::new());
        }
    }
    let pdf_page = load_page(doc, page)?;
    let size = (pdf_page.width().value, pdf_page.height().value);
    let text = pdf_page.text()?;
    if let Some(slot @ None) = cached {
        *slot = Some(squash(&text.all()));
    }
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
    fn file_search_ignores_case_and_accents() {
        let query: Vec<char> = "licitacao".chars().map(fold).collect();
        let text = "Lei 14.133. A LICITAÇÃO é o procedimento.\r\nOutra licitação aqui.";
        let (count, snippet) = find_in_text(text, &query).unwrap();
        assert_eq!(count, 2);
        assert!(snippet.starts_with("Lei 14.133. A LICITAÇÃO"));
        assert!(!snippet.contains('\r'));
        assert!(find_in_text("nada", &query).is_none());
        let long = format!("{}needle{}", "a ".repeat(60), " b".repeat(60));
        let (_, snippet) = find_in_text(&long, &['n', 'e', 'e', 'd', 'l', 'e']).unwrap();
        assert!(snippet.starts_with('…') && snippet.ends_with('…'));
    }

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

    fn boxes(text: &str, y: f32) -> Vec<CharBox> {
        text.chars()
            .enumerate()
            .map(|(i, c)| CharBox {
                c,
                rect: NormRect {
                    x: 0.1 + i as f32 * 0.02,
                    y,
                    width: 0.02,
                    height: 0.03,
                },
            })
            .collect()
    }

    #[test]
    fn selects_across_lines() {
        let mut chars = boxes("hello world", 0.1);
        chars.push(CharBox {
            c: '\n',
            rect: NormRect {
                x: 0.0,
                y: 0.0,
                width: 0.0,
                height: 0.0,
            },
        });
        chars.extend(boxes("second", 0.2));
        // From inside "world" to the middle of "second".
        let (rects, text) = select_text(&chars, (0.23, 0.11), (0.15, 0.21)).unwrap();
        assert_eq!(text, "world\nsec");
        assert_eq!(rects.len(), 2);
        assert!((rects[0].x - 0.22).abs() < 1e-5);
        assert!((rects[0].width - 0.10).abs() < 1e-5);
        // Backwards drags select the same text.
        let (_, back) = select_text(&chars, (0.15, 0.21), (0.23, 0.11)).unwrap();
        assert_eq!(back, text);
        assert!(select_text(&[], (0.0, 0.0), (1.0, 1.0)).is_none());
    }

    #[test]
    fn selects_a_whole_line() {
        let mut chars = boxes("first line", 0.1);
        chars.push(CharBox {
            c: '\n',
            rect: NormRect {
                x: 0.0,
                y: 0.0,
                width: 0.0,
                height: 0.0,
            },
        });
        chars.extend(boxes("second", 0.2));
        // A line without a break char ends where the height changes.
        chars.extend(boxes("third", 0.3));
        assert_eq!(line_range(&chars, 3), (0, 9));
        let (a, b) = line_range(&chars, 13);
        let (_, text) = select_range(&chars, a, b).unwrap();
        assert_eq!(text, "second");
        assert_eq!(line_range(&[], 0), (0, 0));
    }

    #[test]
    fn counts_visual_lines() {
        let mut chars = boxes("one", 0.1);
        chars.extend(boxes("two", 0.2));
        // A second column beside line two counts once.
        chars.extend(boxes("col", 0.205));
        chars.extend(boxes("three", 0.3));
        assert_eq!(line_at(&chars, 0.115), 1);
        assert_eq!(line_at(&chars, 0.215), 2);
        assert_eq!(line_at(&chars, 0.315), 3);
        assert_eq!(line_at(&[], 0.5), 1);
    }

    #[test]
    fn only_web_links_are_safe() {
        assert!(safe_uri("https://example.com"));
        assert!(safe_uri("MAILTO:a@b.c"));
        assert!(!safe_uri("file:///C:/Windows/System32/cmd.exe"));
        assert!(!safe_uri("javascript:alert(1)"));
        assert!(!safe_uri(" https://x.com"));
        assert!(!safe_uri("https://x\n.com"));
    }

    #[test]
    fn selects_across_pages() {
        let pages = [
            boxes("first page", 0.1),
            boxes("middle", 0.1),
            boxes("last page", 0.1),
        ];
        let chars = |page: usize| Chars::from(pages[page].clone());
        // From "page" on page 0 to "last" on page 2, dragged backwards.
        let parts = select_pages((2, (0.165, 0.11)), (0, (0.225, 0.11)), chars);
        let text: Vec<&str> = parts.iter().map(|p| p.text.as_str()).collect();
        assert_eq!(text, ["page", "middle", "last"]);
        assert_eq!(parts.iter().map(|p| p.page).collect::<Vec<_>>(), [0, 1, 2]);
        assert!(select_pages((0, (0.0, 0.0)), (1, (0.0, 0.0)), |_| Chars::from([])).is_empty());
    }

    #[test]
    fn recovered_formulas_replace_the_blanks_pdfium_left() {
        let at = |c: char, x: f32, y: f32| CharBox {
            c,
            rect: NormRect {
                x,
                y,
                width: if c.is_whitespace() { 0.0 } else { 0.02 },
                height: 0.03,
            },
        };
        let glyph = |c: char, x: f32, y: f32, size: f32, baseline: f32| Glyph {
            c,
            rect: NormRect {
                x,
                y,
                width: 0.02,
                height: 0.03,
            },
            baseline,
            size,
        };
        let mut page = boxes("ab", 0.1);
        // PDFium's blanks inside the formula, including a bogus line break.
        page.extend([at(' ', 0.16, 0.1), at('\r', 0.17, 0.1), at('\n', 0.17, 0.1)]);
        page.extend([at(',', 0.25, 0.1), at('\r', 0.27, 0.1), at('\n', 0.27, 0.1)]);
        // A formula ending a line, after which PDFium's line break is real.
        page.extend(boxes("34", 0.3));
        page.extend([
            at('\r', 0.145, 0.3),
            at('\n', 0.145, 0.3),
            at('b', 0.1, 0.4),
        ]);
        let glyphs = vec![
            glyph('𝑥', 0.15, 0.1, 11.0, 0.125),
            glyph('2', 0.17, 0.095, 8.0, 0.115),
            glyph('=', 0.19, 0.1, 11.0, 0.125),
            glyph('0', 0.21, 0.1, 11.0, 0.125),
            // PDFium already reads this one.
            glyph('a', 0.10, 0.1, 11.0, 0.125),
            glyph('c', 0.15, 0.3, 11.0, 0.325),
            glyph('m', 0.17, 0.3, 11.0, 0.325),
        ];
        let text: String = merge_recovered(page, glyphs).iter().map(|b| b.c).collect();
        assert_eq!(text, "ab x²=0 ,\r\n34 cm\r\nb");
    }

    #[test]
    fn refined_search_skips_pages_without_the_shorter_query() {
        let mut first = SearchJob::new(1, "Foo".into(), 10, None);
        assert_eq!(first.pages, (0..10).collect::<Vec<_>>());
        // Stopped after page 5, with matches on 2 and 4.
        while first.next < 6 {
            first.next_page();
        }
        first.hits = vec![2, 4];
        let narrowed = SearchJob::new(2, "foobar".into(), 10, Some(first));
        assert_eq!(narrowed.pages, [2, 4, 6, 7, 8, 9]);
        let other = SearchJob::new(3, "baz".into(), 10, Some(narrowed));
        assert_eq!(other.pages.len(), 10);
        assert!(!refines("", "a"));
        assert_eq!(utf16le(&[b'i', 0, b'v', 0, 0, 0]), "iv");
        assert_eq!(squash("Hol-\r\nmes, É"), "holmesé");
    }

    #[test]
    fn huge_renders_are_capped() {
        assert_eq!(capped_width(1600, (595.0, 842.0)), 1600);
        let w = capped_width(100_000, (595.0, 842.0));
        let h = w as f32 * 842.0 / 595.0;
        assert!(w as f32 * h <= MAX_RENDER_PIXELS && w > 4000);
        // A very long, thin page is capped by area, not width.
        assert!(capped_width(2000, (100.0, 100_000.0)) < 200);
        // So is a very long one by its height.
        assert!(capped_width(2000, (100.0, 20_000.0)) as f32 * 200.0 <= MAX_RENDER_SIDE);
        assert_eq!(capped_width(0, (0.0, 0.0)), 1);
    }

    #[test]
    fn invert_softens() {
        let mut px = [255, 0, 128, 77];
        invert(&mut px);
        assert_eq!(px, [24, 230, 126, 77]);
    }

    /// Timings for a large document: `LEME_BENCH_PDF=<file> cargo test
    /// --release bench_document -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn bench_document() {
        use std::time::Instant;
        let (Ok(pdfium), Ok(path)) = (bind(), std::env::var("LEME_BENCH_PDF")) else {
            eprintln!("needs PDFium and LEME_BENCH_PDF");
            return;
        };
        let ms = |t: Instant| t.elapsed().as_secs_f64() * 1000.0;

        let t = Instant::now();
        let (doc, sizes) = open(&pdfium, Path::new(&path), None).unwrap();
        println!("open, {} pages: {:.1} ms", sizes.len(), ms(t));
        let t = Instant::now();
        let items = outline(&doc).len();
        println!("outline, {items} items: {:.1} ms", ms(t));

        let pages: Vec<usize> = (0..sizes.len())
            .step_by((sizes.len() / 20).max(1))
            .collect();
        for (label, width, forms) in [
            ("page render 1600 px", 1600, true),
            ("thumbnail 180 px", 180, false),
        ] {
            let t = Instant::now();
            for &page in &pages {
                render(&doc, page, width, false, forms).unwrap();
            }
            println!("{label}: {:.1} ms avg", ms(t) / pages.len() as f64);
        }
        let mut bitmap = render(&doc, 0, 1600, false, true).unwrap();
        let t = Instant::now();
        invert(&mut bitmap.rgba);
        println!("invert 1600 px: {:.2} ms", ms(t));
        let t = Instant::now();
        for page in 0..sizes.len() {
            search_page(&doc, page, "holmes", None).unwrap();
        }
        println!("search all pages: {:.0} ms", ms(t));
        // Typing "holmes" letter by letter, each search narrowing the last.
        let t = Instant::now();
        let mut last: Option<SearchJob> = None;
        let mut texts = vec![None; sizes.len()];
        for end in 1..="holmes".len() {
            let mut job = SearchJob::new(0, "holmes"[..end].into(), sizes.len(), last.take());
            while let Some(page) = job.next_page() {
                let text = texts.get_mut(page);
                if !search_page(&doc, page, &job.query, text)
                    .unwrap()
                    .is_empty()
                {
                    job.hits.push(page);
                }
            }
            last = Some(job);
        }
        println!("typed search, 6 queries: {:.0} ms", ms(t));
        let t = Instant::now();
        for page in 0..sizes.len() {
            search_page(&doc, page, "watson", texts.get_mut(page)).unwrap();
        }
        println!("search again, text cached: {:.0} ms", ms(t));
        let t = Instant::now();
        let labels = page_labels(&pdfium, Path::new(&path), None, sizes.len());
        println!("page labels: {:.1} ms", ms(t));
        for (page, label) in labels.iter().enumerate().step_by(97) {
            let slow = load_page(&doc, page)
                .ok()
                .and_then(|p| p.label().map(str::to_owned));
            assert_eq!(*label, slow.unwrap_or_default());
        }
    }

    /// Needs PDFium; skipped when the library can't be loaded.
    #[test]
    fn opens_and_renders_fixture() {
        let Ok(pdfium) = bind() else {
            eprintln!("PDFium unavailable, skipping");
            return;
        };
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/two-pages.pdf");
        let (doc, sizes) = open(&pdfium, &path, None).unwrap();
        assert!(outline(&doc).is_empty());
        assert_eq!(page_labels(&pdfium, &path, None, 2), vec![String::new(); 2]);
        assert_eq!(link_at(&doc, 0, (0.5, 0.5)), None);
        assert_eq!(sizes, vec![(200.0, 300.0); 2]);
        let bitmap = render(&doc, 1, 100, false, true).unwrap();
        assert_eq!((bitmap.width, bitmap.height), (100, 150));
        assert_eq!(bitmap.rgba.len(), 100 * 150 * 4);
        assert!(render(&doc, 5, 100, false, true).is_err());
        assert_eq!(
            search_page(&doc, 0, "anything", None).unwrap(),
            Vec::<Vec<_>>::new()
        );
    }
}
