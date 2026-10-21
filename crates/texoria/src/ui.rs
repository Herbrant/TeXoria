//! Pieces of the interface that several views share.

use std::time::Duration;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{ActiveTheme, Icon, Sizable, h_flex, v_flex};
use gpui_kit::*;

use crate::assets::Glyph;

/// A borderless button showing `icon`, which says what it does in a tooltip.
pub fn icon_button(
    id: impl Into<ElementId>,
    icon: Glyph,
    tooltip: impl Into<SharedString>,
) -> Button {
    Button::new(id).ghost().icon(icon).tooltip(tooltip)
}

/// A strip across the top of the editor: `icon` in `color` on a tint of it, `text`, and
/// `action`.
pub fn banner(
    icon: Glyph,
    color: Hsla,
    tint: f32,
    text: impl Into<SharedString>,
    action: Button,
    cx: &App,
) -> Div {
    let theme = cx.theme();
    h_flex()
        .gap_3()
        .px_3()
        .py_1p5()
        .text_sm()
        .bg(color.opacity(tint))
        .border_b_1()
        .border_color(theme.border)
        .child(Icon::new(icon).xsmall().text_color(color))
        .child(
            div()
                .flex_1()
                .text_color(theme.foreground)
                .child(text.into()),
        )
        .child(action)
}

/// `icon` turning round, for work in progress.
pub fn spinning(icon: Icon, id: impl Into<ElementId>) -> impl IntoElement {
    icon.with_animation(
        id,
        Animation::new(Duration::from_millis(900)).repeat(),
        |icon, t| icon.transform(Transformation::rotate(percentage(t))),
    )
}

/// A sidebar view's column, with its background and the border that separates it from the
/// editor.
pub fn sidebar(cx: &App) -> Div {
    let theme = cx.theme();
    v_flex()
        .size_full()
        .bg(theme.sidebar)
        .border_r_1()
        .border_color(theme.sidebar_border)
}
