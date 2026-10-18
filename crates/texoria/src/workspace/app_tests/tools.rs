//! TeXoria's tools for agents (`on_tool_call`), but `build`, which needs an engine
//! (see `builds.rs`).

use serde_json::json;

use super::*;
use core::prelude::v1::test;
use gpui_kit::gpui;

#[gpui::test]
fn tools_without_a_document(cx: &mut TestAppContext) {
    let app = Open::empty(cx, quiet());
    let error = |r: ToolResult| {
        assert!(r.is_error, "{}", r.text);
        r.text
    };
    assert_eq!(
        error(app.tool(cx, "build", json!({}))),
        "No document is open."
    );
    let r = app.tool(cx, "problems", json!({}));
    assert!(!r.is_error);
    assert!(r.text.starts_with("No TeX engine found."), "{}", r.text);
    assert!(r.text.ends_with("No errors or warnings."), "{}", r.text);
    assert_eq!(
        app.tool(cx, "outline", json!({})).text,
        "The document has no sections, figures or tables yet."
    );
    assert_eq!(
        app.tool(cx, "bib_search", json!({})).text,
        "No entry matches."
    );
    let words = app.tool(cx, "word_count", json!({})).text;
    assert!(words.starts_with("0 words: 0 in the text"), "{words}");
    let no_pdf = "There is no PDF yet: build first.";
    assert_eq!(
        error(app.tool(cx, "pdf_page", json!({ "page": 1 }))),
        no_pdf
    );
    assert_eq!(error(app.tool(cx, "pdf_text", json!({}))), no_pdf);
    assert_eq!(
        error(app.tool(cx, "rm_rf", json!({}))),
        "Unknown tool: rm_rf"
    );
}

#[gpui::test]
fn tools_report_on_the_project(cx: &mut TestAppContext) {
    let app = Open::sample(cx, quiet());
    // No engine: nothing to build with.
    let r = app.tool(cx, "build", json!({}));
    assert!(r.is_error);
    assert!(r.text.contains("No TeX engine is installed"), "{}", r.text);

    let outline = app.tool(cx, "outline", json!({})).text;
    let lines: Vec<&str> = outline.lines().collect();
    assert!(
        lines.contains(&"    Section: Introduction (main.tex:15)"),
        "{outline}"
    );
    assert!(
        lines.contains(&"      Subsection: Discussion (main.tex:31)"),
        "{outline}"
    );
    assert!(
        lines.iter().any(|l| l.contains("(chapters/methods.tex:")),
        "{outline}"
    );

    let all = app.tool(cx, "bib_search", json!({ "query": "" })).text;
    assert_eq!(all.lines().count(), 2, "{all}");
    assert!(
        all.contains("knuth1984 — Donald E. Knuth (1984) The \\TeXbook [refs.bib]"),
        "{all}"
    );
    let found = app
        .tool(cx, "bib_search", json!({ "query": "LAMPORT 1986" }))
        .text;
    assert!(
        found.starts_with("lamport1986 — Leslie Lamport (1986)"),
        "{found}"
    );
    assert_eq!(found.lines().count(), 1);
    let none = app
        .tool(cx, "bib_search", json!({ "query": "knuth 1986" }))
        .text;
    assert_eq!(none, "No entry matches.");

    // `\bibitem`s count too.
    let thebib = "\\begin{thebibliography}{9}\n\\bibitem{turing1950} Turing.\n\
                  \\end{thebibliography}\n";
    let main = app.path("main.tex");
    let end = app.text(cx, &main).find("\\end{document}").unwrap();
    app.type_at(cx, &main, end, thebib);
    app.update(cx, |ws, _, cx| ws.reindex(cx));
    cx.run_until_parked();
    let items = app
        .tool(cx, "bib_search", json!({ "query": "turing" }))
        .text;
    assert_eq!(items, "turing1950 — \\bibitem [main.tex]");

    let words = app.tool(cx, "word_count", json!({})).text;
    let first = words.lines().next().unwrap();
    assert!(first.ends_with("displayed formulas."), "{words}");
    let total: usize = first.split(' ').next().unwrap().parse().unwrap();
    assert!(total > 20, "{words}");
    assert!(words.contains("\nmain.tex: "), "{words}");
    assert!(words.contains("\nchapters/methods.tex: "), "{words}");
}

#[gpui::test]
fn problems_list_the_build_and_the_checks(cx: &mut TestAppContext) {
    let app = Open::sample(cx, quiet());
    let main = app.path("main.tex");
    let diagnostic = |severity, line, message: &str| Diagnostic {
        severity,
        file: Some(main.clone()),
        line: Some(line),
        message: message.into(),
    };
    app.update(cx, |ws, _, cx| {
        ws.state = CompileState::Done {
            status: Status::Failed,
            elapsed: Duration::from_millis(1500),
            errors: 1,
            warnings: 1,
        };
        ws.diagnostics = vec![
            diagnostic(Severity::Error, 17, "Undefined control sequence"),
            diagnostic(Severity::Warning, 30, "Overfull \\hbox"),
            diagnostic(Severity::Info, 2, "Package loaded"),
        ];
        ws.apply_diagnostics(cx);
    });
    let r = app.tool(cx, "problems", json!({})).text;
    assert_eq!(
        r,
        "The last run failed.\nmain.tex:17: error: Undefined control sequence\n\
         main.tex:30: warning: Overfull \\hbox"
    );
    let report = app.read(cx, |ws, _| {
        ws.build_report(Status::Success, Duration::from_millis(1234))
    });
    assert!(
        report.starts_with("Built in 1.2 s.\nmain.tex:17"),
        "{report}"
    );
    let report = app.read(cx, |ws, _| {
        ws.build_report(Status::Cancelled, Duration::ZERO)
    });
    assert!(report.starts_with("The build was stopped.\n"), "{report}");

    // A live check's finding: an undefined reference.
    let main_text = app.text(cx, &main);
    let at = main_text.find("\\section{Results}").unwrap();
    app.type_at(cx, &main, at, "See \\ref{sec:nowhere}.\n");
    app.update(cx, |ws, _, cx| ws.reindex(cx));
    cx.run_until_parked();
    let r = app.tool(cx, "problems", json!({})).text;
    assert!(r.contains("sec:nowhere"), "{r}");
    // Drawn as the Problems list.
    app.update(cx, |ws, _, cx| {
        ws.show_diagnostics = true;
        cx.notify()
    });
    app.draw(cx);
}

#[test]
fn png_of_checks_the_bitmap_size() {
    let bitmap = |width, height, len| pdf_view::Bitmap {
        width,
        height,
        bgra: [0x10, 0x20, 0x30, 0xff].repeat(len),
    };
    assert!(png_of(bitmap(2, 2, 3)).is_err());
    use base64::Engine as _;
    let png = png_of(bitmap(2, 1, 2)).unwrap();
    let png = base64::engine::general_purpose::STANDARD
        .decode(png)
        .unwrap();
    let image = image::load_from_memory(&png).unwrap().to_rgba8();
    // BGRA in, RGBA out.
    assert_eq!(image.get_pixel(1, 0).0, [0x30, 0x20, 0x10, 0xff]);
}

#[test]
fn small_helpers() {
    assert_eq!(thousands(0), "0");
    assert_eq!(thousands(999), "999");
    assert_eq!(thousands(1000), "1,000");
    assert_eq!(thousands(1234567), "1,234,567");
    assert_eq!(selected_words("Hello \\textbf{big} world, 42 — !"), 3);
    assert_eq!(plural(1), "");
    assert_eq!(plural(2), "s");
    assert_eq!(lines_label(Path::new("main.tex"), 4, 4), "main.tex, line 4");
    assert_eq!(
        lines_label(Path::new("main.tex"), 4, 9),
        "main.tex, lines 4–9"
    );
}
