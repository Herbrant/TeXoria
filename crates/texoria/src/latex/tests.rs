use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, RwLock};

use gpui_kit::component::input::{EditorState, Rope};
use gpui_kit::{TestAppContext, VisualTestContext};
use lsp_types::{CompletionItemKind, CompletionResponse, CompletionTextEdit, ShowDocumentParams};

use super::*;
use crate::testing;
// `super::*` brings gpui's `test` attribute, which would shadow the standard one, and
// gpui-kit's `gpui` alias, which the `#[gpui::test]` macro's paths must not find twice.
use core::prelude::v1::test;
use gpui_kit::gpui;

/// The sample project, read from a temporary copy, and its root file.
fn sample() -> (tempfile::TempDir, PathBuf, Arc<RwLock<Project>>) {
    let dir = testing::fixture_copy("sample");
    let main = crate::paths::canonical(&dir.path().join("main.tex"));
    let project = Arc::new(RwLock::new(Project::open(main.clone())));
    (dir, main, project)
}

fn completions(
    cx: &mut VisualTestContext,
    project: &Arc<RwLock<Project>>,
    text: &str,
    offset: usize,
) -> (Vec<CompletionItem>, Vec<CursorFixup>) {
    let provider = LatexCompletions {
        project: project.clone(),
        fixups: Rc::default(),
    };
    let rope = Rope::from(text);
    let task = cx.update(|window, cx| {
        provider.completions(
            &rope,
            offset,
            CompletionContext {
                trigger_kind: lsp_types::CompletionTriggerKind::INVOKED,
                trigger_character: None,
            },
            window,
            cx,
        )
    });
    let items = match cx.foreground_executor().block_test(task).unwrap() {
        CompletionResponse::Array(items) => items,
        CompletionResponse::List(list) => list.items,
    };
    let fixups = provider.fixups.borrow().clone();
    (items, fixups)
}

fn edit(item: &CompletionItem) -> &InsertReplaceEdit {
    match item.text_edit.as_ref() {
        Some(CompletionTextEdit::InsertAndReplace(edit)) => edit,
        other => panic!("not an insert-and-replace edit: {other:?}"),
    }
}

fn kind_of(items: &[CompletionItem], label: &str) -> Option<CompletionItemKind> {
    items.iter().find(|i| i.label == label).and_then(|i| i.kind)
}

#[gpui::test]
fn completes_commands_with_their_range_and_cursor(cx: &mut TestAppContext) {
    let (_dir, _, project) = sample();
    let cx = cx.add_empty_window();
    let text = "Some text\n  \\fra";
    let (items, fixups) = completions(cx, &project, text, text.len());
    let frac = items
        .iter()
        .find(|i| i.label.starts_with("\\frac"))
        .expect("\\frac is offered");
    assert_eq!(frac.kind, Some(CompletionItemKind::FUNCTION));
    let frac_edit = edit(frac);
    // The typed `\fra`, on the second line, in characters.
    let range = lsp_types::Range::new(
        lsp_types::Position::new(1, 2),
        lsp_types::Position::new(1, 6),
    );
    assert_eq!((frac_edit.insert, frac_edit.replace), (range, range));
    // `\frac{|}{}`: the cursor goes back into the first argument.
    let fixup = fixups
        .iter()
        .find(|f| f.insert == frac_edit.new_text)
        .expect("a fixup for \\frac");
    assert!(
        fixup.back > 0 && fixup.back < fixup.insert.len(),
        "{fixup:?}"
    );
    assert!(fixup.insert[..fixup.insert.len() - fixup.back].ends_with('{'));
    // A completion that ends where it is inserted has no fixup.
    for item in &items {
        let new_text = &edit(item).new_text;
        if !new_text.contains('{') {
            assert!(fixups.iter().all(|f| f.insert != *new_text), "{item:?}");
        }
    }
}

#[gpui::test]
fn completion_kinds_follow_what_is_completed(cx: &mut TestAppContext) {
    let (_dir, _, project) = sample();
    let cx = cx.add_empty_window();
    let at_end =
        |cx: &mut VisualTestContext, text: &str| completions(cx, &project, text, text.len()).0;

    let items = at_end(cx, "\\nor");
    assert_eq!(kind_of(&items, "\\norm"), Some(CompletionItemKind::METHOD));
    let items = at_end(cx, "\\ref{sec:");
    assert_eq!(
        kind_of(&items, "sec:intro"),
        Some(CompletionItemKind::REFERENCE)
    );
    let items = at_end(cx, "\\cite{knu");
    assert_eq!(
        kind_of(&items, "knuth1984"),
        Some(CompletionItemKind::VALUE)
    );
    let items = at_end(cx, "\\begin{equ");
    assert_eq!(
        kind_of(&items, "equation"),
        Some(CompletionItemKind::MODULE)
    );
    let items = at_end(cx, "\\usepackage{amsm");
    assert_eq!(kind_of(&items, "amsmath"), Some(CompletionItemKind::MODULE));
    let items = at_end(cx, "\\input{");
    let kinds: Vec<_> = items.iter().filter_map(|i| i.kind).collect();
    assert!(kinds.contains(&CompletionItemKind::FOLDER), "{items:?}");
    let items = at_end(cx, "\\input{chapters/");
    assert_eq!(
        kind_of(&items, "methods.tex").or(kind_of(&items, "methods")),
        Some(CompletionItemKind::FILE),
        "{items:?}"
    );
    let items = at_end(cx, "\\includegraphics{figures/");
    assert!(
        items
            .iter()
            .any(|i| i.kind == Some(CompletionItemKind::FILE)),
        "{items:?}"
    );
}

/// The character after the cursor is read to see an auto-closed brace: one that takes several
/// bytes must not be cut.
#[gpui::test]
fn completes_before_a_multibyte_character(cx: &mut TestAppContext) {
    let (_dir, _, project) = sample();
    let cx = cx.add_empty_window();
    let text = "\\ref{sec:é}";
    let offset = text.find('é').unwrap();
    let (items, _) = completions(cx, &project, text, offset);
    assert!(items.iter().any(|i| i.label == "sec:intro"), "{items:?}");
    // At the very end of the text there is no next character.
    let (items, _) = completions(cx, &project, "\\sec", 4);
    assert!(items.iter().any(|i| i.label.starts_with("\\section")));
}

#[gpui::test]
fn triggers_on_single_typed_token_characters(cx: &mut TestAppContext) {
    let (_dir, _, project) = sample();
    let provider = LatexCompletions {
        project,
        fixups: Rc::default(),
    };
    cx.update(|cx| {
        for c in [
            "a", "Z", "7", "é", "\\", "{", ",", ":", "-", "_", "*", "/", ".",
        ] {
            assert!(provider.is_completion_trigger(0, c, cx), "{c:?}");
        }
        for s in ["", " ", "}", "(", "\n", "$", "ab", "\\b"] {
            assert!(!provider.is_completion_trigger(0, s, cx), "{s:?}");
        }
    });
}

#[test]
fn symbols_are_found_on_their_own_line() {
    let text = "first line\n\\ref{sec:a} and \\cite{k1}\n\\label{l}\n";
    let rope = Rope::from(text);
    let at = |needle: &str| text.find(needle).unwrap() + 1;

    let sym = symbol_at(&rope, at("sec:a")).unwrap();
    let start = text.find("sec:a").unwrap();
    assert_eq!(
        sym,
        Symbol::Reference {
            name: "sec:a".into(),
            range: start..start + 5
        }
    );
    let start = text.find("k1").unwrap();
    assert_eq!(
        symbol_at(&rope, start).unwrap(),
        Symbol::Citation {
            key: "k1".into(),
            range: start..start + 2
        }
    );
    let start = text.find("{l}").unwrap() + 1;
    assert_eq!(
        symbol_at(&rope, start).unwrap(),
        Symbol::Label {
            name: "l".into(),
            range: start..start + 1
        }
    );
    // Nothing on plain text, nor past the end.
    assert_eq!(symbol_at(&rope, 2), None);
    assert_eq!(symbol_at(&rope, text.len() + 10), None);
}

#[test]
fn every_kind_of_symbol_is_moved_to_its_line() {
    let lines = [
        "\\input{chapters/methods}",
        "\\usepackage{amsmath}",
        "\\begin{equation}",
    ];
    let text = format!(
        "% a comment line first\n{}\n\\section{{x}}\n",
        lines.join("\n")
    );
    let rope = Rope::from(text.as_str());
    for line in lines {
        let start = text.find(line).unwrap();
        // Inside the argument, or on the command name.
        let offset = start + line.find('{').map_or(2, |i| i + 2);
        let sym = symbol_at(&rope, offset).unwrap_or_else(|| panic!("nothing in {line}"));
        let alone = nav::symbol_at(line, offset - start).unwrap();
        let shifted = alone.range().start + start..alone.range().end + start;
        assert_eq!(sym.range(), shifted, "{line}");
        assert_eq!(
            std::mem::discriminant(&sym),
            std::mem::discriminant(&alone),
            "{line}"
        );
    }
    let command = text.find("\\section").unwrap() + 2;
    assert!(matches!(
        symbol_at(&rope, command),
        Some(Symbol::Command { name, .. }) if name == "\\section"
    ));
    // Comments hide symbols.
    assert_eq!(symbol_at(&rope, 5), None);
}

#[test]
fn lsp_ranges_count_characters() {
    let rope = Rope::from("é\nàb");
    let r = lsp_range(&rope, 3..6);
    assert_eq!(r.start, lsp_types::Position::new(1, 0));
    assert_eq!(r.end, lsp_types::Position::new(1, 2));
}

/// Calls of the navigation providers, with the window they need.
struct Nav {
    nav: Rc<LatexNavigation>,
    rope: Rope,
}

impl Nav {
    fn new(project: Arc<RwLock<Project>>, text: &str, notes: Vec<HoverNote>) -> Self {
        let mut nav = LatexNavigation::new(project);
        nav.notes = notes;
        Self {
            nav: Rc::new(nav),
            rope: Rope::from(text),
        }
    }

    fn definitions(&self, cx: &mut VisualTestContext, offset: usize) -> Vec<LocationLink> {
        let task = cx.update(|window, cx| self.nav.definitions(&self.rope, offset, window, cx));
        cx.foreground_executor().block_test(task).unwrap()
    }

    fn hover(&self, cx: &mut VisualTestContext, offset: usize) -> Option<Hover> {
        let task = cx.update(|window, cx| self.nav.hover(&self.rope, offset, window, cx));
        cx.foreground_executor().block_test(task).unwrap()
    }

    /// Follow a link of `definitions`: whether it was handled.
    fn show(&self, cx: &mut VisualTestContext, uri: &str) -> bool {
        let handler = self.nav.show_document();
        let params = ShowDocumentParams {
            uri: uri.parse().unwrap(),
            external: None,
            take_focus: None,
            selection: None,
        };
        let handled = cx.update(|window, cx| handler(&params, window, cx));
        cx.run_until_parked();
        handled
    }
}

/// The actions the navigation dispatched, as the workspace would receive them.
fn record_opened(cx: &mut VisualTestContext) -> Rc<RefCell<Vec<Box<dyn Action>>>> {
    let opened: Rc<RefCell<Vec<Box<dyn Action>>>> = Rc::default();
    cx.update(|_, cx| {
        let o = opened.clone();
        cx.on_action(move |a: &OpenLocation, _| o.borrow_mut().push(a.boxed_clone()));
        let o = opened.clone();
        cx.on_action(move |a: &OpenExternal, _| o.borrow_mut().push(a.boxed_clone()));
    });
    opened
}

fn hover_text(hover: &Hover) -> &str {
    match &hover.contents {
        HoverContents::Markup(m) => {
            assert_eq!(m.kind, MarkupKind::Markdown);
            &m.value
        }
        other => panic!("not markup: {other:?}"),
    }
}

#[gpui::test]
fn goes_to_definitions_through_numbered_links(cx: &mut TestAppContext) {
    let (dir, main, project) = sample();
    let methods = crate::paths::canonical(&dir.path().join("chapters/methods.tex"));
    let text = std::fs::read_to_string(&main).unwrap();
    let cx = cx.add_empty_window();
    let opened = record_opened(cx);
    let nav = Nav::new(project, &text, Vec::new());

    let reference = text.find("sec:methods}").unwrap() + 2;
    let links = nav.definitions(cx, reference);
    assert_eq!(links.len(), 1);
    let link = &links[0];
    assert_eq!(link.target_uri.as_str(), "texoria:definition/0");
    let start = text.find("sec:methods}").unwrap();
    assert_eq!(
        link.origin_selection_range,
        Some(lsp_range(&nav.rope, start..start + "sec:methods".len()))
    );
    assert!(nav.show(cx, link.target_uri.as_str()));
    {
        let opened = opened.borrow();
        let open = opened[0].as_any().downcast_ref::<OpenLocation>().unwrap();
        assert_eq!(open.file, methods);
        let label = std::fs::read_to_string(&methods).unwrap();
        assert_eq!(&label[open.range.clone()], "sec:methods");
    }

    // A picture opens with its app.
    let picture = text.find("\\maketitle").unwrap();
    assert!(
        nav.definitions(cx, picture).is_empty(),
        "no definition there"
    );
    let fig = Nav::new(
        nav.nav.project.clone(),
        "\\includegraphics{figures/example.png}",
        Vec::new(),
    );
    let links = fig.definitions(cx, 20);
    assert_eq!(links.len(), 1);
    assert!(fig.show(cx, links[0].target_uri.as_str()));
    {
        let opened = opened.borrow();
        let open = opened[1].as_any().downcast_ref::<OpenExternal>().unwrap();
        assert!(
            open.path.ends_with("figures/example.png"),
            "{:?}",
            open.path
        );
    }

    // Other links are the editor's; a stale number is handled, and opens nothing.
    assert!(!nav.show(cx, "https://example.org/"));
    assert!(!nav.show(cx, "texoria:definition/x"));
    assert!(nav.show(cx, "texoria:definition/7"));
    assert_eq!(opened.borrow().len(), 2);
}

#[gpui::test]
fn hovers_join_the_notes_and_what_the_symbol_is(cx: &mut TestAppContext) {
    let (_dir, main, project) = sample();
    let text = std::fs::read_to_string(&main).unwrap();
    let cx = cx.add_empty_window();
    let reference = text.find("sec:methods}").unwrap() + 2;

    let nav = Nav::new(project.clone(), &text, Vec::new());
    let hover = nav.hover(cx, reference).expect("a hover on a reference");
    let value = hover_text(&hover);
    assert!(value.contains("**Label**"), "{value}");
    assert!(value.contains("chapters/methods.tex:2"), "{value}");
    let start = text.find("sec:methods}").unwrap();
    assert_eq!(
        hover.range,
        Some(lsp_range(&nav.rope, start..start + "sec:methods".len()))
    );
    // Plain text says nothing.
    let prose = text.find("Hello from").unwrap() + 2;
    assert!(nav.hover(cx, prose).is_none());

    // The notes (problems found there) come first, then the symbol.
    let note: HoverNote = Rc::new(move |offset, _| {
        (offset == reference || offset == prose).then(|| "⚠ a problem".to_string())
    });
    let silent: HoverNote = Rc::new(|_, _| None);
    let nav = Nav::new(project, &text, vec![note, silent]);
    let hover = nav.hover(cx, reference).unwrap();
    let value = hover_text(&hover);
    assert!(
        value.starts_with("⚠ a problem\n\n---\n\n**Label**"),
        "{value}"
    );
    let hover = nav.hover(cx, prose).unwrap();
    assert_eq!(hover_text(&hover), "⚠ a problem");
    assert_eq!(hover.range, None);
}

#[gpui::test]
fn finds_the_package_at_the_cursor(cx: &mut TestAppContext) {
    testing::init(cx);
    let cx = cx.add_empty_window();
    let text = "\\usepackage{amsmath,graphicx}\n\\section{x}";
    let editor =
        cx.update(|window, cx| cx.new(|cx| EditorState::new(window, cx).default_value(text)));
    let at = |cx: &mut VisualTestContext, offset: usize| {
        cx.update(|window, cx| {
            let _ = window;
            editor.update(cx, |e, cx| e.set_selected_range(offset..offset, cx));
            package_at_cursor(&editor, cx)
        })
    };
    assert_eq!(
        at(cx, text.find("graphicx").unwrap() + 1).as_deref(),
        Some("graphicx")
    );
    assert_eq!(
        at(cx, text.find("amsmath").unwrap()).as_deref(),
        Some("amsmath")
    );
    assert_eq!(at(cx, text.find("section").unwrap()), None);
}
