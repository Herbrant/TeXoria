//! Tabs, sessions, saving, reloading, opening projects, the toggles and what the editor's
//! commands do to the text.

use super::*;
use core::prelude::v1::test;
use gpui_kit::{Modifiers, VisualTestContext, gpui};

#[gpui::test]
fn tabs_open_activate_and_close(cx: &mut TestAppContext) {
    let settings = tempfile::tempdir().unwrap();
    let app = Open::sample(cx, saved_in(settings.path()));
    let (main, methods, bib) = (
        app.path("main.tex"),
        app.path("chapters/methods.tex"),
        app.path("refs.bib"),
    );
    app.update(cx, |ws, window, cx| {
        ws.activate_file(methods.clone(), window, cx);
        ws.activate_file(bib.clone(), window, cx);
        // Already open: only activated.
        assert_eq!(ws.activate_file(methods.clone(), window, cx), Some(1));
    });
    assert_eq!(
        app.tabs(cx),
        ["main.tex", "chapters/methods.tex", "refs.bib"]
    );
    assert_eq!(app.active(cx), "chapters/methods.tex");
    app.draw(cx);
    // Not a regular file, or not there: an error instead of a tab.
    app.update(cx, |ws, window, cx| {
        assert_eq!(ws.activate_file(app_dir(ws), window, cx), None);
        let missing = ws.project_dir().unwrap().join("missing.tex");
        assert_eq!(ws.activate_file(missing, window, cx), None);
    });
    let CompileState::Failed(e) = app.read(cx, |ws, _| ws.state.clone()) else {
        panic!("no error");
    };
    assert!(e.starts_with("Cannot open "), "{e}");

    // Closing a tab saves it first and activates its neighbor.
    app.type_at(cx, &methods, 0, "% edited\n");
    app.dispatch(cx, crate::CloseTab);
    assert_eq!(app.tabs(cx), ["main.tex", "refs.bib"]);
    assert_eq!(app.active(cx), "refs.bib");
    assert!(
        std::fs::read_to_string(&methods)
            .unwrap()
            .starts_with("% edited\n")
    );
    // A tab's close button closes it even when another one is active, which
    // keeps its tab.
    app.update(cx, |ws, window, cx| {
        ws.activate_file(methods.clone(), window, cx);
        ws.close_buffer(0, cx);
    });
    assert_eq!(app.tabs(cx), ["refs.bib", "chapters/methods.tex"]);
    assert_eq!(app.active(cx), "chapters/methods.tex");
    app.update(cx, |ws, window, cx| {
        ws.activate_file(main.clone(), window, cx);
        ws.close_buffer(0, cx);
        ws.close_buffer(5, cx);
    });
    assert_eq!(app.tabs(cx), ["chapters/methods.tex", "main.tex"]);
    assert_eq!(app.active(cx), "main.tex");
    app.update(cx, |ws, _, cx| ws.close_buffer(0, cx));
    assert_eq!(app.tabs(cx), ["main.tex"]);
    // The last tab stays.
    app.update(cx, |ws, _, cx| ws.close_tab(cx));
    app.update(cx, |ws, _, cx| ws.close_buffer(0, cx));
    assert_eq!(app.tabs(cx), ["main.tex"]);
    app.draw(cx);
    // The session is saved with the settings.
    let saved = Settings::load_from(settings.path().join("settings.toml"));
    let session = saved.session(&main).unwrap();
    assert_eq!(session.files, std::slice::from_ref(&main));
    assert_eq!(session.active.as_ref(), Some(&main));
}

fn app_dir(ws: &Workspace) -> PathBuf {
    ws.project_dir().unwrap()
}

#[gpui::test]
fn a_project_reopens_its_tabs(cx: &mut TestAppContext) {
    let tmp = testing::fixture_copy("sample");
    let dir = paths::canonical(tmp.path());
    let (main, methods, bib) = (
        dir.join("main.tex"),
        dir.join("chapters/methods.tex"),
        dir.join("refs.bib"),
    );
    let outside = tempfile::tempdir().unwrap();
    let theirs = paths::canonical(outside.path()).join("theirs.tex");
    std::fs::write(&theirs, "x").unwrap();
    let startup = || {
        let mut startup = quiet();
        startup.settings.sessions = vec![Session {
            root: main.clone(),
            files: vec![
                methods.clone(),
                theirs.clone(),
                dir.join("gone.tex"),
                dir.join("figures/example.png"),
                bib.clone(),
            ],
            active: Some(methods.clone()),
            export: None,
        }];
        startup
    };
    // The project opened: its tabs, and the one that was active.
    let (_, ws) = testing::open(cx, Some(dir.clone()), startup());
    let tabs = ws.read_with(cx, |ws, _| {
        let paths: Vec<PathBuf> = ws.buffers.iter().map(|b| b.path.clone()).collect();
        (paths, ws.buffers[ws.active].path.clone())
    });
    // The root's tab was closed: it stays closed.
    assert_eq!(tabs.0, [methods.clone(), bib.clone()]);
    assert_eq!(tabs.1, methods);
    // A file of it opened: that file.
    let (_, ws) = testing::open(cx, Some(bib.clone()), startup());
    let active = ws.read_with(cx, |ws, _| ws.buffers[ws.active].path.clone());
    assert_eq!(active, bib);
}

#[gpui::test]
fn opening_projects_and_folders(cx: &mut TestAppContext) {
    let app = Open::empty(cx, quiet());
    app.draw(cx);
    // A folder without a document.
    let empty = tempfile::tempdir().unwrap();
    app.update(cx, |ws, window, cx| {
        ws.open_path(empty.path().to_path_buf(), window, cx)
    });
    let CompileState::Failed(e) = app.read(cx, |ws, _| ws.state.clone()) else {
        panic!("no error");
    };
    assert!(e.starts_with("No LaTeX document "), "{e}");
    assert!(app.read(cx, |ws, _| ws.root.is_none()));

    // Through the Open Folder dialog, a folder with one.
    let tmp = testing::fixture_copy("sample");
    let dir = paths::canonical(tmp.path());
    app.dispatch(cx, crate::OpenFolder);
    assert!(cx.did_prompt_for_paths());
    cx.simulate_path_prompt_response(|_| Some(vec![dir.clone()]));
    cx.run_until_parked();
    assert_eq!(
        app.read(cx, |ws, _| ws.root.clone()),
        Some(dir.join("main.tex"))
    );
    assert_eq!(
        app.read(cx, |ws, _| ws.settings.recent.clone()),
        [dir.join("main.tex")]
    );
    app.draw(cx);

    // A file whose `%!TEX root` points to the main file: the project is the main file's.
    let other = tempfile::tempdir().unwrap();
    let odir = paths::canonical(other.path());
    std::fs::write(
        odir.join("paper.tex"),
        "\\documentclass{article}\n\\begin{document}\n\\input{part}\n\\end{document}\n",
    )
    .unwrap();
    std::fs::write(
        odir.join("part.tex"),
        "%!TEX root = paper.tex\n\\section{Part}\n",
    )
    .unwrap();
    // A build call waiting for the old project is answered.
    let (reply, answer) = mpsc::channel();
    app.update(cx, |ws, _, _| {
        ws.tool_waiters.push(ToolWaiter { reply, after: 0 });
    });
    app.update(cx, |ws, window, cx| {
        ws.open_path(odir.join("part.tex"), window, cx)
    });
    assert_eq!(
        app.read(cx, |ws, _| ws.root.clone()),
        Some(odir.join("paper.tex"))
    );
    let part = odir.join("part.tex").to_string_lossy().replace('\\', "/");
    assert_eq!(app.active(cx), part);
    let r = answer.try_recv().unwrap();
    assert_eq!(
        (r.is_error, r.text.as_str()),
        (true, "Another document was opened.")
    );
    // The welcome screen's recent projects.
    let recent = app.read(cx, |ws, _| ws.settings.recent.clone());
    assert_eq!(recent, [odir.join("paper.tex"), dir.join("main.tex")]);
    // Cancelled: nothing happens.
    app.dispatch(cx, crate::Open);
    cx.simulate_path_prompt_response(|_| None);
    cx.run_until_parked();
    assert_eq!(
        app.read(cx, |ws, _| ws.root.clone()),
        Some(odir.join("paper.tex"))
    );
}

#[gpui::test]
fn saving_writes_the_dirty_buffers(cx: &mut TestAppContext) {
    let app = Open::sample(cx, quiet());
    let (main, methods) = (app.path("main.tex"), app.path("chapters/methods.tex"));
    app.update(cx, |ws, window, cx| {
        ws.activate_file(methods.clone(), window, cx);
    });
    app.type_at(cx, &main, 0, "% one\n");
    app.type_at(cx, &methods, 0, "% two\n");
    app.draw(cx);
    app.dispatch(cx, crate::Save);
    assert!(
        std::fs::read_to_string(&main)
            .unwrap()
            .starts_with("% one\n")
    );
    assert!(
        std::fs::read_to_string(&methods)
            .unwrap()
            .starts_with("% two\n")
    );
    assert!(app.read(cx, |ws, _| ws.buffers.iter().all(|b| !b.dirty)));
    // Typed back to what is saved: not dirty.
    app.type_at(cx, &main, 0, "x");
    assert!(app.read(cx, |ws, _| ws.buffers[0].dirty));
    let editor = app.editor(cx, &main);
    cx.update_window(app.window.into(), |_, window, cx| {
        editor.update(cx, |state, cx| {
            state.set_selected_range(0..1, cx);
            state.replace(String::new(), window, cx);
        })
    })
    .unwrap();
    cx.run_until_parked();
    assert!(!app.read(cx, |ws, _| ws.buffers[0].dirty));

    // A file that cannot be written: the error is shown, the edits are kept, and closing
    // its tab is refused.
    app.type_at(cx, &methods, 0, "% three\n");
    std::fs::remove_dir_all(app.path("chapters")).unwrap();
    assert!(!app.update(cx, |ws, _, cx| ws.save_all(cx)));
    let CompileState::Failed(e) = app.read(cx, |ws, _| ws.state.clone()) else {
        panic!("no error");
    };
    assert!(e.starts_with("Cannot save "), "{e}");
    app.update(cx, |ws, _, cx| ws.close_tab(cx));
    assert_eq!(app.tabs(cx), ["main.tex", "chapters/methods.tex"]);
    assert!(app.text(cx, &methods).starts_with("% three\n"));
}

#[gpui::test]
fn format_on_save_formats_latex_only(cx: &mut TestAppContext) {
    let mut startup = quiet();
    startup.settings.format_on_save = true;
    let app = Open::sample(cx, startup);
    let (main, bib) = (app.path("main.tex"), app.path("refs.bib"));
    let messy = "\\begin{itemize}\n\\item One\n\\end{itemize}\n";
    let at = app.text(cx, &main).find("\\section{Results}").unwrap();
    app.type_at(cx, &main, at, messy);
    app.dispatch(cx, crate::Save);
    let saved = std::fs::read_to_string(&main).unwrap();
    assert!(
        saved.contains("\\begin{itemize}\n  \\item One\n\\end{itemize}"),
        "{saved}"
    );
    assert_eq!(app.text(cx, &main), saved);
    // Formatting again changes nothing.
    app.dispatch(cx, crate::FormatDocument);
    assert_eq!(app.text(cx, &main), saved);
    // Not a LaTeX file: saved as it is.
    app.update(cx, |ws, window, cx| {
        ws.activate_file(bib.clone(), window, cx);
    });
    app.type_at(cx, &bib, 0, "  % note\n");
    app.dispatch(cx, crate::Save);
    assert!(
        std::fs::read_to_string(&bib)
            .unwrap()
            .starts_with("  % note\n")
    );

    // Format Document on its own, in the background.
    app.update(cx, |ws, window, cx| {
        ws.activate_file(main.clone(), window, cx);
    });
    let at = app.text(cx, &main).find("\\section{Results}").unwrap();
    app.type_at(cx, &main, at, messy);
    app.dispatch(cx, crate::FormatDocument);
    assert!(
        app.text(cx, &main)
            .contains("\\begin{itemize}\n  \\item One\n\\end{itemize}\n\\section{Results}")
    );
}

#[gpui::test]
fn files_changed_on_disk_are_reloaded(cx: &mut TestAppContext) {
    let app = Open::sample(cx, quiet());
    let (main, methods, bib) = (
        app.path("main.tex"),
        app.path("chapters/methods.tex"),
        app.path("refs.bib"),
    );
    app.update(cx, |ws, window, cx| {
        ws.activate_file(methods.clone(), window, cx);
        ws.activate_file(bib.clone(), window, cx);
    });
    app.type_at(cx, &bib, 0, "% unsaved\n");
    std::fs::write(
        &main,
        "\\documentclass{article}\n\\begin{document}\n\\section{New}\n\\end{document}\n",
    )
    .unwrap();
    std::fs::write(&bib, "@misc{x}\n").unwrap();
    std::fs::remove_file(&methods).unwrap();
    // As when the window comes back to the front.
    let git = app.read(cx, |ws, _| ws.git.clone());
    app.update(cx, |ws, window, cx| {
        ws.on_git_event(&git, &GitPanelEvent::WorktreeChanged, window, cx)
    });
    cx.run_until_parked();
    // The clean file follows, the deleted one closes, the edited one is left alone.
    assert_eq!(app.tabs(cx), ["main.tex", "refs.bib"]);
    assert!(app.text(cx, &main).contains("\\section{New}"));
    assert!(app.text(cx, &bib).starts_with("% unsaved\n@book"));
    let titles: Vec<String> = app.read(cx, |ws, _| {
        ws.outline.iter().map(|o| o.item.title.clone()).collect()
    });
    assert_eq!(titles, ["New"]);
    assert_eq!(app.active(cx), "refs.bib");
    // The only tab stays even if its file is gone.
    app.update(cx, |ws, _, cx| {
        ws.buffers.truncate(1);
        ws.active = 0;
        cx.notify()
    });
    std::fs::remove_file(&main).unwrap();
    app.update(cx, |ws, window, cx| ws.reload_from_disk(window, cx));
    assert_eq!(app.tabs(cx), ["main.tex"]);
}

#[gpui::test]
fn outline_groups_start_collapsed(cx: &mut TestAppContext) {
    let settings = tempfile::tempdir().unwrap();
    let app = Open::sample(cx, saved_in(settings.path()));
    let main = app.path("main.tex");
    let visible = |cx: &mut TestAppContext| -> Vec<String> {
        app.read(cx, |ws, _| {
            outline::layout(&ws.outline, &ws.outline_fold)
                .into_iter()
                .filter(|r| r.visible)
                .map(|r| ws.outline[r.index].item.title.clone())
                .collect()
        })
    };
    let click = |cx: &mut TestAppContext, selector: &'static str| {
        let mut vcx = VisualTestContext::from_window(app.window.into(), cx);
        let bounds = vcx.debug_bounds(selector).expect(selector);
        vcx.simulate_click(bounds.center(), Modifiers::none());
    };
    app.draw(cx);
    assert_eq!(visible(cx), ["Introduction", "Methods", "Results"]);
    // The arrow opens the group without leaving the cursor's place.
    click(cx, "outline-fold Results");
    assert_eq!(
        visible(cx),
        ["Introduction", "Methods", "Results", "Discussion"]
    );
    assert_eq!(
        app.read(cx, |ws, cx| ws.buffers[ws.active]
            .editor
            .read(cx)
            .cursor_position()
            .line),
        0
    );
    // The title jumps to the heading and leaves the group as it is.
    click(cx, "outline Results");
    assert_eq!(
        visible(cx),
        ["Introduction", "Methods", "Results", "Discussion"]
    );
    let results = app
        .text(cx, &main)
        .lines()
        .position(|l| l == "\\section{Results}");
    assert_eq!(
        app.read(cx, |ws, cx| ws.buffers[ws.active]
            .editor
            .read(cx)
            .cursor_position()
            .line),
        results.unwrap() as u32
    );
    // Re-indexing an edit keeps the open group open, and new groups closed.
    let at = app.text(cx, &main).find("\\section{Results}").unwrap();
    app.type_at(cx, &main, at, "\\subsection{Aside}\n");
    app.update(cx, |ws, _, cx| ws.reindex(cx));
    assert_eq!(
        visible(cx),
        ["Introduction", "Methods", "Results", "Discussion"]
    );
    app.update(cx, |ws, _, _| ws.outline_fold.set_all(true));
    assert_eq!(visible(cx).len(), 6);
    app.draw(cx);
    app.update(cx, |ws, _, _| ws.outline_fold.set_all(false));
    assert_eq!(visible(cx), ["Introduction", "Methods", "Results"]);
}

#[gpui::test]
fn sidebars_and_toggles(cx: &mut TestAppContext) {
    let settings = tempfile::tempdir().unwrap();
    let app = Open::sample(cx, saved_in(settings.path()));
    let sidebar = |cx: &mut TestAppContext| app.read(cx, |ws, _| (ws.show_sidebar, ws.sidebar));
    app.draw(cx);
    app.dispatch(cx, crate::ToggleOutline);
    assert_eq!(sidebar(cx), (false, Sidebar::Outline));
    app.dispatch(cx, crate::ToggleOutline);
    assert_eq!(sidebar(cx), (true, Sidebar::Outline));
    app.dispatch(cx, crate::ToggleFiles);
    assert_eq!(sidebar(cx), (true, Sidebar::Files));
    assert_eq!(app.read(cx, |ws, _| ws.navigator), Sidebar::Files);
    app.draw(cx);
    app.dispatch(cx, crate::ShowSourceControl);
    assert_eq!(sidebar(cx), (true, Sidebar::SourceControl));
    app.draw(cx);
    // Find in Project takes the selection.
    let main = app.path("main.tex");
    let at = app.text(cx, &main).find("Hello").unwrap();
    app.select(cx, &main, at..at + 5);
    app.dispatch(cx, crate::FindInProject);
    assert_eq!(sidebar(cx), (true, Sidebar::Search));
    app.draw(cx);
    app.dispatch(cx, crate::ToggleFiles);
    app.dispatch(cx, crate::ToggleFiles);
    assert_eq!(sidebar(cx), (false, Sidebar::Files));

    // Live preview, soft wrap, diagnostics.
    app.dispatch(cx, crate::ToggleLivePreview);
    assert!(!app.read(cx, |ws, _| ws.settings.live_preview));
    let wrap = app.read(cx, |ws, _| ws.buffers[0].soft_wrap);
    app.dispatch(cx, crate::ToggleSoftWrap);
    assert_eq!(app.read(cx, |ws, _| ws.buffers[0].soft_wrap), !wrap);
    app.update(cx, |ws, window, cx| {
        ws.update_settings(|s| s.soft_wrap = false, window, cx)
    });
    assert!(!app.read(cx, |ws, _| ws.buffers[0].soft_wrap));

    // Font size, within its range.
    let size = |cx: &mut TestAppContext| app.read(cx, |ws, _| ws.settings.font_size());
    app.dispatch(cx, crate::IncreaseFontSize);
    assert_eq!(size(cx), Settings::DEFAULT_FONT_SIZE + 1.);
    app.dispatch(cx, crate::DecreaseFontSize);
    app.dispatch(cx, crate::DecreaseFontSize);
    assert_eq!(size(cx), Settings::DEFAULT_FONT_SIZE - 1.);
    app.update(cx, |ws, window, cx| ws.set_font_size(1000., window, cx));
    assert_eq!(size(cx), Settings::MAX_FONT_SIZE);
    app.dispatch(cx, crate::ResetFontSize);
    assert_eq!(size(cx), Settings::DEFAULT_FONT_SIZE);

    // The theme, light then dark.
    let dark = cx.update(|cx| cx.theme_is_dark());
    app.dispatch(cx, crate::ToggleTheme);
    assert_eq!(cx.update(|cx| cx.theme_is_dark()), !dark);
    assert_eq!(app.read(cx, |ws, _| ws.was_dark), !dark);
    app.draw(cx);
    app.dispatch(cx, crate::ToggleTheme);
    assert_eq!(cx.update(|cx| cx.theme_is_dark()), dark);

    // Vim mode.
    app.dispatch(cx, crate::ToggleVimMode);
    assert!(app.read(cx, |ws, _| ws.vim.is_some()));
    app.draw(cx);
    app.dispatch(cx, crate::ToggleVimMode);
    assert!(app.read(cx, |ws, _| ws.vim.is_none()));

    let saved = Settings::load_from(settings.path().join("settings.toml"));
    assert!(!saved.live_preview && !saved.vim_mode && !saved.soft_wrap);
    assert_eq!(saved.editor_font_size, Settings::DEFAULT_FONT_SIZE);

    // The settings window.
    app.dispatch(cx, crate::OpenSettings);
    let opened = app.read(cx, |ws, _| ws.settings_window);
    assert!(opened.is_some());
    // Asked again: the same window comes to the front.
    app.dispatch(cx, crate::OpenSettings);
    assert_eq!(app.read(cx, |ws, _| ws.settings_window), opened);
}

#[gpui::test]
fn closing_the_window_saves_and_closes_the_others(cx: &mut TestAppContext) {
    let app = Open::sample(cx, quiet());
    let main = app.path("main.tex");
    app.type_at(cx, &main, 0, "% unsaved\n");
    app.dispatch(cx, crate::OpenSettings);
    assert_eq!(cx.update(|cx| cx.windows().len()), 2);

    let mut vcx = VisualTestContext::from_window(app.window.into(), cx);
    assert!(vcx.simulate_close());
    vcx.run_until_parked();
    assert!(
        std::fs::read_to_string(&main)
            .unwrap()
            .starts_with("% unsaved")
    );
    // Only the main window is left, which the platform now closes; then nothing holds the
    // workspace (its compiler, agents and servers), and a new window can take its place.
    let windows = cx.update(|cx| cx.windows());
    assert_eq!(windows, [AnyWindowHandle::from(app.window)]);
    let Open {
        _tmp, window, ws, ..
    } = app;
    let weak = ws.downgrade();
    drop(ws);
    cx.update_window(window.into(), |_, window, _| window.remove_window())
        .unwrap();
    cx.run_until_parked();
    assert!(weak.upgrade().is_none());
}

#[gpui::test]
fn vim_keys_edit_the_active_editor(cx: &mut TestAppContext) {
    let mut startup = quiet();
    startup.settings.vim_mode = true;
    let app = Open::sample(cx, startup);
    let main = app.path("main.tex");
    app.draw(cx);
    let first = app.text(cx, &main).lines().next().unwrap().to_string();
    // `dd` deletes the first line, `u` brings it back.
    cx.simulate_keystrokes(app.window.into(), "d d");
    cx.run_until_parked();
    assert!(!app.text(cx, &main).starts_with(&first));
    cx.simulate_keystrokes(app.window.into(), "u");
    cx.run_until_parked();
    assert!(app.text(cx, &main).starts_with(&first));
    // Insert mode types text; escape goes back to normal mode.
    cx.simulate_keystrokes(app.window.into(), "i");
    cx.simulate_input(app.window.into(), "% vim\n");
    cx.simulate_keystrokes(app.window.into(), "escape");
    cx.run_until_parked();
    assert!(
        app.text(cx, &main).contains("\n% vim\n"),
        "{}",
        app.text(cx, &main)
    );
    app.draw(cx);
    // `:w` saves.
    cx.simulate_keystrokes(app.window.into(), ": w enter");
    cx.run_until_parked();
    assert!(
        std::fs::read_to_string(&main)
            .unwrap()
            .contains("\n% vim\n")
    );
}

#[gpui::test]
fn editor_commands_change_the_text(cx: &mut TestAppContext) {
    let settings = tempfile::tempdir().unwrap();
    let app = Open::sample(cx, saved_in(settings.path()));
    let main = app.path("main.tex");
    app.draw(cx);
    // ⌘/ comments the line out and back in.
    app.select(cx, &main, 0..0);
    app.dispatch(cx, crate::ToggleComment);
    assert!(app.text(cx, &main).starts_with("% \\documentclass"));
    app.dispatch(cx, crate::ToggleComment);
    assert!(app.text(cx, &main).starts_with("\\documentclass"));

    // A spelling correction, and one past the end that no longer applies.
    let at = app.text(cx, &main).find("Hello").unwrap();
    app.dispatch(
        cx,
        ReplaceMisspelling {
            range: at..at + 5,
            text: "Greetings".into(),
        },
    );
    assert!(app.text(cx, &main).contains("Greetings from TeXoria"));
    let len = app.text(cx, &main).len();
    app.dispatch(
        cx,
        ReplaceMisspelling {
            range: len..len + 3,
            text: "x".into(),
        },
    );
    assert_eq!(app.text(cx, &main).len(), len);
    app.dispatch(
        cx,
        AddToDictionary {
            word: "TeXoria".into(),
        },
    );
    app.dispatch(
        cx,
        IgnoreGrammarRule {
            rule: "WHITESPACE".into(),
        },
    );
    app.dispatch(
        cx,
        IgnoreGrammarRule {
            rule: "WHITESPACE".into(),
        },
    );
    let saved = Settings::load_from(settings.path().join("settings.toml"));
    assert_eq!(saved.spell_words, ["TeXoria"]);
    assert_eq!(saved.grammar_rules_off, ["WHITESPACE"]);
    app.update(cx, |ws, window, cx| {
        ws.set_spell_language(Some("fr".into()), window, cx)
    });
    assert_eq!(
        app.read(cx, |ws, _| ws
            .settings
            .spell_language(&main)
            .map(str::to_string)),
        Some("fr".into())
    );

    // A definition elsewhere opens there, its range selected.
    let methods = app.path("chapters/methods.tex");
    app.dispatch(
        cx,
        OpenLocation {
            file: methods.clone(),
            range: 0..8,
        },
    );
    assert_eq!(app.active(cx), "chapters/methods.tex");
    let selected = app
        .editor(cx, &methods)
        .read_with(cx, |e, _| e.selected_range());
    assert_eq!(selected, 0..8);
    // Inverse search from the PDF lands on the line.
    let pdf = app.read(cx, |ws, _| ws.pdf.clone());
    pdf.update(cx, |_, cx| {
        cx.emit(PdfPaneEvent::InverseSearch {
            file: main.clone(),
            line: 15,
        })
    });
    cx.run_until_parked();
    assert_eq!(app.active(cx), "main.tex");
    let line = app
        .editor(cx, &main)
        .read_with(cx, |e, _| e.cursor_position().line);
    assert_eq!(line, 14);
    // Forward search and the presentation need a PDF: nothing happens without one.
    app.dispatch(cx, crate::ForwardSearch);
    app.dispatch(cx, crate::Present);
    pdf.update(cx, |_, cx| cx.emit(PdfPaneEvent::Export));
    cx.run_until_parked();
    assert_eq!(app.notifications(cx), 1);
    // A package name that could be an option is not looked up.
    app.dispatch(cx, PackageDocs { name: "-x".into() });
}

#[gpui::test]
fn a_completion_lands_with_the_cursor_inside(cx: &mut TestAppContext) {
    let app = Open::sample(cx, quiet());
    let main = app.path("main.tex");
    let fixups = app.read(cx, |ws, _| ws.fixups.clone());
    fixups.borrow_mut().push(CursorFixup {
        insert: "\\textbf{}".into(),
        back: 1,
    });
    // Typing one character is not a completion.
    app.type_at(cx, &main, 0, "x");
    assert_eq!(fixups.borrow().len(), 1);
    // The completion's text lands: the cursor goes between the braces.
    app.type_at(cx, &main, 1, "\\textbf{}");
    assert!(fixups.borrow().is_empty());
    let cursor = app.editor(cx, &main).read_with(cx, |e, _| e.cursor());
    assert_eq!(cursor, 1 + "\\textbf{".len());
    // After a multi-byte character: no slicing inside it.
    fixups.borrow_mut().push(CursorFixup {
        insert: "\\é{}".into(),
        back: 1,
    });
    app.type_at(cx, &main, 0, "ab");
    assert_eq!(fixups.borrow().len(), 1);
}

#[gpui::test]
fn citations_are_added_to_the_bibliography(cx: &mut TestAppContext) {
    let app = Open::sample(cx, quiet());
    let (main, bib) = (app.path("main.tex"), app.path("refs.bib"));
    let bibtex = "@article{x, author = {Alan Turing}, title = {Computing Machinery and \
                  Intelligence}, journal = {Mind}, year = {1950}}";
    let at = app.text(cx, &main).find("Hello").unwrap();
    app.select(cx, &main, at..at);
    app.update(cx, |ws, window, cx| ws.add_citation(bibtex, window, cx));
    cx.run_until_parked();
    let keys = |text: &str| -> Vec<String> {
        (text.lines())
            .filter_map(|l| l.strip_prefix("@article{")?.strip_suffix(','))
            .map(str::to_string)
            .collect()
    };
    let first = keys(&std::fs::read_to_string(&bib).unwrap());
    assert_eq!(first.len(), 2, "{first:?}");
    let key = first[1].clone();
    assert!(key.starts_with("turing1950"), "{key}");
    assert!(
        app.text(cx, &main)
            .contains(&format!("\\cite{{{key}}}Hello"))
    );
    // Again, inside the \cite: one more key; the open .bib is edited in its tab.
    app.update(cx, |ws, window, cx| {
        ws.activate_file(bib.clone(), window, cx);
        ws.activate_file(main.clone(), window, cx);
    });
    let at = app.text(cx, &main).find(&format!("{key}}}")).unwrap() + key.len();
    app.select(cx, &main, at..at);
    app.update(cx, |ws, window, cx| ws.add_citation(bibtex, window, cx));
    cx.run_until_parked();
    let second = keys(&app.text(cx, &bib));
    assert_eq!(second.len(), 3, "{second:?}");
    assert_ne!(second[2], key);
    let cite = format!("\\cite{{{key},{}}}", second[2]);
    assert!(app.text(cx, &main).contains(&cite), "{cite}");
    // Not BibTeX.
    let notes = app.notifications(cx);
    app.update(cx, |ws, window, cx| ws.add_citation("nonsense", window, cx));
    assert_eq!(app.notifications(cx), notes + 1);
    // A search's results to choose from; none at all.
    app.update(cx, |ws, window, cx| {
        let paper = tex_cite::Candidate {
            title: "On Computable Numbers".into(),
            authors: vec!["A. Turing".into(), "B".into(), "C".into(), "D".into()],
            year: Some("1936".into()),
            venue: Some("LMS".into()),
            doi: "10.1112/plms/s2-42.1.230".into(),
        };
        ws.choose_citation(vec![paper], window, cx)
    });
    assert!(app.has_dialog(cx));
    app.draw(cx);
    app.update(cx, |_, window, cx| window.close_dialog(cx));
    app.update(cx, |ws, window, cx| {
        ws.choose_citation(Vec::new(), window, cx)
    });
    assert!(!app.has_dialog(cx));
    app.dispatch(cx, crate::AddCitation);
    assert!(app.has_dialog(cx));
}

#[gpui::test]
fn the_word_count_and_the_status_bar(cx: &mut TestAppContext) {
    let app = Open::sample(cx, quiet());
    let main = app.path("main.tex");
    app.update(cx, |ws, window, cx| {
        ws.activate_file(app.path("chapters/methods.tex"), window, cx);
        ws.activate_file(main.clone(), window, cx);
    });
    cx.run_until_parked();
    assert!(app.read(cx, |ws, _| ws.word_counts.len() >= 2));
    let at = app.text(cx, &main).find("Hello").unwrap();
    app.select(cx, &main, at..at + 15);
    app.draw(cx);
    app.update(cx, |ws, window, cx| ws.show_word_count(window, cx));
    assert!(app.has_dialog(cx));
    app.draw(cx);
}

#[gpui::test]
fn trusting_a_repository_lets_git_run(cx: &mut TestAppContext) {
    let Some(s) = testing::GitSandbox::new() else {
        return;
    };
    let dir = s.dir.join("paper");
    copy(&test_support::fixtures().join("sample"), &dir);
    s.run(&dir, &["init", "--quiet"]);
    let settings = tempfile::tempdir().unwrap();
    let (window, ws) = testing::open(cx, Some(dir.join("main.tex")), saved_in(settings.path()));
    let app = Open {
        _tmp: None,
        dir: dir.clone(),
        window,
        ws,
    };
    let git = app.read(cx, |ws, _| ws.git.clone());
    assert!(git.read_with(cx, |g, _| g.repo().is_none()));
    app.update(cx, |ws, window, cx| {
        ws.on_git_event(&git, &GitPanelEvent::Trust, window, cx)
    });
    assert!(app.read(cx, |ws, _| ws.settings.is_trusted(&dir)));
    let saved = Settings::load_from(settings.path().join("settings.toml"));
    assert_eq!(saved.trusted_projects, std::slice::from_ref(&dir));
    // Taken back from the settings window.
    app.update(cx, |ws, window, cx| {
        ws.update_settings(|s| s.trusted_projects.clear(), window, cx)
    });
    assert!(!app.read(cx, |ws, _| ws.settings.is_trusted(&dir)));
}

fn copy(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            copy(&path, &to.join(entry.file_name()));
        } else {
            std::fs::copy(&path, to.join(entry.file_name())).unwrap();
        }
    }
}

#[gpui::test]
fn settings_changes_reach_what_is_running(cx: &mut TestAppContext) {
    let app = Open::sample(cx, quiet());
    app.update(cx, |ws, window, cx| {
        ws.update_settings(
            |s| {
                s.pull_rebase = true;
                s.diff_split = true;
                s.live_checks = false;
                s.grammar_check = true;
                // Not a URL: never contacted.
                s.languagetool_url = "not a url".into();
                s.languagetool_start = false;
                s.spell_check = true;
                s.spell_words = vec!["TeXoria".into()];
                s.custom_agents = vec![tex_agent::CustomAgent {
                    name: "Mine".into(),
                    command: "/nonexistent/agent".into(),
                    args: Vec::new(),
                }];
            },
            window,
            cx,
        )
    });
    cx.run_until_parked();
    assert!(app.read(cx, |ws, _| ws.issues.is_empty()));
    assert!(matches!(
        app.read(cx, |ws, _| ws.grammar.state.clone()),
        ServerState::Unreachable(_)
    ));
    app.draw(cx);
    app.update(cx, |ws, window, cx| {
        ws.update_settings(
            |s| {
                s.grammar_rules_off = vec!["X".into()];
                s.live_checks = true;
                s.grammar_check = false;
            },
            window,
            cx,
        )
    });
    cx.run_until_parked();
    app.draw(cx);
    // The updater's news.
    let updater = app.read(cx, |ws, _| ws.updater.clone());
    let notes = app.notifications(cx);
    updater.update(cx, |_, cx| {
        cx.emit(UpdaterEvent::Notify {
            error: false,
            title: "Update".into(),
            message: "TeXoria 9.9.9 is out".into(),
            version: None,
        });
        cx.emit(UpdaterEvent::Notify {
            error: true,
            title: "Update failed".into(),
            message: "offline".into(),
            version: None,
        });
        cx.emit(UpdaterEvent::UpToDate);
    });
    cx.run_until_parked();
    assert_eq!(app.notifications(cx), notes + 2);
    assert!(app.read(cx, |ws, _| ws.settings.last_update_check.is_some()));
    app.draw(cx);
}
