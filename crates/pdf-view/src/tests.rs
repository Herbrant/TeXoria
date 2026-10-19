use super::*;
use std::ops::Range;
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

#[test]
fn finds_the_nearest_char() {
    let ch = |ch, x, y| TextChar {
        ch,
        rect: Rect {
            x,
            y,
            width: 5.0,
            height: 10.0,
        },
    };
    let chars = vec![ch('a', 0.0, 0.0), ch('b', 6.0, 0.0), ch('c', 0.0, 20.0)];
    assert_eq!(nearest_char(&chars, 7.0, 5.0), Some(1));
    // Past the end of the first line: its last character.
    assert_eq!(nearest_char(&chars, 50.0, 4.0), Some(1));
    assert_eq!(nearest_char(&chars, 2.0, 40.0), Some(2));
    assert_eq!(nearest_char(&[], 0.0, 0.0), None);
    assert_eq!(text_of(&chars, 0..2), "ab");
    assert_eq!(text_of(&chars, 1..9), "bc");
}

fn vendor() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vendor/pdfium")
}

/// A renderer on the repository's `vendor/pdfium`, if PDFium is there. Only a missing
/// library skips: any other error fails the test.
fn renderer(on_event: impl Fn(Event) + Send + 'static) -> Option<Renderer> {
    let library = Pdfium::pdfium_platform_library_name_at_path(&vendor());
    if !library.is_file() {
        test_support::skip("PDFIUM", &format!("no {}", library.display()));
        return None;
    }
    Some(Renderer::new(vec![vendor()], on_event).expect("start the renderer"))
}

/// A channel's receiver that waits at most 10 s for each event.
fn next(rx: &mpsc::Receiver<Event>) -> Event {
    rx.recv_timeout(Duration::from_secs(10)).expect("an event")
}

#[test]
fn several_renderers_share_the_library() {
    let bytes = std::fs::read(test_support::fixtures().join("links/links.pdf")).unwrap();
    let start = || {
        let (tx, rx) = mpsc::channel();
        let r = renderer(move |e| {
            let _ = tx.send(e);
        })?;
        Some((r, rx))
    };
    // Started together from several threads, as tests in one process do.
    let started: Vec<_> = (0..4).map(|_| std::thread::spawn(start)).collect();
    let renderers: Vec<_> = started.into_iter().map(|t| t.join().unwrap()).collect();
    let Some(mut renderers) = renderers.into_iter().collect::<Option<Vec<_>>>() else {
        return;
    };
    // One more once the library is bound, even from directories without it.
    let (tx, rx) = mpsc::channel();
    let late = Renderer::new(vec![PathBuf::from("/nonexistent")], move |e| {
        let _ = tx.send(e);
    })
    .expect("reuse the bound library");
    renderers.push((late, rx));
    for (g, (r, _)) in renderers.iter().enumerate() {
        r.load(g as u64 + 1, bytes.clone());
        r.request(vec![RenderKey {
            generation: g as u64 + 1,
            page: 1,
            scale_milli: 500,
            dark: false,
        }]);
    }
    for (g, (_, rx)) in renderers.iter().enumerate() {
        let generation = g as u64 + 1;
        assert!(matches!(next(rx), Event::Loaded { generation: l, .. } if l == generation));
        let Event::Rendered { key, bitmap } = next(rx) else {
            panic!("expected Rendered")
        };
        assert_eq!((key.generation, key.page), (generation, 1));
        assert_eq!(bitmap.width, 306);
    }
}

/// A renderer with `tests/fixtures/links/links.pdf` loaded, if PDFium is available.
fn links_pdf() -> Option<(Renderer, mpsc::Receiver<Event>)> {
    let bytes = std::fs::read(test_support::fixtures().join("links/links.pdf")).unwrap();
    let (tx, rx) = mpsc::channel();
    let r = renderer(move |e| {
        let _ = tx.send(e);
    })?;
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

#[test]
fn text_of_takes_any_range() {
    let ch = |ch| TextChar {
        ch,
        rect: Rect {
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
        },
    };
    let chars: Vec<_> = "ab\r\ncd".chars().map(ch).collect();
    assert_eq!(text_of(&chars, 0..6), "ab\ncd");
    assert_eq!(text_of(&chars, Range { start: 4, end: 2 }), "");
    assert_eq!(text_of(&chars, Range { start: 9, end: 1 }), "");
    assert_eq!(text_of(&chars, 7..9), "");
    assert_eq!(text_of(&[], 0..1), "");
}

/// A page number that does not fit PDFium's `int` is no page, not another page.
#[cfg(target_pointer_width = "64")]
#[test]
fn huge_page_numbers_are_out_of_range() {
    let Some((r, rx)) = links_pdf() else {
        return;
    };
    let huge = (1usize << 32) + 1;
    r.query(Query::PageText { page: huge });
    let Event::PageText { page, chars, .. } = next(&rx) else {
        panic!("expected PageText")
    };
    assert_eq!(page, huge);
    assert!(
        chars.is_empty(),
        "{}",
        chars.iter().map(|c| c.ch).collect::<String>()
    );
    r.query(Query::Links { page: huge });
    assert!(matches!(next(&rx), Event::Links { links, .. } if links.is_empty()));
    r.query(Query::Text {
        id: 1,
        page: Some(huge),
    });
    assert!(matches!(next(&rx), Event::Text { id: 1, text: None }));
    r.query(Query::Snapshot {
        id: 2,
        page: huge,
        scale: 1.0,
    });
    assert!(matches!(
        next(&rx),
        Event::Snapshot {
            id: 2,
            bitmap: None
        }
    ));
    // Nor is a page to render.
    r.request(vec![
        RenderKey {
            generation: 1,
            page: huge,
            scale_milli: 1000,
            dark: false,
        },
        RenderKey {
            generation: 1,
            page: 0,
            scale_milli: 100,
            dark: false,
        },
    ]);
    let Event::Rendered { key, .. } = next(&rx) else {
        panic!("expected Rendered")
    };
    assert_eq!(key.page, 0);
}

/// Renders the first page of `tests/fixtures/links/links.pdf` if PDFium is available.
#[test]
fn renders_a_page() {
    let bytes = std::fs::read(test_support::fixtures().join("links/links.pdf")).unwrap();
    let (tx, rx) = mpsc::channel();
    let Some(r) = renderer(move |e| {
        let _ = tx.send(e);
    }) else {
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

/// Bitmaps are BGRA, as GPUI and the PNG snapshots read them: red must not come out blue.
#[test]
fn bitmaps_are_bgra() {
    let content = "1 0 0 rg 0 0 10 10 re f";
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Contents 4 0 R >>".to_string(),
        format!(
            "<< /Length {} >>\nstream\n{content}\nendstream",
            content.len()
        ),
    ];
    let (tx, rx) = mpsc::channel();
    let Some(r) = renderer(move |e| {
        let _ = tx.send(e);
    }) else {
        return;
    };
    r.load(1, build_pdf(&objects.each_ref().map(String::as_str)));
    assert!(matches!(next(&rx), Event::Loaded { .. }));
    r.request(vec![RenderKey {
        generation: 1,
        page: 0,
        scale_milli: 1000,
        dark: false,
    }]);
    let Event::Rendered { bitmap, .. } = next(&rx) else {
        panic!("expected Rendered")
    };
    assert_eq!(&bitmap.bgra[..4], &[0, 0, 255, 255]);
}

/// A small PDF made of `objects` (numbered from 1, the first being the catalog).
fn build_pdf(objects: &[&str]) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    let size = objects.len() + 1;
    out.extend(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
    for o in offsets {
        out.extend(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend(
        format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    out
}

/// Two pages: the first (200 × 100 points) has links to the second (300 × 400) with every
/// kind of destination, and links that lead nowhere.
fn destinations_pdf() -> Vec<u8> {
    let link = |y: u32, target: &str| {
        format!(
            "<< /Type /Annot /Subtype /Link /Rect [0 {y} 50 {}] {target} >>",
            y + 10
        )
    };
    let annots = [
        link(90, "/Dest [3 0 R /FitR 10 20 110 120]"),
        link(80, "/Dest [3 0 R /Fit]"),
        link(70, "/Dest [3 0 R /XYZ 0 350 0]"),
        link(60, "/Dest [3 0 R /FitH 300]"),
        link(50, "/A << /S /GoTo /D [3 0 R /XYZ null null null] >>"),
        link(40, "/A << /S /URI /URI (https://example.org/x) >>"),
        link(30, "/A << /S /Launch /F (x.exe) >>"),
        link(20, ""),
        // To something that is not a page.
        link(10, "/Dest [1 0 R /Fit]"),
    ];
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [4 0 R 3 0 R] /Count 2 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] >>".to_string(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Annots [{}] >>",
            (0..annots.len())
                .map(|i| format!("{} 0 R", i + 5))
                .collect::<Vec<_>>()
                .join(" ")
        ),
    ];
    objects.extend(annots);
    let objects: Vec<&str> = objects.iter().map(String::as_str).collect();
    build_pdf(&objects)
}

/// A renderer with `bytes` loaded as generation 1, and its pages.
fn loaded(bytes: Vec<u8>) -> Option<(Renderer, mpsc::Receiver<Event>, Vec<PageSize>)> {
    let (tx, rx) = mpsc::channel();
    let r = renderer(move |e| {
        let _ = tx.send(e);
    })?;
    r.load(1, bytes);
    let Event::Loaded {
        generation: 1,
        pages,
    } = next(&rx)
    else {
        panic!("expected Loaded")
    };
    Some((r, rx, pages))
}

/// No event comes within a short while.
fn quiet(rx: &mpsc::Receiver<Event>) -> bool {
    rx.recv_timeout(Duration::from_millis(300)).is_err()
}

fn key(generation: u64, page: usize, scale_milli: u32, dark: bool) -> RenderKey {
    RenderKey {
        generation,
        page,
        scale_milli,
        dark,
    }
}

#[test]
fn reads_every_kind_of_link() {
    let Some((r, rx, pages)) = loaded(destinations_pdf()) else {
        return;
    };
    assert_eq!(
        pages,
        vec![
            PageSize {
                width: 200.0,
                height: 100.0
            },
            PageSize {
                width: 300.0,
                height: 400.0
            }
        ]
    );
    r.query(Query::Links { page: 0 });
    let Event::Links {
        generation: 1,
        page: 0,
        links,
    } = next(&rx)
    else {
        panic!("expected Links")
    };
    let page = |y| LinkTarget::Page { page: 1, y };
    let targets: Vec<_> = links.iter().map(|l| l.target.clone()).collect();
    assert_eq!(
        targets,
        vec![
            // The top of the rectangle, 120 points up a 400-point page.
            page(Some(280.0)),
            page(None),
            page(Some(50.0)),
            page(Some(100.0)),
            page(None),
            LinkTarget::Uri("https://example.org/x".into()),
        ]
    );
    // Top-down rectangles: the first link is at the top of its 100-point page.
    assert_eq!(
        links[0].rect,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 50.0,
            height: 10.0
        }
    );
    assert_eq!(links[5].rect.y, 50.0);
    // A page without links, and no page.
    r.query(Query::Links { page: 1 });
    assert!(matches!(next(&rx), Event::Links { page: 1, links, .. } if links.is_empty()));
    r.query(Query::Links { page: 2 });
    assert!(matches!(next(&rx), Event::Links { page: 2, links, .. } if links.is_empty()));
    // No text on these pages.
    r.query(Query::PageText { page: 0 });
    assert!(matches!(next(&rx), Event::PageText { chars, .. } if chars.is_empty()));
    r.query(Query::Text { id: 4, page: None });
    assert!(
        matches!(next(&rx), Event::Text { id: 4, text: Some(t) } if t.trim().is_empty() || t.contains('\u{c}'))
    );
}

#[test]
fn renders_in_the_order_asked_and_only_the_current_generation() {
    let Some((r, rx, _)) = links_pdf().map(|(r, rx)| (r, rx, ())) else {
        return;
    };
    r.request(vec![
        key(1, 1, 100, false),
        key(7, 0, 100, false),
        key(1, 9, 100, false),
        key(1, 0, 100, false),
    ]);
    let mut rendered = Vec::new();
    for _ in 0..2 {
        let Event::Rendered { key, bitmap } = next(&rx) else {
            panic!("expected Rendered")
        };
        assert_eq!((bitmap.width, bitmap.height), (61, 79));
        rendered.push(key.page);
    }
    // Page 1 first; generation 7's and the missing page 9 are skipped.
    assert_eq!(rendered, vec![1, 0]);
    assert!(quiet(&rx));
}

#[test]
fn a_new_request_replaces_the_old_one() {
    let Some((r, rx)) = links_pdf() else {
        return;
    };
    // The worker is busy loading while both requests come in.
    let bytes = std::fs::read(test_support::fixtures().join("links/links.pdf")).unwrap();
    r.load(2, bytes);
    r.request(vec![key(2, 0, 100, false), key(2, 1, 100, false)]);
    r.request(vec![key(2, 1, 200, false)]);
    let mut keys = Vec::new();
    while let Ok(e) = rx.recv_timeout(Duration::from_millis(500)) {
        if let Event::Rendered { key, .. } = e {
            keys.push(key);
        }
    }
    assert_eq!(keys.last(), Some(&key(2, 1, 200, false)));
    assert!(keys.len() <= 3, "{keys:?}");
}

#[test]
fn renders_dark_pages() {
    let Some((r, rx)) = links_pdf() else {
        return;
    };
    r.request(vec![key(1, 0, 500, true)]);
    let Event::Rendered { key: k, bitmap } = next(&rx) else {
        panic!("expected Rendered")
    };
    assert!(k.dark);
    let px = bitmap.bgra.as_chunks::<4>().0;
    // Charcoal paper, light ink.
    let paper = px.iter().filter(|p| p[..3] == [32, 32, 32]).count();
    assert!(paper * 10 > px.len() * 8, "{paper} of {}", px.len());
    assert!(
        px.iter()
            .all(|p| p[..3].iter().all(|c| (32..=228).contains(c)))
    );
    assert!(px.iter().any(|p| p[0] > 150));
}

#[test]
fn a_bad_pdf_keeps_the_previous_one() {
    let Some((r, rx)) = links_pdf() else {
        return;
    };
    r.load(2, b"%PDF-1.7 garbage".to_vec());
    let Event::LoadFailed { generation, error } = next(&rx) else {
        panic!("expected LoadFailed")
    };
    assert_eq!(generation, 2);
    assert!(!error.is_empty());
    r.query(Query::PageText { page: 1 });
    let Event::PageText {
        generation, chars, ..
    } = next(&rx)
    else {
        panic!("expected PageText")
    };
    assert_eq!(generation, 1);
    assert!(text_of(&chars, 0..chars.len()).contains("Another needle"));
    // Its renders go on.
    r.request(vec![key(1, 0, 100, false)]);
    assert!(matches!(next(&rx), Event::Rendered { key, .. } if key.generation == 1));
}

#[test]
fn answers_about_one_page_or_none() {
    let Some((r, rx)) = links_pdf() else {
        return;
    };
    r.query(Query::Text {
        id: 1,
        page: Some(1),
    });
    let Event::Text {
        id: 1,
        text: Some(t),
    } = next(&rx)
    else {
        panic!("expected Text")
    };
    assert!(
        t.contains("Another needle") && !t.contains("The needle is here"),
        "{t}"
    );
    r.query(Query::Text {
        id: 2,
        page: Some(2),
    });
    assert!(matches!(next(&rx), Event::Text { id: 2, text: None }));
    r.query(Query::PageText { page: 2 });
    assert!(matches!(next(&rx), Event::PageText { page: 2, chars, .. } if chars.is_empty()));
    r.query(Query::Links { page: 1 });
    assert!(matches!(next(&rx), Event::Links { page: 1, links, .. } if links.is_empty()));
    // Whole words and case.
    for (text, match_case, whole_word, want) in [
        ("NEEDLE", false, false, 2),
        ("NEEDLE", true, false, 0),
        ("needl", false, true, 0),
        ("needl", false, false, 2),
        ("nothing like it", false, false, 0),
    ] {
        r.query(Query::Search {
            id: 5,
            text: text.into(),
            match_case,
            whole_word,
        });
        let Event::Found { hits, .. } = next(&rx) else {
            panic!("expected Found")
        };
        assert_eq!(hits.len(), want, "{text}");
        assert!(hits.iter().all(|h| !h.rects.is_empty()));
    }
    // Snapshots are clamped to a sane scale.
    r.query(Query::Snapshot {
        id: 3,
        page: 0,
        scale: 100.0,
    });
    let Event::Snapshot {
        id: 3,
        bitmap: Some(b),
    } = next(&rx)
    else {
        panic!("expected Snapshot")
    };
    assert_eq!(b.width, 612 * 4);
}

#[test]
fn answers_without_a_document() {
    assert!(matches!(
        answer(None, Query::PageText { page: 0 }),
        Event::PageText { generation: 0, page: 0, chars } if chars.is_empty()
    ));
    assert!(matches!(
        answer(None, Query::Links { page: 3 }),
        Event::Links { generation: 0, page: 3, links } if links.is_empty()
    ));
    assert!(matches!(
        answer(
            None,
            Query::Search {
                id: 1,
                text: "x".into(),
                match_case: false,
                whole_word: false
            }
        ),
        Event::Found { generation: 0, id: 1, hits } if hits.is_empty()
    ));
    assert!(matches!(
        answer(
            None,
            Query::Snapshot {
                id: 2,
                page: 0,
                scale: 1.0
            }
        ),
        Event::Snapshot {
            id: 2,
            bitmap: None
        }
    ));
    assert!(matches!(
        answer(None, Query::Text { id: 3, page: None }),
        Event::Text { id: 3, text: None }
    ));
}

/// A renderer without a worker, to look at what it queues.
fn idle() -> Renderer {
    Renderer {
        shared: Arc::new((Mutex::new(Shared::default()), Condvar::new())),
        thread: None,
    }
}

#[test]
fn queues_queries_and_requests() {
    let r = idle();
    let search = |id, text: &str| Query::Search {
        id,
        text: text.into(),
        match_case: false,
        whole_word: false,
    };
    r.query(Query::PageText { page: 0 });
    r.query(search(1, "a"));
    r.query(Query::PageText { page: 0 });
    r.query(Query::Links { page: 0 });
    r.query(search(2, "b"));
    r.query(Query::Text { id: 1, page: None });
    r.query(Query::Text { id: 1, page: None });
    {
        let s = r.shared.0.lock().unwrap();
        // The newer search replaces the queued one; the same question is asked once.
        assert_eq!(
            Vec::from(s.queries.clone()),
            vec![
                Query::PageText { page: 0 },
                Query::Links { page: 0 },
                search(2, "b"),
                Query::Text { id: 1, page: None },
            ]
        );
    }
    r.request(vec![
        key(1, 0, 1000, false),
        key(2, 1, 1000, false),
        key(3, 2, 1000, true),
    ]);
    r.load(2, b"pdf".to_vec());
    {
        let s = r.shared.0.lock().unwrap();
        // Renders of older documents are dropped, the order kept.
        assert_eq!(
            s.wanted,
            vec![key(2, 1, 1000, false), key(3, 2, 1000, true)]
        );
        assert_eq!(s.load, Some((2, b"pdf".to_vec())));
    }
    r.request(vec![key(2, 0, 500, false)]);
    assert_eq!(
        r.shared.0.lock().unwrap().wanted,
        vec![key(2, 0, 500, false)]
    );
    // Dropping a renderer without a worker just stops.
    drop(r);
}

#[test]
fn sanitizes_page_sizes() {
    let letter = PageSize {
        width: 612.0,
        height: 792.0,
    };
    assert_eq!(
        sane(100.0, 50.5),
        PageSize {
            width: 100.0,
            height: 50.5
        }
    );
    for (w, h) in [
        (0.0, 10.0),
        (10.0, -1.0),
        (f32::NAN, 10.0),
        (10.0, f32::INFINITY),
        (f32::NEG_INFINITY, f32::NAN),
    ] {
        assert_eq!(sane(w, h), letter, "{w} x {h}");
    }
}

#[test]
fn errors_say_what_failed() {
    let e = Error::Library("libpdfium.so: not found".into());
    assert_eq!(
        e.to_string(),
        "could not load the PDFium library: libpdfium.so: not found"
    );
    // No library anywhere: the error tells where it looked.
    if bind(&[]).is_ok() {
        return;
    }
    let Err(Error::Library(why)) = bind(&[PathBuf::from("/nonexistent")]) else {
        panic!("expected an error");
    };
    assert!(why.contains("system"), "{why}");
    // A file that is no library (or one bound already) is named in the error.
    let dir = std::env::temp_dir().join(format!("pdf-view-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let fake = Pdfium::pdfium_platform_library_name_at_path(&dir);
    std::fs::write(&fake, b"not a library").unwrap();
    let result = bind(std::slice::from_ref(&dir));
    std::fs::remove_dir_all(&dir).unwrap();
    let Err(Error::Library(why)) = result else {
        panic!("expected an error");
    };
    assert!(why.contains(&fake.display().to_string()), "{why}");
}

#[test]
fn renders_nothing_without_a_document() {
    let (tx, rx) = mpsc::channel();
    let Some(r) = renderer(move |e| {
        let _ = tx.send(e);
    }) else {
        return;
    };
    r.request(vec![key(0, 0, 1000, false)]);
    r.query(Query::PageText { page: 0 });
    // The query is answered, the render skipped.
    assert!(matches!(next(&rx), Event::PageText { generation: 0, .. }));
    assert!(quiet(&rx));
}

#[test]
fn search_hits_are_capped() {
    let text = "a ".repeat(MAX_HITS + 50);
    let content = format!("BT /F1 2 Tf 0 50 Td ({text}) Tj ET");
    let page = "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10000 100] /Contents 4 0 R \
         /Resources << /Font << /F1 5 0 R >> >> >>";
    let stream = format!(
        "<< /Length {} >>\nstream\n{content}\nendstream",
        content.len()
    );
    let bytes = build_pdf(&[
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        page,
        &stream,
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
    ]);
    let Some((r, rx, _)) = loaded(bytes) else {
        return;
    };
    r.query(Query::Search {
        id: 1,
        text: "a".into(),
        match_case: true,
        whole_word: false,
    });
    let Event::Found { hits, .. } = next(&rx) else {
        panic!("expected Found")
    };
    assert_eq!(hits.len(), MAX_HITS);
    // Positions are top-down: the text sits 50 points from the bottom of a 100-point page.
    assert!(hits.iter().all(|h| h.page == 0 && h.rects[0].y < 50.0));
}

#[test]
fn renders_rects_and_scales() {
    let r = Rect {
        x: 1.0,
        y: 2.0,
        width: 3.0,
        height: 4.0,
    };
    assert!(r.contains(1.0, 2.0) && r.contains(4.0, 6.0) && r.contains(2.5, 3.0));
    assert!(!r.contains(0.9, 3.0) && !r.contains(2.0, 6.1) && !r.contains(f32::NAN, 3.0));
    assert_eq!(key(1, 0, 1500, false).scale(), 1.5);
    assert_eq!(key(1, 0, 0, false).scale(), 0.0);
}

#[test]
fn nearest_char_skips_unboxed_chars() {
    let ch = |ch, x, width| TextChar {
        ch,
        rect: Rect {
            x,
            y: 0.0,
            width,
            height: 10.0,
        },
    };
    // Generated spaces and line breaks have no box.
    let chars = vec![ch('a', 0.0, 5.0), ch(' ', 0.0, 0.0), ch('b', 20.0, 5.0)];
    assert_eq!(nearest_char(&chars, 1.0, 1.0), Some(0));
    assert_eq!(nearest_char(&chars, 21.0, 1.0), Some(2));
    assert_eq!(nearest_char(&chars, 12.0, 5.0), Some(0));
    assert_eq!(nearest_char(&[ch(' ', 0.0, 0.0)], 0.0, 0.0), None);
    assert_eq!(nearest_char(&chars, f32::NAN, f32::NAN), Some(0));
}

mod properties {
    use super::*;
    use proptest::prelude::*;

    fn char_box() -> impl Strategy<Value = TextChar> {
        let side = prop_oneof![Just(0.0), Just(-1.0), Just(f32::NAN), 0.1f32..20.0];
        (0.0f32..100.0, 0.0f32..100.0, side.clone(), side).prop_map(|(x, y, width, height)| {
            TextChar {
                ch: 'x',
                rect: Rect {
                    x,
                    y,
                    width,
                    height,
                },
            }
        })
    }

    fn boxed(c: &TextChar) -> bool {
        c.rect.width > 0.0 && c.rect.height > 0.0
    }

    proptest! {
        #[test]
        fn bitmaps_stay_bounded(
            w in 0.01f32..1.0e7,
            h in 0.01f32..1.0e7,
            scale_milli: u32,
        ) {
            let (bw, bh) = bitmap_size(w, h, key(1, 0, scale_milli, false).scale());
            prop_assert!((1..=MAX_SIDE as i32).contains(&bw), "{bw}");
            prop_assert!((1..=MAX_SIDE as i32).contains(&bh), "{bh}");
            let area = bw as f32 * bh as f32;
            prop_assert!(area <= (MAX_PIXELS * 1.01).max(MAX_SIDE), "{bw} x {bh}");
        }

        #[test]
        fn dark_pages_stay_soft(px in prop::collection::vec(any::<u8>(), 0..64)) {
            let mut dark = px.clone();
            darken(&mut dark);
            prop_assert_eq!(dark.len(), px.len());
            for (d, p) in dark.as_chunks::<4>().0.iter().zip(px.as_chunks::<4>().0) {
                prop_assert!(d[..3].iter().all(|c| (32..=228).contains(c)));
                prop_assert_eq!(d[3], p[3]);
            }
            // A partial pixel at the end is left alone.
            let tail = px.len() / 4 * 4;
            prop_assert_eq!(&dark[tail..], &px[tail..]);
        }

        #[test]
        fn greys_invert_in_order(a: u8, b: u8) {
            let (lo, hi) = (a.min(b), a.max(b));
            let mut px = vec![lo, lo, lo, 255, hi, hi, hi, 255];
            darken(&mut px);
            prop_assert!(px[0] >= px[4]);
            prop_assert!(px[0] == px[1] && px[1] == px[2]);
        }

        #[test]
        fn nearest_char_is_a_boxed_one(
            chars in prop::collection::vec(char_box(), 0..12),
            x in prop_oneof![Just(f32::NAN), -10.0f32..120.0],
            y in -10.0f32..120.0,
        ) {
            match nearest_char(&chars, x, y) {
                Some(i) => {
                    prop_assert!(boxed(&chars[i]));
                    // The one under the point, if any.
                    if let Some(under) = chars.iter().find(|c| boxed(c) && c.rect.contains(x, y)) {
                        prop_assert_eq!(&chars[i], under);
                    }
                }
                None => prop_assert!(!chars.iter().any(boxed)),
            }
        }

        #[test]
        fn text_of_any_range(text in ".{0,20}", start in 0usize..30, end in 0usize..30) {
            let chars: Vec<_> = text
                .chars()
                .map(|ch| TextChar {
                    ch,
                    rect: Rect { x: 0.0, y: 0.0, width: 1.0, height: 1.0 },
                })
                .collect();
            let out = text_of(&chars, start..end);
            prop_assert!(!out.contains('\r'));
            prop_assert!(out.chars().count() <= end.saturating_sub(start));
            let want: String = text.chars().skip(start).take(end.saturating_sub(start)).collect();
            prop_assert_eq!(out, want.replace('\r', ""));
        }
    }
}
