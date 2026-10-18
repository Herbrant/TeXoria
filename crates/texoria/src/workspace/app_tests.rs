//! The workspace driven the way the user and the agents drive it: builds with the fake engine,
//! turns of the fake agent, the agents' tools, tabs, sessions and the toggles.

mod agents;
mod builds;
mod tools;

use std::path::{Path, PathBuf};

use gpui_kit::{Entity, TestAppContext, WindowHandle, base::Root};

use super::*;
use crate::{paths, testing};

/// What the other modules' tests look at.
impl Workspace {
    pub(crate) fn showing_agent(&self) -> bool {
        self.show_sidebar && self.sidebar == Sidebar::Agent
    }

    pub(crate) fn agent_is_busy(&self) -> bool {
        self.agent_busy
    }
}

/// A workspace in its window, on a temporary copy of a project.
struct Open {
    _tmp: Option<tempfile::TempDir>,
    /// The project's folder, canonical.
    dir: PathBuf,
    window: WindowHandle<Root>,
    ws: Entity<Workspace>,
}

impl Open {
    /// The sample project opened on `main.tex` with `startup`.
    fn sample(cx: &mut TestAppContext, startup: Startup) -> Open {
        let tmp = testing::fixture_copy("sample");
        let dir = paths::canonical(tmp.path());
        let (window, ws) = testing::open(cx, Some(dir.join("main.tex")), startup);
        Open {
            _tmp: Some(tmp),
            dir,
            window,
            ws,
        }
    }

    /// The welcome screen, no project.
    fn empty(cx: &mut TestAppContext, startup: Startup) -> Open {
        let (window, ws) = testing::open(cx, None, startup);
        Open {
            _tmp: None,
            dir: PathBuf::new(),
            window,
            ws,
        }
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.dir.join(rel)
    }

    fn update<R>(
        &self,
        cx: &mut TestAppContext,
        f: impl FnOnce(&mut Workspace, &mut Window, &mut Context<Workspace>) -> R,
    ) -> R {
        let ws = self.ws.clone();
        cx.update_window(self.window.into(), |_, window, cx| {
            ws.update(cx, |ws, cx| f(ws, window, cx))
        })
        .unwrap()
    }

    fn read<R>(&self, cx: &mut TestAppContext, f: impl FnOnce(&Workspace, &App) -> R) -> R {
        self.ws.read_with(cx, |ws, cx| f(ws, cx))
    }

    fn draw(&self, cx: &mut TestAppContext) {
        testing::draw(cx, self.window);
    }

    fn notifications(&self, cx: &mut TestAppContext) -> usize {
        testing::notifications(cx, self.window)
    }

    fn has_dialog(&self, cx: &mut TestAppContext) -> bool {
        cx.update_window(self.window.into(), |_, window, cx| {
            window.has_active_dialog(cx)
        })
        .unwrap()
    }

    /// Type `text` in the open dialog's field and press its OK button.
    fn fill_and_confirm(&self, cx: &mut TestAppContext, text: &str) {
        self.draw(cx);
        cx.simulate_input(self.window.into(), text);
        self.draw(cx);
        cx.dispatch_action(
            self.window.into(),
            gpui_kit::base::actions::Confirm { secondary: false },
        );
        cx.run_until_parked();
    }

    fn dispatch(&self, cx: &mut TestAppContext, action: impl gpui_kit::Action) {
        self.draw(cx);
        cx.dispatch_action(self.window.into(), action);
        cx.run_until_parked();
    }

    /// The text of the open file `path`.
    fn text(&self, cx: &mut TestAppContext, path: &Path) -> String {
        self.read(cx, |ws, cx| {
            let b = ws.buffers.iter().find(|b| b.path == path).expect("open");
            b.editor.read(cx).text().to_string()
        })
    }

    fn editor(&self, cx: &mut TestAppContext, path: &Path) -> Entity<EditorState> {
        self.read(cx, |ws, _| {
            let b = ws.buffers.iter().find(|b| b.path == path).expect("open");
            b.editor.clone()
        })
    }

    /// Select `range` (bytes) of the open file `path`.
    fn select(&self, cx: &mut TestAppContext, path: &Path, range: std::ops::Range<usize>) {
        let editor = self.editor(cx, path);
        editor.update(cx, |state, cx| state.set_selected_range(range, cx));
    }

    /// Type `text` at byte `at` of the open file `path`, as an edit of the user.
    fn type_at(&self, cx: &mut TestAppContext, path: &Path, at: usize, text: &str) {
        let editor = self.editor(cx, path);
        cx.update_window(self.window.into(), |_, window, cx| {
            editor.update(cx, |state, cx| {
                state.set_selected_range(at..at, cx);
                state.insert(text.to_string(), window, cx);
            })
        })
        .unwrap();
        cx.run_until_parked();
    }

    fn tabs(&self, cx: &mut TestAppContext) -> Vec<String> {
        let dir = self.dir.clone();
        self.read(cx, |ws, _| {
            (ws.buffers.iter())
                .map(|b| {
                    let rel = b.path.strip_prefix(&dir).unwrap_or(&b.path);
                    rel.to_string_lossy().replace('\\', "/")
                })
                .collect()
        })
    }

    fn active(&self, cx: &mut TestAppContext) -> String {
        let tabs = self.tabs(cx);
        let active = self.read(cx, |ws, _| ws.active);
        tabs[active].clone()
    }

    /// Call TeXoria's tool `name` as an agent would, and wait for its answer.
    fn tool(
        &self,
        cx: &mut TestAppContext,
        name: &str,
        arguments: serde_json::Value,
    ) -> ToolResult {
        let (reply, answer) = mpsc::channel();
        let call = ToolCall {
            name: name.to_string(),
            arguments,
            reply,
        };
        self.update(cx, |ws, _, cx| ws.on_tool_call(call, cx));
        let mut result = None;
        testing::wait_until(cx, &format!("the answer to {name}"), |_| {
            result = answer.try_recv().ok();
            result.is_some()
        });
        result.unwrap()
    }
}

/// A startup like `testing::startup`, with spell checking off (it only adds noise here).
fn quiet() -> Startup {
    let mut startup = testing::startup();
    startup.settings.spell_check = false;
    startup
}

/// A startup whose settings are saved in `dir/settings.toml`.
fn saved_in(dir: &Path) -> Startup {
    let mut startup = quiet();
    let spell_check = startup.settings.spell_check;
    startup.settings = Settings::load_from(dir.join("settings.toml"));
    startup.settings.spell_check = spell_check;
    startup
}
