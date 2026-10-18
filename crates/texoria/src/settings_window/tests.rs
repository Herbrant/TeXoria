use std::path::PathBuf;

use gpui_kit::component::setting::RenderOptions;
use gpui_kit::{TestAppContext, WindowHandle, base::Root};

use super::*;
use crate::testing;
// `super::*` brings gpui's `test` attribute, which would shadow the standard one, and
// gpui-kit's `gpui` alias, which the `#[gpui::test]` macro's paths must not find twice.
use core::prelude::v1::test;
use gpui_kit::gpui;

struct Open {
    _dir: tempfile::TempDir,
    /// Where the settings are saved.
    file: PathBuf,
    main: WindowHandle<Root>,
    workspace: Entity<Workspace>,
    window: AnyWindowHandle,
    view: Entity<SettingsWindow>,
}

impl Open {
    /// The settings as saved.
    fn saved(&self) -> Settings {
        Settings::load_from(self.file.clone())
    }

    fn settings(&self, cx: &mut TestAppContext) -> Settings {
        self.workspace.read_with(cx, |ws, _| ws.settings.clone())
    }

    fn update<R>(
        &self,
        cx: &mut TestAppContext,
        f: impl FnOnce(&mut SettingsWindow, &mut Window, &mut Context<SettingsWindow>) -> R,
    ) -> R {
        let view = self.view.clone();
        let r = cx
            .update_window(self.window, |_, window, cx| {
                view.update(cx, |v, cx| f(v, window, cx))
            })
            .unwrap();
        cx.run_until_parked();
        r
    }

    /// The items of every page, as last rendered.
    fn items(&self, cx: &mut TestAppContext) -> Vec<(&'static str, SettingItem)> {
        // Drawn again first.
        cx.update_window(self.window, |_, window, _| window.refresh())
            .unwrap();
        cx.run_until_parked();
        self.view.read_with(cx, |v, _| {
            v.built
                .borrow()
                .iter()
                .flat_map(|(group, items)| items.iter().map(|i| (*group, i.clone())))
                .collect()
        })
    }
}

/// The sample project with its settings saved in a temporary file, and the Settings window
/// opened from the menu.
fn open(cx: &mut TestAppContext, settings: impl FnOnce(&mut Settings)) -> Open {
    let dir = testing::fixture_copy("sample");
    let file = dir.path().join("settings/settings.toml");
    let mut startup = testing::startup();
    startup.settings = Settings::load_from(file.clone());
    settings(&mut startup.settings);
    let (main, workspace) = testing::open(cx, Some(dir.path().join("main.tex")), startup);
    cx.dispatch_action(main.into(), crate::OpenSettings);
    cx.run_until_parked();
    let main_any: AnyWindowHandle = main.into();
    let window = cx
        .windows()
        .into_iter()
        .find(|w| *w != main_any)
        .expect("the Settings window");
    let view = cx
        .update_window(window, |root, _, cx| {
            let root = root.downcast::<Root>().unwrap();
            root.read(cx)
                .view()
                .clone()
                .downcast::<SettingsWindow>()
                .unwrap()
        })
        .unwrap();
    Open {
        _dir: dir,
        file,
        main,
        workspace,
        window,
        view,
    }
}

/// Settings unlike the defaults wherever the window has a switch, a number or a choice.
fn unusual(s: &mut Settings) {
    let d = Settings::default();
    s.theme = ThemeChoice::Dark;
    s.editor_font_size = 21.0;
    s.soft_wrap = !d.soft_wrap;
    s.vim_mode = !d.vim_mode;
    s.format_on_save = !d.format_on_save;
    s.live_checks = !d.live_checks;
    s.spell_check = !d.spell_check;
    s.grammar_check = !d.grammar_check;
    // Never reached: grammar starts a second after a change, and the clock does not move.
    s.languagetool_url = "http://localhost:1".into();
    s.languagetool_start = !d.languagetool_start;
    s.compile_timeout_secs = d.compile_timeout_secs + 30;
    s.live_preview = !d.live_preview;
    s.live_delay_ms = d.live_delay_ms + 100;
    s.pull_rebase = !d.pull_rebase;
    s.diff_split = !d.diff_split;
    s.git_timeout_secs = d.git_timeout_secs + 30;
    s.check_updates = !d.check_updates;
}

/// What `unusual` changes, as it is now.
fn shown(s: &Settings) -> String {
    format!(
        "{:?} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {}",
        s.theme,
        s.font_size(),
        s.soft_wrap,
        s.vim_mode,
        s.format_on_save,
        s.live_checks,
        s.spell_check,
        s.grammar_check,
        s.languagetool_url,
        s.languagetool_start,
        s.compile_timeout_secs,
        s.live_preview,
        s.live_delay_ms,
        s.pull_rebase,
        s.diff_split,
        s.git_timeout_secs,
        s.check_updates
    )
}

/// Each field reads its own setting, and writes it through the workspace, which saves it:
/// resetting every field brings back every default, and nothing else.
#[gpui::test]
fn every_field_reads_and_writes_its_setting(cx: &mut TestAppContext) {
    let open = open(cx, |s| {
        unusual(s);
        s.spell_words = vec!["texoria".into()];
    });
    let defaults = shown(&Settings::default());
    assert_ne!(shown(&open.settings(cx)), defaults);
    let fields: Vec<_> = open
        .items(cx)
        .into_iter()
        .filter_map(|(group, item)| match item {
            SettingItem::Item { title, field, .. } => Some((group, title, field)),
            SettingItem::Element { .. } => None,
        })
        .collect();
    assert!(fields.len() >= 17, "{}", fields.len());
    let mut reset = Vec::new();
    for (group, title, field) in &fields {
        let resettable = cx.read(|cx| field.is_resettable(cx));
        if !resettable {
            continue;
        }
        reset.push(format!("{group}/{title}"));
        cx.update_window(open.window, |_, window, cx| field.reset(window, cx))
            .unwrap();
        cx.run_until_parked();
        assert!(
            !cx.read(|cx| field.is_resettable(cx)),
            "{group}/{title} kept its value"
        );
    }
    assert_eq!(reset.len(), 17, "{reset:?}");
    assert_eq!(shown(&open.settings(cx)), defaults, "{reset:?}");
    // Saved, the rest untouched.
    let saved = open.saved();
    assert_eq!(shown(&saved), defaults);
    assert_eq!(saved.spell_words, ["texoria"]);
}

#[gpui::test]
fn lists_and_removes_folders_and_words(cx: &mut TestAppContext) {
    let project = PathBuf::from("/projects/thesis");
    let p = project.clone();
    let open = open(cx, move |s| {
        s.spell_words = vec!["texoria".into(), "latexmk".into()];
        s.grammar_rules_off = vec!["EN_QUOTES".into()];
        s.trusted_projects = vec![p.clone()];
        s.rc_allowed = Some(vec![p.clone()]);
        s.shell_escape_allowed = vec![p.clone()];
        s.agent_projects = vec![p];
    });
    let lists: Vec<_> = open
        .items(cx)
        .into_iter()
        .filter_map(|(group, item)| match item {
            SettingItem::Element { render, .. } => Some((group, render)),
            SettingItem::Item { .. } => None,
        })
        .collect();
    let groups: Vec<_> = lists.iter().map(|(g, _)| *g).collect();
    for g in [
        "Spelling",
        "Grammar",
        "Git",
        "latexmkrc",
        "Shell escape",
        "Coding agents",
    ] {
        assert!(groups.contains(&g), "{g} in {groups:?}");
    }
    // Drawn with their items (the window shows one page at a time).
    for (_, render) in &lists {
        cx.update_window(open.window, |_, window, cx| {
            let _ = render(&RenderOptions::new(), window, cx);
        })
        .unwrap();
    }

    // Removing goes through the workspace.
    let target = open.view.read_with(cx, |v, _| v.target.clone());
    cx.update(|cx| {
        target.set(cx, |s| s.spell_words.retain(|w| w != "latexmk"));
    });
    cx.run_until_parked();
    assert_eq!(open.saved().spell_words, ["texoria"]);
}

#[gpui::test]
fn edits_the_overleaf_servers(cx: &mut TestAppContext) {
    let open = open(cx, |s| {
        s.overleaf_servers = vec!["https://a.example".into()]
    });
    let servers = |cx: &mut TestAppContext| open.settings(cx).overleaf_servers;
    open.view
        .read_with(cx, |v, _| assert_eq!(v.servers.len(), 1));
    open.update(cx, |v, window, cx| v.add_server(window, cx));
    let input = open.view.read_with(cx, |v, _| v.servers[1].clone());
    // Saved without its spaces and its last slash; an empty field is not a server.
    assert_eq!(servers(cx), ["https://a.example"]);
    open.update(cx, |v, window, cx| {
        input.update(cx, |i, cx| i.set_value(" https://b.example/ ", window, cx));
        // As each change of the field does.
        v.save_servers(cx);
    });
    assert_eq!(servers(cx), ["https://a.example", "https://b.example"]);
    assert_eq!(
        open.saved().overleaf_servers,
        ["https://a.example", "https://b.example"]
    );
    open.update(cx, |v, _, cx| {
        v.remove_server(0, cx);
        v.remove_server(7, cx);
    });
    assert_eq!(servers(cx), ["https://b.example"]);
}

#[gpui::test]
fn edits_the_custom_agents(cx: &mut TestAppContext) {
    let open = open(cx, |s| {
        s.custom_agents = vec![CustomAgent {
            name: "Mine".into(),
            command: "my-agent".into(),
            args: vec!["--acp".into()],
        }]
    });
    let agents = |cx: &mut TestAppContext| open.settings(cx).custom_agents;
    open.view.read_with(cx, |v, cx| {
        let [name, command, args] = &v.agents[0];
        assert_eq!(name.read(cx).value(), "Mine");
        assert_eq!(command.read(cx).value(), "my-agent");
        assert_eq!(args.read(cx).value(), "--acp");
    });
    // A new one is saved once it has a command, its arguments split at spaces.
    open.update(cx, |v, window, cx| v.add_agent(window, cx));
    let [name, command, args] = open.view.read_with(cx, |v, _| v.agents[1].clone());
    open.update(cx, |v, window, cx| {
        name.update(cx, |i, cx| i.set_value("Other", window, cx));
        v.save_agents(cx);
    });
    assert_eq!(agents(cx).len(), 1);
    cx.update_window(open.window, |_, window, cx| {
        command.update(cx, |i, cx| i.set_value(" other-agent ", window, cx));
        args.update(cx, |i, cx| i.set_value("--stdio  --quiet", window, cx));
    })
    .unwrap();
    // The fields report their changes as they are typed.
    open.update(cx, |v, _, cx| v.save_agents(cx));
    let saved = agents(cx);
    assert_eq!(saved.len(), 2);
    assert_eq!(saved[1].name, "Other");
    assert_eq!(saved[1].command, "other-agent");
    assert_eq!(saved[1].args, ["--stdio", "--quiet"]);
    assert_eq!(open.saved().custom_agents, saved);
    open.update(cx, |v, _, cx| {
        v.remove_agent(0, cx);
        v.remove_agent(9, cx);
    });
    assert_eq!(agents(cx).len(), 1);
    assert_eq!(agents(cx)[0].name, "Other");
}

#[gpui::test]
fn opening_it_again_brings_it_back(cx: &mut TestAppContext) {
    let open = open(cx, |_| {});
    cx.dispatch_action(open.main.into(), crate::OpenSettings);
    cx.run_until_parked();
    assert_eq!(cx.windows().len(), 2);
    // It reads the workspace's settings.
    let target = open.view.read_with(cx, |v, _| v.target.clone());
    let delay = cx.read(|cx| target.get(cx, |s| s.live_delay_ms));
    assert_eq!(delay, open.saved().live_delay_ms);
}
