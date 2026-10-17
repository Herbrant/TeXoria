//! The PDF preview: continuous scrolling, zoom, flicker-free reloads and
//! SyncTeX forward/inverse search.
//!
//! Only pages near the viewport get elements and bitmaps. On reload the
//! previous bitmaps stay on screen until the new ones are rasterized, so the
//! preview never flashes blank while you type.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{
    ActiveTheme, Icon, Selectable as _, Sizable, button::Button, button::ButtonVariants, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pdf_view::{
    Bitmap, Event as RenderEvent, Hit, Link, LinkTarget, PageSize, Query, RenderKey, Renderer,
    TextChar,
};
use synctex::{Rect, SyncTex};

use crate::assets::Glyph;

const MARGIN: f32 = 28.0;
const GAP: f32 = 18.0;
const MIN_ZOOM: f32 = 0.25;
const MAX_ZOOM: f32 = 6.0;
/// Pages around the viewport kept rasterized (and their bitmaps retained).
const RENDER_AHEAD: usize = 1;
const KEEP_AROUND: usize = 6;

actions!(
    pdf,
    [
        NextPage,
        PreviousPage,
        FirstPage,
        LastPage,
        GoToPage,
        FitPage,
        FindInPdf,
        CloseFind,
        CopyText
    ]
);

#[derive(Debug, Clone, Copy, PartialEq)]
enum Zoom {
    FitWidth,
    /// The whole page in the pane.
    FitPage,
    Fixed(f32),
}

pub enum PdfPaneEvent {
    /// The user cmd/ctrl-clicked a point that maps back to the source.
    InverseSearch { file: PathBuf, line: u32 },
    /// Save a copy of the PDF somewhere.
    Export,
    /// Show the PDF full screen, from the current page.
    Present,
}

struct Shown {
    key: RenderKey,
    image: Arc<RenderImage>,
}

/// A page shown by the presentation window, rendered by this pane's renderer.
struct Presenting {
    page: usize,
    scale_milli: u32,
    image: Option<Arc<RenderImage>>,
    /// The bitmap `image` is, or the one awaited.
    key: Option<RenderKey>,
}

/// Text selected by dragging on a page, between two points (PDF points).
struct Selection {
    page: usize,
    anchor: (f32, f32),
    head: (f32, f32),
    dragging: bool,
}

/// The find bar: its field and what it found.
struct Find {
    input: Entity<InputState>,
    match_case: bool,
    hits: Vec<Hit>,
    current: Option<usize>,
    /// Of the search whose hits are awaited.
    id: u64,
    searching: bool,
    _subscription: Subscription,
    _debounce: Task<()>,
}

struct Highlight {
    page: usize,
    rects: Vec<Rect>,
    id: usize,
}

pub struct PdfPane {
    renderer: Option<Renderer>,
    error: Option<SharedString>,
    /// Bumped on every `load`.
    generation: u64,
    /// Generation of the document the renderer actually holds: a reload
    /// that fails to open keeps the previous one.
    doc_generation: u64,
    pages: Vec<PageSize>,
    shown: HashMap<usize, Shown>,
    synctex: Option<Arc<SyncTex>>,
    /// SyncTeX data for an in-flight load, adopted together with its PDF.
    pending_synctex: Option<Arc<SyncTex>>,
    zoom: Zoom,
    scroll: ScrollHandle,
    viewport: Size<Pixels>,
    highlight: Option<Highlight>,
    highlight_seq: usize,
    dark: bool,
    to_drop: Vec<Arc<RenderImage>>,
    last_request: Vec<RenderKey>,
    /// Scroll position to restore once the new layout is known.
    pending_scroll: Option<Point<Pixels>>,
    /// Taken on a click, so that ⌘= and ⌘- zoom the PDF instead of resizing
    /// the editor's text.
    focus: FocusHandle,
    /// The links of the pages asked for so far (they are asked once).
    links: HashMap<usize, Vec<Link>>,
    asked_links: HashSet<usize>,
    find: Option<Find>,
    find_seq: u64,
    /// The characters of the pages asked for so far (to select text).
    texts: HashMap<usize, Vec<TextChar>>,
    asked_text: HashSet<usize>,
    selection: Option<Selection>,
    presenting: Option<Presenting>,
    /// The bitmaps the pane itself wants (the presentation's come first).
    pane_wanted: Vec<RenderKey>,
    /// Snapshots and texts asked for by the agents' tools.
    snapshots: HashMap<u64, smol::channel::Sender<Option<Bitmap>>>,
    texts_wanted: HashMap<u64, smol::channel::Sender<Option<String>>>,
    query_seq: u64,
    /// The "go to page" field of the toolbar, while it is open.
    page_input: Option<Entity<InputState>>,
    _page_input: Option<Subscription>,
    _events: Task<()>,
    _highlight_timer: Task<()>,
}

impl EventEmitter<PdfPaneEvent> for PdfPane {}

impl PdfPane {
    pub fn new(library_dirs: Vec<PathBuf>, cx: &mut Context<Self>) -> Self {
        let (tx, rx) = smol::channel::unbounded::<RenderEvent>();
        let (renderer, error) = match Renderer::new(library_dirs, move |e| {
            let _ = tx.send_blocking(e);
        }) {
            Ok(r) => (Some(r), None),
            Err(e) => {
                log::error!("{e}");
                (
                    None,
                    Some(SharedString::from(format!("PDF preview unavailable: {e}"))),
                )
            }
        };
        let events = cx.spawn(async move |this, cx| {
            while let Ok(event) = rx.recv().await {
                if this
                    .update(cx, |this, cx| this.on_render_event(event, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        PdfPane {
            renderer,
            error,
            generation: 0,
            doc_generation: 0,
            pages: Vec::new(),
            shown: HashMap::new(),
            synctex: None,
            pending_synctex: None,
            zoom: Zoom::FitWidth,
            scroll: ScrollHandle::new(),
            viewport: Size::default(),
            highlight: None,
            highlight_seq: 0,
            dark: false,
            to_drop: Vec::new(),
            last_request: Vec::new(),
            pending_scroll: None,
            focus: cx.focus_handle(),
            links: HashMap::new(),
            asked_links: HashSet::new(),
            find: None,
            find_seq: 0,
            texts: HashMap::new(),
            asked_text: HashSet::new(),
            selection: None,
            presenting: None,
            pane_wanted: Vec::new(),
            snapshots: HashMap::new(),
            texts_wanted: HashMap::new(),
            query_seq: 0,
            page_input: None,
            _page_input: None,
            _events: events,
            _highlight_timer: Task::ready(()),
        }
    }

    /// Load (or reload) a PDF and its SyncTeX data from disk.
    pub fn load(&mut self, pdf: PathBuf, synctex: Option<PathBuf>, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            let loaded = cx
                .background_spawn(async move {
                    let bytes = std::fs::read(&pdf);
                    let st = synctex.and_then(|p| SyncTex::open(&p).ok()).map(Arc::new);
                    (bytes, st)
                })
                .await;
            this.update(cx, |this, cx| {
                // A newer load superseded this one while it was reading.
                if this.generation != generation {
                    return;
                }
                match loaded {
                    (Ok(bytes), st) => {
                        this.pending_synctex = st;
                        if let Some(r) = &this.renderer {
                            r.load(generation, bytes);
                        }
                    }
                    (Err(e), _) => log::warn!("reading PDF: {e}"),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn on_render_event(&mut self, event: RenderEvent, cx: &mut Context<Self>) {
        match event {
            RenderEvent::Loaded { generation, pages } => {
                if generation != self.generation {
                    return;
                }
                // Keep the reading position across reloads.
                if !self.pages.is_empty() {
                    self.pending_scroll = Some(self.scroll.offset());
                }
                self.doc_generation = generation;
                self.links.clear();
                self.asked_links.clear();
                self.texts.clear();
                self.asked_text.clear();
                self.selection = None;
                // The presented page is rendered again from the new document.
                if let Some(p) = &mut self.presenting {
                    p.key = None;
                }
                // Hits move with the text.
                self.search(Duration::ZERO, cx);
                self.synctex = self.pending_synctex.take();
                self.pages = pages;
                self.error = None;
                self.last_request.clear();
                // Bitmaps for pages that no longer exist must go.
                let n = self.pages.len();
                let gone: Vec<usize> = self.shown.keys().copied().filter(|p| *p >= n).collect();
                for p in gone {
                    if let Some(s) = self.shown.remove(&p) {
                        self.to_drop.push(s.image);
                    }
                }
                if let Some(p) = &mut self.presenting {
                    p.page = p.page.min(self.pages.len().saturating_sub(1));
                }
                self.send_requests();
            }
            RenderEvent::LoadFailed { generation, error } => {
                if generation == self.generation {
                    log::warn!("opening PDF: {error}");
                    if self.pages.is_empty() {
                        self.error = Some(format!("Could not open PDF: {error}").into());
                    }
                    // The renderer dropped queued requests when the load
                    // arrived; ask again for the still-loaded document.
                    self.last_request.clear();
                }
            }
            RenderEvent::Rendered { key, bitmap } => {
                if key.page >= self.pages.len() {
                    return;
                }
                // The presentation window draws (and frees) its bitmaps itself.
                if let Some(p) = self.presenting.as_mut().filter(|p| p.key == Some(key))
                    && !self.pane_wanted.contains(&key)
                {
                    p.image = to_render_image(bitmap).or(p.image.take());
                    cx.notify();
                    return;
                }
                let newer = self.shown.get(&key.page).is_none_or(|s| {
                    key.generation > s.key.generation
                        || (key.generation == s.key.generation && key != s.key)
                });
                if !newer {
                    return;
                }
                if let Some(image) = to_render_image(bitmap)
                    && let Some(old) = self.shown.insert(key.page, Shown { key, image })
                {
                    self.to_drop.push(old.image);
                }
            }
            RenderEvent::Links {
                generation,
                page,
                links,
            } => {
                if generation == self.doc_generation {
                    self.links.insert(page, links);
                }
            }
            RenderEvent::Found { id, hits, .. } => {
                if let Some(f) = self.find.as_mut().filter(|f| f.id == id) {
                    f.searching = false;
                    f.hits = hits;
                    // The first hit from where the reader is.
                    let here = self.current_page();
                    let f = self.find.as_mut().unwrap();
                    f.current = f
                        .hits
                        .iter()
                        .position(|h| h.page >= here)
                        .or((!f.hits.is_empty()).then_some(0));
                    self.reveal_hit(cx);
                }
            }
            RenderEvent::PageText {
                generation,
                page,
                chars,
            } => {
                if generation == self.doc_generation {
                    self.texts.insert(page, chars);
                }
            }
            RenderEvent::Snapshot { id, bitmap } => {
                if let Some(tx) = self.snapshots.remove(&id) {
                    let _ = tx.try_send(bitmap);
                }
                return;
            }
            RenderEvent::Text { id, text } => {
                if let Some(tx) = self.texts_wanted.remove(&id) {
                    let _ = tx.try_send(text);
                }
                return;
            }
        }
        cx.notify();
    }

    pub fn set_dark(&mut self, dark: bool, cx: &mut Context<Self>) {
        if self.dark != dark {
            self.dark = dark;
            self.last_request.clear();
            cx.notify();
        }
    }

    // ── Geometry ─────────────────────────────────────────────────────────

    fn zoom_factor(&self) -> f32 {
        match self.zoom {
            Zoom::Fixed(z) => z,
            Zoom::FitPage => {
                let widest = self.pages.iter().map(|p| p.width).fold(0.0, f32::max);
                let tallest = self.pages.iter().map(|p| p.height).fold(0.0, f32::max);
                if widest <= 0.0 || tallest <= 0.0 || self.viewport.height <= px(0.) {
                    return 1.0;
                }
                let w = (f32::from(self.viewport.width) - 2.0 * MARGIN) / widest;
                let h = (f32::from(self.viewport.height) - 2.0 * MARGIN) / tallest;
                w.min(h).clamp(MIN_ZOOM, MAX_ZOOM)
            }
            Zoom::FitWidth => {
                let widest = self.pages.iter().map(|p| p.width).fold(0.0, f32::max);
                if widest <= 0.0 || self.viewport.width <= px(0.) {
                    return 1.0;
                }
                ((f32::from(self.viewport.width) - 2.0 * MARGIN) / widest).clamp(MIN_ZOOM, MAX_ZOOM)
            }
        }
    }

    /// Page rectangles in content coordinates (pixels), plus content size.
    fn layout(&self, zoom: f32) -> (Vec<Bounds<Pixels>>, Size<Pixels>) {
        let widest = self
            .pages
            .iter()
            .map(|p| p.width * zoom)
            .fold(0.0, f32::max);
        let content_w = (widest + 2.0 * MARGIN).max(f32::from(self.viewport.width));
        let mut y = MARGIN;
        let rects = self
            .pages
            .iter()
            .map(|p| {
                let (w, h) = (p.width * zoom, p.height * zoom);
                let r = Bounds::new(point(px((content_w - w) / 2.0), px(y)), size(px(w), px(h)));
                y += h + GAP;
                r
            })
            .collect();
        (rects, size(px(content_w), px(y - GAP + MARGIN)))
    }

    fn set_zoom(&mut self, zoom: Zoom, anchor: Option<Point<Pixels>>, cx: &mut Context<Self>) {
        let old = self.zoom_factor();
        self.zoom = match zoom {
            Zoom::Fixed(z) => Zoom::Fixed(z.clamp(MIN_ZOOM, MAX_ZOOM)),
            z => z,
        };
        let new = self.zoom_factor();
        // Keep the content point under the anchor (or the viewport center)
        // fixed while zooming.
        let anchor = anchor.unwrap_or(point(self.viewport.width / 2.0, self.viewport.height / 2.0));
        let offset = self.scroll.offset();
        let ratio = new / old;
        let cx_ = (anchor.x - offset.x) * ratio - anchor.x;
        let cy_ = (anchor.y - offset.y) * ratio - anchor.y;
        self.pending_scroll = Some(point(-cx_, -cy_));
        cx.notify();
    }

    pub fn zoom_in(&mut self, cx: &mut Context<Self>) {
        let z = self.zoom_factor() * 1.2;
        self.set_zoom(Zoom::Fixed(z), None, cx);
    }

    pub fn zoom_out(&mut self, cx: &mut Context<Self>) {
        let z = self.zoom_factor() / 1.2;
        self.set_zoom(Zoom::Fixed(z), None, cx);
    }

    pub fn zoom_to_fit(&mut self, cx: &mut Context<Self>) {
        self.set_zoom(Zoom::FitWidth, None, cx);
    }

    pub fn fit_page(&mut self, cx: &mut Context<Self>) {
        self.set_zoom(Zoom::FitPage, None, cx);
        let page = self.current_page();
        self.go_to(page, None, cx);
    }

    // ── Pages ────────────────────────────────────────────────────────────

    /// The page in the middle of the viewport (0-based).
    pub fn current_page(&self) -> usize {
        let (rects, _) = self.layout(self.zoom_factor());
        let middle = -self.scroll.offset().y + self.viewport.height / 2.0;
        rects
            .iter()
            .position(|r| r.bottom() + px(GAP) >= middle)
            .unwrap_or(rects.len().saturating_sub(1))
    }

    /// Scroll to `page` (0-based), at `y` points from its top if given.
    pub fn go_to(&mut self, page: usize, y: Option<f32>, cx: &mut Context<Self>) {
        let zoom = self.zoom_factor();
        let (rects, _) = self.layout(zoom);
        let Some(r) = rects.get(page.min(rects.len().saturating_sub(1))) else {
            return;
        };
        let top = match y {
            Some(y) => r.origin.y + px(y * zoom) - px(MARGIN),
            None => r.origin.y - px(MARGIN),
        };
        let offset = self.scroll.offset();
        self.pending_scroll = Some(point(offset.x, -top.max(px(0.))));
        cx.notify();
    }

    fn step_page(&mut self, delta: isize, cx: &mut Context<Self>) {
        let page = (self.current_page() as isize + delta).max(0) as usize;
        self.go_to(page, None, cx);
    }

    // ── Find ─────────────────────────────────────────────────────────────

    /// Show the find bar (⌘F in the preview) with its text selected.
    pub fn open_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.find.is_none() {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder("Find in PDF"));
            let subscription =
                cx.subscribe_in(&input, window, |this, _, ev: &InputEvent, _, cx| match ev {
                    InputEvent::Change => this.search(Duration::from_millis(250), cx),
                    InputEvent::PressEnter { shift, .. } => {
                        this.next_hit(if *shift { -1 } else { 1 }, cx)
                    }
                    _ => {}
                });
            self.find = Some(Find {
                input,
                match_case: false,
                hits: Vec::new(),
                current: None,
                id: 0,
                searching: false,
                _subscription: subscription,
                _debounce: Task::ready(()),
            });
        }
        if let Some(f) = &self.find {
            f.input.update(cx, |i, cx| {
                i.focus(window, cx);
                i.select_all(window, cx);
            });
        }
        cx.notify();
    }

    fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.find.take().is_some() {
            window.focus(&self.focus, cx);
            cx.notify();
        }
    }

    /// Search for the find bar's text after `delay`.
    fn search(&mut self, delay: Duration, cx: &mut Context<Self>) {
        let Some(f) = &mut self.find else {
            return;
        };
        self.find_seq += 1;
        let id = self.find_seq;
        f.id = id;
        let text = f.input.read(cx).value().to_string();
        let match_case = f.match_case;
        if text.trim().is_empty() {
            f.hits.clear();
            f.current = None;
            f.searching = false;
            cx.notify();
            return;
        }
        f.searching = true;
        f._debounce = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            this.update(cx, |this, _| {
                if let Some(r) = &this.renderer {
                    r.query(Query::Search {
                        id,
                        text,
                        match_case,
                        whole_word: false,
                    });
                }
            })
            .ok();
        });
        cx.notify();
    }

    fn next_hit(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(f) = &mut self.find else {
            return;
        };
        let n = f.hits.len() as isize;
        if n == 0 {
            return;
        }
        f.current = Some(f.current.map_or(0, |c| (c as isize + delta).rem_euclid(n)) as usize);
        self.reveal_hit(cx);
    }

    /// Scroll the current hit into view, a third of the way down.
    fn reveal_hit(&mut self, cx: &mut Context<Self>) {
        let Some(hit) = self
            .find
            .as_ref()
            .and_then(|f| f.hits.get(f.current?))
            .cloned()
        else {
            cx.notify();
            return;
        };
        let y = hit.rects.first().map_or(0.0, |r| r.y);
        let above = f32::from(self.viewport.height) * 0.33 / self.zoom_factor();
        self.go_to(hit.page, Some((y - above).max(0.0)), cx);
    }

    fn render_find(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let f = self.find.as_ref()?;
        let theme = cx.theme();
        let count: SharedString = if f.searching {
            "…".into()
        } else if f.input.read(cx).value().is_empty() {
            "".into()
        } else if f.hits.is_empty() {
            "No results".into()
        } else {
            format!("{} of {}", f.current.map_or(0, |c| c + 1), f.hits.len()).into()
        };
        Some(
            h_flex()
                .absolute()
                .top(px(10.))
                .right(px(14.))
                .gap_1()
                .p_1()
                .rounded(theme.radius_lg)
                .bg(theme.popover)
                .border_1()
                .border_color(theme.border)
                .shadow_md()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(div().w(px(200.)).child(Input::new(&f.input).small()))
                .child(div().min_w(px(56.)).child(count))
                .child(
                    Button::new("pdf-find-case")
                        .ghost()
                        .xsmall()
                        .icon(Glyph::CaseSensitive)
                        .tooltip("Match Case")
                        .selected(f.match_case)
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(f) = &mut this.find {
                                f.match_case = !f.match_case;
                            }
                            this.search(Duration::ZERO, cx);
                        })),
                )
                .child(
                    Button::new("pdf-find-prev")
                        .ghost()
                        .xsmall()
                        .icon(Glyph::PrevChange)
                        .tooltip("Previous (⇧↵)")
                        .on_click(cx.listener(|this, _, _, cx| this.next_hit(-1, cx))),
                )
                .child(
                    Button::new("pdf-find-next")
                        .ghost()
                        .xsmall()
                        .icon(Glyph::NextChange)
                        .tooltip("Next (↵)")
                        .on_click(cx.listener(|this, _, _, cx| this.next_hit(1, cx))),
                )
                .child(
                    Button::new("pdf-find-close")
                        .ghost()
                        .xsmall()
                        .icon(Glyph::Close)
                        .on_click(cx.listener(|this, _, window, cx| this.close_find(window, cx))),
                ),
        )
    }

    /// Open the toolbar's "go to page" field.
    pub fn prompt_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pages.is_empty() {
            return;
        }
        let current = (self.current_page() + 1).to_string();
        let input = cx.new(|cx| InputState::new(window, cx).default_value(current));
        input.update(cx, |i, cx| {
            i.focus(window, cx);
            i.select_all(window, cx);
        });
        self._page_input = Some(cx.subscribe_in(
            &input,
            window,
            |this, input, ev: &InputEvent, window, cx| match ev {
                InputEvent::PressEnter { .. } => {
                    let n = input.read(cx).value().trim().parse::<usize>().ok();
                    this.close_page_input(window, cx);
                    if let Some(n) = n {
                        this.go_to(n.saturating_sub(1), None, cx);
                    }
                }
                InputEvent::Blur => this.close_page_input(window, cx),
                _ => {}
            },
        ));
        self.page_input = Some(input);
        cx.notify();
    }

    fn close_page_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.page_input.take().is_some() {
            self._page_input = None;
            window.focus(&self.focus, cx);
            cx.notify();
        }
    }

    // ── SyncTeX ──────────────────────────────────────────────────────────

    /// Source → PDF: scroll to and briefly highlight `line` of `file`.
    pub fn forward_search(&mut self, file: &Path, line: u32, cx: &mut Context<Self>) -> bool {
        let Some(loc) = self.synctex.as_ref().and_then(|s| s.forward(file, line)) else {
            return false;
        };
        let zoom = self.zoom_factor();
        let (rects, content) = self.layout(zoom);
        let Some(page) = rects.get(loc.page) else {
            return false;
        };
        let first = loc.rects[0];
        let target_y = page.origin.y + px(first.y * zoom) - self.viewport.height * 0.35;
        let max_y = (content.height - self.viewport.height).max(px(0.));
        let offset = self.scroll.offset();
        self.pending_scroll = Some(point(offset.x, -target_y.clamp(px(0.), max_y)));

        self.highlight_seq += 1;
        self.highlight = Some(Highlight {
            page: loc.page,
            rects: loc.rects,
            id: self.highlight_seq,
        });
        let id = self.highlight_seq;
        self._highlight_timer = cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1800))
                .await;
            this.update(cx, |this, cx| {
                if this.highlight.as_ref().is_some_and(|h| h.id == id) {
                    this.highlight = None;
                    cx.notify();
                }
            })
            .ok();
        });
        cx.notify();
        true
    }

    /// The page under a window position, and the position in its points;
    /// `page` forces that page (a drag that left it).
    fn page_point(
        &self,
        position: Point<Pixels>,
        page: Option<usize>,
    ) -> Option<(usize, f32, f32)> {
        let zoom = self.zoom_factor();
        let (rects, _) = self.layout(zoom);
        let p = position - self.scroll.bounds().origin - self.scroll.offset();
        let (i, r) = match page {
            Some(i) => (i, *rects.get(i)?),
            None => rects
                .iter()
                .copied()
                .enumerate()
                .find(|(_, r)| r.contains(&p))?,
        };
        let x = f32::from(p.x - r.origin.x) / zoom;
        let y = f32::from(p.y - r.origin.y) / zoom;
        Some((i, x, y))
    }

    /// The characters selected, as a range of the page's text.
    fn selected_range(&self) -> Option<(usize, std::ops::Range<usize>)> {
        let s = self.selection.as_ref()?;
        let chars = self.texts.get(&s.page)?;
        let a = pdf_view::nearest_char(chars, s.anchor.0, s.anchor.1)?;
        let b = pdf_view::nearest_char(chars, s.head.0, s.head.1)?;
        Some((s.page, a.min(b)..a.max(b) + 1))
    }

    fn copy_text(&mut self, cx: &mut Context<Self>) {
        let Some((page, range)) = self.selected_range() else {
            return;
        };
        let text = pdf_view::text_of(&self.texts[&page], range);
        if !text.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn on_mouse_move(&mut self, ev: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(s) = self.selection.as_ref().filter(|s| s.dragging) else {
            return;
        };
        if ev.pressed_button != Some(MouseButton::Left) {
            return;
        }
        if let Some((_, x, y)) = self.page_point(ev.position, Some(s.page)) {
            let s = self.selection.as_mut().unwrap();
            s.head = (x, y);
            cx.notify();
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(s) = &mut self.selection {
            s.dragging = false;
            // A click, not a drag: nothing selected.
            if s.anchor == s.head {
                self.selection = None;
            }
            cx.notify();
        }
    }

    fn on_mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // Before an inverse search, which moves the focus to the editor.
        window.focus(&self.focus, cx);
        if !ev.modifiers.secondary() && ev.click_count < 2 {
            // Start selecting text.
            self.selection = self
                .page_point(ev.position, None)
                .map(|(page, x, y)| Selection {
                    page,
                    anchor: (x, y),
                    head: (x, y),
                    dragging: true,
                });
            if let Some(page) = self.selection.as_ref().map(|s| s.page)
                && self.asked_text.insert(page)
                && let Some(r) = &self.renderer
            {
                r.query(Query::PageText { page });
            }
            cx.notify();
            return;
        }
        let Some(st) = self.synctex.clone() else {
            return;
        };
        let zoom = self.zoom_factor();
        let (rects, _) = self.layout(zoom);
        let origin = self.scroll.bounds().origin;
        let offset = self.scroll.offset();
        let p = ev.position - origin - offset;
        let Some((page, r)) = rects.iter().enumerate().find(|(_, r)| r.contains(&p)) else {
            return;
        };
        let x = f32::from(p.x - r.origin.x) / zoom;
        let y = f32::from(p.y - r.origin.y) / zoom;
        if let Some(loc) = st.inverse(page, x, y) {
            cx.emit(PdfPaneEvent::InverseSearch {
                file: loc.file,
                line: loc.line,
            });
        }
    }

    // ── Rendering ────────────────────────────────────────────────────────

    /// Ask the renderer for whatever visible pages lack a sharp bitmap.
    fn request_bitmaps(&mut self, visible: &[usize], scale: f32) {
        if self.renderer.is_none() {
            return;
        }
        let scale_milli = quantize(scale);
        let mut wanted: Vec<RenderKey> = Vec::new();
        let around = visible.first().map(|f| {
            f.saturating_sub(RENDER_AHEAD)
                ..(visible.last().unwrap() + RENDER_AHEAD + 1).min(self.pages.len())
        });
        // Visible pages first, then the ones just outside.
        let order = visible.iter().copied().chain(
            around
                .into_iter()
                .flatten()
                .filter(|p| !visible.contains(p)),
        );
        for page in order {
            let key = RenderKey {
                generation: self.doc_generation,
                page,
                scale_milli,
                dark: self.dark,
            };
            if self.shown.get(&page).is_none_or(|s| s.key != key) {
                wanted.push(key);
            }
        }
        self.pane_wanted = wanted;
        self.send_requests();
    }

    /// Ask the renderer for the presentation's page, then the pane's.
    fn send_requests(&mut self) {
        let Some(renderer) = &self.renderer else {
            return;
        };
        let mut wanted = Vec::new();
        if let Some(p) = &mut self.presenting
            && p.page < self.pages.len()
        {
            let key = RenderKey {
                generation: self.doc_generation,
                page: p.page,
                scale_milli: p.scale_milli,
                dark: false,
            };
            let have = p.key == Some(key) && p.image.is_some();
            p.key = Some(key);
            if !have {
                wanted.push(key);
            }
        }
        let first = wanted.first().copied();
        wanted.extend(
            self.pane_wanted
                .iter()
                .copied()
                .filter(|k| Some(*k) != first),
        );
        if wanted != self.last_request {
            renderer.request(wanted.clone());
            self.last_request = wanted;
        }
    }

    // ── For the agents ───────────────────────────────────────────────────

    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// A bitmap of `page` (0-based) at `scale` pixels per point.
    pub fn snapshot(&mut self, page: usize, scale: f32) -> smol::channel::Receiver<Option<Bitmap>> {
        let (tx, rx) = smol::channel::bounded(1);
        match &self.renderer {
            Some(r) => {
                self.query_seq += 1;
                self.snapshots.insert(self.query_seq, tx);
                r.query(Query::Snapshot {
                    id: self.query_seq,
                    page,
                    scale,
                });
            }
            None => {
                let _ = tx.try_send(None);
            }
        }
        rx
    }

    /// The text of `page` (0-based), or of every page.
    pub fn text(&mut self, page: Option<usize>) -> smol::channel::Receiver<Option<String>> {
        let (tx, rx) = smol::channel::bounded(1);
        match &self.renderer {
            Some(r) => {
                self.query_seq += 1;
                self.texts_wanted.insert(self.query_seq, tx);
                r.query(Query::Text {
                    id: self.query_seq,
                    page,
                });
            }
            None => {
                let _ = tx.try_send(None);
            }
        }
        rx
    }

    // ── Presentation ─────────────────────────────────────────────────────

    /// Show `page` in the presentation window, at `scale_milli` pixels per
    /// point (thousandths).
    pub fn present(&mut self, page: usize, scale_milli: u32, cx: &mut Context<Self>) {
        let page = page.min(self.pages.len().saturating_sub(1));
        match &mut self.presenting {
            Some(p) if p.page == page && p.scale_milli == scale_milli => return,
            Some(p) => {
                p.page = page;
                p.scale_milli = scale_milli;
            }
            None => {
                self.presenting = Some(Presenting {
                    page,
                    scale_milli,
                    image: None,
                    key: None,
                })
            }
        }
        self.send_requests();
        cx.notify();
    }

    pub fn stop_presenting(&mut self, cx: &mut Context<Self>) {
        if self.presenting.take().is_some() {
            self.send_requests();
            cx.notify();
        }
    }

    /// The presented page's bitmap (the previous one until the new one is
    /// ready) and the number of pages.
    pub fn presentation(&self) -> (Option<Arc<RenderImage>>, usize) {
        (
            self.presenting.as_ref().and_then(|p| p.image.clone()),
            self.pages.len(),
        )
    }

    pub fn page_size(&self, page: usize) -> Option<PageSize> {
        self.pages.get(page).copied()
    }

    /// Ask for the links of the visible pages not asked yet.
    fn request_links(&mut self, visible: &[usize]) {
        let Some(renderer) = &self.renderer else {
            return;
        };
        for &page in visible {
            if self.asked_links.insert(page) {
                renderer.query(Query::Links { page });
            }
        }
    }

    /// Follow a link: to a place of the document, or a web page or email
    /// address (never a file or a program).
    fn follow(&mut self, target: &LinkTarget, cx: &mut Context<Self>) {
        match target {
            LinkTarget::Page { page, y } => {
                // A little above the target, which is usually a heading's baseline.
                self.go_to(*page, y.map(|y| (y - 24.0).max(0.0)), cx)
            }
            LinkTarget::Uri(uri) => {
                let lower = uri.to_ascii_lowercase();
                if ["http://", "https://", "mailto:"]
                    .iter()
                    .any(|p| lower.starts_with(p))
                {
                    cx.open_url(uri);
                } else {
                    log::info!("not following the link {uri}");
                }
            }
        }
    }

    fn evict_far_pages(&mut self, visible: &[usize]) {
        let (Some(&first), Some(&last)) = (visible.first(), visible.last()) else {
            return;
        };
        let far: Vec<usize> = self
            .shown
            .keys()
            .copied()
            .filter(|p| *p + KEEP_AROUND < first || *p > last + KEEP_AROUND)
            .collect();
        for p in far {
            if let Some(s) = self.shown.remove(&p) {
                self.to_drop.push(s.image);
            }
        }
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let zoom_pct = (self.zoom_factor() * 100.0).round() as i32;
        let pages = self.pages.len();
        let page: AnyElement = match &self.page_input {
            Some(input) => div()
                .w(px(56.))
                .child(Input::new(input).xsmall())
                .into_any_element(),
            None => Button::new("pdf-page")
                .ghost()
                .label(format!("{} / {pages}", self.current_page() + 1))
                .tooltip("Go to page…")
                .on_click(cx.listener(|this, _, window, cx| this.prompt_page(window, cx)))
                .into_any_element(),
        };
        h_flex()
            .absolute()
            .bottom(px(14.))
            .right(px(14.))
            .gap_0p5()
            .p_0p5()
            .rounded(cx.theme().radius_lg)
            .bg(cx.theme().popover)
            .border_1()
            .border_color(cx.theme().border)
            .shadow_md()
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .child(page)
            .child(
                Button::new("pdf-present")
                    .ghost()
                    .icon(Glyph::Presentation)
                    .tooltip("Present (⇧⌘↵)")
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(PdfPaneEvent::Present))),
            )
            .child(
                Button::new("pdf-export")
                    .ghost()
                    .icon(Glyph::Export)
                    .tooltip("Export PDF… (⇧⌘S)")
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(PdfPaneEvent::Export))),
            )
            .child(
                Button::new("zoom-out")
                    .ghost()
                    .icon(Glyph::ZoomOut)
                    .on_click(cx.listener(|this, _, _, cx| this.zoom_out(cx))),
            )
            .child(
                Button::new("zoom-fit")
                    .ghost()
                    .label(format!("{zoom_pct}%"))
                    .tooltip(if self.zoom == Zoom::FitWidth {
                        "Fit page"
                    } else {
                        "Fit width"
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        if this.zoom == Zoom::FitWidth {
                            this.fit_page(cx)
                        } else {
                            this.zoom_to_fit(cx)
                        }
                    })),
            )
            .child(
                Button::new("zoom-in")
                    .ghost()
                    .icon(Glyph::ZoomIn)
                    .on_click(cx.listener(|this, _, _, cx| this.zoom_in(cx))),
            )
    }
}

impl Focusable for PdfPane {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for PdfPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        for image in self.to_drop.drain(..) {
            let _ = window.drop_image(image);
        }

        // The viewport is only known after layout; re-render once it changes.
        let bounds = self.scroll.bounds().size;
        if bounds != self.viewport && bounds.width > px(0.) {
            self.viewport = bounds;
            self.last_request.clear();
            window.request_animation_frame();
        }

        let theme = cx.theme();
        let canvas_bg = theme.muted;
        let empty = |msg: SharedString| {
            v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_3()
                .bg(canvas_bg)
                .text_color(theme.muted_foreground)
                .text_sm()
                .child(
                    Icon::new(Glyph::Book)
                        .size_8()
                        .text_color(theme.muted_foreground.opacity(0.5)),
                )
                .child(msg)
                .into_any_element()
        };
        if self.pages.is_empty() {
            let msg = self
                .error
                .clone()
                .unwrap_or_else(|| "The preview appears after the first build.".into());
            return div()
                .id("pdf-pane")
                .size_full()
                .child(empty(msg))
                .into_any_element();
        }

        let zoom = self.zoom_factor();
        let (rects, content) = self.layout(zoom);

        if let Some(offset) = self.pending_scroll.take() {
            let max_x = (content.width - self.viewport.width).max(px(0.));
            let max_y = (content.height - self.viewport.height).max(px(0.));
            self.scroll.set_offset(point(
                offset.x.clamp(-max_x, px(0.)),
                offset.y.clamp(-max_y, px(0.)),
            ));
        }

        let offset = self.scroll.offset();
        let view_top = -offset.y;
        let view_bottom = view_top + self.viewport.height.max(px(1.));
        let visible: Vec<usize> = rects
            .iter()
            .enumerate()
            .filter(|(_, r)| r.bottom() >= view_top && r.top() <= view_bottom)
            .map(|(i, _)| i)
            .collect();
        self.request_bitmaps(&visible, zoom * window.scale_factor());
        self.request_links(&visible);
        self.evict_far_pages(&visible);

        let theme = cx.theme();
        let highlight_color = theme.primary.opacity(0.28);
        let selection_color = theme.selection;
        let selected = self.selected_range();
        let current_hit = theme.warning.opacity(0.55);
        let other_hit = theme.warning.opacity(0.25);
        let shadow_page = theme.is_dark();
        let pages = rects.iter().enumerate().filter_map(|(i, r)| {
            // Elements only for pages near the viewport.
            let near = r.bottom() >= view_top - self.viewport.height
                && r.top() <= view_bottom + self.viewport.height;
            if !near {
                return None;
            }
            let paper = if self.dark {
                rgb(0x202020)
            } else {
                rgb(0xffffff)
            };
            let mut page = div()
                .absolute()
                .left(r.origin.x)
                .top(r.origin.y)
                .w(r.size.width)
                .h(r.size.height)
                .bg(paper)
                .rounded(px(2.))
                .overflow_hidden()
                .when(!shadow_page, |d| d.shadow_md())
                .when(shadow_page, |d| d.border_1().border_color(theme.border));
            if let Some(shown) = self.shown.get(&i) {
                page = page.child(
                    img(shown.image.clone())
                        .size_full()
                        .object_fit(ObjectFit::Fill),
                );
            }
            for (k, link) in self.links.get(&i).into_iter().flatten().enumerate() {
                let target = link.target.clone();
                let r = link.rect;
                page = page.child(
                    div()
                        .id(ElementId::NamedInteger(
                            "pdf-link".into(),
                            (i * 10_000 + k) as u64,
                        ))
                        .absolute()
                        .left(px(r.x * zoom))
                        .top(px(r.y * zoom))
                        .w(px(r.width * zoom))
                        .h(px(r.height * zoom))
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                            // ⌘-click and double click stay inverse search.
                            if !ev.modifiers().secondary() && ev.click_count() < 2 {
                                this.follow(&target, cx);
                            }
                        })),
                );
            }
            if let Some((_, range)) = selected.as_ref().filter(|(p, _)| *p == i)
                && let Some(chars) = self.texts.get(&i)
            {
                for c in &chars[range.clone()] {
                    let r = c.rect;
                    if r.width <= 0.0 || r.height <= 0.0 {
                        continue;
                    }
                    page = page.child(
                        div()
                            .absolute()
                            .left(px(r.x * zoom))
                            .top(px(r.y * zoom))
                            .w(px(r.width * zoom))
                            .h(px(r.height * zoom))
                            .bg(selection_color),
                    );
                }
            }
            if let Some(f) = &self.find {
                for (k, hit) in f.hits.iter().enumerate().filter(|(_, h)| h.page == i) {
                    let color = if f.current == Some(k) {
                        current_hit
                    } else {
                        other_hit
                    };
                    for r in &hit.rects {
                        page = page.child(
                            div()
                                .absolute()
                                .left(px(r.x * zoom - 1.0))
                                .top(px(r.y * zoom - 1.0))
                                .w(px(r.width * zoom + 2.0))
                                .h(px(r.height * zoom + 2.0))
                                .rounded(px(2.))
                                .bg(color),
                        );
                    }
                }
            }
            if let Some(h) = self.highlight.as_ref().filter(|h| h.page == i) {
                for (k, rect) in h.rects.iter().enumerate() {
                    page = page.child(
                        div()
                            .absolute()
                            .left(px(rect.x * zoom - 3.0))
                            .top(px(rect.y * zoom - 2.0))
                            .w(px(rect.width * zoom + 6.0))
                            .h(px(rect.height * zoom + 4.0))
                            .rounded(px(3.))
                            .bg(highlight_color)
                            .with_animation(
                                ElementId::NamedInteger(
                                    "synctex-hl".into(),
                                    (h.id * 1000 + k) as u64,
                                ),
                                Animation::new(Duration::from_millis(1600))
                                    .with_easing(ease_in_out),
                                |el, t| {
                                    el.opacity(if t < 0.5 { 1.0 } else { 1.0 - (t - 0.5) * 2.0 })
                                },
                            ),
                    );
                }
            }
            Some(page)
        });

        div()
            .id("pdf-pane")
            .key_context("PdfPane")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &NextPage, _, cx| this.step_page(1, cx)))
            .on_action(cx.listener(|this, _: &PreviousPage, _, cx| this.step_page(-1, cx)))
            .on_action(cx.listener(|this, _: &FirstPage, _, cx| this.go_to(0, None, cx)))
            .on_action(cx.listener(|this, _: &LastPage, _, cx| this.go_to(usize::MAX, None, cx)))
            .on_action(cx.listener(|this, _: &GoToPage, window, cx| this.prompt_page(window, cx)))
            .on_action(cx.listener(|this, _: &FitPage, _, cx| this.fit_page(cx)))
            .on_action(cx.listener(|this, _: &FindInPdf, window, cx| this.open_find(window, cx)))
            .on_action(cx.listener(|this, _: &CloseFind, window, cx| {
                if this.find.is_some() {
                    this.close_find(window, cx)
                } else {
                    this.selection = None;
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &CopyText, _, cx| this.copy_text(cx)))
            .relative()
            .size_full()
            .bg(canvas_bg)
            .child(
                div()
                    .id("pdf-scroll")
                    .size_full()
                    .overflow_scroll()
                    .track_scroll(&self.scroll)
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
                    .on_mouse_move(cx.listener(Self::on_mouse_move))
                    .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
                    .on_scroll_wheel(cx.listener(|this, ev: &ScrollWheelEvent, _, cx| {
                        if !(ev.modifiers.secondary() || ev.modifiers.control) {
                            return;
                        }
                        let dy = f32::from(ev.delta.pixel_delta(px(16.)).y);
                        let factor = (1.0 + dy / 400.0).clamp(0.8, 1.25);
                        let origin = this.scroll.bounds().origin;
                        let z = this.zoom_factor() * factor;
                        this.set_zoom(Zoom::Fixed(z), Some(ev.position - origin), cx);
                        cx.stop_propagation();
                    }))
                    .on_pinch(cx.listener(|this, ev: &PinchEvent, _, cx| {
                        let origin = this.scroll.bounds().origin;
                        let z = this.zoom_factor() * (1.0 + ev.delta);
                        this.set_zoom(Zoom::Fixed(z), Some(ev.position - origin), cx);
                    }))
                    .child(
                        div()
                            .relative()
                            .w(content.width)
                            .h(content.height)
                            .children(pages),
                    ),
            )
            .child(self.render_toolbar(cx))
            .children(self.render_find(cx))
            .into_any_element()
    }
}

/// Snap to 5% steps so continuous resizing does not re-rasterize per pixel.
fn quantize(scale: f32) -> u32 {
    let step = 1.05f32.ln();
    let snapped = (scale.max(0.05).ln() / step).round() * step;
    (snapped.exp() * 1000.0).round() as u32
}

fn to_render_image(bitmap: Bitmap) -> Option<Arc<RenderImage>> {
    let buffer = image::RgbaImage::from_raw(bitmap.width, bitmap.height, bitmap.bgra)?;
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])))
}
