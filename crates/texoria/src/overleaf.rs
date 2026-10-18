//! Overleaf projects through Overleaf's Git integration. Git signs in with an
//! authentication token, kept in the system keychain (one per server).

use std::collections::HashMap;

use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{
    ActiveTheme, WindowExt as _,
    button::{Button, ButtonVariants},
    v_flex,
};
use gpui_kit::*;
use tex_git::{Credential, overleaf};

pub const TOKEN_NEEDED: &str = "Overleaf asks Git for a token instead of your password.";
pub const TOKEN_REJECTED: &str = "Overleaf refused access: the token may have expired \
     (tokens last a year) or been deleted, or your account may have lost access to this project.";

/// Tokens by server, read from the keychain at most once per run. A token
/// also lives here alone when the keychain cannot store it.
#[derive(Default)]
struct Tokens(HashMap<String, Option<String>>);

impl Global for Tokens {}

pub fn credential(server: &str, token: String) -> Credential {
    Credential {
        url: server.to_string(),
        username: overleaf::USERNAME.to_string(),
        password: token,
    }
}

/// The token for `server`, if the user gave one.
pub fn load_token(server: &str, cx: &mut App) -> Task<Option<String>> {
    if let Some(token) = cx.try_global::<Tokens>().and_then(|t| t.0.get(server)) {
        return Task::ready(token.clone());
    }
    let read = cx.read_credentials(server);
    let server = server.to_string();
    cx.spawn(async move |cx| {
        let token = match read.await {
            Ok(found) => found
                .and_then(|(_, secret)| String::from_utf8(secret).ok())
                .filter(|t| overleaf::valid_token(t)),
            Err(e) => {
                log::warn!("reading the token for {server}: {e:#}");
                None
            }
        };
        cx.update(|cx| {
            cx.default_global::<Tokens>()
                .0
                .insert(server, token.clone())
        });
        token
    })
}

pub fn save_token(server: &str, token: &str, window: &mut Window, cx: &mut App) {
    let known = cx.try_global::<Tokens>().and_then(|t| t.0.get(server));
    if known.is_some_and(|t| t.as_deref() == Some(token)) {
        return;
    }
    cx.default_global::<Tokens>()
        .0
        .insert(server.to_string(), Some(token.to_string()));
    let write = cx.write_credentials(server, overleaf::USERNAME, token.as_bytes());
    let server = server.to_string();
    window
        .spawn(cx, async move |cx| {
            if let Err(e) = write.await {
                log::warn!("saving the token for {server}: {e:#}");
                cx.update(|window, cx| {
                    window.push_notification(
                        Notification::warning(
                            "The system keychain is not available: TeXoria keeps the token \
                             only until it quits.",
                        ),
                        cx,
                    )
                })
                .ok();
            }
        })
        .detach();
}

/// A masked input for a token.
pub fn token_input(window: &mut Window, cx: &mut App) -> Entity<InputState> {
    cx.new(|cx| {
        InputState::new(window, cx)
            .masked(true)
            .placeholder("olp_…")
    })
}

/// The explanation, input and link of a token field.
pub fn token_field(server: &str, input: &Entity<InputState>, cx: &App) -> AnyElement {
    let page = overleaf::token_page(server);
    v_flex()
        .gap_1()
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(
                    "Git signs in to Overleaf with an authentication token: create one in \
                     your account settings, under Git Integration.",
                ),
        )
        .child(Input::new(input).mask_toggle())
        .child(
            div().child(
                Button::new("overleaf-token-page")
                    .link()
                    .label("Open Account Settings")
                    .on_click(move |_, _, cx| cx.open_url(&page)),
            ),
        )
        .into_any_element()
}

/// Ask for a new token for `server`, then hand it to `on_token`.
pub fn prompt_token(
    server: &str,
    reason: &'static str,
    on_token: impl Fn(String, &mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    let input = token_input(window, cx);
    let focus = input.focus_handle(cx);
    let body_input = input.clone();
    let server = server.to_string();
    crate::git_panel::open_form(
        window,
        cx,
        "Overleaf Token",
        "Save",
        Some(focus),
        move |_, cx| {
            v_flex()
                .gap_2()
                .child(div().text_sm().child(reason))
                .child(token_field(&server, &body_input, cx))
                .into_any_element()
        },
        move |window, cx| {
            let token = input.read(cx).value().trim().to_string();
            if token.is_empty() {
                return false;
            }
            if !overleaf::valid_token(&token) {
                window.push_notification(
                    Notification::error("That is not an Overleaf token (olp_…)"),
                    cx,
                );
                return false;
            }
            on_token(token, window, cx);
            true
        },
    );
}

#[cfg(test)]
mod tests;
