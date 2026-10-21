//! The editor area: tabs, banners and the problems list.

use super::*;

impl Workspace {
    /// Scrolls the tab bar to the active tab whenever it changes (a new file's is the last one),
    /// and only then, so the bar stays where the user scrolled it.
    pub(super) fn follow_active_tab(&mut self) {
        let shown = (self.active, self.diff_active, self.buffers.len());
        if self.tab_shown != Some(shown) {
            self.tab_shown = Some(shown);
            let ix = if self.diff_active {
                self.buffers.len()
            } else {
                self.active
            };
            self.tab_scroll.scroll_to_item(ix);
        }
    }

    pub(super) fn render_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let root_dir = self.project_dir();
        let diff_title: Option<SharedString> =
            self.diff.as_ref().map(|d| d.read(cx).title().into());
        // The last tab stays, so it has no close button.
        let closable = self.buffers.len() > 1;
        // Tabs keep their width and the bar scrolls (swipe or wheel) when they overflow.
        h_flex()
            .id("tab-bar")
            .debug_selector(|| "tab-bar".into())
            .flex_none()
            .overflow_x_scroll()
            .track_scroll(&self.tab_scroll)
            .h(px(34.))
            .px_2()
            .gap_1()
            .bg(theme.tab_bar)
            .border_b_1()
            .border_color(theme.border)
            .children(self.buffers.iter().enumerate().map(|(i, b)| {
                let name = crate::paths::relative(&b.path, root_dir.as_deref())
                    .to_string_lossy()
                    .into_owned();
                let active = i == self.active && !self.diff_active;
                let selector = format!("tab {name}");
                h_flex()
                    .id(("tab", i))
                    .debug_selector(|| selector)
                    .flex_none()
                    .whitespace_nowrap()
                    .gap_1p5()
                    .pl_3()
                    .when(closable, |d| d.pr_1())
                    .when(!closable, |d| d.pr_3())
                    .h(px(26.))
                    .rounded(theme.radius)
                    .text_xs()
                    .cursor_pointer()
                    .when(active, |d| {
                        d.bg(theme.tab_active)
                            .text_color(theme.tab_active_foreground)
                            .shadow_xs()
                    })
                    .when(!active, |d| {
                        d.text_color(theme.tab_foreground)
                            .hover(|s| s.bg(theme.accent))
                    })
                    .child(Icon::new(Glyph::File).xsmall())
                    .child(name)
                    .when(b.dirty, |d| {
                        d.child(div().size(px(6.)).rounded_full().bg(theme.muted_foreground))
                    })
                    .when(closable, |d| {
                        d.child(
                            div()
                                .id(("tab-close", i))
                                .p_0p5()
                                .rounded(theme.radius)
                                .hover(|s| s.bg(theme.muted))
                                .child(Icon::new(Glyph::Close).xsmall())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.close_buffer(i, cx);
                                })),
                        )
                        // A middle click closes the tab, as in most editors.
                        .on_aux_click(cx.listener(
                            move |this, e: &ClickEvent, _, cx| {
                                if e.is_middle_click() {
                                    this.close_buffer(i, cx);
                                }
                            },
                        ))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.active = i;
                        this.diff_active = false;
                        this.remember_session();
                        cx.notify();
                    }))
            }))
            .when_some(diff_title, |d, title| {
                let active = self.diff_active;
                d.child(
                    h_flex()
                        .id("tab-diff")
                        .flex_none()
                        .whitespace_nowrap()
                        .gap_1p5()
                        .pl_3()
                        .pr_1()
                        .h(px(26.))
                        .rounded(theme.radius)
                        .text_xs()
                        .italic()
                        .cursor_pointer()
                        .when(active, |d| {
                            d.bg(theme.tab_active)
                                .text_color(theme.tab_active_foreground)
                                .shadow_xs()
                        })
                        .when(!active, |d| {
                            d.text_color(theme.tab_foreground)
                                .hover(|s| s.bg(theme.accent))
                        })
                        .child(Icon::new(Glyph::Branch).xsmall())
                        .child(title)
                        .child(
                            div()
                                .id("tab-diff-close")
                                .p_0p5()
                                .rounded(theme.radius)
                                .hover(|s| s.bg(theme.muted))
                                .child(Icon::new(Glyph::Close).xsmall())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.close_diff(cx);
                                })),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.diff_active = true;
                            cx.notify();
                        })),
                )
            })
    }

    pub(super) fn render_trust_banner(&self, cx: &mut Context<Self>) -> impl IntoElement {
        ui::banner(
            Glyph::Warning,
            cx.theme().warning,
            0.12,
            "This project's latexmkrc can run arbitrary commands and is ignored until you \
             allow it.",
            Button::new("allow-rc")
                .warning()
                .label("Allow latexmkrc")
                .on_click(cx.listener(|this, _, _, cx| this.allow_rc(cx))),
            cx,
        )
    }

    pub(super) fn render_shell_escape_banner(&self, cx: &mut Context<Self>) -> impl IntoElement {
        ui::banner(
            Glyph::Warning,
            cx.theme().warning,
            0.12,
            "This document runs programs while it builds (shell escape, for minted, svg, \
             pythontex…). Allow it only if you trust where the project comes from.",
            Button::new("allow-shell-escape")
                .warning()
                .label("Allow Shell Escape")
                .on_click(cx.listener(|this, _, window, cx| this.allow_shell_escape(window, cx))),
            cx,
        )
    }

    /// Editing is paused while the agent works.
    pub(super) fn render_agent_banner(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let name = self
            .agent
            .read(cx)
            .agent_name()
            .unwrap_or_else(|| "The agent".into());
        ui::banner(
            Glyph::Sparkles,
            cx.theme().primary,
            0.10,
            format!("{name} is working: editing is paused until it is done."),
            Button::new("agent-banner-stop")
                .outline()
                .icon(Glyph::Stop)
                .label("Stop")
                .on_click(
                    cx.listener(|this, _, _, cx| this.agent.update(cx, |a, cx| a.cancel(cx))),
                ),
            cx,
        )
    }

    pub(super) fn render_diagnostics(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let root_dir = self.project_dir();
        let problems = self.problems();
        let rows = problems
            .iter()
            .filter(|d| d.severity != Severity::Info)
            .enumerate()
            .map(|(i, d)| {
                let (icon, color) = match d.severity {
                    Severity::Error => (Glyph::Error, theme.danger),
                    Severity::Warning => (Glyph::Warning, theme.warning),
                    Severity::Info => (Glyph::Info, theme.muted_foreground),
                };
                let location = match (&d.file, d.line) {
                    (Some(f), Some(l)) => {
                        let rel = crate::paths::relative(f, root_dir.as_deref());
                        format!("{}:{l}", rel.display())
                    }
                    (Some(f), None) => crate::paths::file_name(f),
                    _ => String::new(),
                };
                let target = d.file.clone().zip(d.line);
                h_flex()
                    .id(("diag", i))
                    .gap_2()
                    .px_3()
                    .py_1()
                    .text_sm()
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.accent))
                    .child(Icon::new(icon).xsmall().text_color(color))
                    .child(
                        div()
                            .flex_1()
                            .truncate()
                            .text_color(theme.foreground)
                            .child(d.message.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(location),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if let Some((file, line)) = target.clone() {
                            this.reveal(file, line.saturating_sub(1), window, cx);
                        }
                    }))
            })
            .collect::<Vec<_>>();
        let empty = rows.is_empty();
        let fixable = !empty && self.project_dir().is_some() && !self.agent_busy;

        v_flex()
            .h(px(170.))
            .border_t_1()
            .border_color(theme.border)
            .bg(theme.sidebar)
            .child(
                h_flex()
                    .px_3()
                    .py_1p5()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.muted_foreground)
                    .child(div().flex_1().child("PROBLEMS"))
                    .when(fixable, |d| {
                        d.child(
                            Button::new("fix-with-agent")
                                .ghost()
                                .icon(Glyph::Sparkles)
                                .label("Fix with Agent")
                                .tooltip("Ask the coding agent to fix these problems")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.fix_with_agent(window, cx)
                                })),
                        )
                    })
                    .child(
                        Button::new("close-diag")
                            .ghost()
                            .icon(Glyph::Close)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.show_diagnostics = false;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                v_flex()
                    .id("diag-list")
                    .flex_1()
                    .overflow_y_scroll()
                    .when(empty, |d| {
                        d.child(
                            div()
                                .px_3()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child("No problems — nicely done."),
                        )
                    })
                    .children(rows),
            )
    }
}
