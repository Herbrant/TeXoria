//! The welcome screen, before a project is open.

use super::*;

impl Workspace {
    pub(super) fn render_welcome(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let recent =
            self.settings
                .recent
                .iter()
                .take(6)
                .enumerate()
                .map(|(i, p)| {
                    let p = p.clone();
                    let name = crate::paths::file_name(&p);
                    let dir = p
                        .parent()
                        .map(|d| d.display().to_string())
                        .unwrap_or_default();
                    h_flex()
                        .id(("recent", i))
                        .w(px(420.))
                        .gap_3()
                        .px_3()
                        .py_2()
                        .rounded(theme.radius)
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.accent))
                        .child(
                            Icon::new(Glyph::File)
                                .small()
                                .text_color(theme.muted_foreground),
                        )
                        .child(
                            v_flex().child(div().text_sm().child(name)).child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .truncate()
                                    .child(dir),
                            ),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_path(p.clone(), window, cx)
                        }))
                });
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_6()
            .bg(theme.background)
            .child(
                v_flex()
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .text_3xl()
                            .font_weight(FontWeight::LIGHT)
                            .text_color(theme.foreground)
                            .child("TeXoria"),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child("Beautiful documents, beautifully written."),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("open")
                            .primary()
                            .icon(Glyph::Open)
                            .label("Open a document…")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.prompt_open(window, cx)),
                            ),
                    )
                    .child(
                        Button::new("clone")
                            .outline()
                            .icon(Glyph::Branch)
                            .label("Clone a repository…")
                            .disabled(self.cloning.is_some())
                            .on_click(
                                cx.listener(|this, _, window, cx| this.prompt_clone(window, cx)),
                            ),
                    )
                    .child(
                        Button::new("overleaf")
                            .outline()
                            .icon(Glyph::Branch)
                            .label("Open from Overleaf…")
                            .disabled(self.cloning.is_some())
                            .on_click(
                                cx.listener(|this, _, window, cx| this.prompt_overleaf(window, cx)),
                            ),
                    ),
            )
            .when_some(self.cloning.clone(), |d, (url, cancel)| {
                d.child(
                    h_flex()
                        .gap_2()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(ui::spinning(
                            Icon::new(Glyph::Spinner).xsmall(),
                            "clone-spin",
                        ))
                        .child(
                            div()
                                .max_w(px(360.))
                                .truncate()
                                .child(format!("Cloning {url}…")),
                        )
                        .child(
                            Button::new("clone-cancel")
                                .ghost()
                                .label("Cancel")
                                .on_click(move |_, _, _| cancel.cancel()),
                        ),
                )
            })
            .child(v_flex().gap_0p5().children(recent))
    }
}
