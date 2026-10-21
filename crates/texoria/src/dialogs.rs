//! The app's dialogs: forms with Cancel and OK buttons, a question asking for one line of
//! text, and the confirmation of what cannot be undone.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::component::dialog::{DialogAction, DialogClose, DialogFooter};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{
    ActiveTheme, WindowExt as _,
    button::{Button, ButtonVariant, ButtonVariants},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

/// A dialog with a body and Cancel / `ok` buttons; `on_ok` returns whether
/// to close it.
pub fn open_form(
    window: &mut Window,
    cx: &mut App,
    title: &'static str,
    ok: &'static str,
    focus: Option<FocusHandle>,
    body: impl Fn(&mut Window, &mut App) -> AnyElement + 'static,
    on_ok: impl Fn(&mut Window, &mut App) -> bool + 'static,
) {
    let on_ok = Rc::new(on_ok);
    let first = Cell::new(true);
    window.open_dialog(cx, move |dialog, window, cx| {
        if first.replace(false)
            && let Some(f) = &focus
        {
            f.focus(window, cx);
        }
        let on_ok = on_ok.clone();
        dialog
            .title(title)
            .w(px(440.))
            .child(body(window, cx))
            .footer(
                DialogFooter::new()
                    .child(DialogClose::new().trigger(|b| b.outline().label("Cancel")))
                    .child(DialogAction::new().child(Button::new("ok").primary().label(ok))),
            )
            .on_ok(move |_, window, cx| on_ok(window, cx))
    });
}

/// A form's body: `caption` above `input`, and `error` under it.
pub fn caption_input(
    caption: impl Into<SharedString>,
    input: &Entity<InputState>,
    error: Option<&'static str>,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    v_flex()
        .gap_2()
        .child(
            div()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(caption.into()),
        )
        .child(Input::new(input))
        .when_some(error, |d, e| {
            d.child(div().text_sm().text_color(theme.danger).child(e))
        })
        .into_any_element()
}

/// Ask for one line of text: `caption` above an input showing `placeholder`. OK hands the
/// text, trimmed, to `on_submit` (not while it is empty), which returns whether to close.
pub fn prompt_text(
    window: &mut Window,
    cx: &mut App,
    title: &'static str,
    ok: &'static str,
    placeholder: &'static str,
    caption: &'static str,
    on_submit: impl Fn(String, &mut Window, &mut App) -> bool + 'static,
) {
    let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
    let focus = input.focus_handle(cx);
    let body = input.clone();
    open_form(
        window,
        cx,
        title,
        ok,
        Some(focus),
        move |_, cx| caption_input(caption, &body, None, cx),
        move |window, cx| {
            let text = input.read(cx).value().trim().to_string();
            !text.is_empty() && on_submit(text, window, cx)
        },
    );
}

/// Ask whether to do what cannot be undone: `ok` (a danger button) runs `on_ok`.
pub fn confirm_danger(
    window: &mut Window,
    cx: &mut App,
    title: SharedString,
    description: SharedString,
    ok: &'static str,
    on_ok: impl Fn(&mut App) + 'static,
) {
    let on_ok = Rc::new(on_ok);
    window.open_alert_dialog(cx, move |alert, _, _| {
        let on_ok = on_ok.clone();
        alert
            .title(title.clone())
            .description(description.clone())
            .confirm()
            .ok_text(ok)
            .ok_variant(ButtonVariant::Danger)
            .on_ok(move |_, _, cx| {
                on_ok(cx);
                true
            })
    });
}
