//! TeXoria — an elegant, fast LaTeX editor.

mod agent_panel;
mod agent_tools;
mod assets;
mod atomic;
mod checks;
mod diff_view;
mod edit;
mod file_tree;
mod git_panel;
mod grammar;
mod latex;
mod overleaf;
mod pdf_pane;
mod presentation;
mod search_panel;
mod settings;
mod settings_window;
mod spell;
mod theme;
mod updater;
mod vim;
mod workspace;

#[cfg(test)]
mod testing;

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
        IncreaseFontSize,
        DecreaseFontSize,
        ResetFontSize,
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
        ToggleComment,
        FindInProject,
        FormatDocument,
        ExportPdf,
        Present,
        AddCitation,
        ToggleVimMode,
        Quit,
    ]
);

fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("secondary-o", Open, None),
        KeyBinding::new("secondary-s", Save, None),
        KeyBinding::new("secondary-shift-s", ExportPdf, None),
        KeyBinding::new("secondary-shift-enter", Present, None),
        KeyBinding::new("secondary-shift-c", AddCitation, None),
        KeyBinding::new("secondary-r", Build, None),
        KeyBinding::new("secondary-.", CancelBuild, None),
        KeyBinding::new("secondary-j", ForwardSearch, None),
        KeyBinding::new("secondary-shift-l", ToggleTheme, None),
        // The editor's text size, or the PDF's zoom once the preview has focus.
        KeyBinding::new("secondary-=", IncreaseFontSize, None),
        KeyBinding::new("secondary-+", IncreaseFontSize, None),
        KeyBinding::new("secondary--", DecreaseFontSize, None),
        KeyBinding::new("secondary-0", ResetFontSize, None),
        KeyBinding::new("secondary-=", ZoomIn, Some("PdfPane")),
        KeyBinding::new("secondary-+", ZoomIn, Some("PdfPane")),
        KeyBinding::new("secondary--", ZoomOut, Some("PdfPane")),
        KeyBinding::new("secondary-0", ZoomToFit, Some("PdfPane")),
        KeyBinding::new("secondary-9", pdf_pane::FitPage, Some("PdfPane")),
        KeyBinding::new("pagedown", pdf_pane::NextPage, Some("PdfPane")),
        KeyBinding::new("pageup", pdf_pane::PreviousPage, Some("PdfPane")),
        KeyBinding::new("right", pdf_pane::NextPage, Some("PdfPane")),
        KeyBinding::new("left", pdf_pane::PreviousPage, Some("PdfPane")),
        KeyBinding::new("home", pdf_pane::FirstPage, Some("PdfPane")),
        KeyBinding::new("end", pdf_pane::LastPage, Some("PdfPane")),
        KeyBinding::new("secondary-g", pdf_pane::GoToPage, Some("PdfPane")),
        KeyBinding::new("secondary-f", pdf_pane::FindInPdf, Some("PdfPane")),
        KeyBinding::new("escape", pdf_pane::CloseFind, Some("PdfPane")),
        KeyBinding::new("secondary-c", pdf_pane::CopyText, Some("PdfPane")),
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
        KeyBinding::new("secondary-/", ToggleComment, Some("Input")),
        KeyBinding::new("shift-alt-f", FormatDocument, Some("Input")),
        // Find in the project, like other editors; the editor's Replace moves.
        KeyBinding::new("secondary-shift-f", FindInProject, None),
        KeyBinding::new("secondary-shift-f", FindInProject, Some("Input")),
        KeyBinding::new(
            "secondary-alt-f",
            gpui_kit::component::input::Replace,
            Some("Input"),
        ),
        KeyBinding::new(
            "f12",
            gpui_kit::component::input::GoToDefinition,
            Some("Input"),
        ),
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
            MenuItem::action("Export PDF…", ExportPdf),
            MenuItem::action("Clone Repository…", CloneRepository),
            MenuItem::action("Open from Overleaf…", OpenFromOverleaf),
            MenuItem::separator(),
            MenuItem::action("Close Tab", CloseTab),
        ]),
        Menu::new("Edit").items([
            MenuItem::action("Undo", gpui_kit::component::input::Undo),
            MenuItem::action("Redo", gpui_kit::component::input::Redo),
            MenuItem::separator(),
            MenuItem::action("Cut", gpui_kit::component::input::Cut),
            MenuItem::action("Copy", gpui_kit::component::input::Copy),
            MenuItem::action("Paste", gpui_kit::component::input::Paste),
            MenuItem::action("Select All", gpui_kit::component::input::SelectAll),
            MenuItem::separator(),
            MenuItem::action("Find", gpui_kit::component::input::Search),
            MenuItem::action("Replace", gpui_kit::component::input::Replace),
            MenuItem::action("Find in Project", FindInProject),
            MenuItem::separator(),
            MenuItem::action("Toggle Comment", ToggleComment),
            MenuItem::action("Vim Mode", ToggleVimMode),
            MenuItem::action("Format Document", FormatDocument),
            MenuItem::separator(),
            MenuItem::action("Add Citation…", AddCitation),
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
            MenuItem::action("Bigger Text", IncreaseFontSize),
            MenuItem::action("Smaller Text", DecreaseFontSize),
            MenuItem::action("Actual Text Size", ResetFontSize),
            MenuItem::separator(),
            MenuItem::action("Zoom In PDF", ZoomIn),
            MenuItem::action("Zoom Out PDF", ZoomOut),
            MenuItem::action("Fit PDF Width", ZoomToFit),
            MenuItem::action("Fit PDF Page", pdf_pane::FitPage),
            MenuItem::action("Go to PDF Page…", pdf_pane::GoToPage),
            MenuItem::action("Find in PDF…", pdf_pane::FindInPdf),
            MenuItem::action("Present PDF", Present),
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

/// Set up the app: components, fonts, themes, the LaTeX language, key bindings and menus.
fn init(cx: &mut App) {
    gpui_kit::init(cx);
    if let Err(e) = cx.text_system().add_fonts(assets::fonts()) {
        log::warn!("loading bundled fonts: {e}");
    }
    theme::init(None, cx);
    latex::register();
    bind_keys(cx);
    presentation::bind_keys(cx);
    set_menus(cx);
    cx.on_action(|_: &Quit, cx| cx.quit());
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
            init(cx);

            // The title bar moves the window and handles double clicks itself
            // (otherwise macOS does both too, and delays clicks on its buttons).
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::centered(size(px(1440.), px(900.)), cx)),
                window_min_size: Some(size(px(720.), px(480.))),
                // Matches the packaged `texoria.desktop`, so Linux shells pick up its icon.
                app_id: Some("texoria".into()),
                ..component::TitleBar::window_options()
            };
            gpui_kit::open_window(options, cx, move |window, cx| {
                cx.new(|cx| Workspace::new(path, window, cx))
            })
            .expect("failed to open window");
            cx.activate(true);
        });
}
