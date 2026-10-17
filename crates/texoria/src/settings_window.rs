//! The Settings window. `Workspace` owns the settings: this view reads them
//! from it and writes them back through `Workspace::update_settings`, which
//! saves them and applies the change to what is running.

use std::path::PathBuf;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::setting::{
    NumberFieldOptions, SettingField, SettingGroup, SettingItem, SettingPage,
    Settings as SettingsView,
};
use gpui_kit::component::{
    ActiveTheme, TitleBar,
    button::{Button, ButtonVariants},
    h_flex, v_flex,
};
use gpui_kit::*;

use tex_agent::CustomAgent;

use crate::assets::Glyph;
use crate::settings::{Settings, ThemeChoice};
use crate::workspace::Workspace;

/// The main window's workspace, which the settings belong to.
#[derive(Clone)]
struct Target {
    workspace: WeakEntity<Workspace>,
    window: AnyWindowHandle,
}

impl Target {
    fn read<T: Default>(&self, cx: &App, f: impl FnOnce(&Workspace) -> T) -> T {
        self.workspace
            .upgrade()
            .map(|w| f(w.read(cx)))
            .unwrap_or_default()
    }

    fn get<T: Default>(&self, cx: &App, f: impl FnOnce(&Settings) -> T) -> T {
        self.read(cx, |w| f(&w.settings))
    }

    fn set(&self, cx: &mut App, change: impl FnOnce(&mut Settings)) {
        let workspace = self.workspace.clone();
        // The main window, not this one: soft wrap and the theme need it.
        let _ = self.window.update(cx, |_, window, cx| {
            workspace.update(cx, |w, cx| w.update_settings(change, window, cx))
        });
    }
}

pub struct SettingsWindow {
    target: Target,
    /// One input per Overleaf Server Pro instance.
    servers: Vec<Entity<InputState>>,
    /// Name, command and arguments of each agent added by hand.
    agents: Vec<[Entity<InputState>; 3]>,
    _subscriptions: Vec<Subscription>,
}

impl SettingsWindow {
    pub fn new(
        workspace: WeakEntity<Workspace>,
        main: AnyWindowHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let target = Target {
            workspace,
            window: main,
        };
        let mut subscriptions = Vec::new();
        if let Some(w) = target.workspace.upgrade() {
            // Settings also change from the main window (⇧⌘P, the theme button).
            subscriptions.push(cx.observe(&w, |_, _, cx| cx.notify()));
        }
        let mut this = SettingsWindow {
            target,
            servers: Vec::new(),
            agents: Vec::new(),
            _subscriptions: subscriptions,
        };
        for url in this.target.get(cx, |s| s.overleaf_servers.clone()) {
            this.push_server(url, window, cx);
        }
        for agent in this.target.get(cx, |s| s.custom_agents.clone()) {
            this.push_agent(agent, window, cx);
        }
        this
    }

    fn push_server(
        &mut self,
        url: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("https://latex.example.org")
                .default_value(url)
        });
        self._subscriptions.push(cx.subscribe_in(
            &input,
            window,
            |this, _, ev: &InputEvent, _, cx| {
                if let InputEvent::Change = ev {
                    this.save_servers(cx);
                }
            },
        ));
        self.servers.push(input.clone());
        input
    }

    fn add_server(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = self.push_server(String::new(), window, cx);
        input.update(cx, |i, cx| i.focus(window, cx));
        cx.notify();
    }

    fn remove_server(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix < self.servers.len() {
            self.servers.remove(ix);
            self.save_servers(cx);
            cx.notify();
        }
    }

    fn save_servers(&self, cx: &mut Context<Self>) {
        let servers: Vec<String> = self
            .servers
            .iter()
            .map(|i| i.read(cx).value().trim().trim_end_matches('/').to_string())
            .filter(|s| !s.is_empty())
            .collect();
        self.target.set(cx, |s| s.overleaf_servers = servers);
    }

    fn push_agent(
        &mut self,
        agent: CustomAgent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        let fields = [
            ("Name", agent.name),
            ("Command", agent.command),
            ("Arguments", agent.args.join(" ")),
        ]
        .map(|(placeholder, value)| {
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(placeholder)
                    .default_value(value)
            });
            self._subscriptions.push(cx.subscribe_in(
                &input,
                window,
                |this, _, ev: &InputEvent, _, cx| {
                    if let InputEvent::Change = ev {
                        this.save_agents(cx);
                    }
                },
            ));
            input
        });
        let first = fields[0].clone();
        self.agents.push(fields);
        first
    }

    fn add_agent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = self.push_agent(CustomAgent::default(), window, cx);
        input.update(cx, |i, cx| i.focus(window, cx));
        cx.notify();
    }

    fn remove_agent(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix < self.agents.len() {
            self.agents.remove(ix);
            self.save_agents(cx);
            cx.notify();
        }
    }

    fn save_agents(&self, cx: &mut Context<Self>) {
        let agents: Vec<CustomAgent> = self
            .agents
            .iter()
            .map(|[name, command, args]| CustomAgent {
                name: name.read(cx).value().trim().to_string(),
                command: command.read(cx).value().trim().to_string(),
                args: args
                    .read(cx)
                    .value()
                    .split_whitespace()
                    .map(str::to_string)
                    .collect(),
            })
            .filter(|a| !a.command.is_empty())
            .collect();
        self.target.set(cx, |s| s.custom_agents = agents);
    }

    // ── Pages ────────────────────────────────────────────────────────────

    fn editor_page(&self, cx: &App) -> SettingPage {
        let t = self.target.clone();
        let words = t.get(cx, |s| s.spell_words.clone());
        let themes = ThemeChoice::ALL
            .iter()
            .map(|c| (c.key().into(), c.label().into()))
            .collect();
        SettingPage::new("Editor").default_open(true).groups([
            SettingGroup::new().title("Appearance").item(
                SettingItem::new(
                    "Theme",
                    SettingField::dropdown(
                        themes,
                        {
                            let t = t.clone();
                            move |cx| t.get(cx, |s| s.theme).key().into()
                        },
                        {
                            let t = t.clone();
                            move |key: SharedString, cx| {
                                if let Some(choice) = ThemeChoice::from_key(&key) {
                                    t.set(cx, |s| s.theme = choice);
                                }
                            }
                        },
                    )
                    .default_value(ThemeChoice::default().key()),
                )
                .description("System follows the light or dark appearance of the OS."),
            ),
            SettingGroup::new()
                .title("Text")
                .item(
                    SettingItem::new("Font size", {
                        let (t1, t2) = (t.clone(), t.clone());
                        SettingField::number_input(
                            NumberFieldOptions {
                                min: Settings::MIN_FONT_SIZE as f64,
                                max: Settings::MAX_FONT_SIZE as f64,
                                step: 1.,
                            },
                            move |cx| t1.get(cx, |s| s.font_size() as f64),
                            move |v, cx| t2.set(cx, |s| s.editor_font_size = v as f32),
                        )
                        .default_value(Settings::DEFAULT_FONT_SIZE as f64)
                    })
                    .description("Size of the editor's text, in pixels. ⌘= and ⌘- change it, ⌘0 restores it."),
                )
                .item(
                    SettingItem::new(
                        "Soft wrap",
                        bool_field(&t, |s| s.soft_wrap, |s, v| s.soft_wrap = v)
                            .default_value(Settings::default().soft_wrap),
                    )
                    .description(
                        "Wrap long lines in the editor. Very long files always start unwrapped; \
                         ⌥Z toggles the current tab.",
                    ),
                )
                .item(
                    SettingItem::new(
                        "Format on save",
                        bool_field(&t, |s| s.format_on_save, |s, v| s.format_on_save = v)
                            .default_value(Settings::default().format_on_save),
                    )
                    .description(
                        "Indent the file with tex-fmt when you save it with ⌘S (⇧⌥F formats it \
                         at any time). A tex-fmt.toml in the project folder changes its rules.",
                    ),
                ),
            SettingGroup::new().title("Checks").item(
                SettingItem::new(
                    "Check references",
                    bool_field(&t, |s| s.live_checks, |s, v| s.live_checks = v)
                        .default_value(Settings::default().live_checks),
                )
                .description(
                    "Underline, as you type, references and citations to labels and entries \
                     that do not exist, duplicate labels, missing files and pictures, and \
                     flaws of the .bib files (duplicate keys, missing fields, bad DOIs). They \
                     are listed with the problems.",
                ),
            ),
            SettingGroup::new()
                .title("Spelling")
                .item(
                    SettingItem::new(
                        "Check spelling",
                        bool_field(&t, |s| s.spell_check, |s, v| s.spell_check = v)
                            .default_value(Settings::default().spell_check),
                    )
                    .description(
                        "Underline misspelled words. The language of each file comes from a \
                         `% !TeX spellcheck = …` comment, babel or polyglossia, or the text \
                         itself, and can be changed in the status bar. Dictionaries that are not \
                         installed are downloaded the first time they are needed.",
                    ),
                )
                .item(removable_list(
                    "word",
                    words,
                    String::clone,
                    "Words added to the dictionary appear here.",
                    {
                        let t = t.clone();
                        move |word, cx| t.set(cx, |s| s.spell_words.retain(|w| *w != word))
                    },
                )),
        ])
    }

    fn build_page(&self, cx: &App) -> SettingPage {
        let t = self.target.clone();
        let defaults = Settings::default();
        let engines: Vec<(SharedString, SharedString)> = t.read(cx, |w| {
            w.engines
                .iter()
                .map(|e| (e.label().into(), e.label().into()))
                .collect()
        });
        let engine = if engines.is_empty() {
            SettingItem::new(
                "Engine",
                SettingField::render(|_, _, cx| {
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("None found")
                }),
            )
            .description("Install TeX Live or MiKTeX, or place tectonic next to TeXoria.")
        } else {
            SettingItem::new(
                "Engine",
                SettingField::dropdown(
                    engines,
                    {
                        let t = t.clone();
                        move |cx| {
                            t.read(cx, |w| {
                                w.engines
                                    .get(w.engine_ix)
                                    .map(|e| e.label())
                                    .unwrap_or_default()
                            })
                            .into()
                        }
                    },
                    {
                        let t = t.clone();
                        move |label: SharedString, cx| {
                            t.set(cx, |s| s.engine = Some(label.to_string()))
                        }
                    },
                ),
            )
            .description("latexmk with pdfLaTeX, XeLaTeX or LuaLaTeX, or Tectonic.")
        };
        let live = t.get(cx, |s| s.live_preview);
        SettingPage::new("Build").groups([
            SettingGroup::new().title("Engine").item(engine).item(
                SettingItem::new(
                    "Timeout",
                    number_field(
                        &t,
                        NumberFieldOptions {
                            min: 0.,
                            max: 3600.,
                            step: 30.,
                        },
                        |s| s.compile_timeout_secs,
                        |s, v| s.compile_timeout_secs = v,
                    )
                    .default_value(defaults.compile_timeout_secs as f64),
                )
                .description("Stop a build after this many seconds (0: never)."),
            ),
            SettingGroup::new()
                .title("Live preview")
                .item(
                    SettingItem::new(
                        "Live preview",
                        bool_field(&t, |s| s.live_preview, |s, v| s.live_preview = v)
                            .default_value(defaults.live_preview),
                    )
                    .description("Save and build automatically after edits."),
                )
                .item(
                    SettingItem::new(
                        "Delay",
                        number_field(
                            &t,
                            NumberFieldOptions {
                                min: 100.,
                                max: 5000.,
                                step: 100.,
                            },
                            |s| s.live_delay_ms,
                            |s, v| s.live_delay_ms = v,
                        )
                        .default_value(defaults.live_delay_ms as f64),
                    )
                    .description("Pause after the last keystroke, in milliseconds.")
                    .disabled(!live),
                ),
        ])
    }

    fn git_page(&self, cx: &mut Context<Self>) -> SettingPage {
        let t = self.target.clone();
        let defaults = Settings::default();
        let view = cx.entity().downgrade();
        SettingPage::new("Git").groups([
            SettingGroup::new()
                .title("Repository")
                .item(
                    SettingItem::new(
                        "Pull with rebase",
                        bool_field(&t, |s| s.pull_rebase, |s, v| s.pull_rebase = v)
                            .default_value(defaults.pull_rebase),
                    )
                    .description(
                        "Rebase local commits on top of the pulled ones instead of merging.",
                    ),
                )
                .item(
                    SettingItem::new(
                        "Side-by-side diffs",
                        bool_field(&t, |s| s.diff_split, |s, v| s.diff_split = v)
                            .default_value(defaults.diff_split),
                    )
                    .description("Show changes in two columns instead of one."),
                )
                .item(
                    SettingItem::new(
                        "Network timeout",
                        number_field(
                            &t,
                            NumberFieldOptions {
                                min: 0.,
                                max: 3600.,
                                step: 30.,
                            },
                            |s| s.git_timeout_secs,
                            |s, v| s.git_timeout_secs = v,
                        )
                        .default_value(defaults.git_timeout_secs as f64),
                    )
                    .description("Stop a fetch, pull or push after this many seconds (0: never)."),
                ),
            SettingGroup::new().title("Overleaf").item(
                SettingItem::new(
                    "Overleaf Server Pro",
                    SettingField::render(move |_, _, cx| {
                        view.upgrade()
                            .map(|v| v.read(cx).render_servers(&v).into_any_element())
                            .unwrap_or_else(|| div().into_any_element())
                    }),
                )
                .description(
                    "Your own Overleaf instances (overleaf.com is always known). They must use \
                     HTTPS, or TeXoria will not send them your token.",
                )
                .layout(Axis::Vertical),
            ),
        ])
    }

    fn render_servers(&self, view: &Entity<Self>) -> Div {
        let rows = self.servers.iter().enumerate().map(|(ix, input)| {
            let view = view.downgrade();
            h_flex()
                .gap_2()
                .child(div().flex_1().child(Input::new(input)))
                .child(
                    Button::new(("remove-server", ix))
                        .ghost()
                        .icon(Glyph::Close)
                        .tooltip("Remove")
                        .on_click(move |_, _, cx| {
                            let _ = view.update(cx, |this, cx| this.remove_server(ix, cx));
                        }),
                )
        });
        let add = {
            let view = view.downgrade();
            Button::new("add-server")
                .icon(Glyph::Plus)
                .label("Add Server")
                .on_click(move |_, window, cx| {
                    let _ = view.update(cx, |this, cx| this.add_server(window, cx));
                })
        };
        v_flex()
            .w_full()
            .gap_2()
            .children(rows)
            .child(h_flex().child(add))
    }

    fn agents_page(&self, cx: &mut Context<Self>) -> SettingPage {
        let t = self.target.clone();
        let found: Option<Vec<(SharedString, SharedString)>> =
            t.workspace.upgrade().and_then(|w| {
                w.read(cx).agent_panel().read(cx).agents().map(|a| {
                    a.iter()
                        .map(|a| (a.id.clone().into(), a.name.clone().into()))
                        .collect()
                })
            });
        let agent = match found {
            None => SettingItem::new(
                "Agent",
                SettingField::render(|_, _, cx| {
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("Looking for agents…")
                }),
            ),
            Some(list) if list.is_empty() => SettingItem::new(
                "Agent",
                SettingField::render(|_, _, cx| {
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("None found")
                }),
            )
            .description(
                "Install Claude Code, Codex, Gemini CLI, GitHub Copilot, Cursor, OpenCode or \
                 another agent that supports the Agent Client Protocol, or add one below.",
            ),
            Some(list) => {
                let first = list[0].0.clone();
                SettingItem::new(
                    "Agent",
                    SettingField::dropdown(
                        list,
                        {
                            let t = t.clone();
                            move |cx| {
                                t.read(cx, |w| {
                                    w.agent_panel()
                                        .read(cx)
                                        .current()
                                        .map(|a| SharedString::from(a.id.clone()))
                                })
                                .unwrap_or_else(|| first.clone())
                            }
                        },
                        {
                            let t = t.clone();
                            move |id: SharedString, cx| {
                                t.set(cx, |s| s.agent = Some(id.to_string()))
                            }
                        },
                    ),
                )
                .description(
                    "The coding agent of the Agent panel and of the editor's quick edits. It \
                     uses your own account, signed in from its command line.",
                )
            }
        };
        let view = cx.entity().downgrade();
        SettingPage::new("Agents").groups([
            SettingGroup::new().title("Agent").item(agent),
            SettingGroup::new().title("Other agents").item(
                SettingItem::new(
                    "Your agents",
                    SettingField::render(move |_, _, cx| {
                        view.upgrade()
                            .map(|v| v.read(cx).render_agents(&v).into_any_element())
                            .unwrap_or_else(|| div().into_any_element())
                    }),
                )
                .description(
                    "Programs speaking the Agent Client Protocol on their standard input and \
                     output that TeXoria does not know, with the arguments that start them in \
                     that mode (e.g. `--acp`).",
                )
                .layout(Axis::Vertical),
            ),
        ])
    }

    fn render_agents(&self, view: &Entity<Self>) -> Div {
        let rows = self
            .agents
            .iter()
            .enumerate()
            .map(|(ix, [name, command, args])| {
                let view = view.downgrade();
                h_flex()
                    .gap_2()
                    .child(div().w(px(140.)).child(Input::new(name)))
                    .child(div().flex_1().child(Input::new(command)))
                    .child(div().w(px(120.)).child(Input::new(args)))
                    .child(
                        Button::new(("remove-agent", ix))
                            .ghost()
                            .icon(Glyph::Close)
                            .tooltip("Remove")
                            .on_click(move |_, _, cx| {
                                let _ = view.update(cx, |this, cx| this.remove_agent(ix, cx));
                            }),
                    )
            });
        let add = {
            let view = view.downgrade();
            Button::new("add-agent")
                .icon(Glyph::Plus)
                .label("Add Agent")
                .on_click(move |_, window, cx| {
                    let _ = view.update(cx, |this, cx| this.add_agent(window, cx));
                })
        };
        v_flex()
            .w_full()
            .gap_2()
            .children(rows)
            .child(h_flex().child(add))
    }

    fn trust_page(&self, cx: &App) -> SettingPage {
        let t = self.target.clone();
        let git = t.get(cx, |s| s.trusted_projects.clone());
        let rc = t.get(cx, |s| s.rc_allowed.clone().unwrap_or_default());
        let agents = t.get(cx, |s| s.agent_projects.clone());
        SettingPage::new("Trusted Projects")
            .resettable(false)
            .description(
                "A project's Git configuration and latexmkrc can run programs, and coding \
                 agents read and change its files, so TeXoria asks before letting them. \
                 Removing a folder makes TeXoria ask again.",
            )
            .groups([
                SettingGroup::new().title("Git").item(path_list(
                    "git",
                    git,
                    "No project lets Git run yet.",
                    {
                        let t = t.clone();
                        move |dir, cx| t.set(cx, |s| s.trusted_projects.retain(|p| *p != dir))
                    },
                )),
                SettingGroup::new().title("latexmkrc").item(path_list(
                    "rc",
                    rc,
                    "No project lets its latexmkrc run yet.",
                    {
                        let t = t.clone();
                        move |dir, cx| {
                            t.set(cx, |s| {
                                if let Some(dirs) = &mut s.rc_allowed {
                                    dirs.retain(|p| *p != dir);
                                }
                            })
                        }
                    },
                )),
                SettingGroup::new().title("Coding agents").item(path_list(
                    "agents",
                    agents,
                    "No project lets coding agents work in it yet.",
                    move |dir, cx| t.set(cx, |s| s.agent_projects.retain(|p| *p != dir)),
                )),
            ])
    }

    fn updates_page(&self, cx: &App) -> SettingPage {
        let t = self.target.clone();
        let how = t.read(cx, |w| {
            let kind = w.updater().read(cx).kind().clone();
            if kind.self_updating() {
                "When a new version is out, “Update to …” appears in the status bar: TeXoria \
                 downloads it, checks its signature and restarts into it."
                    .to_string()
            } else if let Some(how) = kind.instructions() {
                format!("This copy does not update itself. {how}")
            } else {
                "This copy was built from source.".to_string()
            }
        });
        // Windows and Linux have no menu bar: this is their Check for Updates….
        let updater = t.read(cx, |w| Some(w.updater().downgrade()));
        let check = SettingItem::new(
            "Check now",
            SettingField::render(move |_, _, _| {
                let updater = updater.clone();
                Button::new("check-updates")
                    .label("Check Now")
                    .on_click(move |_, _, cx| {
                        if let Some(u) = updater.as_ref().and_then(WeakEntity::upgrade) {
                            u.update(cx, |u, cx| u.check(true, cx));
                        }
                    })
            }),
        )
        .description("The answer appears in the main window.");
        SettingPage::new("Updates").groups([SettingGroup::new()
            .title("Updates")
            .item(
                SettingItem::new(
                    "Check for updates",
                    bool_field(&t, |s| s.check_updates, |s, v| s.check_updates = v)
                        .default_value(Settings::default().check_updates),
                )
                .description(format!(
                    "Look for a new release at startup, at most once a day. {how}"
                )),
            )
            .item(check)])
    }
}

/// A switch bound to one `bool` of the settings.
fn bool_field(
    t: &Target,
    get: fn(&Settings) -> bool,
    set: fn(&mut Settings, bool),
) -> SettingField<bool> {
    let (t1, t2) = (t.clone(), t.clone());
    SettingField::switch(
        move |cx| t1.get(cx, get),
        move |v, cx| t2.set(cx, |s| set(s, v)),
    )
}

/// A number input bound to one `u64` of the settings.
fn number_field(
    t: &Target,
    options: NumberFieldOptions,
    get: fn(&Settings) -> u64,
    set: fn(&mut Settings, u64),
) -> SettingField<f64> {
    let (t1, t2) = (t.clone(), t.clone());
    SettingField::number_input(
        options,
        move |cx| t1.get(cx, get) as f64,
        move |v, cx| t2.set(cx, |s| set(s, v.round() as u64)),
    )
}

/// Folders, each with a button that removes it.
fn path_list(
    id: &'static str,
    dirs: Vec<PathBuf>,
    empty: &'static str,
    remove: impl Fn(PathBuf, &mut App) + 'static,
) -> SettingItem {
    removable_list(id, dirs, |d| d.display().to_string(), empty, remove)
}

/// Items, each with a button that removes it.
fn removable_list<T: Clone + 'static>(
    id: &'static str,
    items: Vec<T>,
    show: impl Fn(&T) -> String + 'static,
    empty: &'static str,
    remove: impl Fn(T, &mut App) + 'static,
) -> SettingItem {
    let remove = std::rc::Rc::new(remove);
    SettingItem::render(move |_, _, cx| {
        let theme = cx.theme();
        if items.is_empty() {
            return div()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(empty)
                .into_any_element();
        }
        v_flex()
            .w_full()
            .gap_1()
            .children(items.iter().enumerate().map(|(ix, item)| {
                let label = show(item);
                let (item, remove) = (item.clone(), remove.clone());
                h_flex()
                    .w_full()
                    .gap_2()
                    .child(div().flex_1().min_w_0().truncate().text_sm().child(label))
                    .child(
                        Button::new((id, ix))
                            .ghost()
                            .label("Remove")
                            .on_click(move |_, _, cx| remove(item.clone(), cx)),
                    )
            }))
            .into_any_element()
    })
}

impl Render for SettingsWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pages = [
            self.editor_page(cx),
            self.build_page(cx),
            self.git_page(cx),
            self.agents_page(cx),
            self.trust_page(cx),
            self.updates_page(cx),
        ];
        let theme = cx.theme();
        v_flex()
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .font_family(theme.font_family.clone())
            .child(
                TitleBar::new().child(
                    h_flex()
                        .flex_1()
                        .justify_center()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Settings"),
                ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(SettingsView::new("settings").pages(pages)),
            )
    }
}
