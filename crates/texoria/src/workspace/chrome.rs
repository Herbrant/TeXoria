//! The title bar and the status bar.

use super::*;

impl Workspace {
    pub(super) fn render_title_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let title: SharedString = self
            .root
            .as_ref()
            .and_then(|r| r.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "TeXoria".into())
            .into();
        let subtitle: Option<SharedString> = self
            .root
            .as_ref()
            .and_then(|r| r.parent())
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned().into());
        let changes = self.git.read(cx).change_count();
        let has_project = self.root.is_some();

        // On Windows the whole title bar is a drag area: a press there is the
        // system's (it moves the window) unless something on top blocks it, so
        // the buttons are grouped in boxes that do. This also keeps a drag that
        // starts on a button from moving the window elsewhere.
        let buttons = || h_flex().gap_1().h_full().occlude();
        TitleBar::new().child(
            h_flex()
                .flex_1()
                .h_full()
                .gap_1()
                .child(
                    buttons()
                        .child(
                            ui::icon_button(
                                "toggle-outline",
                                Glyph::PanelLeft,
                                "Outline and files (⌘1, ⇧⌘E)",
                            )
                            .on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.toggle_sidebar(this.navigator, cx)
                                }),
                            ),
                        )
                        .when(has_project, |d| {
                            d.child(
                                ui::icon_button(
                                    "toggle-search",
                                    Glyph::Search,
                                    "Find in project (⇧⌘F)",
                                )
                                .on_click(cx.listener(
                                    |this, _, window, cx| {
                                        if this.show_sidebar && this.sidebar == Sidebar::Search {
                                            this.toggle_sidebar(Sidebar::Search, cx);
                                        } else {
                                            this.find_in_project(window, cx);
                                        }
                                    },
                                )),
                            )
                        })
                        .when(has_project, |d| {
                            d.child(
                                Button::new("toggle-git")
                                    .ghost()
                                    .icon(Glyph::Branch)
                                    .when(changes > 0, |b| b.label(changes.to_string()))
                                    .tooltip("Source control (⇧⌘G)")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.toggle_sidebar(Sidebar::SourceControl, cx)
                                    })),
                            )
                        })
                        .when(has_project, |d| {
                            d.child(
                                ui::icon_button("toggle-agent", Glyph::Sparkles, "Agent (⇧⌘K)")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.toggle_agent(window, cx)
                                    })),
                            )
                        }),
                )
                .child(
                    h_flex()
                        .flex_1()
                        .justify_center()
                        .gap_2()
                        .text_sm()
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.foreground)
                                .child(title),
                        )
                        .when_some(subtitle, |el, s| {
                            el.child(
                                div()
                                    .text_color(theme.muted_foreground)
                                    .child(format!("— {s}")),
                            )
                        }),
                )
                .child(
                    buttons()
                        .child(self.render_status_pill(cx))
                        .child(
                            Button::new("build")
                                .primary()
                                .icon(Glyph::Play)
                                .label("Build")
                                .tooltip("Build now (⌘R)")
                                .on_click(cx.listener(|this, _, _, cx| this.build(cx))),
                        )
                        .child(
                            ui::icon_button(
                                "theme",
                                if cx.theme_is_dark() {
                                    Glyph::Sun
                                } else {
                                    Glyph::Moon
                                },
                                "Toggle light/dark",
                            )
                            .on_click(
                                cx.listener(|this, _, window, cx| this.toggle_theme(window, cx)),
                            ),
                        )
                        .child(
                            ui::icon_button("settings", Glyph::Settings, "Settings (⌘,)").on_click(
                                cx.listener(|this, _, window, cx| this.open_settings(window, cx)),
                            ),
                        ),
                )
                .child(div().w(px(6.))),
        )
    }

    pub(super) fn render_status_pill(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (icon, color, label): (Glyph, Hsla, SharedString) = match &self.state {
            CompileState::Idle => (Glyph::Info, theme.muted_foreground, "Ready".into()),
            CompileState::Running => (Glyph::Spinner, theme.primary, "Building…".into()),
            CompileState::Done {
                status: Status::Success,
                elapsed,
                warnings,
                ..
            } => (
                Glyph::Ok,
                theme.success,
                if *warnings > 0 {
                    format!(
                        "{:.1}s · {warnings} warning{}",
                        elapsed.as_secs_f32(),
                        plural(*warnings)
                    )
                    .into()
                } else {
                    format!("Built in {:.1}s", elapsed.as_secs_f32()).into()
                },
            ),
            CompileState::Done {
                errors, warnings, ..
            } => (
                Glyph::Error,
                theme.danger,
                if *errors > 0 {
                    format!("{errors} error{}", plural(*errors)).into()
                } else {
                    format!("Failed · {warnings} warning{}", plural(*warnings)).into()
                },
            ),
            CompileState::Failed(msg) => (Glyph::Warning, theme.warning, msg.clone()),
        };
        let spinning = matches!(self.state, CompileState::Running);
        let icon = Icon::new(icon).xsmall().text_color(color);
        h_flex()
            .id("status-pill")
            .gap_1p5()
            .px_2p5()
            .py_0p5()
            .max_w(px(360.))
            .rounded_full()
            .bg(theme.muted)
            .text_xs()
            .text_color(theme.muted_foreground)
            .cursor_pointer()
            .hover(|s| s.bg(theme.accent))
            .child(if spinning {
                ui::spinning(icon, "spin").into_any_element()
            } else {
                icon.into_any_element()
            })
            .child(div().truncate().child(label))
            .on_click(cx.listener(|this, _, _, cx| {
                this.show_diagnostics = !this.show_diagnostics;
                cx.notify();
            }))
    }

    pub(super) fn render_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let update = self.render_update(cx);
        let grammar = self.render_grammar(cx);
        let language =
            (self.settings.spell_check && !self.diff_active).then(|| self.render_language(cx));
        let theme = cx.theme();
        let cursor = self
            .buffers
            .get(self.active)
            .map(|b| b.editor.read(cx).cursor_position());
        let (errors, warnings) = self.problem_counts();
        let engine: SharedString = self
            .engines
            .get(self.engine_ix)
            .map(|e| e.label())
            .unwrap_or_else(|| "No engine".into())
            .into();
        let live = self.settings.live_preview;
        let vim_status = self
            .vim
            .as_ref()
            .filter(|_| !self.diff_active)
            .map(|v| (format!("-- {} --", v.mode().label()), v.status()));
        let words = (!self.word_counts.is_empty()).then(|| {
            let total = thousands(self.total_words().words());
            let selected = self
                .buffers
                .get(self.active)
                .and_then(|b| selection(&b.editor, cx))
                .map(|(_, _, text)| selected_words(&text));
            match selected {
                Some(n) => format!("{} of {total} words", thousands(n)),
                None => format!("{total} words"),
            }
        });
        let git = self.git.read(cx);
        let branch: Option<SharedString> = git.repo().map(|_| {
            let st = git.status();
            let mut label = st
                .branch
                .clone()
                .or_else(|| st.oid.as_ref().map(|o| o.chars().take(7).collect()))
                .unwrap_or_else(|| "No branch".into());
            if !st.is_clean() {
                label.push('*');
            }
            if st.ahead > 0 {
                label.push_str(&format!(" ↑{}", st.ahead));
            }
            if st.behind > 0 {
                label.push_str(&format!(" ↓{}", st.behind));
            }
            label.into()
        });

        // Clickable items, as big as the buttons next to them.
        let item = |id: &'static str| {
            h_flex()
                .id(id)
                .h_8()
                .px_2()
                .gap_1p5()
                .rounded(theme.radius)
                .cursor_pointer()
                .hover(|s| s.bg(theme.accent).text_color(theme.foreground))
        };
        h_flex()
            .h(px(36.))
            .px_1()
            .gap_1()
            .text_sm()
            .text_color(theme.muted_foreground)
            .bg(theme.status_bar)
            .border_t_1()
            .border_color(theme.status_bar_border)
            .child(
                item("problems")
                    .gap_3()
                    .child(
                        h_flex()
                            .gap_1()
                            .child(Icon::new(Glyph::Error).size_4())
                            .child(errors.to_string()),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .child(Icon::new(Glyph::Warning).size_4())
                            .child(warnings.to_string()),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_diagnostics = !this.show_diagnostics;
                        cx.notify();
                    })),
            )
            .when_some(cursor, |el, c| {
                el.child(
                    div()
                        .px_2()
                        .child(format!("Ln {}, Col {}", c.line + 1, c.character + 1)),
                )
            })
            .when_some(vim_status, |el, (mode, typed)| {
                el.child(
                    h_flex()
                        .px_2()
                        .gap_2()
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.primary)
                                .child(mode),
                        )
                        .child(
                            div()
                                .font_family(theme.mono_font_family.clone())
                                .child(typed),
                        ),
                )
            })
            .when_some(branch, |el, branch| {
                el.child(
                    item("git-status")
                        .child(Icon::new(Glyph::Branch).size_4())
                        .child(branch)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.show_sidebar(Sidebar::SourceControl, cx)
                        })),
                )
            })
            .child(div().flex_1())
            .when_some(words, |el, words| {
                el.child(
                    item("words").child(words).on_click(
                        cx.listener(|this, _, window, cx| this.show_word_count(window, cx)),
                    ),
                )
            })
            .children(update)
            .children(grammar)
            .children(language)
            .child(
                item("live")
                    .when(live, |d| d.text_color(theme.primary))
                    .child(if live { "● Live" } else { "○ Live" })
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_live(cx))),
            )
            .child(
                item("engine")
                    .child(engine)
                    .on_click(cx.listener(|this, _, _, cx| this.next_engine(cx))),
            )
    }
}
