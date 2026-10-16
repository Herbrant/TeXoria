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

use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use pdfium_render::prelude::*;

/// Hard cap on a single bitmap, to bound memory at extreme zoom levels.
const MAX_PIXELS: f32 = 48_000_000.0;
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
}

#[derive(Default)]
struct Shared {
    load: Option<(u64, Vec<u8>)>,
    wanted: Vec<RenderKey>,
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
