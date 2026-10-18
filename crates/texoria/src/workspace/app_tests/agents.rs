//! The workspace and the Agent panel together, with tex-agent's fake agent: editing paused
//! during a turn, files saved before and reloaded after, the review tab and its undo, and the
//! editor's agent commands.

use serde_json::{Value, json};
use tex_agent::prompt::Rewrite;

use super::*;
use crate::testing::FakeAgent;
use core::prelude::v1::test;
use gpui_kit::gpui;

/// The sample project with the fake agent playing `script(dir)`.
fn with_agent(
    cx: &mut TestAppContext,
    script: impl FnOnce(&Path) -> Value,
) -> Option<(Open, FakeAgent)> {
    let (tmp, fake, window, ws) = testing::agent_workspace(cx, script)?;
    let dir = paths::canonical(tmp.path());
    let app = Open {
        _tmp: Some(tmp),
        dir,
        window,
        ws,
    };
    Some((app, fake))
}

fn on_prompt(actions: Vec<Value>) -> Value {
    json!({ "on": { "session/prompt": actions } })
}

fn writes(path: &Path, content: &str) -> Value {
    json!({ "request": { "method": "fs/write_text_file", "params": {
        "sessionId": "s1", "path": path, "content": content,
    }}})
}

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

fn update(update: Value) -> Value {
    json!({ "notify": { "method": "session/update", "params": {
        "sessionId": "s1", "update": update,
    }}})
}

impl Open {
    fn panel(&self, cx: &mut TestAppContext) -> Entity<AgentPanel> {
        self.read(cx, |ws, _| ws.agent.clone())
    }

    fn transcript(&self, cx: &mut TestAppContext) -> Vec<String> {
        let panel = self.panel(cx);
        panel.read_with(cx, |a, cx| a.transcript(cx))
    }

    /// What the panel asks the workspace for, as its buttons and composer do.
    fn request(&self, cx: &mut TestAppContext, event: AgentPanelEvent) {
        let panel = self.panel(cx);
        panel.update(cx, |_, cx| cx.emit(event));
        cx.run_until_parked();
    }

    #[track_caller]
    fn wait_for(&self, cx: &mut TestAppContext, why: &str, f: impl Fn(&AgentPanel, &App) -> bool) {
        let panel = self.panel(cx);
        testing::wait_until(cx, why, |cx| panel.read_with(cx, |a, cx| f(a, cx)));
    }

    /// Wait until the agent has answered `n` prompts and is done.
    #[track_caller]
    fn wait_turns(&self, cx: &mut TestAppContext, fake: &FakeAgent, n: usize) {
        let panel = self.panel(cx);
        testing::wait_until(cx, "the end of the turn", |cx| {
            fake.requests("session/prompt").len() >= n
                && !panel.read_with(cx, |a, _| a.is_running())
        });
    }

    fn readonly(&self, cx: &mut TestAppContext) -> Vec<bool> {
        self.read(cx, |ws, cx| {
            (ws.buffers.iter())
                .map(|b| !b.editor.read(cx).is_editable())
                .collect()
        })
    }

    /// The text of the last prompt the agent got.
    fn last_prompt(&self, fake: &FakeAgent) -> String {
        let prompts = fake.requests("session/prompt");
        prompts.last().expect("a prompt")["params"]["prompt"].to_string()
    }
}

#[gpui::test]
fn a_turn_pauses_editing_and_its_review_can_be_undone(cx: &mut TestAppContext) {
    let new = "\\documentclass{article}\n\\begin{document}\nRewritten.\n\\end{document}\n";
    let Some((app, fake)) = with_agent(cx, |dir| {
        let main = dir.join("main.tex");
        on_prompt(vec![
            update(json!({ "sessionUpdate": "tool_call", "toolCallId": "t1",
                "title": "Edit main.tex", "kind": "edit", "status": "in_progress" })),
            writes(&main, new),
            update(
                json!({ "sessionUpdate": "tool_call_update", "toolCallId": "t1",
                "status": "completed" }),
            ),
            asks("Build"),
        ])
    }) else {
        return;
    };
    let main = app.path("main.tex");
    let methods = app.path("chapters/methods.tex");
    app.update(cx, |ws, window, cx| {
        ws.activate_file(methods.clone(), window, cx);
        ws.activate_file(main.clone(), window, cx);
    });
    // Unsaved: saved before the agent reads the files.
    app.type_at(cx, &main, 0, "% mine\n");
    let mine = app.text(cx, &main);
    app.select(cx, &main, 0..6);
    app.request(cx, AgentPanelEvent::Request(AgentOp::Chat("Fix it".into())));
    app.wait_for(cx, "the question", |a, cx| {
        a.transcript(cx).iter().any(|l| l.ends_with(" ?"))
    });
    // The prompt has the open file and its selection.
    let prompt = app.last_prompt(&fake);
    assert!(prompt.contains("resource_link"), "{prompt}");
    assert!(prompt.contains(&file_uri(&main)), "{prompt}");
    assert!(
        prompt.contains("Selected in `main.tex`, line 1"),
        "{prompt}"
    );
    assert!(
        app.transcript(cx)
            .contains(&"user: Fix it (With the selection in main.tex, line 1)".to_string())
    );
    // Editing is paused, and the editor shows what the agent wrote.
    assert!(app.read(cx, |ws, _| ws.agent_busy));
    assert_eq!(app.readonly(cx), [true, true]);
    assert_eq!(app.text(cx, &main), new);
    app.draw(cx);
    // What would change the files meanwhile waits.
    let notes = app.notifications(cx);
    app.update(cx, |ws, window, cx| {
        let query = tex_project::search::Query {
            text: "a".into(),
            case_sensitive: true,
            whole_word: false,
            regex: false,
        };
        ws.confirm_replace_all(query, "b".into(), vec![main.clone()], false, window, cx);
        ws.restart_to_update(window, cx);
        ws.format_document(window, cx);
        ws.toggle_comment(window, cx);
        ws.cite_at_cursor("knuth1984", window, cx);
    });
    cx.run_until_parked();
    assert_eq!(app.notifications(cx), notes + 2);
    assert_eq!(app.text(cx, &main), new);
    // The banner's Stop is the StopAgent action; answering lets the turn end instead.
    let panel = app.panel(cx);
    panel.update(cx, |a, cx| a.permit("Allow", cx));
    app.wait_turns(cx, &fake, 1);
    assert!(!app.read(cx, |ws, _| ws.agent_busy));
    assert_eq!(app.readonly(cx), [false, false]);

    // The review opens in a tab of its own.
    panel.update(cx, |a, cx| a.review(true, cx));
    cx.run_until_parked();
    let diff = app.read(cx, |ws, _| ws.diff.clone()).expect("the review");
    assert!(diff.read_with(cx, |d, _| d.is_agent()));
    assert!(app.read(cx, |ws, _| ws.diff_active));
    app.draw(cx);
    // A chat message from the review has no file attached.
    let bare = app.read(cx, |ws, cx| ws.chat_prompt("Hello".into(), cx));
    assert_eq!(bare.blocks.len(), 1);
    // Undone from the review: the file is back as saved before the turn.
    diff.update(cx, |_, cx| {
        cx.emit(DiffViewEvent::UndoAgent(Some(PathBuf::from("main.tex"))))
    });
    cx.run_until_parked();
    app.wait_for(cx, "no change left", |a, _| a.changed().is_empty());
    assert_eq!(std::fs::read_to_string(&main).unwrap(), mine);
    assert_eq!(app.text(cx, &main), mine);
    // Nothing left to review: the tab closes.
    assert!(app.read(cx, |ws, _| ws.diff.is_none() && !ws.diff_active));
}

#[gpui::test]
fn the_review_follows_keep_and_undo_all(cx: &mut TestAppContext) {
    let Some((app, fake)) = with_agent(cx, |dir| {
        on_prompt(vec![
            writes(&dir.join("main.tex"), "One.\n"),
            writes(&dir.join("added.tex"), "Two.\n"),
        ])
    }) else {
        return;
    };
    app.request(
        cx,
        AgentPanelEvent::Request(AgentOp::Send(Prompt::new("Go", "Go"))),
    );
    app.wait_turns(cx, &fake, 1);
    app.wait_for(cx, "the changes", |a, _| a.changed().len() == 2);
    // Not shown until asked for.
    assert!(app.read(cx, |ws, _| ws.diff.is_none()));
    let panel = app.panel(cx);
    panel.update(cx, |a, cx| a.review(true, cx));
    cx.run_until_parked();
    let diff = app.read(cx, |ws, _| ws.diff.clone()).unwrap();
    // A refresh keeps the tab where it is.
    app.update(cx, |ws, _, cx| {
        ws.diff_active = false;
        cx.notify()
    });
    panel.update(cx, |a, cx| a.review(false, cx));
    cx.run_until_parked();
    assert!(app.read(cx, |ws, _| ws.diff.is_some() && !ws.diff_active));
    // Keep: the review goes.
    diff.update(cx, |_, cx| cx.emit(DiffViewEvent::KeepAgent));
    cx.run_until_parked();
    assert!(app.read(cx, |ws, _| ws.diff.is_none()));
    assert!(panel.read_with(cx, |a, _| a.changed().is_empty()));
    assert_eq!(
        std::fs::read_to_string(app.path("main.tex")).unwrap(),
        "One.\n"
    );

    // Another turn, undone as a whole.
    let before = std::fs::read_to_string(app.path("main.tex")).unwrap();
    app.request(
        cx,
        AgentPanelEvent::Request(AgentOp::Send(Prompt::new("Go", "Go"))),
    );
    app.wait_turns(cx, &fake, 2);
    std::fs::write(app.path("main.tex"), "Three.\n").unwrap();
    panel.update(cx, |a, cx| a.review(true, cx));
    cx.run_until_parked();
    let diff = app.read(cx, |ws, _| ws.diff.clone()).unwrap();
    diff.update(cx, |_, cx| cx.emit(DiffViewEvent::UndoAgent(None)));
    cx.run_until_parked();
    app.wait_for(cx, "no change left", |a, _| a.changed().is_empty());
    assert_eq!(
        std::fs::read_to_string(app.path("main.tex")).unwrap(),
        before
    );
    assert_eq!(app.text(cx, &app.path("main.tex")), before);
}

#[gpui::test]
fn rewriting_the_selection_sends_the_passage(cx: &mut TestAppContext) {
    let Some((app, fake)) = with_agent(cx, |_| json!({})) else {
        return;
    };
    let main = app.path("main.tex");
    // Nothing selected: nothing to rewrite.
    let notes = app.notifications(cx);
    app.dispatch(
        cx,
        RewriteSelection {
            how: Rewrite::Shorten,
        },
    );
    assert_eq!(app.notifications(cx), notes + 1);
    assert!(fake.requests("session/prompt").is_empty());

    let text = app.text(cx, &main);
    let start = text.find("Hello from TeXoria").unwrap();
    let end = text[start..].find('\n').unwrap() + start + 1;
    app.select(cx, &main, start..end);
    // The first use: the agents are looked for, then the passage is sent.
    app.dispatch(
        cx,
        RewriteSelection {
            how: Rewrite::Shorten,
        },
    );
    app.wait_turns(cx, &fake, 1);
    assert!(app.read(cx, |ws, _| ws.showing_agent()));
    let prompt = app.last_prompt(&fake);
    assert!(
        prompt.contains("This passage is at line 17 of `main.tex`"),
        "{prompt}"
    );
    assert!(prompt.contains("Hello from TeXoria."), "{prompt}");
    assert!(
        app.transcript(cx)
            .contains(&"user: Make Shorter — main.tex, line 17".to_string()),
        "{:?}",
        app.transcript(cx)
    );

    // With instructions, asked for in a dialog.
    app.dispatch(cx, EditWithAgent);
    assert!(app.has_dialog(cx));
    app.fill_and_confirm(cx, "Translate into Italian");
    assert!(!app.has_dialog(cx));
    app.wait_turns(cx, &fake, 2);
    let prompt = app.last_prompt(&fake);
    assert!(prompt.contains("Translate into Italian"), "{prompt}");
    assert!(
        app.transcript(cx)
            .contains(&"user: Translate into Italian (main.tex, line 17)".to_string()),
        "{:?}",
        app.transcript(cx)
    );

    // Nothing selected: no dialog.
    app.select(cx, &main, 0..0);
    let notes = app.notifications(cx);
    app.dispatch(cx, EditWithAgent);
    assert!(!app.has_dialog(cx));
    assert_eq!(app.notifications(cx), notes + 1);
}

#[gpui::test]
fn the_agent_is_asked_to_fix_the_problems(cx: &mut TestAppContext) {
    let Some((app, fake)) = with_agent(cx, |_| json!({})) else {
        return;
    };
    let main = app.path("main.tex");
    // No problem: nothing to ask.
    app.dispatch(cx, FixWithAgent);
    assert!(fake.requests("session/prompt").is_empty());

    let diagnostic = |severity, message: &str| Diagnostic {
        severity,
        file: Some(main.clone()),
        line: Some(3),
        message: message.into(),
    };
    app.update(cx, |ws, _, _| {
        ws.diagnostics = vec![
            diagnostic(Severity::Warning, "Overfull \\hbox"),
            diagnostic(Severity::Warning, "Underfull \\hbox"),
            diagnostic(Severity::Info, "Loaded"),
        ]
    });
    app.request(cx, AgentPanelEvent::FixErrors);
    app.wait_turns(cx, &fake, 1);
    assert!(
        app.transcript(cx)
            .contains(&"user: Fix the 2 warnings".to_string())
    );
    let prompt = app.last_prompt(&fake);
    assert!(prompt.contains("builds with warnings"), "{prompt}");
    assert!(
        prompt.contains("main.tex:3: warning: Overfull \\\\hbox"),
        "{prompt}"
    );

    app.update(cx, |ws, _, _| {
        ws.diagnostics
            .push(diagnostic(Severity::Error, "Undefined control sequence"))
    });
    app.dispatch(cx, FixWithAgent);
    app.wait_turns(cx, &fake, 2);
    assert!(
        app.transcript(cx)
            .contains(&"user: Fix the 1 build error".to_string())
    );
    let prompt = app.last_prompt(&fake);
    assert!(prompt.contains("fails to build"), "{prompt}");
    assert!(!prompt.contains("Overfull"), "{prompt}");
    app.update(cx, |ws, _, cx| {
        ws.show_diagnostics = true;
        cx.notify()
    });
    app.draw(cx);
}

#[gpui::test]
fn the_panels_choices_reach_the_settings(cx: &mut TestAppContext) {
    let Some(fake) = FakeAgent::new(json!({})) else {
        return;
    };
    let settings = tempfile::tempdir().unwrap();
    let mut startup = saved_in(settings.path());
    startup.settings.custom_agents = vec![fake.custom.clone()];
    testing::allow_threads(cx);
    let app = Open::sample(cx, startup);
    let panel = app.panel(cx);
    panel.update(cx, |a, _| a.find_agents_with(testing::custom_agents_only));
    // Not allowed in the project yet.
    app.dispatch(cx, crate::ToggleAgent);
    assert!(app.read(cx, |ws, _| ws.showing_agent()));
    app.draw(cx);
    app.request(cx, AgentPanelEvent::Allow);
    let dir = app.dir.clone();
    assert!(app.read(cx, |ws, _| ws.settings.allows_agents(&dir)));
    app.request(cx, AgentPanelEvent::Choose(FakeAgent::ID.into()));
    let saved = Settings::load_from(settings.path().join("settings.toml"));
    assert_eq!(saved.agent.as_deref(), Some(FakeAgent::ID));
    assert_eq!(saved.agent_projects, std::slice::from_ref(&dir));
    app.draw(cx);
    // Toggled away.
    app.dispatch(cx, crate::ToggleAgent);
    assert!(!app.read(cx, |ws, _| ws.showing_agent()));

    // A tool call's location opens its file there.
    let methods = app.path("chapters/methods.tex");
    app.request(
        cx,
        AgentPanelEvent::Open {
            path: methods.clone(),
            line: Some(3),
        },
    );
    assert_eq!(app.active(cx), "chapters/methods.tex");
    let line = app
        .editor(cx, &methods)
        .read_with(cx, |e, _| e.cursor_position().line);
    assert_eq!(line, 2);
    app.request(
        cx,
        AgentPanelEvent::Open {
            path: app.path("main.tex"),
            line: None,
        },
    );
    assert_eq!(app.active(cx), "main.tex");
}

#[gpui::test]
fn stop_agent_cancels_the_turn(cx: &mut TestAppContext) {
    let Some((app, fake)) = with_agent(cx, |_| {
        on_prompt(vec![
            asks("Build"),
            json!({ "respond": { "stopReason": "cancelled" } }),
        ])
    }) else {
        return;
    };
    app.request(
        cx,
        AgentPanelEvent::Request(AgentOp::Send(Prompt::new("Go", "Go"))),
    );
    app.wait_for(cx, "the question", |a, cx| {
        a.transcript(cx).iter().any(|l| l.ends_with(" ?"))
    });
    app.dispatch(cx, StopAgent);
    app.wait_turns(cx, &fake, 1);
    assert!(app.transcript(cx).contains(&"notice: Stopped.".to_string()));
    assert_eq!(fake.requests("session/cancel").len(), 1);
    assert_eq!(app.readonly(cx), [false]);
}

#[gpui::test]
fn editing_stays_paused_on_screen(cx: &mut TestAppContext) {
    let app = Open::sample(cx, quiet());
    let main = app.path("main.tex");
    app.update(cx, |ws, window, cx| ws.set_agent_busy(true, window, cx));
    // Drawing the editor must not make it editable again.
    app.draw(cx);
    assert_eq!(app.readonly(cx), [true]);
    let before = app.text(cx, &main);
    cx.simulate_input(app.window.into(), "typed");
    assert_eq!(app.text(cx, &main), before);
    app.update(cx, |ws, window, cx| ws.set_agent_busy(false, window, cx));
    app.draw(cx);
    assert_eq!(app.readonly(cx), [false]);
}
