//! The main windows, one per project: opening them, finding the one that shows a project, and
//! keeping their settings in step.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui_kit::*;

use crate::settings::Settings;
use crate::workspace::{Startup, Workspace};

/// How far a new window opens from the active one, so that it does not hide it exactly.
const CASCADE: f32 = 28.;

/// The workspaces open, with their windows (not the PDF's nor the Settings').
#[derive(Default)]
struct Registry(Vec<(AnyWindowHandle, WeakEntity<Workspace>)>);

impl Global for Registry {}

/// Make the workspace of `window` known to the others.
pub fn register(window: AnyWindowHandle, workspace: WeakEntity<Workspace>, cx: &mut App) {
    let registry = cx.default_global::<Registry>();
    registry.0.retain(|(_, w)| w.upgrade().is_some());
    registry.0.push((window, workspace));
}

/// The open workspaces and their windows, the one in front first (as far as the system tells).
pub fn workspaces(cx: &App) -> Vec<(AnyWindowHandle, Entity<Workspace>)> {
    let Some(registry) = cx.try_global::<Registry>() else {
        return Vec::new();
    };
    let open = cx.windows();
    let mut all: Vec<_> = registry
        .0
        .iter()
        .filter(|(window, _)| open.contains(window))
        .filter_map(|(window, w)| Some((*window, w.upgrade()?)))
        .collect();
    // The active window, then the others front to back; the most recently opened first
    // where the system does not tell their order.
    all.reverse();
    let stack = cx.window_stack().unwrap_or_default();
    let active = cx.active_window();
    all.sort_by_key(|(window, _)| {
        if Some(*window) == active {
            0
        } else {
            stack
                .iter()
                .position(|w| w == window)
                .map_or(usize::MAX, |p| p + 1)
        }
    });
    all
}

/// The workspace other than `except` whose project's root file is `root`.
pub fn showing(
    root: &Path,
    except: EntityId,
    cx: &App,
) -> Option<(AnyWindowHandle, Entity<Workspace>)> {
    workspaces(cx)
        .into_iter()
        .filter(|(_, w)| w.entity_id() != except)
        .find(|(_, w)| w.read(cx).root() == Some(root))
}

/// Hand the settings `from` saved to the other workspaces, which take them as theirs.
pub fn share_settings(from: EntityId, settings: &Settings, cx: &mut App) {
    for (window, workspace) in workspaces(cx) {
        if workspace.entity_id() == from {
            continue;
        }
        let settings = settings.clone();
        window
            .update(cx, |_, window, cx| {
                workspace.update(cx, |w, cx| w.adopt_settings(settings, window, cx))
            })
            .inspect_err(|e| log::warn!("sharing the settings with another window: {e}"))
            .ok();
    }
}

/// Open `path` where it belongs: the window of its project, a window still on the start
/// page, or a new one.
pub fn open_document(path: PathBuf, cx: &mut App) {
    let path = crate::paths::canonical(&path);
    let all = workspaces(cx);
    let target = all
        .iter()
        .find(|(_, w)| {
            w.read(cx)
                .project_dir()
                .is_some_and(|d| path.starts_with(d))
        })
        .or_else(|| all.iter().find(|(_, w)| w.read(cx).root().is_none()))
        .cloned();
    let Some((window, workspace)) = target else {
        return open_new(Some(path), cx);
    };
    window
        .update(cx, |_, window, cx| {
            workspace.update(cx, |w, cx| w.open_path(path, window, cx));
            window.activate_window();
        })
        .ok();
}

/// Open a new window on `path` (a `.tex` file, or none for the start page).
pub fn open_new(path: Option<PathBuf>, cx: &mut App) {
    let bounds = workspaces(cx)
        .first()
        .and_then(|(window, _)| {
            window
                .update(cx, |_, window, _| match window.window_bounds() {
                    WindowBounds::Windowed(_) => Some(window.bounds()),
                    _ => None,
                })
                .ok()
                .flatten()
        })
        .map(|b| {
            let origin = b.origin + point(px(CASCADE), px(CASCADE));
            WindowBounds::Windowed(Bounds::new(origin, b.size))
        })
        .unwrap_or_else(|| WindowBounds::centered(size(px(1440.), px(900.)), cx));
    // The title bar moves the window and handles double clicks itself
    // (otherwise macOS does both too, and delays clicks on its buttons).
    let options = WindowOptions {
        window_bounds: Some(bounds),
        window_min_size: Some(size(px(720.), px(480.))),
        // Matches the packaged `texoria.desktop`, so Linux shells pick up its icon.
        app_id: Some("texoria".into()),
        ..component::TitleBar::window_options()
    };
    let startup = startup(cx);
    gpui_kit::open_window(options, cx, move |window, cx| {
        cx.new(|cx| Workspace::with_startup(path, startup, window, cx))
    })
    .expect("failed to open window");
    cx.activate(true);
}

/// Makes what new windows start from, in place of the machine's (tests).
struct NewStartup(Rc<dyn Fn() -> Startup>);

impl Global for NewStartup {}

/// Start new windows from what `startup` makes instead of the machine's settings, engines
/// and PDFium.
#[cfg(test)]
pub fn set_startup(startup: impl Fn() -> Startup + 'static, cx: &mut App) {
    cx.set_global(NewStartup(Rc::new(startup)));
}

fn startup(cx: &App) -> Startup {
    match cx.try_global::<NewStartup>() {
        Some(make) => (make.0)(),
        None => Startup::detect(),
    }
}
