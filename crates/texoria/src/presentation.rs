//! The presentation window: the PDF full screen, one page at a time.
//!
//! Its pages come from the preview's renderer (`PdfPane::present`), so that
//! PDFium is not loaded twice, and they follow rebuilds of the document.

use std::sync::Arc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::pdf_pane::{self, PdfPane};

actions!(
    presentation,
    [
        NextSlide,
        PreviousSlide,
        FirstSlide,
        LastSlide,
        EndPresentation
    ]
);

pub struct Presentation {
    pane: Entity<PdfPane>,
    page: usize,
    focus: FocusHandle,
    /// The bitmap drawn last, freed from this window once replaced.
    drawn: Option<Arc<RenderImage>>,
    _subscriptions: Vec<Subscription>,
}

impl Presentation {
    pub fn new(
        pane: Entity<PdfPane>,
        page: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let subscriptions = vec![
            cx.observe(&pane, |_, _, cx| cx.notify()),
            cx.on_release(|this: &mut Self, cx: &mut App| {
                this.pane.update(cx, |p, cx| p.stop_presenting(cx))
            }),
        ];
        Self {
            pane,
            page,
            focus,
            drawn: None,
            _subscriptions: subscriptions,
        }
    }

    fn go(&mut self, page: isize, cx: &mut Context<Self>) {
        let pages = self.pane.read(cx).presentation().1;
        self.page = page.clamp(0, pages.saturating_sub(1) as isize) as usize;
        cx.notify();
    }
}

impl Render for Presentation {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport = window.viewport_size();
        // A rebuild may have removed pages.
        let pages = self.pane.read(cx).presentation().1;
        self.page = self.page.min(pages.saturating_sub(1));
        let page = self.page;
        let size = self.pane.read(cx).page_size(page);
        // The page as large as fits.
        let fit = size.map(|s| {
            (f32::from(viewport.width) / s.width)
                .min(f32::from(viewport.height) / s.height)
                .max(0.01)
        });
        let scale_factor = window.scale_factor();
        if let Some(fit) = fit {
            let scale_milli = pdf_pane::scale_milli(fit, scale_factor);
            self.pane
                .update(cx, |p, cx| p.present(page, scale_milli, cx));
        }
        let (image, _) = self.pane.read(cx).presentation();
        if let Some(old) = self
            .drawn
            .take_if(|d| image.as_ref().is_none_or(|i| !Arc::ptr_eq(i, d)))
        {
            let _ = window.drop_image(old);
        }
        self.drawn = image.clone();

        div()
            .id("presentation")
            .key_context("Presentation")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui_kit::black())
            .cursor_pointer()
            .on_action(
                cx.listener(|this, _: &NextSlide, _, cx| this.go(this.page as isize + 1, cx)),
            )
            .on_action(
                cx.listener(|this, _: &PreviousSlide, _, cx| this.go(this.page as isize - 1, cx)),
            )
            .on_action(cx.listener(|this, _: &FirstSlide, _, cx| this.go(0, cx)))
            .on_action(cx.listener(|this, _: &LastSlide, _, cx| this.go(isize::MAX, cx)))
            .on_action(cx.listener(|_, _: &EndPresentation, window, _| window.remove_window()))
            .on_click(cx.listener(|this, _, _, cx| this.go(this.page as isize + 1, cx)))
            .when_some(image.zip(size).zip(fit), |d, ((image, size), fit)| {
                d.child(
                    // Sized as the bitmap, so that it is drawn pixel for pixel.
                    img(image)
                        .w(px(pdf_pane::page_side(size.width, fit, scale_factor)))
                        .h(px(pdf_pane::page_side(size.height, fit, scale_factor)))
                        .object_fit(ObjectFit::Fill),
                )
            })
    }
}

/// The presentation's keys, as in presentation software.
pub fn bind_keys(cx: &mut App) {
    let context = Some("Presentation");
    cx.bind_keys([
        KeyBinding::new("right", NextSlide, context),
        KeyBinding::new("down", NextSlide, context),
        KeyBinding::new("space", NextSlide, context),
        KeyBinding::new("pagedown", NextSlide, context),
        KeyBinding::new("enter", NextSlide, context),
        KeyBinding::new("left", PreviousSlide, context),
        KeyBinding::new("up", PreviousSlide, context),
        KeyBinding::new("pageup", PreviousSlide, context),
        KeyBinding::new("backspace", PreviousSlide, context),
        KeyBinding::new("home", FirstSlide, context),
        KeyBinding::new("end", LastSlide, context),
        KeyBinding::new("escape", EndPresentation, context),
    ]);
}

#[cfg(test)]
mod tests;
