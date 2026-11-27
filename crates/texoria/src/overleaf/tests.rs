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

/// Stores for the keychain, the first one where tokens go, the second where an older version
/// put them.
fn keychain(cx: &mut TestAppContext, current: &keychain::Memory, legacy: &keychain::Memory) {
    let keychain = keychain::Keychain {
        current: std::rc::Rc::new(current.clone()),
        legacy: Some(std::rc::Rc::new(legacy.clone())),
    };
    cx.update(|cx| cx.set_global(keychain));
}

fn stored(store: &keychain::Memory) -> Vec<(String, String)> {
    let mut all: Vec<_> = store.tokens.borrow().clone().into_iter().collect();
    all.sort();
    all
}

#[gpui::test]
fn tokens_move_out_of_gpuis_entries(cx: &mut TestAppContext) {
    window(cx);
    let (current, legacy) = (keychain::Memory::default(), keychain::Memory::default());
    legacy.tokens.borrow_mut().extend([
        (SERVER.to_string(), "olp_old0token".to_string()),
        (
            "https://latex.example.org".to_string(),
            "not a token".to_string(),
        ),
    ]);
    keychain(cx, &current, &legacy);

    assert_eq!(load(cx, SERVER).as_deref(), Some("olp_old0token"));
    assert_eq!(
        stored(&current),
        [(SERVER.to_string(), "olp_old0token".to_string())]
    );
    // What is not a token stays where it was.
    assert_eq!(load(cx, "https://latex.example.org"), None);
    assert_eq!(
        stored(&legacy),
        [(
            "https://latex.example.org".to_string(),
            "not a token".to_string()
        )]
    );
}

#[gpui::test]
fn a_token_in_both_places_is_read_from_texorias(cx: &mut TestAppContext) {
    window(cx);
    let (current, legacy) = (keychain::Memory::default(), keychain::Memory::default());
    current
        .tokens
        .borrow_mut()
        .insert(SERVER.to_string(), "olp_new0token".to_string());
    legacy
        .tokens
        .borrow_mut()
        .insert(SERVER.to_string(), "olp_old0token".to_string());
    keychain(cx, &current, &legacy);

    assert_eq!(load(cx, SERVER).as_deref(), Some("olp_new0token"));
    assert_eq!(stored(&legacy).len(), 1);
}

#[gpui::test]
fn without_a_keychain_tokens_last_until_quitting(cx: &mut TestAppContext) {
    let window = window(cx);
    let failing = keychain::Memory {
        failing: true,
        ..Default::default()
    };
    keychain(cx, &failing, &failing);

    assert_eq!(load(cx, SERVER), None);
    cx.update_window(window, |_, window, cx| {
        save_token(SERVER, "olp_first0token", window, cx)
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(load(cx, SERVER).as_deref(), Some("olp_first0token"));
    let notes = cx
        .update_window(window, |_, window, cx| window.notifications(cx).len())
        .unwrap();
    assert_eq!(notes, 1);
}
