//! Helpers for the UI tests: the app set up as `main` does, in GPUI's headless test platform,
//! and workspaces that never read the machine's settings or look for its TeX engines.

use std::path::{Path, PathBuf};

use gpui_kit::{AppContext, Entity, TestAppContext, WindowHandle, WindowOptions, base::Root};

use crate::settings::Settings;
use crate::workspace::{Startup, Workspace};

/// Initialize the app in `cx` like `main` does.
pub fn init(cx: &mut TestAppContext) {
    cx.update(crate::init);
}

/// What a test workspace starts from: default settings, no engine, no PDFium.
pub fn startup() -> Startup {
    Startup {
        settings: Settings::default(),
        engines: Vec::new(),
        pdfium_dirs: Vec::new(),
    }
}

/// A workspace window opened on `path`, the app already initialized.
pub fn open(
    cx: &mut TestAppContext,
    path: Option<PathBuf>,
    startup: Startup,
) -> (WindowHandle<Root>, Entity<Workspace>) {
    init(cx);
    let (window, workspace) = cx.update(|cx| {
        let (window, workspace) =
            gpui_kit::open_window(WindowOptions::default(), cx, move |window, cx| {
                cx.new(|cx| Workspace::with_startup(path, startup, window, cx))
            })
            .expect("open test window");
        (window.downcast::<Root>().expect("a Root window"), workspace)
    });
    cx.run_until_parked();
    (window, workspace)
}

/// A copy of `tests/fixtures/<name>` in a temporary folder (tests must not write into the
/// repository: building would add a `.texoria` folder there).
pub fn fixture_copy(name: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    copy_dir(&test_support::fixtures().join(name), dir.path());
    dir
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let (name, path) = (entry.file_name(), entry.path());
        if name == ".texoria" {
            continue;
        }
        if path.is_dir() {
            copy_dir(&path, &to.join(name));
        } else {
            std::fs::copy(&path, to.join(name)).unwrap();
        }
    }
}
