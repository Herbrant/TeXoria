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
    test_support::copy_dir(&test_support::fixtures().join(name), dir.path());
    dir
}

/// git with the user's and the system's configuration out of the way, in a temporary folder
/// (also git's `HOME`) for the repositories of a test.
pub struct GitSandbox {
    pub git: tex_git::Git,
    /// The temporary folder, canonical.
    pub dir: PathBuf,
    raw: test_support::git::Isolated,
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
        let raw = test_support::git::Isolated::new(found.program(), tmp.path());
        let git =
            (raw.env().iter()).fold(tex_git::Git::at(found.program()), |g, (k, v)| g.env(k, v));
        let dir = crate::paths::canonical(tmp.path());
        Some(GitSandbox {
            git,
            dir,
            raw,
            _tmp: tmp,
        })
    }

    /// git run in `dir` for the states a test sets up; it must succeed. Its output.
    #[track_caller]
    pub fn run(&self, dir: &Path, args: &[&str]) -> String {
        self.raw.ok(dir, args)
    }
}

/// What `task` resolves to, once everything pending has run.
pub fn resolve<T: 'static>(cx: &mut TestAppContext, task: gpui_kit::Task<T>) -> T {
    let out = std::rc::Rc::new(std::cell::RefCell::new(None));
    let slot = out.clone();
    cx.foreground_executor()
        .spawn(async move { *slot.borrow_mut() = Some(task.await) })
        .detach();
    cx.run_until_parked();
    out.take().expect("the task is still pending")
}

/// A program another crate of the workspace builds for its tests, next to this test executable
/// (`target/<profile>/deps/texoria-<hash>`; `cargo test --workspace` builds them all in the same
/// folder). `None`, and the test passes trivially, when it was not built, unless
/// `TEXORIA_REQUIRE_FAKES=1` (as in CI).
pub fn fake_program(name: &str, package: &str) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let file = format!("{name}{}", std::env::consts::EXE_SUFFIX);
    let path = exe.parent()?.parent()?.join(file);
    if path.is_file() {
        return Some(path);
    }
    let why = format!("{name} not built (cargo build -p {package} --bin {name})");
    test_support::skip("FAKES", &why);
    None
}

/// tex-compile's `fake-engine` doing `steps` (see `crates/tex-compile/tests/bin/fake_engine.rs`).
pub fn fake_engine(steps: &[&str]) -> Option<tex_compile::Engine> {
    Some(tex_compile::Engine {
        kind: tex_compile::EngineKind::Custom {
            program: fake_program("fake-engine", "tex-compile")?,
            args: steps.iter().map(|s| s.to_string()).collect(),
        },
        extra_path: Vec::new(),
    })
}

/// tex-agent's `fake-acp-agent` playing a script (see
/// `crates/tex-agent/tests/bin/fake_acp_agent.rs`), as a custom agent of the settings.
pub struct FakeAgent {
    pub custom: tex_agent::CustomAgent,
    dir: tempfile::TempDir,
}

impl FakeAgent {
    /// The agent's name, and its id in the settings once found.
    pub const NAME: &str = "Fake";
    pub const ID: &str = "custom:Fake";

    pub fn new(mut script: serde_json::Value) -> Option<FakeAgent> {
        let program = fake_program("fake-acp-agent", "tex-agent")?;
        let dir = tempfile::tempdir().unwrap();
        script["log"] = serde_json::json!(dir.path().join("log.jsonl"));
        let path = dir.path().join("script.json");
        std::fs::write(&path, script.to_string()).unwrap();
        let custom = tex_agent::CustomAgent {
            name: Self::NAME.into(),
            command: program.to_string_lossy().into_owned(),
            args: vec![path.to_string_lossy().into_owned()],
        };
        Some(FakeAgent { custom, dir })
    }

    /// What the agent read from TeXoria so far, one JSON-RPC message each.
    pub fn log(&self) -> Vec<serde_json::Value> {
        std::fs::read_to_string(self.dir.path().join("log.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    /// The requests TeXoria sent for `method`.
    pub fn requests(&self, method: &str) -> Vec<serde_json::Value> {
        (self.log().into_iter())
            .filter(|m| m["method"] == method)
            .collect()
    }

    /// TeXoria's answer to the agent's request number `id` (they start at 1001), once sent.
    pub fn answer(&self, id: i64) -> Option<serde_json::Value> {
        (self.log().into_iter()).find(|m| m["id"] == id && m.get("method").is_none())
    }
}

/// The agents of the settings whose command is a path, as `AgentPanel` would find them, without
/// looking for the agents installed on this machine nor asking the login shell for its `PATH`.
pub fn custom_agents_only(custom: &[tex_agent::CustomAgent]) -> Vec<tex_agent::AgentSpec> {
    (custom.iter())
        .filter(|c| Path::new(&c.command).is_absolute())
        .map(|c| tex_agent::AgentSpec {
            id: format!("custom:{}", c.name),
            name: c.name.clone(),
            program: PathBuf::from(&c.command),
            args: c.args.clone(),
            env: vec![("PATH".into(), std::env::var("PATH").unwrap_or_default())],
            login_hint: None,
        })
        .collect()
}

/// Let real threads (an engine's, an agent's, PDFium's) wake the test's tasks: GPUI's test
/// scheduler takes that for a mistake otherwise.
pub fn allow_threads(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
}

/// Run the app's tasks until `done` holds, waiting for real processes and threads meanwhile
/// (see [`allow_threads`]); fails after 20 seconds.
#[track_caller]
pub fn wait_until(
    cx: &mut TestAppContext,
    what: &str,
    mut done: impl FnMut(&mut TestAppContext) -> bool,
) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        cx.run_until_parked();
        if done(cx) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting: {what}"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// A copy of the sample project opened with the fake agent playing `script(project dir)`:
/// agents are allowed in the project and the fake is the chosen one (the only one found).
#[allow(clippy::type_complexity)]
pub fn agent_workspace(
    cx: &mut TestAppContext,
    script: impl FnOnce(&Path) -> serde_json::Value,
) -> Option<(
    tempfile::TempDir,
    FakeAgent,
    WindowHandle<Root>,
    Entity<Workspace>,
)> {
    let tmp = fixture_copy("sample");
    let dir = crate::paths::canonical(tmp.path());
    let agent = FakeAgent::new(script(&dir))?;
    let mut startup = startup();
    startup.settings.custom_agents = vec![agent.custom.clone()];
    startup.settings.agent = Some(FakeAgent::ID.into());
    startup.settings.agent_projects = vec![dir.clone()];
    startup.settings.spell_check = false;
    allow_threads(cx);
    let (window, workspace) = open(cx, Some(dir.join("main.tex")), startup);
    workspace.update(cx, |ws, cx| {
        (ws.agent_panel()).update(cx, |a, _| a.find_agents_with(custom_agents_only))
    });
    Some((tmp, agent, window, workspace))
}

/// Lay the window out and paint it, as a frame on screen would.
pub fn draw(cx: &mut TestAppContext, window: WindowHandle<Root>) {
    cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
        .unwrap();
}

/// The notifications shown in `window`.
pub fn notifications(cx: &mut TestAppContext, window: WindowHandle<Root>) -> usize {
    use gpui_kit::component::WindowExt as _;
    cx.update_window(window.into(), |_, window, cx| {
        window.notifications(cx).len()
    })
    .unwrap()
}
