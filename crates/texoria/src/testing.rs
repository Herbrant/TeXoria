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

/// What a test workspace starts from: default settings, no engine, no PDFium, and no tool
/// server for agents (its threads would wake the test scheduler from outside).
pub fn startup() -> Startup {
    Startup {
        settings: Settings::default(),
        engines: Vec::new(),
        pdfium_dirs: Vec::new(),
        tool_server: false,
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

/// git with the user's and the system's configuration out of the way, in a temporary folder
/// (also git's `HOME`) for the repositories of a test.
pub struct GitSandbox {
    pub git: tex_git::Git,
    /// The temporary folder, canonical.
    pub dir: PathBuf,
    env: Vec<(&'static str, PathBuf)>,
    _tmp: tempfile::TempDir,
}

impl GitSandbox {
    /// `None` (the test then passes trivially) when git is not installed.
    pub fn new() -> Option<GitSandbox> {
        let Some(found) = tex_git::Git::find() else {
            test_support::skip("GIT", "git not found");
            return None;
        };
        let tmp = tempfile::tempdir().unwrap();
        let env = test_support::git::isolated_env(tmp.path());
        let git = (env.iter()).fold(tex_git::Git::at(found.program()), |g, (k, v)| g.env(k, v));
        let dir = crate::paths::canonical(tmp.path());
        Some(GitSandbox {
            git,
            dir,
            env,
            _tmp: tmp,
        })
    }

    /// git run in `dir` for the states a test sets up; it must succeed. Its output.
    #[track_caller]
    pub fn run(&self, dir: &Path, args: &[&str]) -> String {
        let mut cmd = std::process::Command::new(self.git.program());
        cmd.current_dir(dir).args(args).env("LC_ALL", "C");
        cmd.envs(self.env.iter().map(|(k, v)| (k, v)));
        let out = cmd.output().unwrap();
        let text = |b: &[u8]| String::from_utf8_lossy(b).trim().to_string();
        assert!(out.status.success(), "git {args:?}: {}", text(&out.stderr));
        text(&out.stdout)
    }
}
