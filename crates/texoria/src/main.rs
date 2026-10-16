//! TeXoria — an elegant, fast LaTeX editor.

mod assets;
mod atomic;
mod diff_view;
mod file_tree;
mod git_panel;
mod latex;
mod overleaf;
mod pdf_pane;
mod settings;
mod settings_window;
mod spell;
mod theme;
mod workspace;

use std::path::PathBuf;

use gpui_kit::*;

use crate::workspace::Workspace;

actions!(
    texoria,
    [
        Open,
        Save,
        Typeset,
        CancelTypeset,
        ForwardSearch,
        ToggleTheme,
        ZoomIn,
        ZoomOut,
        ZoomToFit,
        ToggleOutline,
        ToggleFiles,
        ToggleDiagnostics,
        ToggleLivePreview,
        ToggleSoftWrap,
        CloseTab,
        ShowSourceControl,
        Commit,
        Fetch,
        Pull,
        Push,
        NewBranch,
        CloneRepository,
        OpenFromOverleaf,
        SyncOverleaf,
        OpenSettings,
        Quit,
    ]
);

fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("secondary-o", Open, None),
        KeyBinding::new("secondary-s", Save, None),
        KeyBinding::new("secondary-r", Typeset, None),
        KeyBinding::new("secondary-.", CancelTypeset, None),
        KeyBinding::new("secondary-j", ForwardSearch, None),
        KeyBinding::new("secondary-shift-l", ToggleTheme, None),
        KeyBinding::new("secondary-=", ZoomIn, None),
        KeyBinding::new("secondary-+", ZoomIn, None),
        KeyBinding::new("secondary--", ZoomOut, None),
        KeyBinding::new("secondary-0", ZoomToFit, None),
        KeyBinding::new("secondary-1", ToggleOutline, None),
        KeyBinding::new("secondary-shift-e", ToggleFiles, None),
        KeyBinding::new("secondary-shift-m", ToggleDiagnostics, None),
        KeyBinding::new("secondary-shift-p", ToggleLivePreview, None),
        KeyBinding::new("alt-z", ToggleSoftWrap, None),
        KeyBinding::new("secondary-w", CloseTab, None),
        KeyBinding::new("secondary-shift-g", ShowSourceControl, None),
        KeyBinding::new("secondary-,", OpenSettings, None),
        KeyBinding::new("secondary-q", Quit, None),
    ]);
}

fn set_menus(cx: &mut App) {
    cx.set_menus([
        Menu::new("TeXoria").items([
            MenuItem::action("Settings…", OpenSettings),
            MenuItem::separator(),
            MenuItem::action("Quit TeXoria", Quit),
        ]),
        Menu::new("File").items([
            MenuItem::action("Open…", Open),
            MenuItem::action("Save", Save),
            MenuItem::action("Clone Repository…", CloneRepository),
            MenuItem::action("Open from Overleaf…", OpenFromOverleaf),
            MenuItem::separator(),
            MenuItem::action("Close Tab", CloseTab),
        ]),
        Menu::new("Typeset").items([
            MenuItem::action("Typeset", Typeset),
            MenuItem::action("Stop Typesetting", CancelTypeset),
            MenuItem::action("Show in PDF", ForwardSearch),
            MenuItem::action("Live Preview", ToggleLivePreview),
        ]),
        Menu::new("View").items([
            MenuItem::action("Outline", ToggleOutline),
            MenuItem::action("Files", ToggleFiles),
            MenuItem::action("Problems", ToggleDiagnostics),
            MenuItem::action("Soft Wrap", ToggleSoftWrap),
            MenuItem::separator(),
            MenuItem::action("Zoom In", ZoomIn),
            MenuItem::action("Zoom Out", ZoomOut),
            MenuItem::action("Fit Width", ZoomToFit),
            MenuItem::separator(),
            MenuItem::action("Light/Dark", ToggleTheme),
        ]),
        Menu::new("Git").items([
            MenuItem::action("Source Control", ShowSourceControl),
            MenuItem::separator(),
            MenuItem::action("Commit", Commit),
            MenuItem::action("Pull", Pull),
            MenuItem::action("Push", Push),
            MenuItem::action("Fetch", Fetch),
            MenuItem::action("Sync with Overleaf", SyncOverleaf),
            MenuItem::separator(),
            MenuItem::action("New Branch…", NewBranch),
            MenuItem::action("Clone Repository…", CloneRepository),
            MenuItem::action("Open from Overleaf…", OpenFromOverleaf),
        ]),
    ]);
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let path = std::env::args_os().nth(1).map(PathBuf::from);

    gpui_kit::application()
        .with_assets(assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            if let Err(e) = cx.text_system().add_fonts(assets::fonts()) {
                log::warn!("loading bundled fonts: {e}");
            }
            theme::init(None, cx);
            latex::register();
            bind_keys(cx);
            set_menus(cx);
            cx.on_action(|_: &Quit, cx| cx.quit());

            let options = WindowOptions {
                titlebar: Some(component::TitleBar::title_bar_options()),
                window_bounds: Some(WindowBounds::centered(size(px(1440.), px(900.)), cx)),
                window_min_size: Some(size(px(720.), px(480.))),
                // Matches the packaged `texoria.desktop`, so Linux shells pick up its icon.
                app_id: Some("texoria".into()),
                ..Default::default()
            };
            gpui_kit::open_window(options, cx, move |window, cx| {
                cx.new(|cx| Workspace::new(path, window, cx))
            })
            .expect("failed to open window");
            cx.activate(true);
        });
}
