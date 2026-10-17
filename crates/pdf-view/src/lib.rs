//! PDF rasterization on a dedicated thread with PDFium.
//!
//! PDFium is not thread-safe and its documents borrow the library handle, so
//! one worker thread owns both. The UI pushes the set of pages it currently
//! wants (in priority order) and receives BGRA bitmaps — the pixel format
//! GPUI uploads directly.
//!
//! Documents are loaded from memory, never from the file on disk: on
//! Windows an open file handle would stop the TeX engine from overwriting
//! the PDF on the next compile.
//!
//! Besides bitmaps, the worker answers [`Query`]s about the loaded document:
//! its text, its links, a search, a snapshot of a page. Positions are in PDF
//! points from the page's top-left corner, y downwards (like SyncTeX's).

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use pdfium_render::prelude::*;

/// Hard cap on a single bitmap, to bound memory at extreme zoom levels.
const MAX_PIXELS: f32 = 48_000_000.0;
/// Search hits past this many are not reported.
const MAX_HITS: usize = 2000;
/// Hard cap on either side of a bitmap (degenerate page sizes).
const MAX_SIDE: f32 = 16_384.0;
/// Used for pages whose size is missing or nonsensical.
const FALLBACK_PAGE: PageSize = PageSize {
    width: 612.0,
    height: 792.0,
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not load the PDFium library: {0}")]
    Library(String),
}

/// Page size in PDF points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageSize {
    pub width: f32,
    pub height: f32,
}

/// Identifies one rendered bitmap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RenderKey {
    /// Document generation (bumped on every reload).
    pub generation: u64,
    pub page: usize,
    /// Device pixels per PDF point, in thousandths (keeps the key hashable).
    pub scale_milli: u32,
    /// Invert lightness for a dark theme, preserving hue.
    pub dark: bool,
}

impl RenderKey {
    pub fn scale(&self) -> f32 {
        self.scale_milli as f32 / 1000.0
    }
}

pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    /// Tightly packed BGRA8 (premultiplied alpha is irrelevant: pages are opaque).
    pub bgra: Vec<u8>,
}

/// A rectangle in PDF points, from the page's top-left corner.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x <= self.x + self.width && y >= self.y && y <= self.y + self.height
    }
}

/// A character of a page's text, in reading order.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextChar {
    pub ch: char,
    pub rect: Rect,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LinkTarget {
    /// A place in the document: a page, and how far down it (points).
    Page {
        page: usize,
        y: Option<f32>,
    },
    Uri(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Link {
    pub rect: Rect,
    pub target: LinkTarget,
}

/// A search hit: the rectangles of its text (one per line it spans).
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub page: usize,
    pub rects: Vec<Rect>,
}

/// A question about the loaded document, answered by an [`Event`].
#[derive(Debug, Clone, PartialEq)]
pub enum Query {
    /// The characters of a page → [`Event::PageText`].
    PageText { page: usize },
    /// The links of a page → [`Event::Links`].
    Links { page: usize },
    /// Every occurrence of `text` → [`Event::Found`].
    Search {
        id: u64,
        text: String,
        match_case: bool,
        whole_word: bool,
    },
    /// A page as a bitmap at `scale` pixels per point → [`Event::Snapshot`].
    Snapshot { id: u64, page: usize, scale: f32 },
    /// The text of a page, or of every page → [`Event::Text`].
    Text { id: u64, page: Option<usize> },
}

pub enum Event {
    Loaded {
        generation: u64,
        pages: Vec<PageSize>,
    },
    LoadFailed {
        generation: u64,
        error: String,
    },
    Rendered {
        key: RenderKey,
        bitmap: Bitmap,
    },
    PageText {
        generation: u64,
        page: usize,
        chars: Vec<TextChar>,
    },
    Links {
        generation: u64,
        page: usize,
        links: Vec<Link>,
    },
    Found {
        generation: u64,
        id: u64,
        hits: Vec<Hit>,
    },
    /// `None` if there is no document or no such page.
    Snapshot {
        id: u64,
        bitmap: Option<Bitmap>,
    },
    Text {
        id: u64,
        text: Option<String>,
    },
}

#[derive(Default)]
struct Shared {
    load: Option<(u64, Vec<u8>)>,
    wanted: Vec<RenderKey>,
    queries: VecDeque<Query>,
    shutdown: bool,
}

pub struct Renderer {
    shared: Arc<(Mutex<Shared>, Condvar)>,
    thread: Option<JoinHandle<()>>,
}

impl Renderer {
    /// Start the worker. `library_dirs` are searched for the PDFium shared
    /// library before falling back to the system library path.
    pub fn new(
        library_dirs: Vec<PathBuf>,
        on_event: impl Fn(Event) + Send + 'static,
    ) -> Result<Self, Error> {
        let bindings = bind(&library_dirs)?;
        let shared = Arc::new((Mutex::new(Shared::default()), Condvar::new()));
        let worker_shared = shared.clone();
        let thread = std::thread::Builder::new()
            .name("pdf-render".into())
            .spawn(move || worker(Pdfium::new(bindings), worker_shared, Box::new(on_event)))
            .expect("spawn pdf thread");
        Ok(Renderer {
            shared,
            thread: Some(thread),
        })
    }

    /// Replace the current document. Pending renders for older generations
    /// are dropped. If the new bytes do not open, the previous document stays
    /// loaded (and keeps serving its generation).
    pub fn load(&self, generation: u64, bytes: Vec<u8>) {
        let (lock, cvar) = &*self.shared;
        let mut s = lock.lock().unwrap();
        s.load = Some((generation, bytes));
        s.wanted.retain(|k| k.generation >= generation);
        cvar.notify_one();
    }

    /// Ask something about the loaded document; the answer comes as an event.
    pub fn query(&self, query: Query) {
        let (lock, cvar) = &*self.shared;
        let mut s = lock.lock().unwrap();
        // A newer search replaces one not started yet.
        if matches!(query, Query::Search { .. }) {
            s.queries.retain(|q| !matches!(q, Query::Search { .. }));
        }
        if !s.queries.contains(&query) {
            s.queries.push_back(query);
        }
        cvar.notify_one();
    }

    /// Set the bitmaps the view needs, highest priority first. Replaces any
    /// previous request, so pages scrolled out of view are never rendered.
    pub fn request(&self, keys: Vec<RenderKey>) {
        let (lock, cvar) = &*self.shared;
        let mut s = lock.lock().unwrap();
        s.wanted = keys;
        cvar.notify_one();
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        {
            let (lock, cvar) = &*self.shared;
            lock.lock().unwrap().shutdown = true;
            cvar.notify_one();
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn bind(dirs: &[PathBuf]) -> Result<Box<dyn PdfiumLibraryBindings>, Error> {
    let mut last_err = String::from("not found");
    for dir in dirs {
        let path = Pdfium::pdfium_platform_library_name_at_path(dir);
        if !path.exists() {
            continue;
        }
        match Pdfium::bind_to_library(&path) {
            Ok(b) => return Ok(b),
            Err(e) => last_err = format!("{}: {e}", path.display()),
        }
    }
    Pdfium::bind_to_system_library().map_err(|e| Error::Library(format!("{last_err}; system: {e}")))
}

fn worker(
    pdfium: Pdfium,
    shared: Arc<(Mutex<Shared>, Condvar)>,
    on_event: Box<dyn Fn(Event) + Send>,
) {
    let mut doc: Option<(u64, PdfDocument)> = None;
    loop {
        enum Work {
            Load(u64, Vec<u8>),
            Query(Query),
            Render(RenderKey),
        }
        let work = {
            let (lock, cvar) = &*shared;
            let mut s = lock.lock().unwrap();
            loop {
                if s.shutdown {
                    return;
                }
                if let Some((g, bytes)) = s.load.take() {
                    break Work::Load(g, bytes);
                }
                if let Some(q) = s.queries.pop_front() {
                    break Work::Query(q);
                }
                if !s.wanted.is_empty() {
                    break Work::Render(s.wanted.remove(0));
                }
                s = cvar.wait(s).unwrap();
            }
        };

        match work {
            Work::Load(generation, bytes) => match pdfium.load_pdf_from_byte_vec(bytes, None) {
                Ok(d) => {
                    let pages = d.pages().iter().map(|p| page_size(&p)).collect();
                    doc = Some((generation, d));
                    on_event(Event::Loaded { generation, pages });
                }
                Err(e) => on_event(Event::LoadFailed {
                    generation,
                    error: e.to_string(),
                }),
            },
            Work::Query(query) => {
                let doc = doc.as_ref().map(|(g, d)| (*g, d));
                on_event(answer(doc, query));
            }
            Work::Render(key) => {
                let Some((generation, d)) = &doc else {
                    continue;
                };
                if *generation != key.generation {
                    continue;
                }
                match render_page(d, key) {
                    Ok(bitmap) => on_event(Event::Rendered { key, bitmap }),
                    Err(e) => log::warn!("render page {}: {e}", key.page),
                }
            }
        }
    }
}

/// Answer a query about `doc` (with its generation).
fn answer(doc: Option<(u64, &PdfDocument)>, query: Query) -> Event {
    let generation = doc.map_or(0, |(g, _)| g);
    match query {
        Query::PageText { page } => Event::PageText {
            generation,
            page,
            chars: doc
                .and_then(|(_, d)| page_chars(d, page).ok())
                .unwrap_or_default(),
        },
        Query::Links { page } => Event::Links {
            generation,
            page,
            links: doc
                .and_then(|(_, d)| page_links(d, page).ok())
                .unwrap_or_default(),
        },
        Query::Search {
            id,
            text,
            match_case,
            whole_word,
        } => Event::Found {
            generation,
            id,
            hits: doc
                .map(|(_, d)| search(d, &text, match_case, whole_word))
                .unwrap_or_default(),
        },
        Query::Snapshot { id, page, scale } => Event::Snapshot {
            id,
            bitmap: doc.and_then(|(g, d)| {
                let key = RenderKey {
                    generation: g,
                    page,
                    scale_milli: (scale.clamp(0.1, 4.0) * 1000.0) as u32,
                    dark: false,
                };
                (page < d.pages().len() as usize)
                    .then(|| render_page(d, key).ok())
                    .flatten()
            }),
        },
        Query::Text { id, page } => Event::Text {
            id,
            text: doc.and_then(|(_, d)| page_texts(d, page)),
        },
    }
}

/// Converts PDFium's rectangles (y upwards) into top-down ones.
fn top_down(r: PdfRect, page_height: f32) -> Rect {
    Rect {
        x: r.left().value,
        y: page_height - r.top().value,
        width: r.width().value,
        height: r.height().value,
    }
}

fn page_chars(doc: &PdfDocument, page: usize) -> Result<Vec<TextChar>, PdfiumError> {
    let page = doc.pages().get(page as PdfPageIndex)?;
    let height = page.height().value;
    let text = page.text()?;
    Ok(text
        .chars()
        .iter()
        .filter_map(|c| {
            let ch = c.unicode_char()?;
            // Generated line breaks and spaces have no box; they still
            // separate words when text is copied.
            let rect = c
                .loose_bounds()
                .map(|r| top_down(r, height))
                .unwrap_or(Rect {
                    x: 0.0,
                    y: 0.0,
                    width: 0.0,
                    height: 0.0,
                });
            Some(TextChar { ch, rect })
        })
        .collect())
}

fn page_links(doc: &PdfDocument, index: usize) -> Result<Vec<Link>, PdfiumError> {
    let page = doc.pages().get(index as PdfPageIndex)?;
    let height = page.height().value;
    let pages = doc.pages();
    let mut out = Vec::new();
    for link in page.links().iter() {
        let Ok(rect) = link.rect() else {
            continue;
        };
        let action = link.action();
        let destination = link.destination().or_else(|| {
            action
                .as_ref()
                .and_then(|a| a.as_local_destination_action()?.destination().ok())
        });
        let target = if let Some(dest) = destination {
            let Ok(target_page) = dest.page_index() else {
                continue;
            };
            let target_height = pages
                .get(target_page)
                .map(|p| p.height().value)
                .unwrap_or(0.0);
            let y = match dest.view_settings() {
                Ok(PdfDestinationViewSettings::SpecificCoordinatesAndZoom(_, Some(y), _))
                | Ok(PdfDestinationViewSettings::FitPageHorizontallyToWindow(Some(y))) => {
                    Some(target_height - y.value)
                }
                Ok(PdfDestinationViewSettings::FitPageToRectangle(r)) => {
                    Some(target_height - r.top().value)
                }
                _ => None,
            };
            LinkTarget::Page {
                page: target_page as usize,
                y,
            }
        } else if let Some(uri) = action
            .as_ref()
            .and_then(|a| a.as_uri_action().and_then(|u| u.uri().ok()))
        {
            LinkTarget::Uri(uri)
        } else {
            continue;
        };
        out.push(Link {
            rect: top_down(rect, height),
            target,
        });
    }
    Ok(out)
}

fn search(doc: &PdfDocument, text: &str, match_case: bool, whole_word: bool) -> Vec<Hit> {
    let options = PdfSearchOptions::new()
        .match_case(match_case)
        .match_whole_word(whole_word);
    let mut hits = Vec::new();
    for (index, page) in doc.pages().iter().enumerate() {
        let height = page.height().value;
        let Ok(page_text) = page.text() else {
            continue;
        };
        let Ok(search) = page_text.search(text, &options) else {
            continue;
        };
        while let Some(segments) = search.find_next() {
            let rects = segments
                .iter()
                .map(|s| top_down(s.bounds(), height))
                .collect();
            hits.push(Hit { page: index, rects });
            if hits.len() >= MAX_HITS {
                return hits;
            }
        }
    }
    hits
}

/// The text of one page, or of every page separated by form feeds.
fn page_texts(doc: &PdfDocument, page: Option<usize>) -> Option<String> {
    let pages = doc.pages();
    let text_of = |p: PdfPage| p.text().ok().map(|t| t.all());
    match page {
        Some(i) => text_of(pages.get(i as PdfPageIndex).ok()?),
        None => Some(
            pages
                .iter()
                .filter_map(text_of)
                .collect::<Vec<_>>()
                .join("\n\u{c}\n"),
        ),
    }
}

/// The page size in points, sanitized: a crafted PDF can claim zero,
/// negative or non-finite dimensions.
fn page_size(page: &PdfPage) -> PageSize {
    let (width, height) = (page.width().value, page.height().value);
    let sane = |v: f32| v.is_finite() && v > 0.0;
    if sane(width) && sane(height) {
        PageSize { width, height }
    } else {
        FALLBACK_PAGE
    }
}

/// Bitmap size for a page of `w_pt` × `h_pt` points at `scale`, bounded in
/// area and per side.
fn bitmap_size(w_pt: f32, h_pt: f32, scale: f32) -> (i32, i32) {
    let mut scale = scale;
    let pixels = w_pt * h_pt * scale * scale;
    if pixels > MAX_PIXELS {
        scale *= (MAX_PIXELS / pixels).sqrt();
    }
    let longest = w_pt.max(h_pt) * scale;
    if longest > MAX_SIDE {
        scale *= MAX_SIDE / longest;
    }
    let side = |pt: f32| (pt * scale).round().clamp(1.0, MAX_SIDE) as i32;
    (side(w_pt), side(h_pt))
}

fn render_page(doc: &PdfDocument, key: RenderKey) -> Result<Bitmap, PdfiumError> {
    let page = doc.pages().get(key.page as PdfPageIndex)?;
    let PageSize { width, height } = page_size(&page);
    let (width, height) = bitmap_size(width, height, key.scale());
    let config = PdfRenderConfig::new()
        .set_fixed_size(width, height)
        .set_format(PdfBitmapFormat::BGRA)
        .render_annotations(true)
        .set_clear_color(PdfColor::WHITE);
    let bitmap = page.render_with_config(&config)?;
    let (bw, bh) = (bitmap.width() as u32, bitmap.height() as u32);
    let mut bgra = bitmap.as_raw_bytes();
    // PDFium rows are 4-byte aligned; with 4 bytes per pixel there is no
    // padding, but guard against a surprising stride anyway.
    bgra.truncate(bw as usize * bh as usize * 4);
    if key.dark {
        darken(&mut bgra);
    }
    Ok(Bitmap {
        width: bw,
        height: bh,
        bgra,
    })
}

/// Invert lightness while keeping hue and saturation, then compress the
/// range so white paper becomes a soft charcoal rather than pure black.
fn darken(bgra: &mut [u8]) {
    const BG: u16 = 32;
    const FG: u16 = 228;
    for px in bgra.as_chunks_mut::<4>().0 {
        let (b, g, r) = (px[0] as u16, px[1] as u16, px[2] as u16);
        let max = b.max(g).max(r);
        let min = b.min(g).min(r);
        // HSL lightness inversion: c' = c + 255 - (max + min)
        let shift = 255 - (max + min) as i32;
        for c in &mut px[..3] {
            let inv = (*c as i32 + shift).clamp(0, 255) as u16;
            *c = (BG + inv * (FG - BG) / 255) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn bitmap_size_is_bounded() {
        assert_eq!(bitmap_size(612.0, 792.0, 2.0), (1224, 1584));
        // Extreme aspect ratio: area is small, but one side would explode.
        let (w, h) = bitmap_size(1.0e9, 1.0e-3, 1.0);
        assert!(w as f32 <= MAX_SIDE && h >= 1, "{w}x{h}");
        let (w, h) = bitmap_size(14_400.0, 14_400.0, 6.0);
        assert!((w as f32) * (h as f32) <= MAX_PIXELS * 1.01);
    }

    #[test]
    fn darken_preserves_hue() {
        // white, black, pure red (BGRA)
        let mut px = vec![255, 255, 255, 255, 0, 0, 0, 255, 0, 0, 255, 255];
        darken(&mut px);
        assert_eq!(&px[0..3], &[32, 32, 32]);
        assert_eq!(&px[4..7], &[228, 228, 228]);
        // Red stays red (lightness 50% is a fixed point).
        assert!(px[10] > px[8] && px[10] > px[9]);
    }

    /// A renderer with `tests/fixtures/links/links.pdf` loaded, if PDFium is available.
    fn links_pdf() -> Option<(Renderer, mpsc::Receiver<Event>)> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let bytes = std::fs::read(root.join("tests/fixtures/links/links.pdf")).unwrap();
        let (tx, rx) = mpsc::channel();
        let Ok(r) = Renderer::new(vec![root.join("vendor/pdfium")], move |e| {
            let _ = tx.send(e);
        }) else {
            eprintln!("PDFium not available; skipping");
            return None;
        };
        r.load(1, bytes);
        match rx.recv_timeout(Duration::from_secs(10)).unwrap() {
            Event::Loaded { pages, .. } => assert_eq!(pages.len(), 2),
            _ => panic!("expected Loaded"),
        }
        Some((r, rx))
    }

    #[test]
    fn answers_queries() {
        let Some((r, rx)) = links_pdf() else {
            return;
        };
        let next = || rx.recv_timeout(Duration::from_secs(10)).unwrap();

        r.query(Query::Links { page: 0 });
        let Event::Links { page: 0, links, .. } = next() else {
            panic!("expected Links")
        };
        assert!(
            links
                .iter()
                .any(|l| matches!(l.target, LinkTarget::Page { page: 1, .. })),
            "{links:?}"
        );
        assert!(
            links
                .iter()
                .any(|l| l.target == LinkTarget::Uri("https://example.org".into()))
        );
        // Top-down coordinates: the links are near the top of the page.
        assert!(links.iter().all(|l| l.rect.y < 300.0 && l.rect.width > 0.0));

        r.query(Query::Search {
            id: 7,
            text: "needle".into(),
            match_case: false,
            whole_word: false,
        });
        let Event::Found { id: 7, hits, .. } = next() else {
            panic!("expected Found")
        };
        assert_eq!(hits.iter().map(|h| h.page).collect::<Vec<_>>(), vec![0, 1]);

        r.query(Query::PageText { page: 0 });
        let Event::PageText { chars, .. } = next() else {
            panic!("expected PageText")
        };
        let text: String = chars.iter().map(|c| c.ch).collect();
        assert!(text.contains("The needle is here."), "{text}");

        r.query(Query::Text { id: 3, page: None });
        let Event::Text {
            id: 3,
            text: Some(all),
        } = next()
        else {
            panic!("expected Text")
        };
        assert!(all.contains("Another needle"));

        r.query(Query::Snapshot {
            id: 9,
            page: 1,
            scale: 1.0,
        });
        let Event::Snapshot {
            id: 9,
            bitmap: Some(b),
        } = next()
        else {
            panic!("expected Snapshot")
        };
        assert_eq!(b.width, 612);
        r.query(Query::Snapshot {
            id: 10,
            page: 5,
            scale: 1.0,
        });
        assert!(matches!(
            next(),
            Event::Snapshot {
                id: 10,
                bitmap: None
            }
        ));
    }

    /// Renders the sample PDF if PDFium and a compiled sample are available.
    #[test]
    fn renders_sample_pdf() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let pdf = root.join("tests/fixtures/sample/.texoria/build/main.pdf");
        let Ok(bytes) = std::fs::read(&pdf) else {
            eprintln!("sample PDF not built; skipping");
            return;
        };
        let (tx, rx) = mpsc::channel();
        let Ok(r) = Renderer::new(vec![root.join("vendor/pdfium")], move |e| {
            let _ = tx.send(e);
        }) else {
            eprintln!("PDFium not available; skipping");
            return;
        };
        r.load(1, bytes);
        let pages = match rx.recv_timeout(Duration::from_secs(10)).unwrap() {
            Event::Loaded {
                generation: 1,
                pages,
            } => pages,
            _ => panic!("expected Loaded"),
        };
        assert_eq!(pages.len(), 2);
        assert!((pages[0].width - 612.0).abs() < 1.0, "US letter width");

        let key = RenderKey {
            generation: 1,
            page: 0,
            scale_milli: 2000,
            dark: false,
        };
        r.request(vec![key]);
        match rx.recv_timeout(Duration::from_secs(10)).unwrap() {
            Event::Rendered { key: k, bitmap } => {
                assert_eq!(k, key);
                assert_eq!(bitmap.width, 1224);
                assert_eq!(
                    bitmap.bgra.len(),
                    (bitmap.width * bitmap.height * 4) as usize
                );
                // Mostly white paper.
                let white = bitmap.bgra.chunks(4).filter(|p| p[0] > 250).count();
                assert!(white * 10 > (bitmap.width * bitmap.height * 8) as usize);
            }
            _ => panic!("expected Rendered"),
        }
    }
}
