use gpui_kit::{TestAppContext, WindowOptions};

use super::*;
use crate::testing;
// `super::*` brings gpui's `test` attribute, which would shadow the standard one, and
// gpui-kit's `gpui` alias, which the `#[gpui::test]` macro's paths must not find twice.
use core::prelude::v1::test;
use gpui_kit::gpui;

const SERVER: &str = "https://git.overleaf.com";

#[test]
fn credentials_sign_in_as_git() {
    let c = credential(SERVER, "olp_token123".into());
    assert_eq!(c.url, SERVER);
    assert_eq!(c.username, "git");
    assert_eq!(c.password, "olp_token123");
}

/// A window to save tokens from (a failed save shows a notification there).
fn window(cx: &mut TestAppContext) -> AnyWindowHandle {
    testing::init(cx);
    cx.update(|cx| {
        gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| cx.new(|_| Empty)).unwrap()
    })
    .0
}

struct Empty;

impl Render for Empty {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

fn load(cx: &mut TestAppContext, server: &str) -> Option<String> {
    let task = cx.update(|cx| load_token(server, cx));
    testing::resolve(cx, task)
}

fn known(cx: &mut TestAppContext, server: &str) -> Option<Option<String>> {
    cx.update(|cx| {
        cx.try_global::<Tokens>()
            .and_then(|t| t.0.get(server).cloned())
    })
}

#[gpui::test]
fn reads_the_keychain_once_per_server(cx: &mut TestAppContext) {
    window(cx);
    assert_eq!(known(cx, SERVER), None);
    // The test platform's keychain is empty.
    assert_eq!(load(cx, SERVER), None);
    // Remembered, even when there is none: no second keychain prompt.
    assert_eq!(known(cx, SERVER), Some(None));
    assert_eq!(load(cx, SERVER), None);
}

#[gpui::test]
fn saved_tokens_are_read_back(cx: &mut TestAppContext) {
    let window = window(cx);
    let save = |cx: &mut TestAppContext, server: &str, token: &str| {
        cx.update_window(window, |_, window, cx| {
            save_token(server, token, window, cx)
        })
        .unwrap();
        cx.run_until_parked();
    };
    save(cx, SERVER, "olp_first0token");
    assert_eq!(load(cx, SERVER).as_deref(), Some("olp_first0token"));
    // Saving it again changes nothing; a new one replaces it.
    save(cx, SERVER, "olp_first0token");
    save(cx, SERVER, "olp_second0token");
    assert_eq!(load(cx, SERVER).as_deref(), Some("olp_second0token"));
    // One per server.
    assert_eq!(load(cx, "https://latex.example.org"), None);
    let notes = cx
        .update_window(window, |_, window, cx| window.notifications(cx).len())
        .unwrap();
    assert_eq!(notes, 0);
}
