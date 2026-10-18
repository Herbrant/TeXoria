use std::path::{Path, PathBuf};

use gpui_kit::{Entity, TestAppContext, WindowHandle, base::Root};
use serde_json::{Value, json};

use super::*;
use crate::testing::{self, FakeAgent};
use crate::workspace::Workspace;
// `super::*` brings gpui's `test` attribute, which would shadow the standard one, and
// gpui-kit's `gpui` alias, which the `#[gpui::test]` macro's paths must not find twice.
use core::prelude::v1::test;
use gpui_kit::gpui;

/// What the other modules' tests look at and do.
impl AgentPanel {
    pub(crate) fn find_agents_with(&mut self, detector: fn(&[CustomAgent]) -> Vec<AgentSpec>) {
        self.detector = detector;
    }

    pub(crate) fn is_running(&self) -> bool {
        self.running
    }

    /// The chat, one line per entry.
    pub(crate) fn transcript(&self, cx: &App) -> Vec<String> {
        (self.entries.iter())
            .map(|e| match e {
                Entry::User { text, caption } => match caption {
                    Some(c) => format!("user: {text} ({c})"),
                    None => format!("user: {text}"),
                },
                Entry::Agent(state) => {
                    format!("agent: {}", state.read(cx).rendered_text().source())
                }
                Entry::Thought { text, .. } => format!("thought: {text}"),
                Entry::Tool(t) => match &t.detail {
                    Some(d) => format!("tool: {} {:?} {:?}: {d}", t.title, t.kind, t.status),
                    None => format!("tool: {} {:?} {:?}", t.title, t.kind, t.status),
                },
                Entry::Plan(entries) => {
                    let items: Vec<&str> = entries.iter().map(|e| e.content.as_str()).collect();
                    format!("plan: {}", items.join(", "))
                }
                Entry::Permission(p) => match &p.chosen {
                    Some(c) => format!("permission: {} -> {c}", p.title),
                    None => format!("permission: {} ?", p.title),
                },
                Entry::Notice(t) => format!("notice: {t}"),
                Entry::Error(t) => format!("error: {t}"),
            })
            .collect()
    }

    /// Why the agent is not running, and its details.
    pub(crate) fn failure(&self) -> Option<(String, Option<String>)> {
        match &self.conn {
            Conn::Failed { message, details } => {
                Some((message.to_string(), details.as_ref().map(|d| d.to_string())))
            }
            _ => None,
        }
    }

    /// The files the last turn changed (relative paths).
    pub(crate) fn changed(&self) -> Vec<PathBuf> {
        self.changes.iter().map(|c| c.path.clone()).collect()
    }

    /// Compare the project with the checkpoint again (`open`: as the Review button does).
    pub(crate) fn review(&mut self, open: bool, cx: &mut Context<Self>) {
        self.refresh_changes(open, cx);
    }

    /// Answer the pending permission request with its option `name`.
    pub(crate) fn permit(&mut self, name: &str, cx: &mut Context<Self>) {
        let (ix, option) = (self.entries.iter().enumerate())
            .find_map(|(ix, e)| match e {
                Entry::Permission(p) if p.reply.is_some() => {
                    Some((ix, p.options.iter().position(|o| o.name == name)?))
                }
                _ => None,
            })
            .expect("a pending permission request with that option");
        self.choose_permission(ix, option, cx);
    }
}

// ── Scripts ─────────────────────────────────────────────────────────────

/// A `session/update` of the default session, `s1`.
fn update(update: Value) -> Value {
    json!({ "notify": { "method": "session/update", "params": {
        "sessionId": "s1", "update": update,
    }}})
}

fn says(text: &str) -> Value {
    update(json!({
        "sessionUpdate": "agent_message_chunk",
        "content": { "type": "text", "text": text },
    }))
}

/// The agent asks to `title`, offering to allow or reject it.
fn asks(title: &str) -> Value {
    json!({ "request": { "method": "session/request_permission", "params": {
        "sessionId": "s1",
        "toolCall": { "toolCallId": "p1", "title": title },
        "options": [
            { "optionId": "yes", "name": "Allow", "kind": "allow_once" },
            { "optionId": "no", "name": "Reject", "kind": "reject_once" },
        ],
    }}})
}

fn writes(path: &Path, content: &str) -> Value {
    json!({ "request": { "method": "fs/write_text_file", "params": {
        "sessionId": "s1", "path": path, "content": content,
    }}})
}

fn reads(path: &Path, line: Option<u32>, limit: Option<u32>) -> Value {
    json!({ "request": { "method": "fs/read_text_file", "params": {
        "sessionId": "s1", "path": path, "line": line, "limit": limit,
    }}})
}

/// Answers to `session/prompt`.
fn on_prompt(actions: Vec<Value>) -> Value {
    json!({ "on": { "session/prompt": actions } })
}

// ── Driving the panel ───────────────────────────────────────────────────

struct Chat {
    _tmp: tempfile::TempDir,
    dir: PathBuf,
    fake: FakeAgent,
    window: WindowHandle<Root>,
    workspace: Entity<Workspace>,
    panel: Entity<AgentPanel>,
}

impl Chat {
    fn new(cx: &mut TestAppContext, script: impl FnOnce(&Path) -> Value) -> Option<Chat> {
        let (tmp, fake, window, workspace) = testing::agent_workspace(cx, script)?;
        let panel = workspace.read_with(cx, |ws, _| ws.agent_panel().clone());
        let dir = crate::paths::canonical(tmp.path());
        Some(Chat {
            _tmp: tmp,
            dir,
            fake,
            window,
            workspace,
            panel,
        })
    }

    fn send(&self, cx: &mut TestAppContext, text: &str) {
        let prompt = Prompt::new(text.to_string(), text.to_string());
        self.panel.update(cx, |a, cx| a.send(prompt, cx));
    }

    fn read<R>(&self, cx: &mut TestAppContext, f: impl FnOnce(&AgentPanel, &App) -> R) -> R {
        self.panel.read_with(cx, |a, cx| f(a, cx))
    }

    fn update<R>(
        &self,
        cx: &mut TestAppContext,
        f: impl FnOnce(&mut AgentPanel, &mut Context<AgentPanel>) -> R,
    ) -> R {
        self.panel.update(cx, f)
    }

    /// Wait until `what` holds of the panel.
    #[track_caller]
    fn wait(&self, cx: &mut TestAppContext, why: &str, what: impl Fn(&AgentPanel, &App) -> bool) {
        let panel = self.panel.clone();
        testing::wait_until(cx, why, |cx| panel.read_with(cx, |a, cx| what(a, cx)));
    }

    /// Wait for the end of the turn, once it started.
    #[track_caller]
    fn wait_turn(&self, cx: &mut TestAppContext) {
        self.wait(cx, "the end of the turn", |a, _| {
            a.agents.is_some() && !a.running && !matches!(a.conn, Conn::Starting)
        });
    }

    fn transcript(&self, cx: &mut TestAppContext) -> Vec<String> {
        self.read(cx, |a, cx| a.transcript(cx))
    }

    /// Show the Agent sidebar and paint the window.
    fn draw(&self, cx: &mut TestAppContext) {
        testing::draw(cx, self.window);
        let shown = self.workspace.read_with(cx, |ws, _| ws.showing_agent());
        if !shown {
            cx.dispatch_action(self.window.into(), crate::ToggleAgent);
        }
        testing::draw(cx, self.window);
    }
}

#[gpui::test]
fn a_turn_shows_what_the_agent_says_and_does(cx: &mut TestAppContext) {
    let Some(chat) = Chat::new(cx, |dir| {
        let main = dir.join("main.tex");
        json!({ "on": {
            "session/new": [{ "respond": {
                "sessionId": "s1",
                "modes": {
                    "currentModeId": "ask",
                    "availableModes": [
                        { "id": "ask", "name": "Ask" },
                        { "id": "code", "name": "Code" },
                    ],
                },
            }}],
            "session/prompt": [[
                update(json!({ "sessionUpdate": "agent_thought_chunk",
                    "content": { "type": "text", "text": "Let me " } })),
                update(json!({ "sessionUpdate": "agent_thought_chunk",
                    "content": { "type": "text", "text": "think." } })),
                says("Hello"),
                says(" world"),
                update(json!({ "sessionUpdate": "plan", "entries": [
                    { "content": "Read", "priority": "high", "status": "in_progress" },
                ]})),
                update(json!({ "sessionUpdate": "plan", "entries": [
                    { "content": "Read", "priority": "high", "status": "completed" },
                    { "content": "Write", "priority": "low", "status": "pending" },
                ]})),
                update(json!({ "sessionUpdate": "tool_call", "toolCallId": "t1",
                    "title": "Read main.tex", "kind": "read", "status": "pending",
                    "locations": [{ "path": main, "line": 3 }] })),
                update(json!({ "sessionUpdate": "tool_call_update", "toolCallId": "t1",
                    "status": "completed",
                    "content": [{ "type": "content",
                        "content": { "type": "text", "text": "\\documentclass" } }] })),
                // An update for a call the agent never announced.
                update(json!({ "sessionUpdate": "tool_call_update", "toolCallId": "t2",
                    "title": "Edit main.tex", "kind": "edit", "status": "completed",
                    "locations": [{ "path": main }],
                    "content": [{ "type": "diff", "path": main,
                        "oldText": "a\nb\n", "newText": "a\nc\nd\n" }] })),
                update(json!({ "sessionUpdate": "current_mode_update",
                    "currentModeId": "code" })),
                // Another session's: not shown.
                json!({ "notify": { "method": "session/update", "params": {
                    "sessionId": "other", "update": {
                        "sessionUpdate": "agent_message_chunk",
                        "content": { "type": "text", "text": "Not mine" } } } } }),
                { "respond": { "stopReason": "max_tokens" } },
            ], [
                says("Again"),
            ]],
        }})
    }) else {
        return;
    };
    chat.send(cx, "Hi");
    chat.wait_turn(cx);
    let transcript = chat.transcript(cx);
    let has = |line: &str| transcript.iter().any(|l| l == line);
    assert!(has("user: Hi"), "{transcript:#?}");
    assert!(has("thought: Let me think."), "{transcript:#?}");
    assert!(has("agent: Hello world"), "{transcript:#?}");
    assert!(has("plan: Read, Write"), "{transcript:#?}");
    assert!(
        has("tool: Read main.tex Read Completed: \\documentclass"),
        "{transcript:#?}"
    );
    assert!(
        has("tool: Edit main.tex Edit Completed: main.tex: +2 −1"),
        "{transcript:#?}"
    );
    assert!(
        has("notice: The agent reached its output limit."),
        "{transcript:#?}"
    );
    assert!(!transcript.iter().any(|l| l.contains("Not mine")));
    assert_eq!(
        transcript.iter().filter(|l| l.starts_with("plan")).count(),
        1
    );
    let mode = chat.read(cx, |a, _| {
        a.live_session()
            .and_then(|(_, s)| s.modes.clone())
            .unwrap()
            .current_mode_id
    });
    assert_eq!(mode.to_string(), "code");
    chat.draw(cx);

    // The first message of a session comes with the project's context, the next ones not.
    chat.update(cx, |a, cx| {
        a.set_mode(acp::SessionModeId::new("ask"), cx);
    });
    chat.send(cx, "And now?");
    chat.wait_turn(cx);
    let prompts = chat.fake.requests("session/prompt");
    assert_eq!(prompts.len(), 2);
    let first = prompts[0]["params"]["prompt"].to_string();
    assert!(first.contains("You are working in TeXoria"), "{first}");
    assert!(first.contains("Hi"), "{first}");
    let second = prompts[1]["params"]["prompt"].to_string();
    assert!(!second.contains("You are working in TeXoria"), "{second}");
    assert_eq!(chat.fake.requests("session/new").len(), 1);
    let set = chat.fake.requests("session/set_mode");
    assert_eq!(set.len(), 1);
    assert_eq!(set[0]["params"]["modeId"], "ask");
    let mode = chat.read(cx, |a, _| {
        a.live_session()
            .and_then(|(_, s)| s.modes.clone())
            .unwrap()
            .current_mode_id
    });
    assert_eq!(mode.to_string(), "ask");
    // Every tool call row, open with its details.
    chat.update(cx, |a, cx| {
        for e in &mut a.entries {
            match e {
                Entry::Tool(t) => t.open = true,
                Entry::Thought { open, .. } => *open = true,
                _ => {}
            }
        }
        cx.notify();
    });
    chat.draw(cx);

    // A new chat starts a new session of the same agent.
    chat.update(cx, |a, cx| a.new_chat(cx));
    assert!(chat.transcript(cx).is_empty());
    chat.send(cx, "Fresh");
    chat.wait_turn(cx);
    assert_eq!(chat.fake.requests("session/new").len(), 2);
    assert_eq!(chat.fake.requests("initialize").len(), 1);
}

#[gpui::test]
fn permissions_are_asked_in_the_chat(cx: &mut TestAppContext) {
    let Some(chat) = Chat::new(cx, |_| {
        on_prompt(vec![asks("Run latexmk"), asks("Delete everything")])
    }) else {
        return;
    };
    chat.send(cx, "Build it");
    chat.wait(cx, "the first request", |a, cx| {
        a.transcript(cx)
            .contains(&"permission: Run latexmk ?".to_string())
    });
    assert!(chat.read(cx, |a, _| a.running));
    chat.draw(cx);
    chat.update(cx, |a, cx| a.permit("Allow", cx));
    chat.wait(cx, "the second request", |a, cx| {
        a.transcript(cx)
            .contains(&"permission: Delete everything ?".to_string())
    });
    chat.update(cx, |a, cx| a.permit("Reject", cx));
    chat.wait_turn(cx);
    let transcript = chat.transcript(cx);
    assert!(transcript.contains(&"permission: Run latexmk -> Allow".to_string()));
    assert!(transcript.contains(&"permission: Delete everything -> Reject".to_string()));
    let outcome = |id| chat.fake.answer(id).unwrap()["result"]["outcome"].clone();
    assert_eq!(
        outcome(1001),
        json!({ "outcome": "selected", "optionId": "yes" })
    );
    assert_eq!(
        outcome(1002),
        json!({ "outcome": "selected", "optionId": "no" })
    );
}

#[gpui::test]
fn stopping_cancels_the_turn_and_its_questions(cx: &mut TestAppContext) {
    let Some(chat) = Chat::new(cx, |_| {
        on_prompt(vec![
            asks("Run latexmk"),
            json!({ "respond": { "stopReason": "cancelled" } }),
        ])
    }) else {
        return;
    };
    chat.send(cx, "Build it");
    chat.wait(cx, "the request", |a, cx| {
        a.transcript(cx)
            .contains(&"permission: Run latexmk ?".to_string())
    });
    chat.update(cx, |a, cx| a.cancel(cx));
    chat.wait_turn(cx);
    let transcript = chat.transcript(cx);
    assert!(transcript.contains(&"permission: Run latexmk -> Cancelled".to_string()));
    assert!(
        transcript.contains(&"notice: Stopped.".to_string()),
        "{transcript:#?}"
    );
    assert_eq!(
        chat.fake.answer(1001).unwrap()["result"]["outcome"],
        json!({ "outcome": "cancelled" })
    );
    assert_eq!(chat.fake.requests("session/cancel").len(), 1);
    // Nothing to stop any more.
    chat.update(cx, |a, cx| a.cancel(cx));
    assert_eq!(chat.fake.requests("session/cancel").len(), 1);
}

#[gpui::test]
fn files_are_read_and_written_only_inside_the_project(cx: &mut TestAppContext) {
    let outside = tempfile::tempdir().unwrap();
    let theirs = crate::paths::canonical(outside.path()).join("theirs.tex");
    std::fs::write(&theirs, "secret").unwrap();
    let elsewhere = theirs.clone();
    let Some(chat) = Chat::new(cx, move |dir| {
        std::fs::create_dir(dir.join(".git")).unwrap();
        std::fs::write(dir.join(".git/config"), "[core]\n").unwrap();
        on_prompt(vec![
            reads(&dir.join("main.tex"), Some(1), Some(1)),
            reads(&elsewhere, None, None),
            reads(&dir.join(".git/config"), None, None),
            writes(&dir.join("chapters/new/notes.tex"), "Notes\n"),
            writes(&dir.join("latexmkrc"), "system('x');\n"),
            writes(&elsewhere, "mine"),
            writes(&dir.join(".git/config"), "[core]\n\tfsmonitor = x\n"),
            json!({ "request": { "method": "terminal/create", "params": {
                "sessionId": "s1", "command": "ls" } } }),
        ])
    }) else {
        return;
    };
    let main = std::fs::read_to_string(chat.dir.join("main.tex")).unwrap();
    chat.send(cx, "Go");
    chat.wait_turn(cx);
    let answer = |id| chat.fake.answer(id).unwrap();
    let first_line = main.split_inclusive('\n').next().unwrap();
    assert_eq!(answer(1001)["result"]["content"], first_line);
    for id in [1002, 1003, 1005, 1006, 1007, 1008] {
        assert!(answer(id).get("error").is_some(), "{id}: {}", answer(id));
    }
    assert!(answer(1004).get("result").is_some(), "{}", answer(1004));
    assert_eq!(
        std::fs::read_to_string(chat.dir.join("chapters/new/notes.tex")).unwrap(),
        "Notes\n"
    );
    assert!(!chat.dir.join("latexmkrc").exists());
    assert_eq!(std::fs::read_to_string(&theirs).unwrap(), "secret");
    assert_eq!(
        std::fs::read_to_string(chat.dir.join(".git/config")).unwrap(),
        "[core]\n"
    );
    assert!(
        answer(1005)["error"]["message"]
            .to_string()
            .contains("latexmkrc")
    );
    // The review lists what changed.
    chat.wait(cx, "the review", |a, _| !a.changes.is_empty());
    assert_eq!(
        chat.read(cx, |a, _| a.changed()),
        [PathBuf::from("chapters/new/notes.tex")]
    );
    chat.draw(cx);
}

#[gpui::test]
fn undo_and_keep_follow_the_checkpoint(cx: &mut TestAppContext) {
    let Some(chat) = Chat::new(cx, |dir| {
        on_prompt(vec![
            writes(
                &dir.join("main.tex"),
                "\\documentclass{article}\nRewritten.\n",
            ),
            writes(&dir.join("added.tex"), "New\n"),
            asks("Finish"),
        ])
    }) else {
        return;
    };
    let main = chat.dir.join("main.tex");
    let methods = chat.dir.join("chapters/methods.tex");
    let (before_main, before_methods) = (
        std::fs::read_to_string(&main).unwrap(),
        std::fs::read_to_string(&methods).unwrap(),
    );
    chat.send(cx, "Rewrite");
    chat.wait(cx, "the question", |a, cx| {
        a.transcript(cx).iter().any(|l| l.ends_with('?'))
    });
    // What the agent writes on its own is found too, and Git's configuration is watched.
    std::fs::write(&methods, "Changed behind TeXoria's back.\n").unwrap();
    std::fs::create_dir_all(chat.dir.join(".git/hooks")).unwrap();
    std::fs::write(chat.dir.join(".git/hooks/pre-commit"), "#!/bin/sh\n").unwrap();
    chat.update(cx, |a, cx| a.permit("Allow", cx));
    chat.wait_turn(cx);
    chat.wait(cx, "the review", |a, _| a.changes.len() == 3);
    let mut changed = chat.read(cx, |a, _| a.changed());
    changed.sort();
    let expected: Vec<PathBuf> = ["added.tex", "chapters/methods.tex", "main.tex"]
        .iter()
        .map(PathBuf::from)
        .collect();
    assert_eq!(changed, expected);
    let transcript = chat.transcript(cx);
    assert!(
        transcript
            .iter()
            .any(|l| l.starts_with("error: The agent changed Git") && l.contains("pre-commit")),
        "{transcript:#?}"
    );
    chat.draw(cx);

    // One file back.
    chat.update(cx, |a, cx| {
        a.undo(Some(vec![PathBuf::from("main.tex")]), cx)
    });
    chat.wait(cx, "two changes left", |a, _| a.changes.len() == 2);
    assert_eq!(std::fs::read_to_string(&main).unwrap(), before_main);
    assert!(chat.dir.join("added.tex").exists());
    // The rest.
    chat.update(cx, |a, cx| a.undo(None, cx));
    chat.wait(cx, "no change left", |a, _| a.changes.is_empty());
    assert!(!chat.dir.join("added.tex").exists());
    assert_eq!(std::fs::read_to_string(&methods).unwrap(), before_methods);

    // Changes kept are no longer offered, and not undone.
    std::fs::write(&methods, "Kept.\n").unwrap();
    chat.update(cx, |a, cx| a.refresh_changes(false, cx));
    chat.wait(cx, "the new change", |a, _| a.changes.len() == 1);
    chat.update(cx, |a, cx| a.keep(cx));
    assert!(chat.read(cx, |a, _| a.changes.is_empty()));
    chat.update(cx, |a, cx| a.refresh_changes(false, cx));
    cx.run_until_parked();
    assert!(chat.read(cx, |a, _| a.changes.is_empty()));
    chat.update(cx, |a, cx| a.undo(None, cx));
    cx.run_until_parked();
    assert_eq!(std::fs::read_to_string(&methods).unwrap(), "Kept.\n");
}

#[gpui::test]
fn an_agent_that_crashes_mid_turn_is_reported(cx: &mut TestAppContext) {
    let Some(chat) = Chat::new(cx, |_| {
        on_prompt(vec![
            says("Starting"),
            json!({ "stderr": "panicked at the disco\n" }),
            json!({ "exit": 3 }),
        ])
    }) else {
        return;
    };
    chat.send(cx, "Go");
    chat.wait(cx, "the failure", |a, _| {
        a.failure().is_some() && !a.running
    });
    let (message, details) = chat.read(cx, |a, _| a.failure()).unwrap();
    assert_eq!(message, "Fake stopped.");
    assert_eq!(details.as_deref(), Some("panicked at the disco"));
    // The closed connection is not reported twice.
    let transcript = chat.transcript(cx);
    assert!(
        !transcript.iter().any(|l| l.starts_with("error")),
        "{transcript:#?}"
    );
    chat.draw(cx);
    // A new chat starts it again.
    chat.update(cx, |a, cx| a.new_chat(cx));
    assert!(chat.read(cx, |a, _| a.failure().is_none() && a.can_send()));
}

#[gpui::test]
fn agents_that_cannot_start_say_why(cx: &mut TestAppContext) {
    // `initialize` fails, with something on stderr.
    let Some(chat) = Chat::new(cx, |_| {
        json!({
            "start": [{ "stderr": "no API key\n" }],
            "on": { "initialize": [{ "respond_error": { "code": -32603, "message": "boom" } }] },
        })
    }) else {
        return;
    };
    chat.send(cx, "Go");
    chat.wait_turn(cx);
    let (message, details) = chat.read(cx, |a, _| a.failure()).unwrap();
    assert!(message.starts_with("Fake did not start: "), "{message}");
    assert!(details.unwrap().contains("no API key"));
    chat.draw(cx);

    // A session cannot be created.
    let Some(chat) = Chat::new(cx, |_| {
        json!({ "on": {
            "session/new": [{ "respond_error": { "code": -32603, "message": "full" } }],
        }})
    }) else {
        return;
    };
    chat.send(cx, "Go");
    chat.wait_turn(cx);
    let (message, _) = chat.read(cx, |a, _| a.failure()).unwrap();
    assert!(
        message.starts_with("Fake could not start a session: "),
        "{message}"
    );

    // The program is gone.
    let Some(chat) = Chat::new(cx, |_| json!({})) else {
        return;
    };
    chat.update(cx, |a, cx| {
        let mut custom = a.custom.clone();
        custom[0].command = chat
            .dir
            .join("missing-agent")
            .to_string_lossy()
            .into_owned();
        a.configure(Some(FakeAgent::ID.into()), custom, cx);
    });
    chat.send(cx, "Go");
    chat.wait_turn(cx);
    let (message, details) = chat.read(cx, |a, _| a.failure()).unwrap();
    assert!(
        message.starts_with("Fake could not be started: "),
        "{message}"
    );
    assert!(details.unwrap().contains("missing-agent"));
}

#[gpui::test]
fn an_agent_that_never_answers_times_out(cx: &mut TestAppContext) {
    let Some(chat) = Chat::new(
        cx,
        |_| json!({ "on": { "initialize": [{ "sleep_ms": 20000 }] } }),
    ) else {
        return;
    };
    chat.send(cx, "Go");
    chat.wait(cx, "the start", |a, _| matches!(a.conn, Conn::Starting));
    chat.draw(cx);
    cx.executor()
        .advance_clock(START_TIMEOUT + Duration::from_secs(1));
    chat.wait_turn(cx);
    let (message, _) = chat.read(cx, |a, _| a.failure()).unwrap();
    assert_eq!(message, "Fake did not answer.");
}

#[gpui::test]
fn stopping_a_starting_agent_gives_up_on_it(cx: &mut TestAppContext) {
    let Some(chat) = Chat::new(
        cx,
        |_| json!({ "on": { "initialize": [{ "sleep_ms": 20000 }] } }),
    ) else {
        return;
    };
    chat.send(cx, "Go");
    chat.wait(cx, "the start", |a, _| matches!(a.conn, Conn::Starting));
    chat.update(cx, |a, cx| a.cancel(cx));
    assert!(chat.read(cx, |a, _| !a.running && matches!(a.conn, Conn::Idle)));
    let busy = chat.workspace.read_with(cx, |ws, _| ws.agent_is_busy());
    assert!(!busy);
}

#[gpui::test]
fn signs_in_when_the_agent_asks_then_sends_again(cx: &mut TestAppContext) {
    let Some(chat) = Chat::new(cx, |_| {
        json!({ "on": {
            "initialize": [{ "respond": {
                "protocolVersion": 1,
                "authMethods": [{ "id": "login", "name": "Log in", "description": "In a browser" }],
            }}],
            "session/new": [
                [{ "respond_error": { "code": -32000, "message": "Authentication required" } }],
                [],
            ],
            "authenticate": [
                [{ "respond_error": { "code": -32603, "message": "denied" } }],
                [],
            ],
            "session/prompt": [says("Signed in")],
        }})
    }) else {
        return;
    };
    chat.send(cx, "Hello");
    chat.wait_turn(cx);
    assert!(chat.read(cx, |a, _| a.auth.is_some() && a.pending.is_some()));
    chat.draw(cx);
    // Refused the first time.
    chat.update(cx, |a, cx| a.sign_in(acp::AuthMethodId::new("login"), cx));
    chat.wait(cx, "the refusal", |a, _| !a.signing_in);
    let transcript = chat.transcript(cx);
    assert!(
        transcript
            .iter()
            .any(|l| l.starts_with("error: Signing in failed")),
        "{transcript:#?}"
    );
    chat.update(cx, |a, cx| a.sign_in(acp::AuthMethodId::new("login"), cx));
    chat.wait(cx, "the answer", |a, cx| {
        a.transcript(cx).iter().any(|l| l == "agent: Signed in") && !a.running
    });
    let transcript = chat.transcript(cx);
    assert_eq!(
        transcript
            .iter()
            .filter(|l| l.as_str() == "user: Hello")
            .count(),
        1,
        "{transcript:#?}"
    );
    assert!(chat.read(cx, |a, _| a.auth.is_none()));
    assert_eq!(chat.fake.requests("authenticate").len(), 2);
}

#[gpui::test]
fn messages_wait_for_the_agents_and_the_users_consent(cx: &mut TestAppContext) {
    let Some(chat) = Chat::new(cx, |_| on_prompt(vec![says("Hi")])) else {
        return;
    };
    // Not allowed in the project: the message waits.
    let (dir, root) = (chat.dir.clone(), chat.dir.join("main.tex"));
    chat.update(cx, |a, cx| {
        a.set_project(Some(dir.clone()), Some(root.clone()), false, cx)
    });
    chat.send(cx, "Queued");
    cx.run_until_parked();
    assert!(chat.read(cx, |a, _| a.queued.is_some() && !a.running));
    assert!(chat.read(cx, |a, _| a.current().is_some()));
    chat.draw(cx);
    chat.update(cx, |a, cx| {
        a.set_project(Some(dir.clone()), Some(root.clone()), true, cx)
    });
    chat.wait_turn(cx);
    assert!(chat.transcript(cx).contains(&"user: Queued".to_string()));

    // Another project: the chat starts over.
    let other = chat.dir.join("chapters");
    chat.update(cx, |a, cx| a.set_project(Some(other), None, true, cx));
    assert!(chat.transcript(cx).is_empty());
    // No agent at all.
    chat.update(cx, |a, _| a.find_agents_with(|_| Vec::new()));
    chat.update(cx, |a, cx| a.detect(cx));
    cx.run_until_parked();
    assert!(chat.read(cx, |a, _| a.agents().is_some_and(<[_]>::is_empty)
        && !a.can_send()));
    chat.draw(cx);
    // No project.
    chat.update(cx, |a, cx| a.set_project(None, None, false, cx));
    chat.draw(cx);
}

#[test]
fn rc_files_are_refused_under_any_spelling() {
    let dir = tempfile::tempdir().unwrap();
    let d = std::fs::canonicalize(dir.path()).unwrap();
    // Windows drops trailing dots and spaces, and `name:stream` writes to `name`.
    for name in [
        "latexmkrc.",
        "latexmkrc ",
        "LatexMkRc. .",
        "latexmkrc::$DATA",
        ".latexmkrc:x",
    ] {
        assert!(writable(&d, &d.join(name)).is_err(), "{name:?}");
    }
    assert!(writable(&d, &d.join("latexmkrc.tex")).is_ok());
}

#[cfg(unix)]
#[test]
fn rc_files_are_refused_through_a_link() {
    let dir = tempfile::tempdir().unwrap();
    let d = std::fs::canonicalize(dir.path()).unwrap();
    std::fs::write(d.join("latexmkrc"), "").unwrap();
    std::os::unix::fs::symlink(d.join("latexmkrc"), d.join("notes.tex")).unwrap();
    assert!(writable(&d, &d.join("notes.tex")).is_err());
}

#[cfg(unix)]
#[test]
fn undo_removes_added_files_only_inside_the_project() {
    use super::remove_added;
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let d = std::fs::canonicalize(dir.path()).unwrap();
    std::fs::write(outside.path().join("new.tex"), "theirs").unwrap();
    // The folder of an added file replaced by a link that leads out of the project.
    std::os::unix::fs::symlink(outside.path(), d.join("ch")).unwrap();
    assert!(remove_added(&d, &d.join("ch/new.tex")).is_err());
    assert!(outside.path().join("new.tex").exists());

    // An added link is removed itself, never what it points to.
    std::os::unix::fs::symlink(outside.path().join("new.tex"), d.join("link.tex")).unwrap();
    remove_added(&d, &d.join("link.tex")).unwrap();
    assert!(outside.path().join("new.tex").exists());
    std::fs::write(d.join("added.tex"), "").unwrap();
    remove_added(&d, &d.join("added.tex")).unwrap();
    assert!(!d.join("added.tex").exists());
}

#[test]
fn reads_only_project_text_and_slices_lines() {
    let dir = tempfile::tempdir().unwrap();
    let d = std::fs::canonicalize(dir.path()).unwrap();
    std::fs::write(d.join("main.tex"), "one\ntwo\nthree\n").unwrap();
    let main = d.join("main.tex");
    assert_eq!(
        read_text(&d, &main, None, None).unwrap(),
        "one\ntwo\nthree\n"
    );
    assert_eq!(read_text(&d, &main, Some(2), None).unwrap(), "two\nthree\n");
    assert_eq!(read_text(&d, &main, Some(2), Some(1)).unwrap(), "two\n");
    // Lines count from 1; 0 reads from the start too.
    assert_eq!(read_text(&d, &main, Some(0), Some(1)).unwrap(), "one\n");
    assert_eq!(read_text(&d, &main, Some(9), None).unwrap(), "");
    assert!(read_text(&d, &d.join("missing.tex"), None, None).is_err());
    assert!(read_text(&d, &d, None, None).is_err(), "a folder");
    std::fs::create_dir(d.join(".git")).unwrap();
    std::fs::write(d.join(".git/config"), "secret").unwrap();
    assert!(read_text(&d, &d.join(".git/config"), None, None).is_err());
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("x.tex"), "theirs").unwrap();
    assert!(read_text(&d, &outside.path().join("x.tex"), None, None).is_err());
    // Not UTF-8: read anyway, with replacement characters.
    std::fs::write(d.join("latin1.tex"), b"caf\xe9").unwrap();
    assert_eq!(
        read_text(&d, &d.join("latin1.tex"), None, None).unwrap(),
        "caf\u{fffd}"
    );
}

#[test]
fn large_files_are_not_read() {
    let dir = tempfile::tempdir().unwrap();
    let d = std::fs::canonicalize(dir.path()).unwrap();
    let big = std::fs::File::create(d.join("big.tex")).unwrap();
    big.set_len(tex_project::files::MAX_TEXT_BYTES + 1).unwrap();
    let e = read_text(&d, &d.join("big.tex"), None, None).unwrap_err();
    assert!(e.contains("too large"), "{e}");
}

#[test]
fn writes_stay_in_the_project() {
    let dir = tempfile::tempdir().unwrap();
    let d = std::fs::canonicalize(dir.path()).unwrap();
    std::fs::create_dir(d.join("ch")).unwrap();
    assert!(inside(&d, &d.join("main.tex")));
    assert!(inside(&d, &d.join("ch/new/intro.tex")));
    assert!(!inside(&d, &d.join("../elsewhere.tex")));
    assert!(!inside(&d, &d.join("ch/../../x.tex")));
    assert!(!inside(&d, &d.join(".git/config")));
    assert!(!inside(&d, &d.join(".GIT/hooks/pre-commit")));
    assert!(!inside(&d, &d.join("ch/.Git/config")));
    assert!(!inside(&d, std::path::Path::new("main.tex")));
    assert!(writable(&d, &d.join("ch/intro.tex")).is_ok());
    assert!(writable(&d, &d.join("latexmkrc")).is_err());
    assert!(writable(&d, &d.join("ch/.LatexMkRc")).is_err());
}

#[cfg(unix)]
#[test]
fn undo_replaces_links_instead_of_following_them() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let d = std::fs::canonicalize(dir.path()).unwrap();
    let target = outside.path().join("victim.txt");
    std::fs::write(&target, "theirs").unwrap();
    std::os::unix::fs::symlink(&target, d.join("main.tex")).unwrap();
    super::restore(&d, &d.join("main.tex"), b"mine").unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), b"theirs");
    assert_eq!(std::fs::read(d.join("main.tex")).unwrap(), b"mine");
    assert!(!d.join("main.tex").symlink_metadata().unwrap().is_symlink());
}

#[test]
fn tool_details_are_short() {
    let text = acp::ToolCallContent::Content(acp::Content::new(acp::ContentBlock::Text(
        acp::TextContent::new("x".repeat(5000)),
    )));
    let d = detail(&[text]).unwrap();
    assert!(d.chars().count() <= super::DETAIL_CHARS + 1);
    let diff = acp::ToolCallContent::Diff(
        acp::Diff::new("/p/main.tex", "a\nc\n").old_text(Some("a\nb\n".to_string())),
    );
    assert_eq!(detail(&[diff]).unwrap(), "main.tex: +1 −1");
    assert_eq!(detail(&[]), None);
}
