//! Vim mode: keys typed in the active editor go to `tex_vim`, whose actions
//! are applied to the editor. In insert mode keys reach the editor as usual.

use std::ops::Range;

use gpui_kit::component::input::{
    EditorState, RangeDecoration, RangeDecorationCollection, RangeDecorationStyle, Redo, RopeExt,
    Undo,
};
use gpui_kit::*;
use tex_vim::{Action, Ctx, Key, Mode, Vim};

/// Lines Ctrl-D and Ctrl-U move by, doubled.
const PAGE_LINES: usize = 30;

/// What the workspace does for Vim (the editor's own work is done here).
pub enum Request {
    Save,
    CloseTab,
}

/// The key Vim gets for a keystroke, if it wants it in `mode`.
pub fn key_of(k: &Keystroke, mode: Mode) -> Option<Key> {
    let m = &k.modifiers;
    // ⌘ (and Ctrl on Windows and Linux) shortcuts stay the app's.
    if m.platform || m.function {
        return None;
    }
    let named = match k.key.as_str() {
        "escape" => Some(Key::Escape),
        "enter" => Some(Key::Enter),
        "backspace" => Some(Key::Backspace),
        "tab" => Some(Key::Tab),
        "left" => Some(Key::Left),
        "right" => Some(Key::Right),
        "up" => Some(Key::Up),
        "down" => Some(Key::Down),
        _ => None,
    };
    if m.control {
        let c = k
            .key
            .chars()
            .next()
            .filter(|_| k.key.chars().count() == 1)?;
        // Only those Vim uses: the others stay the app's (Ctrl+S, Ctrl+O…).
        return matches!(c, 'r' | 'd' | 'u' | '[' | 'c').then_some(Key::Ctrl(c));
    }
    if mode == Mode::Insert {
        // Arrows and the like move the editor's cursor themselves.
        return match named {
            Some(Key::Escape | Key::Enter | Key::Backspace | Key::Tab) => named,
            Some(_) => None,
            None => single_char(k),
        };
    }
    if named.is_some() {
        return named;
    }
    if m.alt {
        return None;
    }
    single_char(k)
}

fn single_char(k: &Keystroke) -> Option<Key> {
    let text = k.key_char.as_deref().unwrap_or(&k.key);
    let mut chars = text.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if !c.is_control() => Some(Key::Char(c)),
        _ => None,
    }
}

/// Feed `key` to `vim` and apply what it says to `editor`. Returns whether
/// the key was taken (it must not reach the editor), and what the workspace
/// must do.
pub fn handle(
    vim: &mut Vim,
    editor: &Entity<EditorState>,
    key: Key,
    window: &mut Window,
    cx: &mut App,
) -> (bool, Vec<Request>) {
    let text = editor.read(cx).text().to_string();
    let cursor = editor.read(cx).cursor();
    let clipboard = cx.read_from_clipboard().and_then(|c| c.text());
    let ctx = Ctx {
        text: &text,
        cursor,
        clipboard: clipboard.as_deref(),
        page_lines: PAGE_LINES,
    };
    let actions = vim.key(&ctx, key);
    let mut taken = true;
    let mut requests = Vec::new();
    for action in actions {
        match action {
            Action::Pass => taken = false,
            Action::Edit { range, text } => editor.update(cx, |state, cx| {
                let len = state.text().len();
                if range.end > len {
                    return;
                }
                state.set_selected_range(range, cx);
                state.replace(text, window, cx);
            }),
            Action::Cursor(at) => editor.update(cx, |state, cx| {
                let at = at.min(state.text().len());
                // Moving the cursor this way scrolls to it.
                let position = state.text().offset_to_position(at);
                state.set_cursor_position(position, window, cx);
                state.set_selected_range(at..at, cx);
            }),
            Action::Select(range) => editor.update(cx, |state, cx| {
                let len = state.text().len();
                let (start, end) = (range.start.min(len), range.end.min(len));
                let position = state.text().offset_to_position(end);
                state.set_cursor_position(position, window, cx);
                state.set_selected_range(start..end, cx);
            }),
            Action::Undo => window.dispatch_action(Box::new(Undo), cx),
            Action::Redo => window.dispatch_action(Box::new(Redo), cx),
            Action::Yank(text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
            Action::Write => requests.push(Request::Save),
            Action::Quit => requests.push(Request::CloseTab),
        }
    }
    (taken, requests)
}

/// The block cursor of normal and visual mode, drawn over the character.
pub struct Block {
    collection: RangeDecorationCollection,
    shown: Option<Range<usize>>,
}

impl Block {
    pub fn new(editor: &Entity<EditorState>, cx: &mut App) -> Self {
        let collection = editor.update(cx, |state, cx| {
            state.create_range_decorations_collection(vec![], cx)
        });
        Self {
            collection,
            shown: None,
        }
    }

    /// Show the block at the editor's cursor, or none (`vim` is `None` or
    /// in insert mode).
    pub fn update(
        &mut self,
        vim: Option<&Vim>,
        editor: &Entity<EditorState>,
        color: Hsla,
        cx: &mut App,
    ) {
        let range = vim.filter(|v| v.mode() != Mode::Insert).and_then(|_| {
            let state = editor.read(cx);
            // With a selection, the editor shows it already.
            if !state.selected_range().is_empty() {
                return None;
            }
            let text = state.text();
            let mut at = state.cursor();
            // Normal mode stays off the end of a line.
            if text.char_at(at).is_none_or(|c| c == '\n') && at > 0 {
                let mut before = at - 1;
                while before > 0 && !text.is_char_boundary(before) {
                    before -= 1;
                }
                if text.char_at(before).is_some_and(|c| c != '\n') {
                    at = before;
                }
            }
            let c = text.char_at(at).filter(|c| *c != '\n')?;
            Some(at..at + c.len_utf8())
        });
        if range == self.shown {
            return;
        }
        let decorations = range
            .iter()
            .map(|r| {
                RangeDecoration::new(r.clone())
                    .with_style(RangeDecorationStyle::Fill)
                    .with_color(color)
            })
            .collect();
        self.collection.set(decorations, cx);
        self.shown = range;
    }
}

#[cfg(test)]
mod tests {
    use core::prelude::v1::test;

    use gpui_kit::Keystroke;
    use tex_vim::{Key, Mode};

    use super::key_of;

    fn key(keys: &str, mode: Mode) -> Option<Key> {
        key_of(&Keystroke::parse(keys).unwrap(), mode)
    }

    #[test]
    fn app_shortcuts_stay_the_apps() {
        for mode in [Mode::Normal, Mode::Insert, Mode::Visual] {
            assert_eq!(key("cmd-s", mode), None);
            assert_eq!(key("cmd-r", mode), None);
            assert_eq!(key("ctrl-s", mode), None, "Save on Windows and Linux");
            assert_eq!(key("ctrl-o", mode), None);
            assert_eq!(key("f12", mode), None);
        }
    }

    /// The Ctrl keys Vim takes, as docs/writing.md says (Ctrl-R is redo, not Build).
    #[test]
    fn vim_takes_its_ctrl_keys_in_every_mode() {
        for mode in [Mode::Normal, Mode::Insert, Mode::Visual] {
            for c in ['r', 'd', 'u', 'c', '['] {
                assert_eq!(key(&format!("ctrl-{c}"), mode), Some(Key::Ctrl(c)), "{c}");
            }
        }
    }

    #[test]
    fn insert_mode_leaves_moves_to_the_editor() {
        assert_eq!(key("a", Mode::Insert), Some(Key::Char('a')));
        assert_eq!(key("escape", Mode::Insert), Some(Key::Escape));
        assert_eq!(key("enter", Mode::Insert), Some(Key::Enter));
        assert_eq!(key("left", Mode::Insert), None);
        assert_eq!(key("left", Mode::Normal), Some(Key::Left));
        // As the platform reports Shift-A: the key, and the character it types.
        let shift_a = Keystroke {
            key_char: Some("A".into()),
            ..Keystroke::parse("shift-a").unwrap()
        };
        assert_eq!(key_of(&shift_a, Mode::Normal), Some(Key::Char('A')));
        // Alt combinations type characters on some layouts: not Vim commands.
        assert_eq!(key("alt-x", Mode::Normal), None);
        let space = Keystroke {
            key_char: Some(" ".into()),
            ..Keystroke::parse("space").unwrap()
        };
        assert_eq!(key_of(&space, Mode::Normal), Some(Key::Char(' ')));
    }
}
