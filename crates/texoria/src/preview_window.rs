//! The PDF in a window of its own, to put on another display.
//!
//! It shows the preview's pane itself, moved out of the main window
//! (`Workspace::toggle_preview_window`), so that its zoom, its place in the
//! document and its renderer stay the same. The main window's actions (build,
//! forward search…) typed here are passed on to it.

use gpui_kit::component::{ActiveTheme, TitleBar, h_flex, v_flex};
use gpui_kit::*;

use crate::pdf_pane::PdfPane;
use crate::settings::{WindowPlacement, WindowState};
use crate::workspace::Workspace;
use crate::{
    Build, CancelBuild, CloseTab, ExportPdf, ForwardSearch, OpenSettings, Save, ToggleLivePreview,
    TogglePreviewWindow, ToggleTheme, ZoomIn, ZoomOut, ZoomToFit,
};

/// The size the window opens at the first time.
const DEFAULT_SIZE: Size<Pixels> = size(px(820.), px(1000.));
const MIN_SIZE: Size<Pixels> = size(px(320.), px(320.));

pub struct PreviewWindow {
    pane: Entity<PdfPane>,
    title: SharedString,
    workspace: WeakEntity<Workspace>,
    main: AnyWindowHandle,
    /// The workspace's focus, where the actions passed on start from.
    main_focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl PreviewWindow {
    pub fn new(
        pane: Entity<PdfPane>,
        root_name: Option<String>,
        workspace: WeakEntity<Workspace>,
        main: AnyWindowHandle,
        main_focus: FocusHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // The pane's keys (zoom, pages, find) work at once.
        let focus = pane.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        let subscriptions = vec![cx.observe_window_bounds(window, |this, window, cx| {
            let placement = placement(window, cx);
            this.workspace
                .update(cx, |w, _| w.settings.preview_window = Some(placement))
                .ok();
        })];
        let title = title(root_name);
        window.set_window_title(&title);
        Self {
            pane,
            title,
            workspace,
            main,
            main_focus,
            _subscriptions: subscriptions,
        }
    }

    /// Name the window after the document's root file.
    pub fn set_root_name(
        &mut self,
        name: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.title = title(name);
        window.set_window_title(&self.title);
        cx.notify();
    }

    /// Run `A` in the main window, as if it had been typed there.
    fn forward<A: Action>(&self) -> impl Fn(&A, &mut Window, &mut App) + 'static {
        let (main, focus) = (self.main, self.main_focus.clone());
        move |action, _, cx| {
            let action = action.boxed_clone();
            let focus = focus.clone();
            // It may close this window, which is being updated.
            cx.defer(move |cx| {
                main.update(cx, |_, window, cx| {
                    focus.dispatch_action(action.as_ref(), window, cx)
                })
                .ok();
            });
        }
    }
}

impl Render for PreviewWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Never read the workspace here: an entity read while a window draws
        // belongs to that window afterwards, and the workspace's events would
        // run in this one.
        let theme = cx.theme();
        v_flex()
            .id("preview-window")
            .key_context("PreviewWindow")
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .font_family(theme.font_family.clone())
            .on_action(self.forward::<Build>())
            .on_action(self.forward::<CancelBuild>())
            .on_action(self.forward::<ForwardSearch>())
            .on_action(self.forward::<ToggleLivePreview>())
            .on_action(self.forward::<Save>())
            .on_action(self.forward::<ExportPdf>())
            .on_action(self.forward::<OpenSettings>())
            .on_action(self.forward::<ToggleTheme>())
            .on_action(self.forward::<TogglePreviewWindow>())
            // The workspace zooms the PDF (⌘= / ⌘- / ⌘0 in the pane, the View menu).
            .on_action(self.forward::<ZoomIn>())
            .on_action(self.forward::<ZoomOut>())
            .on_action(self.forward::<ZoomToFit>())
            // ⌘W closes this window: the PDF goes back to the main one.
            .on_action({
                let toggle = self.forward::<TogglePreviewWindow>();
                move |_: &CloseTab, window, cx| toggle(&TogglePreviewWindow, window, cx)
            })
            .child(
                TitleBar::new().child(
                    h_flex()
                        .flex_1()
                        .justify_center()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(self.title.clone()),
                ),
            )
            .child(div().flex_1().min_h_0().child(self.pane.clone()))
    }
}

fn title(root_name: Option<String>) -> SharedString {
    root_name.map_or_else(|| "PDF".into(), |name| format!("{name} — PDF").into())
}

/// Where `window` is, to open it there again.
pub fn placement(window: &Window, cx: &App) -> WindowPlacement {
    let (state, bounds) = match window.window_bounds() {
        WindowBounds::Windowed(b) => (WindowState::Windowed, b),
        WindowBounds::Maximized(b) => (WindowState::Maximized, b),
        WindowBounds::Fullscreen(b) => (WindowState::Fullscreen, b),
    };
    WindowPlacement {
        display: window
            .display(cx)
            .and_then(|d| d.uuid().ok())
            .map(|u| u.to_string()),
        x: f32::from(bounds.origin.x),
        y: f32::from(bounds.origin.y),
        width: f32::from(bounds.size.width),
        height: f32::from(bounds.size.height),
        state,
    }
}

/// The options of a preview window opened where `placement` says: on its
/// display, if it is still connected, else centered on the main display.
pub fn options(placement: Option<&WindowPlacement>, cx: &App) -> WindowOptions {
    let saved = placement.and_then(|p| {
        let display = cx
            .displays()
            .into_iter()
            .find(|d| d.uuid().ok().map(|u| u.to_string()) == p.display)?;
        let sane = [p.x, p.y, p.width, p.height].iter().all(|v| v.is_finite())
            && p.width >= f32::from(MIN_SIZE.width)
            && p.height >= f32::from(MIN_SIZE.height);
        sane.then(|| {
            let bounds = Bounds::new(point(px(p.x), px(p.y)), size(px(p.width), px(p.height)));
            let bounds = match p.state {
                WindowState::Windowed => WindowBounds::Windowed(bounds),
                WindowState::Maximized => WindowBounds::Maximized(bounds),
                WindowState::Fullscreen => WindowBounds::Fullscreen(bounds),
            };
            (display.id(), bounds)
        })
    });
    let (display_id, bounds) = match saved {
        Some((id, bounds)) => (Some(id), bounds),
        None => (None, WindowBounds::centered(DEFAULT_SIZE, cx)),
    };
    WindowOptions {
        window_bounds: Some(bounds),
        display_id,
        window_min_size: Some(MIN_SIZE),
        app_id: Some("texoria".into()),
        ..TitleBar::window_options()
    }
}

#[cfg(test)]
mod tests;
