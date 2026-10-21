//! The sidebar: its views and the outline.

use super::*;

impl Workspace {
    pub(super) fn show_sidebar(&mut self, kind: Sidebar, cx: &mut Context<Self>) {
        self.show_sidebar = true;
        self.sidebar = kind;
        match kind {
            Sidebar::Outline => self.navigator = kind,
            Sidebar::Files => {
                self.navigator = kind;
                self.files.update(cx, |f, cx| f.refresh(cx));
            }
            Sidebar::Search | Sidebar::SourceControl => {}
            Sidebar::Agent => self.agent.update(cx, |a, cx| a.activate(cx)),
        }
        cx.notify();
    }

    pub(super) fn toggle_sidebar(&mut self, kind: Sidebar, cx: &mut Context<Self>) {
        if self.show_sidebar && self.sidebar == kind {
            self.show_sidebar = false;
            cx.notify();
        } else {
            self.show_sidebar(kind, cx);
        }
    }

    /// The sidebar's Outline and Files views, with a switch between them.
    pub(super) fn render_navigator(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let files = self.sidebar == Sidebar::Files;
        let tab = |id: &'static str, label: &'static str, kind: Sidebar, cx: &mut Context<Self>| {
            let active = self.sidebar == kind;
            let theme = cx.theme();
            div()
                .id(id)
                .px_2p5()
                .py_1()
                .rounded(theme.radius)
                .text_sm()
                .cursor_pointer()
                .when(active, |d| {
                    d.bg(theme.sidebar_accent)
                        .text_color(theme.foreground)
                        .font_weight(FontWeight::MEDIUM)
                })
                .when(!active, |d| {
                    d.text_color(theme.muted_foreground)
                        .hover(|s| s.text_color(theme.foreground))
                })
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| this.show_sidebar(kind, cx)))
        };
        let file_button = |id: &'static str, icon: Glyph, tooltip: &'static str| {
            ui::icon_button(id, icon, tooltip).disabled(self.root.is_none())
        };
        let rows = outline::layout(&self.outline, &self.outline_fold);
        let open = outline::any_open(&rows);
        let header = h_flex()
            .px_3()
            .pt_3()
            .pb_2()
            .gap_1()
            .child(tab("nav-outline", "Outline", Sidebar::Outline, cx))
            .child(tab("nav-files", "Files", Sidebar::Files, cx))
            .child(div().flex_1())
            .when(!files, |d| {
                d.child(
                    ui::icon_button(
                        "outline-fold-all",
                        if open {
                            Glyph::CollapseAll
                        } else {
                            Glyph::ExpandAll
                        },
                        if open { "Collapse All" } else { "Expand All" },
                    )
                    .disabled(!rows.iter().any(|r| r.group.is_some()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.outline_fold.set_all(!open);
                        cx.notify();
                    })),
                )
            })
            .when(files, |d| {
                d.child(
                    file_button("files-new-file", Glyph::NewFile, "New File").on_click(
                        cx.listener(|this, _, window, cx| {
                            this.files
                                .update(cx, |f, cx| f.prompt_new(false, window, cx))
                        }),
                    ),
                )
                .child(
                    file_button("files-new-folder", Glyph::NewFolder, "New Folder").on_click(
                        cx.listener(|this, _, window, cx| {
                            this.files
                                .update(cx, |f, cx| f.prompt_new(true, window, cx))
                        }),
                    ),
                )
                .child(
                    file_button("files-refresh", Glyph::Refresh, "Refresh").on_click(
                        cx.listener(|this, _, _, cx| this.files.update(cx, |f, cx| f.refresh(cx))),
                    ),
                )
            });
        let body = if files {
            div()
                .flex_1()
                .min_h_0()
                .pb_2()
                .child(self.files.clone())
                .into_any_element()
        } else {
            self.render_outline(rows, cx).into_any_element()
        };

        ui::sidebar(cx).child(header).child(body)
    }

    pub(super) fn render_outline(
        &self,
        rows: Vec<outline::Row>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        // The section containing the cursor is highlighted, or the
        // collapsed group it is in.
        let current = self.buffers.get(self.active).and_then(|b| {
            let line = b.editor.read(cx).cursor_position().line;
            let i = self
                .outline
                .iter()
                .enumerate()
                .filter(|(_, o)| o.file == b.path && o.item.line <= line)
                .map(|(i, _)| i)
                .next_back()?;
            outline::shown(&rows, i)
        });

        // Only the rows in view are laid out: a long document has hundreds, and the
        // window is drawn again for every blink of the cursor.
        let rows: Vec<outline::Row> = rows.into_iter().filter(|r| r.visible).collect();
        if self.outline.is_empty() {
            return div()
                .flex_1()
                .px_4()
                .text_base()
                .text_color(theme.muted_foreground)
                .child("No sections yet")
                .into_any_element();
        }
        uniform_list(
            "outline-list",
            rows.len(),
            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                range
                    .map(|k| this.render_outline_row(&rows[k], current, cx))
                    .collect::<Vec<_>>()
            }),
        )
        .flex_1()
        .min_h_0()
        .pb_4()
        .into_any_element()
    }

    /// A row of the outline; `current` is the one holding the cursor.
    pub(super) fn render_outline_row(
        &self,
        row: &outline::Row,
        current: Option<usize>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let theme = cx.theme();
        let i = row.index;
        let o = &self.outline[i];
        let kind = o.item.kind;
        let indent = row.depth as f32 * 12.0;
        let icon = match kind {
            OutlineKind::Figure => Glyph::Figure,
            OutlineKind::Table => Glyph::Table,
            _ => Glyph::Section,
        };
        let active = current == Some(i);
        let title: SharedString = if o.item.title.is_empty() {
            "Untitled".into()
        } else {
            o.item.title.clone().into()
        };
        // A fixed slot, so that titles line up with or without a chevron.
        let chevron = div()
            .id(("outline-fold", i))
            .debug_selector(|| format!("outline-fold {title}"))
            .flex_none()
            .size(px(14.))
            .flex()
            .items_center()
            .justify_center()
            .when_some(row.group.clone(), |d, key| {
                d.child(
                    Icon::new(Glyph::Expand)
                        .small()
                        .text_color(theme.muted_foreground)
                        .when(row.open, |i| {
                            i.transform(Transformation::rotate(percentage(0.25)))
                        }),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.outline_fold.toggle(key.clone());
                    cx.notify();
                }))
            });
        let (file, line) = (o.file.clone(), o.item.line);
        h_flex()
            .id(("outline", i))
            .debug_selector(|| format!("outline {title}"))
            .gap_1p5()
            .pl(px(4.0 + indent))
            .pr_2()
            .py_1()
            .mx_1()
            .rounded(theme.radius)
            .text_base()
            .cursor_pointer()
            .when(active, |d| {
                d.bg(theme.sidebar_accent).text_color(theme.foreground)
            })
            .when(!active, |d| d.text_color(theme.sidebar_foreground))
            .when(kind.depth() <= 2, |d| d.font_weight(FontWeight::MEDIUM))
            .hover(|s| s.bg(theme.sidebar_accent))
            .child(chevron)
            .child(
                Icon::new(icon)
                    .small()
                    .text_color(theme.muted_foreground.opacity(0.8)),
            )
            .child(div().truncate().child(title))
            .on_click(
                cx.listener(move |this, _, window, cx| this.reveal(file.clone(), line, window, cx)),
            )
    }
}
