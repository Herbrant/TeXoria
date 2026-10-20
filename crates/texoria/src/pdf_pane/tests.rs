use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use gpui_kit::{AnyWindowHandle, Modifiers, TestAppContext, VisualTestContext};

use super::*;
use crate::testing;
// `super::*` brings gpui's `test` attribute, which would shadow the standard one, and
// gpui-kit's `gpui` alias, which the `#[gpui::test]` macro's paths must not find twice.
use core::prelude::v1::test;
use gpui_kit::gpui;

fn page(width: f32, height: f32) -> PageSize {
    PageSize { width, height }
}

fn viewport(w: f32, h: f32) -> Size<Pixels> {
    size(px(w), px(h))
}

#[test]
fn zoom_factors() {
    let pages = [page(600.0, 800.0), page(400.0, 1000.0)];
    let g = |zoom, view| Geometry {
        pages: &pages,
        viewport: view,
        zoom,
        scale_factor: 2.0,
    };
    // The widest page fills the width, margins aside.
    let fit = g(Zoom::FitWidth, viewport(656.0, 500.0)).zoom_factor();
    assert!((fit - 1.0).abs() < 1e-4, "{fit}");
    // The tallest page fits in the height.
    let fit = g(Zoom::FitPage, viewport(1256.0, 556.0)).zoom_factor();
    assert!((fit - 0.5).abs() < 1e-4, "{fit}");
    assert_eq!(g(Zoom::Fixed(2.5), viewport(10.0, 10.0)).zoom_factor(), 2.5);
    // Within bounds, whatever the window.
    assert_eq!(
        g(Zoom::FitWidth, viewport(100_000.0, 10.0)).zoom_factor(),
        MAX_ZOOM
    );
    assert_eq!(
        g(Zoom::FitPage, viewport(60.0, 60.0)).zoom_factor(),
        MIN_ZOOM
    );
    // Not laid out yet, or no pages.
    assert_eq!(g(Zoom::FitWidth, viewport(0.0, 0.0)).zoom_factor(), 1.0);
    assert_eq!(g(Zoom::FitPage, viewport(500.0, 0.0)).zoom_factor(), 1.0);
    let none = Geometry {
        pages: &[],
        viewport: viewport(500.0, 500.0),
        zoom: Zoom::FitPage,
        scale_factor: 1.0,
    };
    assert_eq!(none.zoom_factor(), 1.0);
    assert_eq!(none.layout(1.0).0.len(), 0);
    assert_eq!(none.current_page(px(0.)), 0);
}

#[test]
fn pages_are_stacked_and_centered() {
    let pages = [page(600.0, 800.0), page(400.0, 1000.0)];
    let g = Geometry {
        pages: &pages,
        viewport: viewport(1000.0, 500.0),
        zoom: Zoom::Fixed(0.5),
        scale_factor: 1.0,
    };
    let (rects, content) = g.layout(0.5);
    // As wide as the viewport, each page in its middle.
    assert_eq!(content.width, px(1000.0));
    assert_eq!(rects[0].origin, point(px(350.0), px(MARGIN)));
    assert_eq!(rects[0].size, size(px(300.0), px(400.0)));
    assert_eq!(rects[1].origin, point(px(400.0), px(MARGIN + 400.0 + GAP)));
    assert_eq!(content.height, px(MARGIN + 400.0 + GAP + 500.0 + MARGIN));
    // Wider than the viewport: the content is.
    let (rects, content) = g.layout(4.0);
    assert_eq!(content.width, px(2400.0 + 2.0 * MARGIN));
    assert_eq!(rects[0].origin.x, px(MARGIN));

    // The page in the middle of the viewport.
    assert_eq!(g.current_page(px(0.)), 0);
    assert_eq!(g.current_page(px(300.)), 1);
    assert_eq!(g.current_page(px(100_000.)), 1);

    // Points of a page, from its top left.
    let (i, x, y) = g
        .page_point(point(px(350.0 + 50.0), px(MARGIN + 100.0)), None)
        .unwrap();
    assert_eq!((i, x, y), (0, 100.0, 200.0));
    assert_eq!(g.page_point(point(px(10.0), px(10.0)), None), None);
    // A drag that left its page stays on it.
    let (i, x, _) = g.page_point(point(px(10.0), px(10.0)), Some(1)).unwrap();
    assert_eq!((i, x), (1, -780.0));
    assert_eq!(g.page_point(point(px(10.0), px(10.0)), Some(7)), None);
}

#[test]
fn a_place_is_found_again_at_another_width() {
    let pages = [page(600.0, 800.0), page(600.0, 800.0), page(600.0, 800.0)];
    let g = |width| Geometry {
        pages: &pages,
        viewport: viewport(width, 500.0),
        zoom: Zoom::FitWidth,
        scale_factor: 2.0,
    };
    let (narrow, wide) = (g(656.0), g(1256.0));
    // A third down the second page.
    let (rects, _) = narrow.layout(narrow.zoom_factor());
    let top = rects[1].origin.y + rects[1].size.height / 3.0;
    let place = narrow.place_at(top).unwrap();
    assert_eq!(place.page, 1);
    assert!((place.down - 1.0 / 3.0).abs() < 1e-4, "{place:?}");
    // Twice as wide, the pages are twice as tall: the same place is further down.
    let there = wide.top_of(place).unwrap();
    let (rects, _) = wide.layout(wide.zoom_factor());
    assert!((f32::from(there - rects[1].origin.y) - 1600.0 / 3.0).abs() < 0.1);
    let back = wide.place_at(there).unwrap();
    assert_eq!(back.page, 1);
    assert!((back.down - place.down).abs() < 1e-4, "{back:?}");
    // The margin above the first page is above it; past the last, on the last.
    assert!(narrow.place_at(px(0.)).unwrap().down < 0.0);
    assert_eq!(narrow.place_at(px(100_000.)).unwrap().page, 2);
    assert_eq!(narrow.top_of(Place { page: 7, down: 0.0 }), None);
    let none = Geometry {
        pages: &[],
        ..g(656.0)
    };
    assert_eq!(none.place_at(px(0.)), None);
}

#[test]
fn pages_rendered_and_kept() {
    assert_eq!(render_order(&[3, 4], 10), [3, 4, 2, 5]);
    assert_eq!(render_order(&[0], 1), [0]);
    assert_eq!(render_order(&[0], 3), [0, 1]);
    assert!(render_order(&[], 3).is_empty());

    let mut far = far_pages([0, 1, 5, 10, 17, 18].into_iter(), &[10, 11]);
    far.sort();
    assert_eq!(far, [0, 1, 18]);
    assert!(far_pages([1, 2].into_iter(), &[]).is_empty());
}

#[test]
fn pages_are_whole_device_pixels() {
    assert_eq!(scale_milli(1.0, 2.0), 2000);
    assert_eq!(scale_milli(0.919_934_6, 1.0), 920);
    // Never zero.
    assert_eq!(scale_milli(0.0, 2.0), 1);

    // An A4 page at a zoom that fills some pane, on a Retina display and on
    // a low-resolution one: as wide as its bitmap, which is drawn 1:1.
    let a4 = page(595.276, 841.89);
    for scale_factor in [1.0, 1.5, 2.0] {
        let g = Geometry {
            pages: &[a4],
            viewport: viewport(600.0, 500.0),
            zoom: Zoom::FitWidth,
            scale_factor,
        };
        let zoom = g.zoom_factor();
        let (rects, _) = g.layout(zoom);
        let key = RenderKey {
            generation: 1,
            page: 0,
            scale_milli: scale_milli(zoom, scale_factor),
            dark: false,
        };
        let device = |side: Pixels| f32::from(side) * scale_factor;
        let width = device(rects[0].size.width);
        let height = device(rects[0].size.height);
        assert_eq!(width, (a4.width * key.scale()).round());
        assert_eq!(height, (a4.height * key.scale()).round());
        // Within a device pixel of the exact size.
        assert!(
            (width - a4.width * zoom * scale_factor).abs() <= 1.0,
            "{width}"
        );
    }
}

#[test]
fn bitmaps_become_images() {
    let bitmap = Bitmap {
        width: 2,
        height: 1,
        bgra: vec![0; 8],
    };
    assert!(to_render_image(bitmap).is_some());
    // A buffer of the wrong size is refused.
    let bitmap = Bitmap {
        width: 2,
        height: 2,
        bgra: vec![0; 8],
    };
    assert!(to_render_image(bitmap).is_none());
}

fn fixture(path: &str) -> PathBuf {
    test_support::fixtures().join(path)
}

/// The repository's PDFium, if it was fetched.
fn pdfium() -> Option<Vec<PathBuf>> {
    let dir = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../vendor/pdfium"));
    let found = std::fs::read_dir(&dir).is_ok_and(|mut d| {
        d.any(|e| {
            e.is_ok_and(|e| {
                let n = e.file_name().to_string_lossy().to_lowercase();
                n.contains("pdfium") && !n.ends_with(".txt")
            })
        })
    });
    if !found {
        test_support::skip("PDFIUM", "no PDFium in vendor/pdfium");
        return None;
    }
    Some(vec![dir])
}

/// A pane alone in a window.
fn pane(cx: &mut TestAppContext, dirs: Vec<PathBuf>) -> (AnyWindowHandle, Entity<PdfPane>) {
    testing::init(cx);
    let (window, pane) = cx.update(|cx| {
        gpui_kit::open_window(WindowOptions::default(), cx, move |_, cx| {
            cx.new(|cx| PdfPane::new(dirs, cx))
        })
        .unwrap()
    });
    cx.run_until_parked();
    (window, pane)
}

/// Wait for the PDF worker (a thread of its own) until `done`, five seconds at most.
#[track_caller]
fn wait(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
    pane: &Entity<PdfPane>,
    what: &str,
    mut done: impl FnMut(&mut TestAppContext) -> bool,
) {
    let start = Instant::now();
    loop {
        pane.update(cx, |p, cx| p.pump(cx));
        // Tests have no frame loop: the frames the pane asked for.
        cx.update_window(window, |_, window, cx| window.simulate_next_frame(cx))
            .unwrap();
        cx.run_until_parked();
        if done(cx) {
            return;
        }
        if start.elapsed() > Duration::from_secs(5) {
            let state = pane.read_with(cx, |p, _| {
                format!(
                    "pages {} shown {:?} links {:?} viewport {:?} last_request {:?}",
                    p.pages.len(),
                    p.shown.keys().collect::<Vec<_>>(),
                    p.links.keys().collect::<Vec<_>>(),
                    p.viewport,
                    p.last_request
                )
            });
            panic!("timed out waiting for {what}: {state}");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn update<R>(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
    pane: &Entity<PdfPane>,
    f: impl FnOnce(&mut PdfPane, &mut Window, &mut Context<PdfPane>) -> R,
) -> R {
    let r = cx
        .update_window(window, |_, window, cx| {
            pane.update(cx, |p, cx| f(p, window, cx))
        })
        .unwrap();
    cx.run_until_parked();
    r
}

/// `links.pdf` loaded and its first page drawn.
fn loaded(cx: &mut TestAppContext) -> Option<(AnyWindowHandle, Entity<PdfPane>)> {
    let dirs = pdfium()?;
    let (window, pane) = pane(cx, dirs);
    let synctex = fixture("synctex/sample.synctex.gz");
    update(cx, window, &pane, |p, _, cx| {
        p.load(fixture("links/links.pdf"), Some(synctex), cx)
    });
    wait(cx, window, &pane, "the PDF", |cx| {
        pane.read_with(cx, |p, _| p.page_count() == 2)
    });
    wait(cx, window, &pane, "the first page", |cx| {
        pane.read_with(cx, |p, _| {
            p.shown.contains_key(&0) && p.links.contains_key(&0)
        })
    });
    // Clicked, as the user would before its shortcuts.
    let focus = pane.read_with(cx, |p, _| p.focus.clone());
    cx.update_window(window, |_, window, cx| window.focus(&focus, cx))
        .unwrap();
    cx.run_until_parked();
    Some((window, pane))
}

#[gpui::test]
fn without_pdfium_there_is_no_preview(cx: &mut TestAppContext) {
    let (window, pane) = pane(cx, vec![PathBuf::from("/nowhere")]);
    update(cx, window, &pane, |p, _, _| {
        // PDFium is bound once per process: another test may have found it.
        match &p.error {
            Some(error) => assert!(error.starts_with("PDF preview unavailable"), "{error}"),
            None => p.renderer = None,
        }
    });
    let (snapshot, text) = update(cx, window, &pane, |p, _, cx| {
        p.load(fixture("links/links.pdf"), None, cx);
        (p.snapshot(0, 1.0), p.text(None))
    });
    assert_eq!(snapshot.try_recv().unwrap().map(|b| b.width), None);
    assert_eq!(text.try_recv().unwrap(), None);
    pane.read_with(cx, |p, _| {
        assert_eq!(p.page_count(), 0);
        assert!(p.pages.is_empty());
    });
    // Nothing to go to.
    update(cx, window, &pane, |p, window, cx| {
        p.go_to(3, None, cx);
        p.prompt_page(window, cx);
        assert!(p.page_input.is_none());
    });
}

#[gpui::test]
fn shows_the_pages_and_their_links(cx: &mut TestAppContext) {
    let Some((window, pane)) = loaded(cx) else {
        return;
    };
    pane.read_with(cx, |p, _| {
        assert!(p.error.is_none());
        let size = p.pages[0];
        assert!(size.width > 500.0 && size.height > 700.0, "{size:?}");
        assert!(p.viewport.width > px(0.), "laid out");
        let links = &p.links[&0];
        assert!(
            links
                .iter()
                .any(|l| matches!(&l.target, LinkTarget::Uri(u) if u.contains("example.org"))),
            "{links:?}"
        );
        assert!(
            links
                .iter()
                .any(|l| matches!(l.target, LinkTarget::Page { page: 1, .. })),
            "{links:?}"
        );
    });
    // Follow the internal link.
    let target = pane.read_with(cx, |p, _| {
        p.links[&0]
            .iter()
            .find(|l| matches!(l.target, LinkTarget::Page { .. }))
            .unwrap()
            .target
            .clone()
    });
    update(cx, window, &pane, |p, _, cx| p.follow(&target, cx));
    assert_eq!(pane.read_with(cx, |p, _| p.current_page()), 1);

    // Web links open in the browser; anything else stays closed.
    let uri = |u: &str| LinkTarget::Uri(u.into());
    update(cx, window, &pane, |p, _, cx| {
        p.follow(&uri("file:///etc/passwd"), cx)
    });
    assert_eq!(cx.opened_url(), None);
    update(cx, window, &pane, |p, _, cx| {
        p.follow(&uri("HTTPS://example.org/x"), cx)
    });
    assert_eq!(cx.opened_url().as_deref(), Some("HTTPS://example.org/x"));
    update(cx, window, &pane, |p, _, cx| {
        p.follow(&uri("mailto:a@example.org"), cx)
    });
    assert_eq!(cx.opened_url().as_deref(), Some("mailto:a@example.org"));
}

#[gpui::test]
fn moves_between_pages(cx: &mut TestAppContext) {
    let Some((window, pane)) = loaded(cx) else {
        return;
    };
    let focus = pane.read_with(cx, |p, _| p.focus.clone());
    cx.update_window(window, |_, window, cx| window.focus(&focus, cx))
        .unwrap();
    let page = |cx: &mut TestAppContext| pane.read_with(cx, |p, _| p.current_page());
    cx.dispatch_action(window, NextPage);
    assert_eq!(page(cx), 1);
    cx.dispatch_action(window, NextPage);
    assert_eq!(page(cx), 1);
    cx.dispatch_action(window, PreviousPage);
    assert_eq!(page(cx), 0);
    cx.dispatch_action(window, PreviousPage);
    assert_eq!(page(cx), 0);
    cx.dispatch_action(window, LastPage);
    assert_eq!(page(cx), 1);
    cx.dispatch_action(window, FirstPage);
    assert_eq!(page(cx), 0);
    cx.dispatch_action(window, FitPage);
    pane.read_with(cx, |p, _| assert_eq!(p.zoom, Zoom::FitPage));

    // The page field.
    cx.dispatch_action(window, GoToPage);
    let input = pane
        .read_with(cx, |p, _| p.page_input.clone())
        .expect("the field");
    cx.read(|cx| assert_eq!(input.read(cx).value(), "1"));
    cx.simulate_input(window, "2");
    cx.simulate_keystrokes(window, "enter");
    pane.read_with(cx, |p, _| assert!(p.page_input.is_none()));
    assert_eq!(page(cx), 1);
    // Not a number: stays put, and the field closes.
    cx.dispatch_action(window, GoToPage);
    cx.simulate_input(window, "x");
    cx.simulate_keystrokes(window, "enter");
    assert_eq!(page(cx), 1);
    pane.read_with(cx, |p, _| assert!(p.page_input.is_none()));
}

#[gpui::test]
fn pages_are_drawn_again_once_the_scale_settles(cx: &mut TestAppContext) {
    // No PDFium needed: this is the pane's own bookkeeping.
    let (_, pane) = pane(cx, Vec::new());
    let notified = Rc::new(RefCell::new(0));
    let _observer = cx.update(|cx| {
        let notified = notified.clone();
        cx.observe(&pane, move |_, _| *notified.borrow_mut() += 1)
    });
    let settled =
        |cx: &mut TestAppContext, scale| pane.update(cx, |p, cx| p.settled_scale(scale, cx));
    // The first scale, and one after a pause, at once.
    assert_eq!(settled(cx, 2000), 2000);
    cx.executor().advance_clock(SCALE_SETTLE);
    assert_eq!(settled(cx, 1000), 1000);
    // Changing on every frame (a live resize, a move to another display):
    // the pages stay as they are until it stops.
    assert_eq!(settled(cx, 1100), 1000);
    assert_eq!(settled(cx, 1200), 1000);
    cx.run_until_parked();
    let before = *notified.borrow();
    cx.executor().advance_clock(SCALE_SETTLE);
    // Then the pane draws again, at the last scale.
    assert!(*notified.borrow() > before);
    assert_eq!(settled(cx, 1200), 1200);
}

#[gpui::test]
fn zooms(cx: &mut TestAppContext) {
    let Some((window, pane)) = loaded(cx) else {
        return;
    };
    let zoom = |cx: &mut TestAppContext| pane.read_with(cx, |p, _| p.zoom_factor());
    let fit = zoom(cx);
    update(cx, window, &pane, |p, _, cx| p.zoom_in(cx));
    assert!((zoom(cx) - fit * 1.2).abs() < 1e-3);
    update(cx, window, &pane, |p, _, cx| {
        p.zoom_out(cx);
        p.zoom_out(cx);
    });
    assert!((zoom(cx) - fit / 1.2).abs() < 1e-3);
    for _ in 0..40 {
        update(cx, window, &pane, |p, _, cx| p.zoom_in(cx));
    }
    assert_eq!(zoom(cx), MAX_ZOOM);
    update(cx, window, &pane, |p, _, cx| p.zoom_to_fit(cx));
    assert_eq!(zoom(cx), fit);
    // The pages are drawn again at the new scale, once it settles.
    cx.executor().advance_clock(SCALE_SETTLE);
    wait(cx, window, &pane, "sharper bitmaps", |cx| {
        pane.read_with(cx, |p, _| {
            p.shown[&0].key.scale_milli == scale_milli(fit, p.scale_factor)
        })
    });

    // ⌘ + wheel zooms around the pointer; the wheel alone scrolls.
    let bounds = pane.read_with(cx, |p, _| p.scroll.bounds());
    let mut vcx = VisualTestContext::from_window(window, cx);
    let wheel = |modifiers| ScrollWheelEvent {
        position: bounds.center(),
        delta: ScrollDelta::Pixels(point(px(0.), px(80.))),
        modifiers,
        touch_phase: TouchPhase::Moved,
    };
    vcx.simulate_event(wheel(Modifiers::secondary_key()));
    let zoomed = vcx.read(|cx| pane.read(cx).zoom_factor());
    assert!((zoomed - fit * 1.2).abs() < 1e-3, "{zoomed} {fit}");
    vcx.simulate_event(wheel(Modifiers::none()));
    assert_eq!(vcx.read(|cx| pane.read(cx).zoom_factor()), zoomed);
    vcx.simulate_event(PinchEvent {
        position: bounds.center(),
        delta: 0.5,
        modifiers: Modifiers::none(),
        phase: TouchPhase::Moved,
    });
    let pinched = vcx.read(|cx| pane.read(cx).zoom_factor());
    assert!((pinched - zoomed * 1.5).abs() < 1e-3, "{pinched}");
}

#[gpui::test]
fn finds_text(cx: &mut TestAppContext) {
    let Some((window, pane)) = loaded(cx) else {
        return;
    };
    cx.dispatch_action(window, FindInPdf);
    let input = pane.read_with(cx, |p, _| p.find.as_ref().unwrap().input.clone());
    cx.simulate_input(window, "Needle");
    pane.read_with(cx, |p, _| assert!(p.find.as_ref().unwrap().searching));
    cx.executor().advance_clock(Duration::from_millis(300));
    let hits = |cx: &mut TestAppContext| {
        pane.read_with(cx, |p, _| {
            let f = p.find.as_ref().unwrap();
            (!f.searching).then(|| (f.hits.iter().map(|h| h.page).collect::<Vec<_>>(), f.current))
        })
    };
    wait(cx, window, &pane, "the hits", |cx| hits(cx).is_some());
    assert_eq!(hits(cx), Some((vec![0, 1], Some(0))));
    // Enter goes to the next one, ⇧Enter back, around the ends.
    cx.simulate_keystrokes(window, "enter");
    // Not searched again: the text is the same (a search would go back to the hit of the
    // page, and Enter would never get past it).
    assert_eq!(hits(cx).unwrap().1, Some(1));
    cx.executor().advance_clock(Duration::from_millis(300));
    wait(cx, window, &pane, "nothing", |_| true);
    assert_eq!(hits(cx).unwrap().1, Some(1));
    assert_eq!(pane.read_with(cx, |p, _| p.current_page()), 1);
    cx.simulate_keystrokes(window, "enter");
    assert_eq!(hits(cx).unwrap().1, Some(0));
    cx.simulate_keystrokes(window, "shift-enter");
    assert_eq!(hits(cx).unwrap().1, Some(1));

    // Matching case: "Needle" is not in the document.
    update(cx, window, &pane, |p, _, cx| {
        p.find.as_mut().unwrap().match_case = true;
        p.search(Duration::ZERO, cx);
    });
    wait(cx, window, &pane, "no hits", |cx| hits(cx).is_some());
    assert_eq!(hits(cx), Some((vec![], None)));
    update(cx, window, &pane, |p, _, cx| p.next_hit(1, cx));
    assert_eq!(hits(cx), Some((vec![], None)));

    // An empty field finds nothing, at once.
    cx.update_window(window, |_, window, cx| {
        input.update(cx, |i, cx| i.set_value("  ", window, cx))
    })
    .unwrap();
    update(cx, window, &pane, |p, _, cx| p.search(Duration::ZERO, cx));
    assert_eq!(hits(cx), Some((vec![], None)));

    // Reopening keeps the bar; Escape closes it, then the selection.
    cx.dispatch_action(window, FindInPdf);
    cx.dispatch_action(window, CloseFind);
    pane.read_with(cx, |p, _| assert!(p.find.is_none()));
    cx.dispatch_action(window, CloseFind);
    update(cx, window, &pane, |p, _, cx| {
        p.next_hit(1, cx);
        p.search(Duration::ZERO, cx);
    });
}

#[gpui::test]
fn reloads_keep_the_reading_position(cx: &mut TestAppContext) {
    let Some((window, pane)) = loaded(cx) else {
        return;
    };
    // The hits follow the new document too.
    cx.dispatch_action(window, FindInPdf);
    cx.simulate_input(window, "needle");
    cx.executor().advance_clock(Duration::from_millis(300));
    wait(cx, window, &pane, "the hits", |cx| {
        pane.read_with(cx, |p, _| p.find.as_ref().unwrap().hits.len() == 2)
    });
    // Read on, away from the hits.
    update(cx, window, &pane, |p, _, cx| p.go_to(1, Some(400.0), cx));
    let offset = pane.read_with(cx, |p, _| p.scroll.offset());
    assert!(offset.y < px(0.));

    update(cx, window, &pane, |p, _, cx| {
        p.load(fixture("links/links.pdf"), None, cx)
    });
    wait(cx, window, &pane, "the reload", |cx| {
        pane.read_with(cx, |p, _| {
            p.doc_generation == p.generation && p.generation == 2
        })
    });
    wait(cx, window, &pane, "its pages", |cx| {
        pane.read_with(cx, |p, _| {
            p.shown.get(&1).is_some_and(|s| s.key.generation == 2)
        })
    });
    pane.read_with(cx, |p, _| {
        assert_eq!(p.scroll.offset(), offset);
        assert!(p.synctex.is_none(), "the new document has no SyncTeX");
    });
    wait(cx, window, &pane, "the hits again", |cx| {
        pane.read_with(cx, |p, _| {
            let f = p.find.as_ref().unwrap();
            !f.searching && f.hits.len() == 2
        })
    });
    // Without moving the reader back to a hit.
    pane.read_with(cx, |p, _| assert_eq!(p.scroll.offset(), offset));

    // A reload with fewer pages drops the bitmaps of the others.
    update(cx, window, &pane, |p, _, cx| {
        p.load(fixture("pdf-pane/one-page.pdf"), None, cx)
    });
    wait(cx, window, &pane, "one page", |cx| {
        pane.read_with(cx, |p, _| p.page_count() == 1)
    });
    pane.read_with(cx, |p, _| assert!(!p.shown.contains_key(&1)));

    // One that does not open keeps what is shown.
    update(cx, window, &pane, |p, _, cx| {
        p.load(fixture("pdf-pane/not-a.pdf"), None, cx)
    });
    wait(cx, window, &pane, "the failure", |cx| {
        pane.read_with(cx, |p, _| p.generation == 4 && p.last_request.is_empty())
    });
    pane.read_with(cx, |p, _| {
        assert_eq!(p.page_count(), 1);
        assert!(p.error.is_none());
    });
    // A file that is gone neither.
    update(cx, window, &pane, |p, _, cx| {
        p.load(fixture("pdf-pane/missing.pdf"), None, cx)
    });
    pane.read_with(cx, |p, _| assert_eq!(p.page_count(), 1));
}

#[gpui::test]
fn a_pdf_that_does_not_open_says_so(cx: &mut TestAppContext) {
    let Some(dirs) = pdfium() else {
        return;
    };
    let (window, pane) = pane(cx, dirs);
    update(cx, window, &pane, |p, _, cx| {
        p.load(fixture("pdf-pane/not-a.pdf"), None, cx)
    });
    wait(cx, window, &pane, "the failure", |cx| {
        pane.read_with(cx, |p, _| p.error.is_some())
    });
    pane.read_with(cx, |p, _| {
        let error = p.error.as_ref().unwrap();
        assert!(error.starts_with("Could not open PDF"), "{error}");
    });
    // A newer load wins over one still being read.
    update(cx, window, &pane, |p, _, cx| {
        p.load(fixture("links/links.pdf"), None, cx);
        p.load(fixture("pdf-pane/one-page.pdf"), None, cx);
    });
    wait(cx, window, &pane, "the last one", |cx| {
        pane.read_with(cx, |p, _| p.page_count() == 1)
    });
}

#[gpui::test]
fn dark_pages_are_rendered_again(cx: &mut TestAppContext) {
    let Some((window, pane)) = loaded(cx) else {
        return;
    };
    update(cx, window, &pane, |p, _, cx| {
        p.set_dark(true, cx);
        // Again: nothing to do.
        p.set_dark(true, cx);
    });
    wait(cx, window, &pane, "dark bitmaps", |cx| {
        pane.read_with(cx, |p, _| p.shown[&0].key.dark)
    });
}

#[gpui::test]
fn answers_the_agents(cx: &mut TestAppContext) {
    let Some((window, pane)) = loaded(cx) else {
        return;
    };
    let (snapshot, text, page, nothing) = update(cx, window, &pane, |p, _, _| {
        (
            p.snapshot(0, 0.5),
            p.text(None),
            p.text(Some(1)),
            p.snapshot(9, 1.0),
        )
    });
    let mut got = (None, None, None, None);
    wait(cx, window, &pane, "the answers", |_| {
        got.0 = got.0.take().or(snapshot.try_recv().ok());
        got.1 = got.1.take().or(text.try_recv().ok());
        got.2 = got.2.take().or(page.try_recv().ok());
        got.3 = got.3.take().or(nothing.try_recv().ok());
        got.0.is_some() && got.1.is_some() && got.2.is_some() && got.3.is_some()
    });
    let bitmap = got.0.unwrap().unwrap();
    let size = pane.read_with(cx, |p, _| p.pages[0]);
    assert_eq!(bitmap.width, (size.width * 0.5).round() as u32);
    let all = got.1.unwrap().unwrap();
    assert!(all.contains("The needle is here") && all.contains("Another needle"));
    let second = got.2.unwrap().unwrap();
    assert!(second.contains("Another needle") && !second.contains("The needle is here"));
    assert!(got.3.unwrap().is_none());
}

/// The window position of a point of `page` (PDF points from its top left).
fn position_of(pane: &PdfPane, page: usize, x: f32, y: f32) -> Point<Pixels> {
    let zoom = pane.zoom_factor();
    let (rects, _) = pane.layout(zoom);
    let r = rects[page];
    pane.scroll.bounds().origin
        + pane.scroll.offset()
        + r.origin
        + point(px(x * zoom), px(y * zoom))
}

#[gpui::test]
fn selects_and_copies_text(cx: &mut TestAppContext) {
    let Some((window, pane)) = loaded(cx) else {
        return;
    };
    // Where "needle" is on the first page.
    update(cx, window, &pane, |p, window, cx| {
        p.open_find(window, cx);
        p.find
            .as_ref()
            .unwrap()
            .input
            .update(cx, |i, cx| i.set_value("needle", window, cx));
        p.search(Duration::ZERO, cx);
    });
    wait(cx, window, &pane, "the hit", |cx| {
        pane.read_with(cx, |p, _| !p.find.as_ref().unwrap().hits.is_empty())
    });
    let r = pane.read_with(cx, |p, _| p.find.as_ref().unwrap().hits[0].rects[0]);
    update(cx, window, &pane, |p, window, cx| {
        p.close_find(window, cx);
        p.go_to(0, None, cx);
    });
    let (from, to) = pane.read_with(cx, |p, _| {
        (
            position_of(p, 0, r.x + 1.0, r.y + r.height / 2.0),
            position_of(p, 0, r.x + r.width - 1.0, r.y + r.height / 2.0),
        )
    });
    let mut vcx = VisualTestContext::from_window(window, cx);
    vcx.simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
    vcx.simulate_mouse_move(to, Some(MouseButton::Left), Modifiers::none());
    vcx.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
    wait(cx, window, &pane, "the page's text", |cx| {
        pane.read_with(cx, |p, _| p.texts.contains_key(&0))
    });
    cx.dispatch_action(window, CopyText);
    let copied = cx.read_from_clipboard().and_then(|c| c.text());
    assert_eq!(copied.as_deref(), Some("needle"));
    // A move without the button pressed changes nothing.
    let mut vcx = VisualTestContext::from_window(window, cx);
    vcx.simulate_mouse_move(from, None, Modifiers::none());
    pane.read_with(cx, |p, _| assert!(p.selected_range().is_some()));
    // Escape drops the selection; a click selects nothing.
    cx.dispatch_action(window, CloseFind);
    pane.read_with(cx, |p, _| assert!(p.selection.is_none()));
    let mut vcx = VisualTestContext::from_window(window, cx);
    vcx.simulate_click(from, Modifiers::none());
    pane.read_with(cx, |p, _| assert!(p.selection.is_none()));
    cx.write_to_clipboard(ClipboardItem::new_string("before".into()));
    cx.dispatch_action(window, CopyText);
    let copied = cx.read_from_clipboard().and_then(|c| c.text());
    assert_eq!(copied.as_deref(), Some("before"));
}

#[gpui::test]
fn synctex_both_ways(cx: &mut TestAppContext) {
    let Some((window, pane)) = loaded(cx) else {
        return;
    };
    let searched: Rc<RefCell<Vec<(PathBuf, u32)>>> = Rc::default();
    let s = searched.clone();
    cx.update(|cx| {
        cx.subscribe(&pane, move |_, ev: &PdfPaneEvent, _| {
            if let PdfPaneEvent::InverseSearch { file, line } = ev {
                s.borrow_mut().push((file.clone(), *line));
            }
        })
        .detach();
    });
    // Source → PDF: the place is highlighted for a while.
    let found = update(cx, window, &pane, |p, _, cx| {
        p.forward_search(Path::new("main.tex"), 17, cx)
    });
    assert!(found);
    let (rect, page) = pane.read_with(cx, |p, _| {
        let h = p.highlight.as_ref().unwrap();
        (h.rects[0], h.page)
    });
    assert_eq!(page, 0);
    cx.executor().advance_clock(Duration::from_millis(2000));
    cx.run_until_parked();
    pane.read_with(cx, |p, _| assert!(p.highlight.is_none()));
    assert!(!update(cx, window, &pane, |p, _, cx| {
        p.forward_search(Path::new("unknown.tex"), 1, cx)
    }));

    // PDF → source: ⌘-click.
    let at = pane.read_with(cx, |p, _| {
        position_of(p, 0, rect.x + rect.width / 2.0, rect.y + rect.height / 2.0)
    });
    let mut vcx = VisualTestContext::from_window(window, cx);
    vcx.simulate_click(at, Modifiers::secondary_key());
    let searched = searched.borrow();
    assert_eq!(searched.len(), 1, "{searched:?}");
    assert!(searched[0].0.ends_with("main.tex"), "{searched:?}");
    assert!((17..=19).contains(&searched[0].1), "{searched:?}");
}

#[gpui::test]
fn leaves_its_window_with_the_bitmaps_drawn_there(cx: &mut TestAppContext) {
    let Some((window, pane)) = loaded(cx) else {
        return;
    };
    update(cx, window, &pane, |p, window, cx| {
        p.open_find(window, cx);
        p.prompt_page(window, cx);
        assert!(p.find.is_some() && p.page_input.is_some());
        p.set_detached(true, cx);
    });
    let drawn = update(cx, window, &pane, |p, _, cx| {
        let shown = p.shown.len();
        let drawn = p.leave_window(cx);
        assert_eq!(drawn.len(), shown);
        assert!(p.to_drop.is_empty());
        // Fields of the old window are gone; the new layout asks for pages again.
        assert!(p.find.is_none() && p.page_input.is_none());
        assert!(p.last_request.is_empty());
        drawn
    });
    assert!(!drawn.is_empty());
    wait(cx, window, &pane, "the pages laid out again", |cx| {
        pane.read_with(cx, |p, _| p.viewport.width > px(0.))
    });
    pane.read_with(cx, |p, _| assert!(p.is_detached()));
}

#[gpui::test]
fn keeps_the_reading_place_in_a_wider_window(cx: &mut TestAppContext) {
    let Some((window, pane)) = loaded(cx) else {
        return;
    };
    update(cx, window, &pane, |p, _, cx| p.go_to(1, Some(300.0), cx));
    wait(cx, window, &pane, "the second page", |cx| {
        pane.read_with(cx, |p, _| p.scroll.offset().y < px(0.))
    });
    let (place, width) = pane.read_with(cx, |p, _| {
        let place = p.geometry().place_at(-p.scroll.offset().y).unwrap();
        (place, p.viewport.width)
    });
    assert_eq!(place.page, 1);

    // Moved to a window twice as wide: the pages are taller, the reader is still there.
    update(cx, window, &pane, |p, window, cx| {
        for image in p.leave_window(cx) {
            let _ = window.drop_image(image);
        }
    });
    let bounds = cx
        .update_window(window, |_, window, _| window.bounds().size)
        .unwrap();
    cx.simulate_window_resize(window, size(bounds.width * 2.0, bounds.height));
    wait(cx, window, &pane, "the wider layout", |cx| {
        pane.read_with(cx, |p, _| {
            p.viewport.width > width * 1.5 && p.pending_scroll.is_none()
        })
    });
    pane.read_with(cx, |p, _| {
        let now = p.geometry().place_at(-p.scroll.offset().y).unwrap();
        assert_eq!(now.page, place.page);
        assert!((now.down - place.down).abs() < 0.01, "{now:?} {place:?}");
    });
    // Until the reader scrolls.
    assert!(pane.read_with(cx, |p, _| p.keep_place.is_some()));
    pane.update(cx, |p, cx| {
        p.scroll.set_offset(point(px(0.), px(-10.)));
        cx.notify();
    });
    wait(cx, window, &pane, "the place let go", |cx| {
        pane.read_with(cx, |p, _| p.keep_place.is_none())
    });
    pane.read_with(cx, |p, _| assert_eq!(p.scroll.offset().y, px(-10.)));
}
