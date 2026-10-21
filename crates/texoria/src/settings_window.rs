//! The Settings window. `Workspace` owns the settings: this view shows a copy
//! of them and writes them back through `Workspace::update_settings`, which
//! saves them and applies the change to what is running.

use std::cell::{Ref, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

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
use crate::ui;
use crate::updater::Updater;
use crate::workspace::Workspace;

/// The main window's workspace, which the settings belong to.
#[derive(Clone)]
struct Target {
    workspace: WeakEntity<Workspace>,
    window: AnyWindowHandle,
    shown: Rc<RefCell<Shown>>,
}

/// What this window shows of the workspace, copied whenever it changes. This
/// window must never read the workspace (nor its agent panel or updater) while
/// it draws: GPUI would then take it for this window's, and run the
/// workspace's window-bound callbacks (`subscribe_in`, `spawn_in`…) here.
#[derive(Default)]
struct Shown {
    settings: Settings,
    /// The engines' labels, and the one in use.
    engines: Vec<SharedString>,
    engine: Option<SharedString>,
    /// The agents found (id, name), `None` while they are looked for.
    agents: Option<Vec<(SharedString, SharedString)>>,
    agent: Option<SharedString>,
    /// How this copy of TeXoria is updated.
    updates: String,
    updater: Option<WeakEntity<Updater>>,
}

impl Target {
    fn shown(&self) -> Ref<'_, Shown> {
        self.shown.borrow()
    }

    fn get<T>(&self, f: impl FnOnce(&Settings) -> T) -> T {
        f(&self.shown().settings)
    }

    fn set(&self, cx: &mut App, change: impl FnOnce(&mut Settings)) {
        let workspace = self.workspace.clone();
        // The main window, not this one: soft wrap and the theme need it.
        let _ = self.window.update(cx, |_, window, cx| {
            workspace.update(cx, |w, cx| w.update_settings(change, window, cx))
        });
        self.refresh(cx);
    }

    /// Copy what is shown from the workspace again.
    fn refresh(&self, cx: &App) {
        let Some(w) = self.workspace.upgrade() else {
            return;
        };
        let w = w.read(cx);
        let kind = w.updater().read(cx).kind().clone();
        let updates = if kind.self_updating() {
            "When a new version is out, “Update to …” appears in the status bar: TeXoria \
             downloads it, checks its signature and restarts into it."
                .to_string()
        } else if let Some(how) = kind.instructions() {
            format!("This copy does not update itself. {how}")
        } else {
            "This copy was built from source.".to_string()
        };
        let agents = w.agent_panel().read(cx);
        *self.shown.borrow_mut() = Shown {
            settings: w.settings.clone(),
            engines: w.engines.iter().map(|e| e.label().into()).collect(),
            engine: w.engines.get(w.engine_ix).map(|e| e.label().into()),
            agents: agents.agents().map(|list| {
                list.iter()
                    .map(|a| (a.id.clone().into(), a.name.clone().into()))
                    .collect()
            }),
            agent: agents.current().map(|a| a.id.clone().into()),
            updates,
            updater: Some(w.updater().downgrade()),
        };
    }
}

pub struct SettingsWindow {
    target: Target,
    /// The items of the pages rendered last, for the tests.
    #[cfg(test)]
    built: std::cell::RefCell<Vec<(&'static str, Vec<SettingItem>)>>,
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
            shown: Default::default(),
        };
        target.refresh(cx);
        let mut subscriptions = Vec::new();
        if let Some(w) = target.workspace.upgrade() {
            // Settings also change from the main window (⇧⌘P, the theme button),
            // agents are found and the updater learns how this copy updates.
            let (agents, updater) = (
                w.read(cx).agent_panel().clone(),
                w.read(cx).updater().clone(),
            );
            subscriptions.push(cx.observe(&w, Self::refresh));
            subscriptions.push(cx.observe(&agents, Self::refresh));
            subscriptions.push(cx.observe(&updater, Self::refresh));
        }
        let mut this = SettingsWindow {
            target,
            #[cfg(test)]
            built: Default::default(),
            servers: Vec::new(),
            agents: Vec::new(),
            _subscriptions: subscriptions,
        };
        for url in this.target.get(|s| s.overleaf_servers.clone()) {
            this.push_server(url, window, cx);
        }
        for agent in this.target.get(|s| s.custom_agents.clone()) {
            this.push_agent(agent, window, cx);
        }
        this
    }

    fn refresh<E>(&mut self, _: Entity<E>, cx: &mut Context<Self>) {
        self.target.refresh(cx);
        cx.notify();
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

    /// The groups of a page.
    fn groups(&self, groups: impl IntoIterator<Item = Group>) -> Vec<SettingGroup> {
        groups
            .into_iter()
            .map(|g| {
                #[cfg(test)]
                self.built.borrow_mut().push((g.title, g.items.clone()));
                g.items
                    .into_iter()
                    .fold(SettingGroup::new().title(g.title), SettingGroup::item)
            })
            .collect()
    }

    fn editor_page(&self) -> SettingPage {
        let t = self.target.clone();
        let words = t.get(|s| s.spell_words.clone());
        let rules = t.get(|s| s.grammar_rules_off.clone());
        let themes = ThemeChoice::ALL
            .iter()
            .map(|c| (c.key().into(), c.label().into()))
            .collect();
        SettingPage::new("Editor").default_open(true).groups(self.groups([
            Group::new().title("Appearance").item(
                SettingItem::new(
                    "Theme",
                    SettingField::dropdown(
                        themes,
                        {
                            let t = t.clone();
                            move |_| t.get(|s| s.theme).key().into()
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
            Group::new()
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
                            move |_| t1.get(|s| s.font_size() as f64),
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
                        "Vim mode",
                        bool_field(&t, |s| s.vim_mode, |s, v| s.vim_mode = v)
                            .default_value(Settings::default().vim_mode),
                    )
                    .description(
                        "Edit with Vim's keys: normal, insert and visual modes, operators, \
                         motions and text objects (ie/ae for environments, i$/a$ for math), \
                         search and :w/:q. ⌘ shortcuts keep working.",
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
            Group::new().title("Checks").item(
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
            Group::new()
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
            Group::new()
                .title("Grammar")
                .item(
                    SettingItem::new(
                        "Check grammar",
                        bool_field(&t, |s| s.grammar_check, |s, v| s.grammar_check = v)
                            .default_value(Settings::default().grammar_check),
                    )
                    .description(
                        "Underline grammar and style mistakes found by LanguageTool, in the \
                         language of the spelling. LanguageTool runs on your computer: install \
                         it (brew install languagetool, or the server from languagetool.org).",
                    ),
                )
                .item(
                    SettingItem::new("LanguageTool server", {
                        let (t1, t2) = (t.clone(), t.clone());
                        SettingField::input(
                            move |_| t1.get(|s| s.languagetool_url.clone()).into(),
                            move |v: SharedString, cx| {
                                t2.set(cx, |s| s.languagetool_url = v.trim().to_string())
                            },
                        )
                        .default_value(Settings::DEFAULT_LANGUAGETOOL)
                    })
                    .description(
                        "Its address. Text is sent to it as you type: with a server that is \
                         not on this computer, it leaves it.",
                    ),
                )
                .item(
                    SettingItem::new(
                        "Start the server",
                        bool_field(&t, |s| s.languagetool_start, |s, v| s.languagetool_start = v)
                            .default_value(Settings::default().languagetool_start),
                    )
                    .description(
                        "Run languagetool-server when the server is on this computer and not \
                         running yet, and stop it when TeXoria quits.",
                    ),
                )
                .item(removable_list(
                    "grammar-rule",
                    rules,
                    String::clone,
                    "Rules you ignore (right-click a mistake) appear here.",
                    {
                        let t = t.clone();
                        move |rule, cx| t.set(cx, |s| s.grammar_rules_off.retain(|r| *r != rule))
                    },
                )),
        ]))
    }

    fn build_page(&self) -> SettingPage {
        let t = self.target.clone();
        let defaults = Settings::default();
        let engines: Vec<(SharedString, SharedString)> = t
            .shown()
            .engines
            .iter()
            .map(|e| (e.clone(), e.clone()))
            .collect();
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
                        move |_| t.shown().engine.clone().unwrap_or_default()
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
        let live = t.get(|s| s.live_preview);
        SettingPage::new("Build").groups(
            self.groups([
                Group::new().title("Engine").item(engine).item(
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
                Group::new()
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
            ]),
        )
    }

    fn git_page(&self, cx: &mut Context<Self>) -> SettingPage {
        let t = self.target.clone();
        let defaults = Settings::default();
        let view = cx.entity().downgrade();
        SettingPage::new("Git").groups(
            self.groups([
                Group::new()
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
                        .description(
                            "Stop a fetch, pull or push after this many seconds (0: never).",
                        ),
                    ),
                Group::new().title("Overleaf").item(
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
            ]),
        )
    }

    fn render_servers(&self, view: &Entity<Self>) -> Div {
        let rows = self.servers.iter().enumerate().map(|(ix, input)| {
            let view = view.downgrade();
            h_flex()
                .gap_2()
                .child(div().flex_1().child(Input::new(input)))
                .child(
                    ui::icon_button(("remove-server", ix), Glyph::Close, "Remove").on_click(
                        move |_, _, cx| {
                            let _ = view.update(cx, |this, cx| this.remove_server(ix, cx));
                        },
                    ),
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
        let found = t.shown().agents.clone();
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
                            move |_| t.shown().agent.clone().unwrap_or_else(|| first.clone())
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
        SettingPage::new("Agents").groups(
            self.groups([
                Group::new().title("Agent").item(agent),
                Group::new().title("Other agents").item(
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
            ]),
        )
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
                        ui::icon_button(("remove-agent", ix), Glyph::Close, "Remove").on_click(
                            move |_, _, cx| {
                                let _ = view.update(cx, |this, cx| this.remove_agent(ix, cx));
                            },
                        ),
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

    fn trust_page(&self) -> SettingPage {
        let t = self.target.clone();
        let git = t.get(|s| s.trusted_projects.clone());
        let rc = t.get(|s| s.rc_allowed.clone().unwrap_or_default());
        let shell = t.get(|s| s.shell_escape_allowed.clone());
        let agents = t.get(|s| s.agent_projects.clone());
        SettingPage::new("Trusted Projects")
            .resettable(false)
            .description(
                "A project's Git configuration, latexmkrc and documents with shell escape can \
                 run programs, and coding agents read and change its files, so TeXoria asks \
                 before letting them. Removing a folder makes TeXoria ask again.",
            )
            .groups(self.groups([
                Group::new().title("Git").item(path_list(
                    "git",
                    git,
                    "No project lets Git run yet.",
                    {
                        let t = t.clone();
                        move |dir, cx| t.set(cx, |s| s.trusted_projects.retain(|p| *p != dir))
                    },
                )),
                Group::new().title("latexmkrc").item(path_list(
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
                Group::new().title("Shell escape").item(path_list(
                    "shell-escape",
                    shell,
                    "No project may run programs while it builds yet.",
                    {
                        let t = t.clone();
                        move |dir, cx| t.set(cx, |s| s.shell_escape_allowed.retain(|p| *p != dir))
                    },
                )),
                Group::new().title("Coding agents").item(path_list(
                    "agents",
                    agents,
                    "No project lets coding agents work in it yet.",
                    move |dir, cx| t.set(cx, |s| s.agent_projects.retain(|p| *p != dir)),
                )),
            ]))
    }

    fn updates_page(&self) -> SettingPage {
        let t = self.target.clone();
        let how = t.shown().updates.clone();
        // Windows and Linux have no menu bar: this is their Check for Updates….
        let updater = t.shown().updater.clone();
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
        SettingPage::new("Updates").groups(
            self.groups([Group::new()
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
                .item(check)]),
        )
    }
}

/// A group of settings, as [`SettingGroup`] (whose items cannot be read back).
#[derive(Default)]
struct Group {
    title: &'static str,
    items: Vec<SettingItem>,
}

impl Group {
    fn new() -> Self {
        Self::default()
    }

    fn title(mut self, title: &'static str) -> Self {
        self.title = title;
        self
    }

    fn item(mut self, item: SettingItem) -> Self {
        self.items.push(item);
        self
    }
}

/// A switch bound to one `bool` of the settings.
fn bool_field(
    t: &Target,
    get: fn(&Settings) -> bool,
    set: fn(&mut Settings, bool),
) -> SettingField<bool> {
    let (t1, t2) = (t.clone(), t.clone());
    SettingField::switch(move |_| t1.get(get), move |v, cx| t2.set(cx, |s| set(s, v)))
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
        move |_| t1.get(get) as f64,
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
        #[cfg(test)]
        self.built.borrow_mut().clear();
        let pages = [
            self.editor_page(),
            self.build_page(),
            self.git_page(cx),
            self.agents_page(cx),
            self.trust_page(),
            self.updates_page(),
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

#[cfg(test)]
mod tests;
