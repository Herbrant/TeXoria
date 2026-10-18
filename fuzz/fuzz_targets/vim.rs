//! Vim keys on any text: every action must stay inside the text, on characters.
#![no_main]

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use tex_vim::{Action, Ctx, Key, Vim};

#[derive(Debug, Arbitrary)]
enum FuzzKey {
    Char(char),
    Ctrl(u8),
    Escape,
    Enter,
    Backspace,
    Tab,
    Left,
    Right,
    Up,
    Down,
}

impl FuzzKey {
    fn key(&self) -> Key {
        match *self {
            FuzzKey::Char(c) => Key::Char(c),
            FuzzKey::Ctrl(c) => Key::Ctrl(['r', 'd', 'u', '[', 'c'][c as usize % 5]),
            FuzzKey::Escape => Key::Escape,
            FuzzKey::Enter => Key::Enter,
            FuzzKey::Backspace => Key::Backspace,
            FuzzKey::Tab => Key::Tab,
            FuzzKey::Left => Key::Left,
            FuzzKey::Right => Key::Right,
            FuzzKey::Up => Key::Up,
            FuzzKey::Down => Key::Down,
        }
    }
}

fuzz_target!(|input: (String, usize, Vec<FuzzKey>)| {
    let (mut text, cursor, keys) = input;
    let mut cursor = cursor % (text.len() + 1);
    while !text.is_char_boundary(cursor) {
        cursor -= 1;
    }
    let mut vim = Vim::new();
    for key in keys.iter().take(64) {
        let ctx = Ctx {
            text: &text,
            cursor,
            clipboard: None,
            page_lines: 10,
        };
        for action in vim.key(&ctx, key.key()) {
            match action {
                Action::Edit { range, text: new } => {
                    assert!(range.start <= range.end && range.end <= text.len());
                    assert!(text.is_char_boundary(range.start) && text.is_char_boundary(range.end));
                    text.replace_range(range.clone(), &new);
                    cursor = range.start + new.len();
                }
                Action::Cursor(c) => {
                    assert!(c <= text.len() && text.is_char_boundary(c), "cursor {c}");
                    cursor = c;
                }
                // Reversed when the head is before the anchor.
                Action::Select(r) => {
                    assert!(r.start.max(r.end) <= text.len());
                    assert!(text.is_char_boundary(r.start) && text.is_char_boundary(r.end));
                }
                // The editor's own undo would change the text: start over from here.
                Action::Undo | Action::Redo => vim.reset(),
                Action::Pass | Action::Yank(_) | Action::Write | Action::Quit => {}
            }
        }
    }
});
