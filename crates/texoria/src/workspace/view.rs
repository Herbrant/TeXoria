//! The workspace's window: its layout and the actions it handles.

use super::*;

impl Render for Workspace {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_theme(cx);
        if self.vim.is_some() {
            self.update_block(cx);
        }
        let theme = cx.theme().clone();

        let body = if self.buffers.is_empty() {
            self.render_welcome(cx).into_any_element()
        } else {
            self.follow_active_tab();
            let center = match self.diff.clone().filter(|_| self.diff_active) {
                Some(diff) => diff.into_any_element(),
                None => Editor::new(&self.buffers[self.active].editor)
                    .context_menu({
                        let b = &self.buffers[self.active];
                        let spelling = spell::context_menu(
                            b.editor.clone(),
                            b.spell.underlines.clone(),
                            b.spell.speller.clone(),
                        );
                        let editor = b.editor.clone();
                        let grammar = b.grammar.context_menu(b.editor.clone());
                        move |menu, window, cx| {
                            let selected = !editor.read(cx).selected_range().is_empty();
                            let mut menu = if selected { agent_menu(menu) } else { menu };
                            menu = grammar(menu, cx);
                            if let Some(name) = latex::package_at_cursor(&editor, cx) {
                                menu = menu
                                    .menu(
                                        format!("Documentation of {name}"),
                                        Box::new(PackageDocs { name }),
                                    )
                                    .separator();
                            }
                            menu = menu
                                .menu("Show in PDF", Box::new(ForwardSearch))
                                .separator();
                            spelling(menu, window, cx)
                        }
                    })
                    // The element sets the editor's mode on every frame: what
                    // `set_agent_busy` set would not last.
                    .readonly(self.agent_busy)
                    .bordered(false)
                    .h(relative(1.))
                    .font_family(theme.mono_font_family.clone())
                    .text_size(px(self.settings.font_size()))
                    .line_height(relative(1.6))
                    .into_any_element(),
            };
            let editor_column = v_flex()
                .size_full()
                .bg(theme.background)
                .when(self.untrusted_rc, |d| d.child(self.render_trust_banner(cx)))
                .when(self.wants_shell_escape, |d| {
                    d.child(self.render_shell_escape_banner(cx))
                })
                .when(self.agent_busy, |d| d.child(self.render_agent_banner(cx)))
                .when(self.buffers.len() > 1 || self.diff.is_some(), |d| {
                    d.child(self.render_tabs(cx))
                })
                .child(div().flex_1().min_h_0().child(center))
                .when(self.show_diagnostics, |d| {
                    d.child(self.render_diagnostics(cx))
                });
            let active = (!self.diff_active).then(|| self.buffers[self.active].path.clone());
            self.files.update(cx, |f, cx| f.set_active(active, cx));
            let sidebar = match self.sidebar {
                Sidebar::Outline | Sidebar::Files => self.render_navigator(cx).into_any_element(),
                Sidebar::Search => div()
                    .size_full()
                    .bg(theme.sidebar)
                    .border_r_1()
                    .border_color(theme.sidebar_border)
                    .child(self.search.clone())
                    .into_any_element(),
                Sidebar::SourceControl => self.git.clone().into_any_element(),
                Sidebar::Agent => self.agent.clone().into_any_element(),
            };

            h_resizable("main")
                .child(
                    resizable_panel()
                        .size(px(320.))
                        .size_range(px(200.)..px(640.))
                        .visible(self.show_sidebar)
                        .child(sidebar),
                )
                .child(resizable_panel().child(editor_column))
                .child(
                    resizable_panel()
                        .size(px(620.))
                        .visible(!self.preview_detached)
                        .child(
                            div()
                                .size_full()
                                .border_l_1()
                                .border_color(theme.border)
                                .child(self.pdf.clone()),
                        ),
                )
                .into_any_element()
        };

        v_flex()
            .id("workspace")
            .key_context("Workspace")
            .track_focus(&self.focus)
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .font_family(theme.font_family.clone())
            .on_action(
                cx.listener(|this, _: &Open, window, cx| this.prompt_open(false, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &OpenFolder, window, cx| this.prompt_open(true, window, cx)),
            )
            .on_action(cx.listener(|this, _: &Save, window, cx| this.save(window, cx)))
            .on_action(
                cx.listener(|this, _: &FormatDocument, window, cx| {
                    this.format_document(window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &Build, _, cx| this.build(cx)))
            .on_action(cx.listener(|this, _: &CheckForUpdates, _, cx| {
                this.updater.update(cx, |u, cx| u.check(true, cx))
            }))
            .on_action(cx.listener(|this, _: &CancelBuild, _, _| this.cancel_build()))
            .on_action(cx.listener(|this, _: &ForwardSearch, _, cx| this.forward_search(cx)))
            .on_action(
                cx.listener(|this, _: &ToggleTheme, window, cx| this.toggle_theme(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &OpenSettings, window, cx| this.open_settings(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &ZoomIn, _, cx| this.pdf.update(cx, |p, cx| p.zoom_in(cx))),
            )
            .on_action(
                cx.listener(|this, _: &ZoomOut, _, cx| this.pdf.update(cx, |p, cx| p.zoom_out(cx))),
            )
            .on_action(cx.listener(|this, _: &crate::pdf_pane::FitPage, _, cx| {
                this.pdf.update(cx, |p, cx| p.fit_page(cx))
            }))
            .on_action(
                cx.listener(|this, _: &crate::pdf_pane::FindInPdf, window, cx| {
                    this.in_pdf_window(window, cx, PdfPane::open_find)
                }),
            )
            .on_action(cx.listener(|this, _: &TogglePreviewWindow, window, cx| {
                this.toggle_preview_window(window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &AddCitation, window, cx| this.prompt_citation(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &crate::pdf_pane::GoToPage, window, cx| {
                    this.in_pdf_window(window, cx, PdfPane::prompt_page)
                }),
            )
            .on_action(cx.listener(|this, _: &ZoomToFit, _, cx| {
                this.pdf.update(cx, |p, cx| p.zoom_to_fit(cx))
            }))
            .on_action(cx.listener(|this, _: &IncreaseFontSize, window, cx| {
                this.set_font_size(this.settings.font_size() + 1., window, cx)
            }))
            .on_action(cx.listener(|this, _: &DecreaseFontSize, window, cx| {
                this.set_font_size(this.settings.font_size() - 1., window, cx)
            }))
            .on_action(cx.listener(|this, _: &ResetFontSize, window, cx| {
                this.set_font_size(Settings::DEFAULT_FONT_SIZE, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleOutline, _, cx| {
                this.toggle_sidebar(Sidebar::Outline, cx)
            }))
            .on_action(
                cx.listener(|this, _: &ToggleFiles, _, cx| this.toggle_sidebar(Sidebar::Files, cx)),
            )
            .on_action(cx.listener(|this, _: &ShowSourceControl, window, cx| {
                this.show_sidebar(Sidebar::SourceControl, cx);
                this.git.update(cx, |g, cx| g.focus_message(window, cx));
            }))
            .on_action(cx.listener(|this, _: &Commit, window, cx| {
                this.show_sidebar(Sidebar::SourceControl, cx);
                this.git.update(cx, |g, cx| g.commit(window, cx));
            }))
            .on_action(
                cx.listener(|this, _: &Fetch, _, cx| this.git.update(cx, |g, cx| g.fetch(cx))),
            )
            .on_action(cx.listener(|this, _: &Pull, _, cx| this.git.update(cx, |g, cx| g.pull(cx))))
            .on_action(cx.listener(|this, _: &Push, window, cx| {
                this.git.update(cx, |g, cx| g.push(window, cx))
            }))
            .on_action(cx.listener(|this, _: &SyncOverleaf, window, cx| {
                this.git.update(cx, |g, cx| g.sync(window, cx))
            }))
            .on_action(cx.listener(|this, _: &NewBranch, window, cx| {
                this.git.update(cx, |g, cx| g.prompt_new_branch(window, cx))
            }))
            .on_action(
                cx.listener(|this, _: &CloneRepository, window, cx| this.prompt_clone(window, cx)),
            )
            .on_action(cx.listener(|this, _: &OpenFromOverleaf, window, cx| {
                this.prompt_overleaf(window, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleDiagnostics, _, cx| {
                this.show_diagnostics = !this.show_diagnostics;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ToggleLivePreview, _, cx| this.toggle_live(cx)))
            .on_action(cx.listener(|this, _: &ToggleSoftWrap, window, cx| {
                if let Some(b) = this.buffers.get_mut(this.active) {
                    b.soft_wrap = !b.soft_wrap;
                    let wrap = b.soft_wrap;
                    b.editor
                        .update(cx, |state, cx| state.set_soft_wrap(wrap, window, cx));
                }
            }))
            .on_action(cx.listener(|this, _: &CloseTab, _, cx| this.close_tab(cx)))
            .on_action(
                cx.listener(|this, _: &ToggleAgent, window, cx| this.toggle_agent(window, cx)),
            )
            .on_action(cx.listener(|this, _: &EditWithAgent, window, cx| {
                this.prompt_edit_with_agent(window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &FixWithAgent, window, cx| this.fix_with_agent(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &StopAgent, _, cx| {
                    this.agent.update(cx, |a, cx| a.cancel(cx))
                }),
            )
            .on_action(cx.listener(|this, a: &RewriteSelection, window, cx| {
                this.rewrite_selection(a.how.clone(), window, cx)
            }))
            .on_action(cx.listener(|this, a: &OpenLocation, window, cx| {
                this.reveal_range(a.file.clone(), a.range.clone(), window, cx)
            }))
            .on_action(cx.listener(|this, _: &ExportPdf, window, cx| this.export_pdf(window, cx)))
            .on_action(
                cx.listener(|this, _: &FindInProject, window, cx| this.find_in_project(window, cx)),
            )
            .on_action(cx.listener(|this, a: &PackageDocs, window, cx| {
                this.package_docs(a.name.clone(), window, cx)
            }))
            .on_action(cx.listener(|this, a: &OpenExternal, window, cx| {
                this.open_external(&a.path, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleVimMode, window, cx| {
                let on = !this.settings.vim_mode;
                this.update_settings(|s| s.vim_mode = on, window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &ToggleComment, window, cx| this.toggle_comment(window, cx)),
            )
            .on_action(cx.listener(|this, a: &ReplaceMisspelling, window, cx| {
                this.replace_misspelling(a, window, cx)
            }))
            .on_action(cx.listener(|this, a: &IgnoreGrammarRule, window, cx| {
                let rule = a.rule.clone();
                this.update_settings(
                    |s| {
                        if !s.grammar_rules_off.contains(&rule) {
                            s.grammar_rules_off.push(rule);
                        }
                    },
                    window,
                    cx,
                )
            }))
            .on_action(cx.listener(|this, a: &AddToDictionary, window, cx| {
                this.update_settings(|s| s.add_spell_word(&a.word), window, cx)
            }))
            .child(self.render_title_bar(cx))
            .child(div().flex_1().min_h_0().child(body))
            .when(!self.buffers.is_empty(), |d| {
                d.child(self.render_status_bar(cx))
            })
    }
}
