//! The agents' tools that read the PDF, with PDFium (`scripts/fetch-pdfium.sh`).

use serde_json::json;

use super::*;
use core::prelude::v1::test;
use gpui_kit::gpui;

use test_support::pdfium;

#[gpui::test]
fn pdf_tools_read_the_pdf_on_screen(cx: &mut TestAppContext) {
    let Some(pdfium) = pdfium() else {
        return;
    };
    testing::allow_threads(cx);
    let mut startup = quiet();
    startup.pdfium_dirs = vec![pdfium];
    let app = Open::empty(cx, startup);
    let pdf = test_support::fixtures().join("links/links.pdf");
    let pane = app.read(cx, |ws, _| ws.pdf.clone());
    pane.update(cx, |p, cx| p.load(pdf, None, cx));
    // The renderer's events reach the pane only when a test hands them over.
    let pump = |cx: &mut TestAppContext| pane.update(cx, |p, cx| p.pump(cx));
    testing::wait_until(cx, "the PDF", |cx| {
        pump(cx);
        pane.read_with(cx, |p, _| p.page_count() == 2)
    });
    let tool = |cx: &mut TestAppContext, name: &str, arguments| {
        let (reply, answer) = mpsc::channel();
        let call = ToolCall {
            name: name.to_string(),
            arguments,
            reply,
        };
        app.update(cx, |ws, _, cx| ws.on_tool_call(call, cx));
        let mut result = None;
        testing::wait_until(cx, name, |cx| {
            pump(cx);
            result = answer.try_recv().ok();
            result.is_some()
        });
        result.unwrap()
    };

    let page = tool(cx, "pdf_page", json!({ "page": 2, "scale": 0.5 }));
    assert!(!page.is_error, "{}", page.text);
    assert_eq!(page.text, "Page 2 of 2.");
    assert_eq!(page.images.len(), 1);
    assert_eq!(page.images[0].mime_type, "image/png");
    use base64::Engine as _;
    let png = base64::engine::general_purpose::STANDARD
        .decode(&page.images[0].data)
        .unwrap();
    assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
    // Half a point per pixel: a US letter page is 306 pixels wide.
    let image = image::load_from_memory(&png).unwrap();
    assert_eq!(image.width(), 306);

    for bad in [json!({ "page": 3 }), json!({ "page": 0 }), json!({})] {
        let r = tool(cx, "pdf_page", bad);
        assert!(r.is_error);
        assert_eq!(r.text, "The PDF has pages 1 to 2.");
    }
    let r = tool(cx, "pdf_text", json!({ "page": 9 }));
    assert_eq!(
        (r.is_error, r.text.as_str()),
        (true, "The PDF has pages 1 to 2.")
    );

    let all = tool(cx, "pdf_text", json!({})).text;
    let pages: Vec<&str> = all.split('\u{c}').collect();
    assert_eq!(pages.len(), 2, "{all:?}");
    assert!(pages[0].contains("The needle is here."), "{all:?}");
    assert!(pages[1].contains("Another needle."), "{all:?}");
    let second = tool(cx, "pdf_text", json!({ "page": 2 })).text;
    assert!(second.contains("Another needle.") && !second.contains("The needle"));
}
