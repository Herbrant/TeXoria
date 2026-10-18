use std::path::PathBuf;
use std::time::{Duration, Instant};

use gpui_kit::{AnyWindowHandle, TestAppContext, WindowOptions};

use super::*;
use crate::testing;
// `super::*` brings gpui's `test` attribute, which would shadow the standard one, and
// gpui-kit's `gpui` alias, which the `#[gpui::test]` macro's paths must not find twice.
use core::prelude::v1::test;
use gpui_kit::gpui;

fn fixture(path: &str) -> PathBuf {
    test_support::fixtures().join(path)
}

/// The repository's PDFium, if it was fetched.
fn pdfium() -> Option<PathBuf> {
    let dir = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../vendor/pdfium"));
    let found = std::fs::read_dir(&dir).is_ok_and(|d| {
        d.flatten()
            .any(|e| e.file_name().to_string_lossy().contains("pdfium"))
    });
    if !found {
        test_support::skip("PDFIUM", "no PDFium in vendor/pdfium");
    }
    found.then_some(dir)
}

struct Shown {
    pane: Entity<PdfPane>,
    pane_window: AnyWindowHandle,
    window: AnyWindowHandle,
    /// Weak: the window owns it, and ends it when it closes.
    presentation: WeakEntity<Presentation>,
}

/// Hand the renderer's answers over and draw the frames `windows` asked for, until `done`
/// (five seconds at most: the renderer is a thread of its own).
#[track_caller]
fn wait(
    cx: &mut TestAppContext,
    pane: &Entity<PdfPane>,
    windows: &[AnyWindowHandle],
    what: &str,
    mut done: impl FnMut(&mut TestAppContext) -> bool,
) {
    let start = Instant::now();
    loop {
        pane.update(cx, |p, cx| p.pump(cx));
        for w in windows {
            let _ = cx.update_window(*w, |_, window, cx| window.simulate_next_frame(cx));
        }
        cx.run_until_parked();
        if done(cx) {
            return;
        }
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

impl Shown {
    #[track_caller]
    fn wait(
        &self,
        cx: &mut TestAppContext,
        what: &str,
        done: impl FnMut(&mut TestAppContext) -> bool,
    ) {
        wait(cx, &self.pane, &[self.pane_window, self.window], what, done);
    }

    fn page(&self, cx: &mut TestAppContext) -> usize {
        self.presentation
            .upgrade()
            .unwrap()
            .read_with(cx, |p, _| p.page)
    }

    /// Whether the presentation drew the bitmap of its page.
    fn drawn(&self, cx: &mut TestAppContext) -> bool {
        let presentation = self.presentation.upgrade().unwrap();
        presentation.read_with(cx, |p, cx| {
            let (image, _) = p.pane.read(cx).presentation();
            p.drawn.is_some() && image.is_some()
        })
    }

    fn presented(&self, cx: &mut TestAppContext) -> Option<usize> {
        self.pane.read_with(cx, |p, _| p.presented_page())
    }
}

/// `links.pdf` (two pages) in a preview, presented from `page`.
fn present(cx: &mut TestAppContext, page: usize) -> Option<Shown> {
    let dir = pdfium()?;
    testing::init(cx);
    let (pane_window, pane) = cx.update(|cx| {
        gpui_kit::open_window(WindowOptions::default(), cx, move |_, cx| {
            cx.new(|cx| PdfPane::new(vec![dir], cx))
        })
        .unwrap()
    });
    pane.update(cx, |p, cx| p.load(fixture("links/links.pdf"), None, cx));
    cx.run_until_parked();
    let pane_window: AnyWindowHandle = pane_window;
    wait(cx, &pane, &[pane_window], "the document", |cx| {
        pane.read_with(cx, |p, _| p.page_count() == 2)
    });
    let (window, presentation) = cx.update(|cx| {
        let pane = pane.clone();
        gpui_kit::open_window(WindowOptions::default(), cx, move |window, cx| {
            cx.new(|cx| Presentation::new(pane, page, window, cx))
        })
        .unwrap()
    });
    let shown = Shown {
        pane,
        pane_window,
        window,
        presentation: presentation.downgrade(),
    };
    shown.wait(cx, "the first slide", |cx| shown.drawn(cx));
    Some(shown)
}

#[gpui::test]
fn goes_through_the_slides(cx: &mut TestAppContext) {
    let Some(shown) = present(cx, 0) else {
        return;
    };
    let window = shown.window;
    assert_eq!(shown.presented(cx), Some(0));
    cx.dispatch_action(window, NextSlide);
    assert_eq!(shown.page(cx), 1);
    shown.wait(cx, "the second slide", |cx| {
        shown.presented(cx) == Some(1) && shown.drawn(cx)
    });
    // Not past the end, nor before the start.
    cx.dispatch_action(window, NextSlide);
    assert_eq!(shown.page(cx), 1);
    cx.dispatch_action(window, FirstSlide);
    assert_eq!(shown.page(cx), 0);
    cx.dispatch_action(window, PreviousSlide);
    assert_eq!(shown.page(cx), 0);
    cx.dispatch_action(window, LastSlide);
    assert_eq!(shown.page(cx), 1);
    // The keys of presentation software.
    cx.simulate_keystrokes(window, "home");
    assert_eq!(shown.page(cx), 0);
    cx.simulate_keystrokes(window, "space");
    assert_eq!(shown.page(cx), 1);
    cx.simulate_keystrokes(window, "backspace");
    assert_eq!(shown.page(cx), 0);
    cx.simulate_keystrokes(window, "end");
    assert_eq!(shown.page(cx), 1);

    // Escape ends it, and the preview stops rendering for it.
    cx.simulate_keystrokes(window, "escape");
    cx.run_until_parked();
    assert!(cx.update_window(window, |_, _, _| ()).is_err(), "closed");
    assert_eq!(shown.presented(cx), None);
}

/// A rebuild may remove pages: the presentation goes to the last one left rather than show
/// nothing.
#[gpui::test]
fn follows_a_rebuild_with_fewer_pages(cx: &mut TestAppContext) {
    let Some(shown) = present(cx, 1) else {
        return;
    };
    assert_eq!(shown.presented(cx), Some(1));
    shown.pane.update(cx, |p, cx| {
        p.load(fixture("pdf-pane/one-page.pdf"), None, cx)
    });
    shown.wait(cx, "the rebuild", |cx| {
        shown.pane.read_with(cx, |p, _| p.page_count() == 1)
    });
    shown.wait(cx, "its slide", |cx| shown.page(cx) == 0 && shown.drawn(cx));
    let presentation = shown.presentation.upgrade().unwrap();
    presentation.read_with(cx, |p, cx| {
        let size = p.pane.read(cx).page_size(p.page).unwrap();
        assert_eq!((size.width, size.height), (300.0, 200.0));
    });
}
