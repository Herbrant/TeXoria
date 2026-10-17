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
    ActiveTheme, Icon, Sizable, button::Button, button::ButtonVariants, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pdf_view::{
    Bitmap, Event as RenderEvent, Link, LinkTarget, PageSize, Query, RenderKey, Renderer,
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
        FitPage
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
}

struct Shown {
    key: RenderKey,
    image: Arc<RenderImage>,
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
            RenderEvent::PageText { .. }
            | RenderEvent::Found { .. }
            | RenderEvent::Snapshot { .. }
            | RenderEvent::Text { .. } => {}
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

    fn on_mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // Before an inverse search, which moves the focus to the editor.
        window.focus(&self.focus, cx);
        if !ev.modifiers.secondary() && ev.click_count < 2 {
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
        let Some(renderer) = &self.renderer else {
            return;
        };
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
        if wanted != self.last_request {
            renderer.request(wanted.clone());
            self.last_request = wanted;
        }
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
