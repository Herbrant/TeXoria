//! Wavy underlines that follow edits (a `TextDecorationCollection`, unlike editor diagnostics,
//! which are cleared on every edit), as the spelling, the grammar and the live checks draw
//! them, and the corrections they offer.

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

use gpui_kit::component::input::{EditorState, TextDecoration, TextDecorationCollection};
use gpui_kit::*;
use lsp_types::{CodeAction, CodeActionKind};
use serde_json::{Value, json};

use crate::spell::ReplaceMisspelling;

/// A wavy underline of `color`.
pub fn wavy(color: Hsla) -> HighlightStyle {
    HighlightStyle {
        underline: Some(UnderlineStyle {
            thickness: px(1.),
            color: Some(color),
            wavy: true,
        }),
        ..Default::default()
    }
}

/// The range of `underlines` at `offset` (where it is now).
pub fn range_at(
    underlines: &TextDecorationCollection,
    offset: usize,
    cx: &App,
) -> Option<Range<usize>> {
    underlines
        .get_ranges(cx)
        .into_iter()
        .find(|r| r.start <= offset && offset <= r.end)
}

/// Underlines, each with what it is about. The ranges of deleted text disappear from a
/// decoration collection, so an underline's note is the one with the same text nearest to it.
pub struct Notes<T> {
    underlines: TextDecorationCollection,
    notes: Rc<RefCell<Vec<Note<T>>>>,
}

impl<T> Clone for Notes<T> {
    fn clone(&self) -> Self {
        Self {
            underlines: self.underlines.clone(),
            notes: self.notes.clone(),
        }
    }
}

/// An underline's note: where it was, the text there, and what it is about.
struct Note<T> {
    start: usize,
    text: String,
    about: T,
}

impl<T: Clone + 'static> Notes<T> {
    /// No underlines yet, in `editor`.
    pub fn new(editor: &Entity<EditorState>, cx: &mut App) -> Self {
        let underlines = editor.update(cx, |state, cx| {
            state.create_decorations_collection(vec![], cx)
        });
        Self {
            underlines,
            notes: Rc::default(),
        }
    }

    /// Underline the `(range, color, about)` of `items` in the current text of `editor`,
    /// instead of the previous ones. Ranges that are empty, past the end or not on character
    /// boundaries are left out.
    pub fn set(
        &self,
        editor: &Entity<EditorState>,
        items: impl IntoIterator<Item = (Range<usize>, Hsla, T)>,
        cx: &mut App,
    ) {
        let text = editor.read(cx).text().clone();
        let mut notes = Vec::new();
        let mut decorations = Vec::new();
        for (range, color, about) in items {
            if range.is_empty()
                || range.end > text.len()
                || !text.is_char_boundary(range.start)
                || !text.is_char_boundary(range.end)
            {
                continue;
            }
            notes.push(Note {
                start: range.start,
                text: text.slice(range.clone()).to_string(),
                about,
            });
            decorations.push(TextDecoration::new(range, wavy(color)));
        }
        *self.notes.borrow_mut() = notes;
        self.underlines.set(decorations, cx);
    }

    pub fn clear(&self, cx: &mut App) {
        self.notes.borrow_mut().clear();
        self.underlines.clear(cx);
    }

    /// Where the underlines are now.
    #[cfg(test)]
    pub fn ranges(&self, cx: &App) -> Vec<Range<usize>> {
        self.underlines.get_ranges(cx)
    }

    /// The text of each underline when it was set, and what it is about.
    #[cfg(test)]
    pub fn noted(&self) -> Vec<(String, T)> {
        let notes = self.notes.borrow();
        notes
            .iter()
            .map(|n| (n.text.clone(), n.about.clone()))
            .collect()
    }

    /// The underline at `offset` (where it is now) and what it is about.
    pub fn at(
        &self,
        editor: &Entity<EditorState>,
        offset: usize,
        cx: &App,
    ) -> Option<(Range<usize>, T)> {
        let range = range_at(&self.underlines, offset, cx)?;
        let text = editor.read(cx).text().slice(range.clone()).to_string();
        let notes = self.notes.borrow();
        let note = notes
            .iter()
            .filter(|n| n.text == text)
            .min_by_key(|n| n.start.abs_diff(range.start))?;
        Some((range, note.about.clone()))
    }
}

/// A replacement as a menu item (a space alone would show nothing).
pub fn display(text: &str) -> String {
    if text.trim().is_empty() {
        "(remove)".to_string()
    } else {
        text.to_string()
    }
}

/// The code action (⌘. / Ctrl+.) that replaces `range` with `text`, run by [`perform`].
pub fn replacement(range: &Range<usize>, text: &str) -> CodeAction {
    CodeAction {
        title: format!("Change to “{}”", display(text)),
        kind: Some(CodeActionKind::QUICKFIX),
        data: Some(json!({ "start": range.start, "end": range.end, "text": text })),
        ..Default::default()
    }
}

/// Run a code action: a [`replacement`] as [`ReplaceMisspelling`], any other as the action
/// `other` makes of its data. They are dispatched: the editor is being updated, so the
/// workspace applies them afterwards.
pub fn perform(
    action: CodeAction,
    other: impl FnOnce(&Value) -> Option<Box<dyn Action>>,
    window: &mut Window,
    cx: &mut App,
) {
    let data = action.data.unwrap_or_default();
    let offset = |k: &str| data.get(k).and_then(|v| v.as_u64()).map(|v| v as usize);
    let text = data
        .get("text")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    if let Some(action) = other(&data) {
        window.dispatch_action(action, cx);
    } else if let (Some(start), Some(end), Some(text)) = (offset("start"), offset("end"), text) {
        let action = ReplaceMisspelling {
            range: start..end,
            text,
        };
        window.dispatch_action(Box::new(action), cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn replacements_show_what_they_write() {
        let action = replacement(&(3..5), "the");
        assert_eq!(action.title, "Change to “the”");
        assert_eq!(action.kind, Some(CodeActionKind::QUICKFIX));
        let data = action.data.unwrap();
        assert_eq!(
            (data["start"].as_u64(), data["end"].as_u64()),
            (Some(3), Some(5))
        );
        assert_eq!(replacement(&(0..1), " ").title, "Change to “(remove)”");
    }

    #[test]
    fn underlines_are_wavy() {
        let style = wavy(gpui_kit::red()).underline.unwrap();
        assert!(style.wavy);
        assert_eq!(style.color, Some(gpui_kit::red()));
    }
}
