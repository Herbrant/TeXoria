//! Builds with tex-compile's `fake-engine`: the state shown, the problems, the agents' `build`
//! calls, live preview, cancel, timeouts, the trust prompts and exporting the PDF.

use serde_json::json;

use super::*;
use core::prelude::v1::test;
use gpui_kit::gpui;

/// An error at line 17 of `main.tex`, as pdfLaTeX logs it.
const ERROR_LOG: &str = "log=(./main.tex\\n! Undefined control sequence.\\nl.17 \\foo\\n\\n)\\n";

/// The sample project built by the fake engine doing `steps` (the engines first run opens it).
fn built(
    cx: &mut TestAppContext,
    steps: &[&str],
    change: impl FnOnce(&mut Startup),
) -> Option<Open> {
    let mut startup = quiet();
    startup.engines = vec![testing::fake_engine(steps)?];
    change(&mut startup);
    testing::allow_threads(cx);
    let app = Open::sample(cx, startup);
    wait_runs(cx, &app, 1);
    Some(app)
}

/// Wait until `n` runs have finished.
#[track_caller]
fn wait_runs(cx: &mut TestAppContext, app: &Open, n: u64) {
    let ws = app.ws.clone();
    testing::wait_until(cx, &format!("run {n}"), |cx| {
        ws.read_with(cx, |ws, _| {
            ws.last_run >= n && !matches!(ws.state, CompileState::Running)
        })
    });
}

fn state(cx: &mut TestAppContext, app: &Open) -> CompileState {
    app.read(cx, |ws, _| ws.state.clone())
}

/// A `build` call made now; its answer comes later.
fn call_build(cx: &mut TestAppContext, app: &Open) -> mpsc::Receiver<ToolResult> {
    let (reply, answer) = mpsc::channel();
    let call = ToolCall {
        name: "build".into(),
        arguments: json!({}),
        reply,
    };
    app.update(cx, |ws, _, cx| ws.on_tool_call(call, cx));
    answer
}

#[track_caller]
fn answer(cx: &mut TestAppContext, rx: &mpsc::Receiver<ToolResult>) -> ToolResult {
    let mut got = None;
    testing::wait_until(cx, "the build's answer", |_| {
        got = rx.try_recv().ok();
        got.is_some()
    });
    got.unwrap()
}

#[gpui::test]
fn a_failed_build_shows_its_problems(cx: &mut TestAppContext) {
    let Some(app) = built(cx, &[ERROR_LOG, "pdf", "exit=1"], |_| {}) else {
        return;
    };
    let main = app.path("main.tex");
    let CompileState::Done { status, errors, .. } = state(cx, &app) else {
        panic!("{:?}", state(cx, &app));
    };
    assert_eq!((status, errors), (Status::Failed, 1));
    let diagnostics = app.read(cx, |ws, _| ws.diagnostics.clone());
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(diagnostics[0].file.as_deref(), Some(main.as_path()));
    assert_eq!(diagnostics[0].line, Some(17));
    // The PDF it wrote is shown, and the agent offers to fix the error.
    assert!(app.read(cx, |ws, _| ws.show_old_pdf));
    let problems = app.tool(cx, "problems", json!({})).text;
    assert_eq!(
        problems,
        "The last run failed.\nmain.tex:17: error: Undefined control sequence \\foo"
    );
    app.dispatch(cx, crate::ToggleDiagnostics);
    assert!(app.read(cx, |ws, _| ws.show_diagnostics));
    app.draw(cx);
    // A diagnostic in a file that is opened later underlines it too.
    app.update(cx, |ws, window, cx| {
        ws.reveal(main.clone(), 16, window, cx);
    });
    let cursor = app
        .editor(cx, &main)
        .read_with(cx, |e, _| e.cursor_position().line);
    assert_eq!(cursor, 16);
    app.dispatch(cx, crate::ToggleDiagnostics);
    assert!(!app.read(cx, |ws, _| ws.show_diagnostics));
}

#[gpui::test]
fn a_build_call_is_answered_by_the_next_run(cx: &mut TestAppContext) {
    let Some(app) = built(cx, &["sleep=200", "pdf"], |_| {}) else {
        return;
    };
    assert!(matches!(
        state(cx, &app),
        CompileState::Done {
            status: Status::Success,
            ..
        }
    ));
    let r = app.tool(cx, "build", json!({}));
    assert!(!r.is_error, "{}", r.text);
    assert!(r.text.starts_with("Built in "), "{}", r.text);
    assert!(r.text.ends_with("\nNo errors or warnings."), "{}", r.text);
    assert_eq!(app.read(cx, |ws, _| ws.last_run), 2);

    // A call made while a run is going is answered by the run after it, which saw the
    // files as they were when the agent asked.
    app.update(cx, |ws, _, cx| ws.build(cx));
    let ws = app.ws.clone();
    testing::wait_until(cx, "run 3", |cx| ws.read_with(cx, |ws, _| ws.last_run == 3));
    let rx = call_build(cx, &app);
    let r = answer(cx, &rx);
    assert!(!r.is_error, "{}", r.text);
    assert_eq!(app.read(cx, |ws, _| ws.last_run), 4);
    assert!(app.read(cx, |ws, _| ws.tool_waiters.is_empty()));
}

#[gpui::test]
fn stopping_a_build_answers_the_calls_waiting_for_it(cx: &mut TestAppContext) {
    let Some(app) = built(cx, &["pdf"], |s| {
        // The first run waits on the second engine; the test swaps them below.
        s.engines
            .push(testing::fake_engine(&["sleep=20000"]).unwrap());
    }) else {
        return;
    };
    app.update(cx, |ws, _, cx| ws.set_engine(1, cx));
    let ws = app.ws.clone();
    testing::wait_until(cx, "the slow run", |cx| {
        ws.read_with(cx, |ws, _| {
            ws.last_run == 2 && matches!(ws.state, CompileState::Running)
        })
    });
    let rx = call_build(cx, &app);
    app.dispatch(cx, crate::CancelBuild);
    let r = answer(cx, &rx);
    assert_eq!(
        (r.is_error, r.text.as_str()),
        (true, "The build was stopped.")
    );
    let ws = app.ws.clone();
    testing::wait_until(cx, "the cancelled run", |cx| {
        ws.read_with(cx, |ws, _| matches!(ws.state, CompileState::Idle))
    });
    let summary = app.tool(cx, "problems", json!({})).text;
    assert!(
        summary.starts_with("The document has not been built yet."),
        "{summary}"
    );
}

#[gpui::test]
fn an_engine_that_cannot_start_is_reported(cx: &mut TestAppContext) {
    let Some(_) = testing::fake_engine(&[]) else {
        return;
    };
    let mut startup = quiet();
    startup.engines = vec![tex_compile::Engine {
        kind: tex_compile::EngineKind::Custom {
            program: PathBuf::from("/nonexistent/texoria-engine"),
            args: Vec::new(),
        },
        extra_path: Vec::new(),
    }];
    testing::allow_threads(cx);
    let app = Open::sample(cx, startup);
    let ws = app.ws.clone();
    testing::wait_until(cx, "the failure", |cx| {
        ws.read_with(cx, |ws, _| matches!(ws.state, CompileState::Failed(_)))
    });
    let rx = call_build(cx, &app);
    let r = answer(cx, &rx);
    assert!(r.is_error);
    assert!(r.text.starts_with("The engine failed: "), "{}", r.text);
    app.draw(cx);
}

#[gpui::test]
fn a_run_past_its_timeout_is_stopped(cx: &mut TestAppContext) {
    let Some(app) = built(cx, &["sleep=20000"], |s| {
        s.settings.compile_timeout_secs = 1
    }) else {
        return;
    };
    let CompileState::Done { status, errors, .. } = state(cx, &app) else {
        panic!("{:?}", state(cx, &app));
    };
    assert_eq!((status, errors), (Status::Failed, 1));
    let problems = app.tool(cx, "problems", json!({})).text;
    assert!(
        problems.contains("The build stopped after 1s"),
        "{problems}"
    );
    // A new timeout reaches the compiler.
    app.update(cx, |ws, window, cx| {
        ws.update_settings(|s| s.compile_timeout_secs = 0, window, cx)
    });
    assert_eq!(app.read(cx, |ws, _| ws.settings.compile_timeout()), None);
}

#[gpui::test]
fn live_preview_builds_after_a_pause_in_typing(cx: &mut TestAppContext) {
    let Some(app) = built(cx, &["pdf"], |_| {}) else {
        return;
    };
    let main = app.path("main.tex");
    let delay = app.read(cx, |ws, _| ws.settings.live_delay_ms);
    app.type_at(cx, &main, 0, "% typed\n");
    assert!(app.read(cx, |ws, _| ws.buffers[0].dirty));
    // Not before the pause.
    cx.executor()
        .advance_clock(Duration::from_millis(delay / 2));
    cx.run_until_parked();
    assert_eq!(app.read(cx, |ws, _| ws.last_run), 1);
    cx.executor().advance_clock(Duration::from_millis(delay));
    wait_runs(cx, &app, 2);
    // Saved first: the engine builds what is on screen.
    assert!(
        std::fs::read_to_string(&main)
            .unwrap()
            .starts_with("% typed\n")
    );
    assert!(!app.read(cx, |ws, _| ws.buffers[0].dirty));

    // Off: only the outline follows.
    app.dispatch(cx, crate::ToggleLivePreview);
    assert!(!app.read(cx, |ws, _| ws.settings.live_preview));
    let at = app.text(cx, &main).find("\\maketitle").unwrap();
    app.type_at(cx, &main, at, "\\section{Typed}\n");
    cx.executor()
        .advance_clock(Duration::from_millis(delay * 2));
    cx.run_until_parked();
    std::thread::sleep(Duration::from_millis(100));
    cx.run_until_parked();
    assert_eq!(app.read(cx, |ws, _| ws.last_run), 2);
    let titles: Vec<String> = app.read(cx, |ws, _| {
        ws.outline.iter().map(|o| o.item.title.clone()).collect()
    });
    assert_eq!(
        titles.first().map(String::as_str),
        Some("Typed"),
        "{titles:?}"
    );
    app.draw(cx);
}

#[gpui::test]
fn switching_engines_builds_again(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let Some(app) = built(cx, &["pdf"], |s| {
        s.engines
            .push(testing::fake_engine(&["pdf", "exit=1"]).unwrap());
        s.settings.file = Some(dir.path().join("settings.toml"));
    }) else {
        return;
    };
    app.update(cx, |ws, _, cx| ws.next_engine(cx));
    wait_runs(cx, &app, 2);
    assert_eq!(app.read(cx, |ws, _| ws.engine_ix), 1);
    assert!(matches!(
        state(cx, &app),
        CompileState::Done {
            status: Status::Failed,
            ..
        }
    ));
    let saved = Settings::load_from(dir.path().join("settings.toml"));
    // A custom engine is labelled with its program's file name.
    let label = format!("fake-engine{}", std::env::consts::EXE_SUFFIX);
    assert_eq!(saved.engine.as_deref(), Some(label.as_str()));
    app.update(cx, |ws, _, cx| ws.next_engine(cx));
    wait_runs(cx, &app, 3);
    assert_eq!(app.read(cx, |ws, _| ws.engine_ix), 0);
    app.draw(cx);
}

#[gpui::test]
fn shell_escape_is_allowed_per_project(cx: &mut TestAppContext) {
    let minted =
        "log=! Package minted Error: You must invoke LaTeX with the -shell-escape flag.\\n";
    let Some(app) = built(cx, &[minted, "exit=1"], |_| {}) else {
        return;
    };
    assert!(app.read(cx, |ws, _| ws.wants_shell_escape));
    app.draw(cx);
    app.update(cx, |ws, window, cx| ws.allow_shell_escape(window, cx));
    let dir = app.dir.clone();
    assert!(app.read(cx, |ws, _| ws.settings.allows_shell_escape(&dir)));
    assert!(app.read(cx, |ws, _| ws.job().unwrap().shell_escape));
    // It builds again, with shell escape: the log no longer asks for it.
    wait_runs(cx, &app, 2);
    assert!(!app.read(cx, |ws, _| ws.wants_shell_escape));
}

#[gpui::test]
fn a_projects_latexmkrc_runs_only_once_allowed(cx: &mut TestAppContext) {
    let Some(engine) = testing::fake_engine(&["pdf"]) else {
        return;
    };
    let tmp = testing::fixture_copy("sample");
    std::fs::write(tmp.path().join("latexmkrc"), "$pdf_mode = 1;\n").unwrap();
    let mut startup = quiet();
    startup.engines = vec![engine];
    testing::allow_threads(cx);
    let (window, ws) = testing::open(cx, Some(tmp.path().join("main.tex")), startup);
    let app = Open {
        dir: paths::canonical(tmp.path()),
        _tmp: Some(tmp),
        window,
        ws,
    };
    wait_runs(cx, &app, 1);
    assert!(app.read(cx, |ws, _| ws.untrusted_rc));
    assert!(!app.read(cx, |ws, _| ws.job().unwrap().allow_project_rc));
    app.draw(cx);
    app.update(cx, |ws, _, cx| ws.allow_rc(cx));
    assert!(!app.read(cx, |ws, _| ws.untrusted_rc));
    assert!(app.read(cx, |ws, _| ws.job().unwrap().allow_project_rc));
    wait_runs(cx, &app, 2);
    // Taken back in the settings: ignored again.
    app.update(cx, |ws, window, cx| {
        ws.update_settings(|s| s.rc_allowed = Some(Vec::new()), window, cx)
    });
    assert!(app.read(cx, |ws, _| ws.untrusted_rc));
    wait_runs(cx, &app, 3);
}

#[gpui::test]
fn exports_the_pdf_where_the_user_chooses(cx: &mut TestAppContext) {
    let settings = tempfile::tempdir().unwrap();
    let file = settings.path().join("settings.toml");
    let Some(app) = built(cx, &["pdf"], |s| s.settings.file = Some(file.clone())) else {
        return;
    };
    let out = tempfile::tempdir().unwrap();
    let dest = paths::canonical(out.path()).join("paper.pdf");
    app.dispatch(cx, crate::ExportPdf);
    let mut asked_in = None;
    cx.simulate_new_path_selection(|dir| {
        asked_in = Some(dir.to_path_buf());
        Some(dest.clone())
    });
    cx.run_until_parked();
    // First in the project's folder.
    assert_eq!(asked_in.as_deref(), Some(app.dir.as_path()));
    assert_eq!(std::fs::read(&dest).unwrap(), b"%PDF-1.5\n");
    assert_eq!(app.notifications(cx), 1);
    let root = app.path("main.tex");
    let saved = Settings::load_from(file.clone());
    assert_eq!(
        saved.session(&root).unwrap().export.as_deref(),
        Some(dest.as_path())
    );

    // Then where the last copy went; a failed copy says why.
    app.update(cx, |ws, window, cx| ws.export_pdf(window, cx));
    let mut asked_in = None;
    let missing = out.path().join("gone/paper.pdf");
    cx.simulate_new_path_selection(|dir| {
        asked_in = Some(dir.to_path_buf());
        Some(missing)
    });
    cx.run_until_parked();
    assert_eq!(asked_in.as_deref(), dest.parent());
    assert_eq!(app.notifications(cx), 2);
    // Cancelled: nothing happens.
    app.update(cx, |ws, window, cx| ws.export_pdf(window, cx));
    cx.simulate_new_path_selection(|_| None);
    cx.run_until_parked();
    assert_eq!(app.notifications(cx), 2);
}

#[gpui::test]
fn only_pdfs_texoria_built_are_shown_or_exported(cx: &mut TestAppContext) {
    // A project that came with its build folder: its PDF is not trusted.
    let tmp = testing::fixture_copy("sample");
    let build = tmp.path().join(".texoria/build");
    std::fs::create_dir_all(&build).unwrap();
    std::fs::write(build.join("main.pdf"), "%PDF-1.5\n").unwrap();
    let (window, ws) = testing::open(cx, Some(tmp.path().join("main.tex")), quiet());
    let app = Open {
        dir: paths::canonical(tmp.path()),
        _tmp: Some(tmp),
        window,
        ws,
    };
    assert!(!app.read(cx, |ws, _| ws.show_old_pdf));
    app.update(cx, |ws, window, cx| ws.export_pdf(window, cx));
    assert!(!cx.did_prompt_for_new_path());
    assert_eq!(app.notifications(cx), 1);

    // Opened before: it is.
    let root = app.path("main.tex");
    let mut startup = quiet();
    startup.settings.recent = vec![root.clone()];
    let (_, ws) = testing::open(cx, Some(root), startup);
    assert!(ws.read_with(cx, |ws, _| ws.show_old_pdf));
}
