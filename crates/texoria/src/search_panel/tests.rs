use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::{TestAppContext, WindowHandle, base::Root};

use super::*;
use crate::testing;
use crate::workspace::Workspace;
// `super::*` brings gpui's `test` attribute, which would shadow the standard one, and
// gpui-kit's `gpui` alias, which the `#[gpui::test]` macro's paths must not find twice.
use core::prelude::v1::test;
use gpui_kit::gpui;

struct Open {
    dir: tempfile::TempDir,
    window: WindowHandle<Root>,
    workspace: Entity<Workspace>,
    search: Entity<SearchPanel>,
    /// What the panel told the workspace, but searches.
    events: Rc<RefCell<Vec<String>>>,
}

impl Open {
    fn panel<R>(
        &self,
        cx: &mut TestAppContext,
        f: impl FnOnce(&mut SearchPanel, &mut Window, &mut Context<SearchPanel>) -> R,
    ) -> R {
        let search = self.search.clone();
        let r = cx
            .update_window(self.window.into(), |_, window, cx| {
                search.update(cx, |s, cx| f(s, window, cx))
            })
            .unwrap();
        cx.run_until_parked();
        r
    }

    /// Search for `text`, as typed in the field, and wait for the results.
    fn find(&self, cx: &mut TestAppContext, text: &str) {
        let text = text.to_string();
        self.panel(cx, |s, window, cx| s.focus(Some(text), window, cx));
        settle(cx);
    }

    /// The files found, relative to the project, with their number of matches.
    fn found(&self, cx: &mut TestAppContext) -> Vec<(String, usize)> {
        self.search.read_with(cx, |s, _| {
            s.results
                .files
                .iter()
                .map(|f| {
                    let rel = s.relative(&f.path).to_string_lossy().replace('\\', "/");
                    (rel, f.matches.len())
                })
                .collect()
        })
    }
}

fn settle(cx: &mut TestAppContext) {
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_secs(1));
    cx.run_until_parked();
}

fn open(cx: &mut TestAppContext) -> Open {
    let dir = testing::fixture_copy("sample");
    // Edits stay in the editor.
    let mut startup = testing::startup();
    startup.settings.live_preview = false;
    let (window, workspace) = testing::open(cx, Some(dir.path().join("main.tex")), startup);
    let search = workspace.read_with(cx, |ws, _| ws.search_panel().clone());
    let events: Rc<RefCell<Vec<String>>> = Rc::default();
    let log = events.clone();
    cx.update(|cx| {
        cx.subscribe(&search, move |_, ev: &SearchPanelEvent, _| match ev {
            SearchPanelEvent::Search => {}
            SearchPanelEvent::Open { path, range } => {
                let name = path.file_name().unwrap().to_string_lossy();
                log.borrow_mut().push(format!("open {name} {range:?}"));
            }
            SearchPanelEvent::ReplaceAll {
                query,
                replacement,
                files,
                truncated,
            } => log.borrow_mut().push(format!(
                "replace {} with {replacement} in {} files{}",
                query.text,
                files.len(),
                if *truncated { ", truncated" } else { "" }
            )),
        })
        .detach();
    });
    Open {
        dir,
        window,
        workspace,
        search,
        events,
    }
}

#[gpui::test]
fn searches_the_project_from_the_workspace(cx: &mut TestAppContext) {
    let open = open(cx);
    cx.dispatch_action(open.window.into(), crate::FindInProject);
    open.find(cx, "sec:");
    let found = open.found(cx);
    assert_eq!(
        found,
        [("chapters/methods.tex".into(), 1), ("main.tex".into(), 4)],
        "{found:?}"
    );
    open.search.read_with(cx, |s, _| {
        assert!(!s.searching && s.error.is_none());
        // A row per file, then its matches.
        assert_eq!(s.rows.len(), 7);
        let m = &s.results.files[0].matches[0];
        assert_eq!(&m.preview[m.highlight.clone()], "sec:");
    });

    // Hiding a file's matches, and showing them again.
    let methods = open
        .search
        .read_with(cx, |s, _| s.results.files[0].path.clone());
    open.panel(cx, |s, _, cx| s.toggle_file(&methods, cx));
    open.search
        .read_with(cx, |s, _| assert_eq!(s.rows.len(), 6));
    open.panel(cx, |s, _, cx| s.toggle_file(&methods, cx));
    open.search
        .read_with(cx, |s, _| assert_eq!(s.rows.len(), 7));

    // Nothing found.
    open.find(cx, "no such text anywhere");
    assert!(open.found(cx).is_empty());
}

#[gpui::test]
fn opens_a_match_in_the_editor(cx: &mut TestAppContext) {
    let open = open(cx);
    open.find(cx, "We work in");
    let found = open.found(cx);
    assert_eq!(found, [("chapters/methods.tex".into(), 1)]);
    open.panel(cx, |s, _, cx| {
        s.open_match(0, 0, cx);
        // Gone since: nothing happens.
        s.open_match(0, 9, cx);
        s.open_match(5, 0, cx);
    });
    let start = std::fs::read_to_string(open.dir.path().join("chapters/methods.tex"))
        .unwrap()
        .find("We work in")
        .unwrap();
    assert_eq!(
        *open.events.borrow(),
        [format!("open methods.tex {:?}", start..start + 10)]
    );
    let methods = crate::paths::canonical(&open.dir.path().join("chapters/methods.tex"));
    open.workspace.read_with(cx, |ws, cx| {
        let (editor, _, _) = ws.buffer_parts(&methods).expect("opened");
        let state = editor.read(cx);
        assert_eq!(state.selected_range(), start..start + 10);
    });
}

#[gpui::test]
fn searches_the_unsaved_text_of_open_files(cx: &mut TestAppContext) {
    let open = open(cx);
    let main = crate::paths::canonical(&open.dir.path().join("main.tex"));
    let editor = open
        .workspace
        .read_with(cx, |ws, _| ws.buffer_parts(&main).unwrap().0.clone());
    cx.update_window(open.window.into(), |_, window, cx| {
        editor.update(cx, |state, cx| {
            state.set_selected_range(0..0, cx);
            state.replace("% Zanzibar\n", window, cx);
        })
    })
    .unwrap();
    open.find(cx, "Zanzibar");
    assert_eq!(open.found(cx), [("main.tex".into(), 1)]);
    assert!(!std::fs::read_to_string(&main).unwrap().contains("Zanzibar"));
}

#[gpui::test]
fn options_change_the_query(cx: &mut TestAppContext) {
    let open = open(cx);
    open.find(cx, "introduction");
    assert_eq!(open.found(cx), [("main.tex".into(), 1)]);
    open.panel(cx, |s, _, cx| s.toggle(|t| &mut t.case_sensitive, cx));
    settle(cx);
    assert!(open.found(cx).is_empty());
    open.panel(cx, |s, _, cx| s.toggle(|t| &mut t.case_sensitive, cx));
    settle(cx);

    open.panel(cx, |s, _, cx| s.toggle(|t| &mut t.whole_word, cx));
    open.find(cx, "Introduc");
    assert!(open.found(cx).is_empty(), "{:?}", open.found(cx));
    open.panel(cx, |s, _, cx| s.toggle(|t| &mut t.whole_word, cx));
    settle(cx);
    assert_eq!(open.found(cx).len(), 1);

    open.panel(cx, |s, _, cx| s.toggle(|t| &mut t.regex, cx));
    open.find(cx, r"sec:(intro|results)\}");
    assert_eq!(open.found(cx), [("main.tex".into(), 3)]);
    let query = open.search.read_with(cx, |s, cx| s.query(cx)).unwrap();
    assert!(query.regex && !query.case_sensitive && !query.whole_word);

    // A broken expression is an error, not a search.
    open.find(cx, "sec:(");
    open.search.read_with(cx, |s, _| {
        assert!(s.error.is_some());
        assert!(s.results.files.is_empty() && s.rows.is_empty() && !s.searching);
    });
    open.find(cx, "sec:");
    open.search.read_with(cx, |s, _| assert!(s.error.is_none()));
}

/// A search still running when the field is cleared (or its expression broken) must not bring
/// its results back once it ends.
#[gpui::test]
fn a_cleared_search_stays_empty(cx: &mut TestAppContext) {
    let open = open(cx);
    open.find(cx, "sec:");
    assert!(!open.found(cx).is_empty());
    for cleared in ["", "("] {
        open.panel(cx, |s, window, cx| {
            s.focus(Some("Hello".into()), window, cx)
        });
        let regex = cleared == "(";
        let search = open.search.clone();
        cx.update_window(open.window.into(), |_, window, cx| {
            search.update(cx, |s, cx| {
                s.regex = regex;
                // Started, and replaced before it could end, but after it went through the
                // files: the field is cleared while the search finishes.
                s.run(HashMap::new(), cx);
                let running = s.cancel.clone();
                s.query.update(cx, |q, cx| q.set_value(cleared, window, cx));
                s.run(HashMap::new(), cx);
                running.store(false, Ordering::Relaxed);
            })
        })
        .unwrap();
        cx.run_until_parked();
        assert!(
            open.found(cx).is_empty(),
            "after {cleared:?}: {:?}",
            open.found(cx)
        );
        open.search.read_with(cx, |s, _| assert!(s.rows.is_empty()));
    }
}

#[gpui::test]
fn replace_all_asks_the_workspace(cx: &mut TestAppContext) {
    let open = open(cx);
    // Nothing to replace yet.
    open.panel(cx, |s, _, cx| s.replace_all(cx));
    open.find(cx, "nothing like this");
    open.panel(cx, |s, _, cx| s.replace_all(cx));
    assert!(open.events.borrow().is_empty());

    open.find(cx, "sec:");
    open.panel(cx, |s, window, cx| {
        s.show_replace = true;
        s.replacement
            .update(cx, |r, cx| r.set_value("s:", window, cx));
        s.replace_all(cx);
    });
    assert_eq!(*open.events.borrow(), ["replace sec: with s: in 2 files"]);
    open.panel(cx, |s, _, cx| {
        s.results.truncated = true;
        s.replace_all(cx);
    });
    assert_eq!(
        open.events.borrow().last().unwrap(),
        "replace sec: with s: in 2 files, truncated"
    );
    // The workspace asks first: nothing changed yet.
    let methods = std::fs::read_to_string(open.dir.path().join("chapters/methods.tex")).unwrap();
    assert!(methods.contains("sec:methods"));
}

#[gpui::test]
fn follows_the_project_folder(cx: &mut TestAppContext) {
    let open = open(cx);
    open.find(cx, "sec:");
    let dir = open.search.read_with(cx, |s, _| s.dir.clone());
    assert!(dir.is_some());
    // The same folder keeps the results.
    open.panel(cx, |s, _, cx| s.set_dir(dir.clone(), cx));
    assert_eq!(open.found(cx).len(), 2);
    // No project: nothing to search.
    open.panel(cx, |s, _, cx| s.set_dir(None, cx));
    settle(cx);
    assert!(open.found(cx).is_empty());
    open.search.read_with(cx, |s, cx| {
        assert!(!s.searching);
        // Outside the folder, paths stay whole.
        let elsewhere = Path::new("/elsewhere/a.tex");
        assert_eq!(s.relative(elsewhere), elsewhere);
        assert!(s.query(cx).is_some());
    });
    open.panel(cx, |s, _, cx| s.set_dir(dir, cx));
    settle(cx);
    assert_eq!(open.found(cx).len(), 2);
}

#[gpui::test]
fn typing_searches_after_a_pause(cx: &mut TestAppContext) {
    let open = open(cx);
    cx.dispatch_action(open.window.into(), crate::FindInProject);
    settle(cx);
    cx.simulate_input(open.window.into(), "Lovelace");
    cx.run_until_parked();
    // Not before the pause.
    assert!(open.found(cx).is_empty());
    cx.executor().advance_clock(DELAY);
    cx.run_until_parked();
    assert_eq!(open.found(cx), [("main.tex".into(), 1)]);
}
