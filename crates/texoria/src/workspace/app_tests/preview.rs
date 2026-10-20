//! The PDF in a window of its own, and back next to the editor.

use super::*;
use crate::preview_window::PreviewWindow;
use crate::settings::WindowState;
use core::prelude::v1::test;
use gpui_kit::{AnyWindowHandle, VisualTestContext, gpui};

impl Open {
    /// The PDF's own window, if it is open.
    fn preview(&self, cx: &mut TestAppContext) -> Option<WindowHandle<Root>> {
        let handle = self.read(cx, |ws, _| ws.preview_window)?;
        let root = handle.downcast::<Root>().expect("a Root window");
        let view = cx.read(|cx| root.read(cx).unwrap().view().clone());
        assert!(view.downcast::<PreviewWindow>().is_ok());
        Some(root)
    }

    fn detached(&self, cx: &mut TestAppContext) -> bool {
        let pane = self.read(cx, |ws, _| ws.pdf.clone());
        let detached = self.read(cx, |ws, _| ws.preview_detached);
        assert_eq!(pane.read_with(cx, |p, _| p.is_detached()), detached);
        detached
    }
}

/// `action` typed in `window`, where the PDF has the focus.
fn dispatch_in(cx: &mut TestAppContext, window: WindowHandle<Root>, action: impl gpui::Action) {
    testing::draw(cx, window);
    cx.dispatch_action(window.into(), action);
    cx.run_until_parked();
}

#[gpui::test]
fn the_pdf_moves_to_a_window_of_its_own_and_back(cx: &mut TestAppContext) {
    let settings = tempfile::tempdir().unwrap();
    let app = Open::sample(cx, saved_in(settings.path()));
    assert_eq!(cx.windows().len(), 1);
    assert!(!app.detached(cx));

    app.dispatch(cx, crate::TogglePreviewWindow);
    assert!(app.detached(cx));
    let preview = app.preview(cx).expect("the PDF's window");
    assert_eq!(cx.windows().len(), 2);
    app.draw(cx);
    testing::draw(cx, preview);

    // The main window's actions work from there.
    let live = app.read(cx, |ws, _| ws.settings.live_preview);
    dispatch_in(cx, preview, crate::ToggleLivePreview);
    assert_eq!(app.read(cx, |ws, _| ws.settings.live_preview), !live);
    dispatch_in(cx, preview, crate::ToggleLivePreview);
    assert_eq!(app.read(cx, |ws, _| ws.settings.live_preview), live);

    // ⌘W there brings the PDF back, and where its window was is saved.
    dispatch_in(cx, preview, crate::CloseTab);
    assert!(!app.detached(cx));
    assert_eq!(app.read(cx, |ws, _| ws.preview_window), None);
    assert_eq!(cx.windows().len(), 1);
    app.draw(cx);
    let saved = Settings::load_from(settings.path().join("settings.toml"));
    let placement = saved.preview_window.expect("the window's place");
    assert_eq!(placement.state, WindowState::Windowed);
    assert!(placement.display.is_some());
    assert_eq!(app.read(cx, |ws, _| ws.buffers.len()), 1, "no tab closed");

    // It opens again where it was, and the same shortcut docks it.
    app.dispatch(cx, crate::TogglePreviewWindow);
    let preview = app.preview(cx).unwrap();
    let bounds = cx
        .update_window(preview.into(), |_, window, _| window.bounds())
        .unwrap();
    assert_eq!(f32::from(bounds.size.width), placement.width);
    assert_eq!(f32::from(bounds.origin.x), placement.x);
    app.dispatch(cx, crate::TogglePreviewWindow);
    assert!(!app.detached(cx));
    assert_eq!(cx.windows().len(), 1);
}

#[gpui::test]
fn closing_the_pdfs_window_docks_it(cx: &mut TestAppContext) {
    let app = Open::sample(cx, quiet());
    let pane = app.read(cx, |ws, _| ws.pdf.clone());
    pane.update(cx, |_, cx| cx.emit(PdfPaneEvent::ToggleWindow));
    cx.run_until_parked();
    let preview = app.preview(cx).expect("the PDF's window");
    assert!(app.detached(cx));
    testing::draw(cx, preview);

    // A jump to the source from the PDF brings the editor's window forward.
    let main = app.path("main.tex");
    cx.update_window(preview.into(), |_, window, _| window.activate_window())
        .unwrap();
    pane.update(cx, |_, cx| {
        cx.emit(PdfPaneEvent::InverseSearch {
            file: main.clone(),
            line: 3,
        })
    });
    cx.run_until_parked();
    let active = cx.update(|cx| cx.active_window());
    assert_eq!(active, Some(AnyWindowHandle::from(app.window)));

    let mut vcx = VisualTestContext::from_window(preview.into(), cx);
    assert!(vcx.simulate_close());
    cx.run_until_parked();
    assert!(!app.detached(cx));
    assert_eq!(app.read(cx, |ws, _| ws.preview_window), None);
    assert!(app.read(cx, |ws, _| ws.settings.preview_window.is_some()));
    app.draw(cx);
}

#[gpui::test]
fn the_pdfs_window_closes_with_the_main_one(cx: &mut TestAppContext) {
    let app = Open::sample(cx, quiet());
    // Docked again before its window opened: it does not stay open.
    app.update(cx, |ws, window, cx| {
        ws.toggle_preview_window(window, cx);
        ws.toggle_preview_window(window, cx);
    });
    cx.run_until_parked();
    assert_eq!(cx.windows().len(), 1);
    assert!(!app.detached(cx));
    app.dispatch(cx, crate::TogglePreviewWindow);
    assert_eq!(cx.windows().len(), 2);

    let mut vcx = VisualTestContext::from_window(app.window.into(), cx);
    assert!(vcx.simulate_close());
    cx.run_until_parked();
    assert_eq!(cx.windows().len(), 1, "only the main window is left");
    // Where the PDF's window was is kept for next time.
    assert!(app.read(cx, |ws, _| ws.settings.preview_window.is_some()));
}
