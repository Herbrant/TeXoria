use std::collections::HashSet;
use std::path::PathBuf;

use gpui_kit::{TestAppContext, VisualTestContext};

use super::*;
use crate::testing;
// `super::*` brings gpui's `test` attribute, which would shadow the standard one, and
// gpui-kit's `gpui` alias, which the `#[gpui::test]` macro's paths must not find twice.
use core::prelude::v1::test;
use gpui_kit::gpui;

fn issue(range: std::ops::Range<usize>, level: Level, message: &str) -> Issue {
    Issue {
        file: PathBuf::from("main.tex"),
        range,
        line: 3,
        level,
        code: "undefined-reference",
        message: message.into(),
        key: Some("k".into()),
    }
}

#[test]
fn tells_duplicate_log_warnings() {
    let keys = HashSet::from(["sec:x", "knuth"]);
    let d = |m: &str| Diagnostic {
        severity: Severity::Warning,
        file: None,
        line: None,
        message: m.into(),
    };
    assert!(duplicates(
        &d("Reference `sec:x' on page 1 undefined on input line 5."),
        &keys
    ));
    assert!(duplicates(
        &d("Citation `knuth' on page 2 undefined on input line 9."),
        &keys
    ));
    assert!(!duplicates(
        &d("Reference `other' on page 1 undefined"),
        &keys
    ));
    assert!(!duplicates(&d("There were undefined references."), &keys));
    // Not about a key, or not about something undefined.
    assert!(!duplicates(&d("Reference undefined"), &keys));
    assert!(!duplicates(&d("Reference `sec:x' multiply defined"), &keys));
    assert!(!duplicates(&d("Label `sec:x' undefined"), &keys));
}

#[test]
fn issues_become_problems() {
    let d = diagnostic(&issue(0..1, Level::Error, "Undefined"));
    assert_eq!(d.severity, Severity::Error);
    assert_eq!(d.file, Some(PathBuf::from("main.tex")));
    // Problems count lines from 1.
    assert_eq!(d.line, Some(4));
    assert_eq!(d.message, "Undefined");
    let d = diagnostic(&issue(0..1, Level::Warning, "Flaw"));
    assert_eq!(d.severity, Severity::Warning);
}

const TEXT: &str = "See \\ref{nope} and \\ref{nope}; é \\cite{x}.";

fn editor(cx: &mut VisualTestContext, text: &str) -> Entity<EditorState> {
    let text = text.to_string();
    cx.update(|window, cx| cx.new(|cx| EditorState::new(window, cx).default_value(text)))
}

fn colors() -> (Hsla, Hsla) {
    (gpui_kit::red(), gpui_kit::yellow())
}

#[gpui::test]
fn underlines_only_ranges_of_the_text(cx: &mut TestAppContext) {
    testing::init(cx);
    let cx = cx.add_empty_window();
    let editor = editor(cx, TEXT);
    let first = TEXT.find("nope").unwrap();
    let second = TEXT.rfind("nope").unwrap();
    let e = TEXT.find('é').unwrap();
    let issues = [
        issue(first..first + 4, Level::Error, "No label nope (1)"),
        issue(second..second + 4, Level::Warning, "No label nope (2)"),
        // Empty, inside a character, past the end: skipped.
        issue(3..3, Level::Error, "empty"),
        issue(e + 1..e + 2, Level::Error, "split"),
        issue(TEXT.len() + 5..TEXT.len() + 9, Level::Error, "gone"),
        // Clamped to the end of the text.
        issue(TEXT.len() - 2..TEXT.len() + 50, Level::Warning, "tail"),
    ];
    let refs: Vec<&Issue> = issues.iter().collect();
    let checks = cx.update(|_, cx| {
        let checks = BufferChecks::new(&editor, cx);
        checks.set(&editor, &refs, colors(), cx);
        checks
    });
    let ranges = cx.read(|cx| checks.underlines.get_ranges(cx));
    assert_eq!(
        ranges,
        [
            first..first + 4,
            second..second + 4,
            TEXT.len() - 2..TEXT.len()
        ]
    );
    let notes = checks.notes.borrow();
    let messages: Vec<&str> = notes.iter().map(|n| n.message.as_str()).collect();
    assert_eq!(messages, ["No label nope (1)", "No label nope (2)", "tail"]);
    assert_eq!(notes[2].text, "}.");
    drop(notes);

    let hover = checks.hover_note(&editor);
    let note = |cx: &mut VisualTestContext, offset| cx.read(|cx| hover(offset, cx));
    // Two underlines of the same text: each has its own message.
    assert_eq!(note(cx, first + 1).as_deref(), Some("⚠ No label nope (1)"));
    assert_eq!(note(cx, second + 4).as_deref(), Some("⚠ No label nope (2)"));
    assert_eq!(note(cx, 1), None);

    // Underlines follow edits: the message too, by text and nearest place.
    cx.update(|window, cx| {
        editor.update(cx, |state, cx| {
            state.set_selected_range(0..0, cx);
            state.replace("Also ", window, cx);
        })
    });
    assert_eq!(note(cx, first + 6).as_deref(), Some("⚠ No label nope (1)"));
    assert_eq!(note(cx, second + 6).as_deref(), Some("⚠ No label nope (2)"));
    // Retyped inside: the underline no longer matches any message.
    cx.update(|window, cx| {
        editor.update(cx, |state, cx| {
            state.set_selected_range(first + 6..first + 7, cx);
            state.replace("X", window, cx);
        })
    });
    assert_eq!(note(cx, first + 6), None);

    cx.update(|_, cx| checks.clear(cx));
    assert!(cx.read(|cx| checks.underlines.get_ranges(cx)).is_empty());
    assert!(checks.notes.borrow().is_empty());
    assert_eq!(note(cx, second + 6), None);

    // Once the editor is gone, there is nothing to say.
    cx.update(|_, cx| checks.set(&editor, &refs[..1], colors(), cx));
    drop(editor);
    cx.run_until_parked();
    assert_eq!(note(cx, first + 1), None);
}

#[test]
fn underline_style() {
    let style = underline(gpui_kit::red()).underline.unwrap();
    assert!(style.wavy);
    assert_eq!(style.color, Some(gpui_kit::red()));
}

#[gpui::test]
fn the_workspace_underlines_what_the_checks_find(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let text = "\\documentclass{article}\n\\begin{document}\n\\label{a}\\label{a}\nSee \
                \\ref{missing}.\n\\end{document}\n";
    std::fs::write(dir.path().join("main.tex"), text).unwrap();
    let main = crate::paths::canonical(&dir.path().join("main.tex"));
    let (window, workspace) = testing::open(cx, Some(main.clone()), testing::startup());
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(5));
    cx.run_until_parked();
    let underlined = |cx: &mut TestAppContext| {
        workspace.read_with(cx, |ws, cx| {
            let (editor, _, checks) = ws.buffer_parts(&main).unwrap();
            let text = editor.read(cx).text().to_string();
            checks
                .underlines
                .get_ranges(cx)
                .into_iter()
                .map(|r| text[r].to_string())
                .collect::<Vec<_>>()
        })
    };
    // Found when the project opens, and underlined at once (not only after an edit).
    workspace.read_with(cx, |ws, _| {
        assert!(
            ws.issues()
                .iter()
                .any(|i| i.key.as_deref() == Some("missing"))
        );
    });
    let found = underlined(cx);
    assert!(found.contains(&"missing".to_string()), "{found:?}");
    assert_eq!(found.iter().filter(|t| *t == "a").count(), 2, "{found:?}");

    // Turned off: nothing left.
    cx.update_window(window.into(), |_, window, cx| {
        workspace.update(cx, |ws, cx| {
            ws.update_settings(|s| s.live_checks = false, window, cx)
        })
    })
    .unwrap();
    cx.run_until_parked();
    assert!(underlined(cx).is_empty());
    workspace.read_with(cx, |ws, _| assert!(ws.issues().is_empty()));
}
