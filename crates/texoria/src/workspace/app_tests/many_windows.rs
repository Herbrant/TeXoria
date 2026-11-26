//! More than one window: their shared settings, a project in one window only, and the
//! documents opened from outside.

use super::*;
use crate::windows;
use core::prelude::v1::test;
use gpui_kit::{AnyWindowHandle, VisualTestContext, gpui};

/// The workspace whose project's root file is `root` (one the app opened, not the test).
fn workspace_showing(cx: &mut TestAppContext, root: &Path) -> Option<Entity<Workspace>> {
    cx.update(|cx| {
        windows::workspaces(cx)
            .into_iter()
            .map(|(_, w)| w)
            .find(|w| w.read(cx).root() == Some(root))
    })
}

#[gpui::test]
fn the_windows_share_their_settings(cx: &mut TestAppContext) {
    let settings = tempfile::tempdir().unwrap();
    let a = Open::sample(cx, saved_in(settings.path()));
    let b = Open::empty(cx, saved_in(settings.path()));

    let was_live = a.read(cx, |ws, _| ws.settings.live_preview);
    a.dispatch(cx, crate::ToggleLivePreview);
    let live = a.read(cx, |ws, _| ws.settings.live_preview);
    assert_ne!(live, was_live);
    assert_eq!(b.read(cx, |ws, _| ws.settings.live_preview), live);
    // A change in the other window keeps this one's, and takes effect in both.
    b.dispatch(cx, crate::ToggleVimMode);
    assert!(a.read(cx, |ws, _| ws.settings.vim_mode && ws.vim.is_some()));
    // So does opening a project (the recent ones).
    let other = testing::fixture_copy("sample");
    let other_main = paths::canonical(other.path()).join("main.tex");
    b.update(cx, |ws, window, cx| {
        ws.open_path(other_main.clone(), window, cx)
    });
    cx.run_until_parked();
    assert!(a.read(cx, |ws, _| ws.settings.recent.contains(&other_main)));

    let saved = Settings::load_from(settings.path().join("settings.toml"));
    assert_eq!(saved.live_preview, live);
    assert!(saved.vim_mode);
    assert!(saved.recent.contains(&a.path("main.tex")));
    assert!(saved.recent.contains(&other_main));
}

#[gpui::test]
fn a_project_open_in_another_window_comes_forward(cx: &mut TestAppContext) {
    let a = Open::sample(cx, quiet());
    let b = Open::empty(cx, quiet());
    let methods = a.path("chapters/methods.tex");
    b.update(cx, |ws, window, cx| {
        ws.open_path(methods.clone(), window, cx)
    });
    cx.run_until_parked();
    // Never the same project in two windows: it would build twice in one folder.
    assert!(b.read(cx, |ws, _| ws.root.is_none()));
    assert_eq!(a.active(cx), "chapters/methods.tex");
}

#[gpui::test]
fn documents_from_outside_open_where_they_belong(cx: &mut TestAppContext) {
    let a = Open::sample(cx, quiet());
    cx.update(|cx| windows::set_startup(quiet, cx));
    let open = |cx: &mut TestAppContext, path: PathBuf| {
        cx.update(|cx| windows::open_document(path, cx));
        cx.run_until_parked();
    };

    // A file of an open project: in its window.
    open(cx, a.path("chapters/methods.tex"));
    assert_eq!(a.active(cx), "chapters/methods.tex");
    assert_eq!(cx.update(|cx| cx.windows().len()), 1);

    // Another project: in a window still on the start page.
    let b = Open::empty(cx, quiet());
    let second = testing::fixture_copy("sample");
    let second_main = paths::canonical(second.path()).join("main.tex");
    open(cx, second_main.clone());
    assert_eq!(b.read(cx, |ws, _| ws.root.clone()), Some(second_main));
    assert_eq!(
        a.read(cx, |ws, _| ws.root.clone()),
        Some(a.path("main.tex"))
    );

    // No window free: a new one.
    let third = testing::fixture_copy("sample");
    let third_main = paths::canonical(third.path()).join("main.tex");
    open(cx, third_main.clone());
    assert_eq!(cx.update(|cx| cx.windows().len()), 3);
    assert!(workspace_showing(cx, &third_main).is_some());
}

#[gpui::test]
fn new_window_opens_the_start_page(cx: &mut TestAppContext) {
    let a = Open::sample(cx, quiet());
    cx.update(|cx| windows::set_startup(quiet, cx));
    a.dispatch(cx, crate::NewWindow);
    let all = cx.update(|cx| windows::workspaces(cx));
    assert_eq!(all.len(), 2);
    assert_eq!(cx.update(|cx| cx.windows().len()), 2);
    let (window, new) = all
        .iter()
        .find(|(_, w)| *w != a.ws)
        .expect("a new workspace");
    assert!(new.read_with(cx, |ws, _| ws.root.is_none()));
    // Its shortcuts work at once: the start page has the focus.
    let focused = cx
        .update_window(*window, |_, window, cx| {
            new.read(cx).focus.is_focused(window)
        })
        .unwrap();
    assert!(focused);
}

#[gpui::test]
fn closing_a_window_keeps_the_others(cx: &mut TestAppContext) {
    let a = Open::sample(cx, quiet());
    let b = Open::empty(cx, quiet());
    a.dispatch(cx, crate::OpenSettings);
    assert_eq!(cx.update(|cx| cx.windows().len()), 3);

    let mut vcx = VisualTestContext::from_window(a.window.into(), cx);
    assert!(vcx.simulate_close());
    vcx.run_until_parked();
    // Its Settings window went with it, not the other project's window (the platform
    // removes the closed one itself).
    let mut windows = cx.update(|cx| cx.windows());
    windows.sort_by_key(|w| w.window_id());
    let mut expected = vec![AnyWindowHandle::from(a.window), b.window.into()];
    expected.sort_by_key(|w| w.window_id());
    assert_eq!(windows, expected);
}
