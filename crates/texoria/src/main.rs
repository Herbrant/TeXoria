//! TeXoria — an elegant, fast LaTeX editor.

mod agent_panel;
mod agent_tools;
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
mod updater;
mod workspace;

use std::path::PathBuf;

use gpui_kit::*;

use crate::workspace::Workspace;

actions!(
    texoria,
    [
        Open,
        Save,
        Build,
        CancelBuild,
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
        CheckForUpdates,
        ToggleAgent,
        EditWithAgent,
        FixWithAgent,
        StopAgent,
        Quit,
    ]
);

fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("secondary-o", Open, None),
        KeyBinding::new("secondary-s", Save, None),
        KeyBinding::new("secondary-r", Build, None),
        KeyBinding::new("secondary-.", CancelBuild, None),
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
        KeyBinding::new("secondary-shift-k", ToggleAgent, None),
        KeyBinding::new("secondary-k", EditWithAgent, None),
        KeyBinding::new("secondary-q", Quit, None),
    ]);
}

fn set_menus(cx: &mut App) {
    cx.set_menus([
        Menu::new("TeXoria").items([
            MenuItem::action("Check for Updates…", CheckForUpdates),
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
        Menu::new("Build").items([
            MenuItem::action("Build", Build),
            MenuItem::action("Stop Building", CancelBuild),
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
        Menu::new("Agent").items([
            MenuItem::action("Agent", ToggleAgent),
            MenuItem::separator(),
            MenuItem::action("Edit Selection with Instructions…", EditWithAgent),
            MenuItem::action("Fix Build Problems", FixWithAgent),
            MenuItem::action("Stop", StopAgent),
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
    // Started by a coding agent: relay its MCP connection to the app's tools.
    if std::env::args_os().nth(1).as_deref() == Some(tex_agent::mcp::BRIDGE_FLAG.as_ref()) {
        if let Err(e) = tex_agent::mcp::bridge() {
            eprintln!("texoria {}: {e}", tex_agent::mcp::BRIDGE_FLAG);
            std::process::exit(1);
        }
        return;
    }
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
