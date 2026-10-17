//! The live checks of `tex_project::check` in the editor: underlines that
//! follow edits (editor diagnostics are cleared on every edit) and their
//! message on hover.

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::component::input::{EditorState, TextDecoration, TextDecorationCollection};
use gpui_kit::*;
use tex_log::{Diagnostic, Severity};
use tex_project::check::{Issue, Level};

use crate::latex::HoverNote;

/// An underlined issue: where it was, the text there, and what is wrong.
struct Note {
    start: usize,
    text: String,
    message: String,
}

/// A buffer's underlined issues.
pub struct BufferChecks {
    underlines: TextDecorationCollection,
    notes: Rc<RefCell<Vec<Note>>>,
}

impl BufferChecks {
    pub fn new(editor: &Entity<EditorState>, cx: &mut App) -> Self {
        let underlines = editor.update(cx, |state, cx| {
            state.create_decorations_collection(vec![], cx)
        });
        Self {
            underlines,
            notes: Rc::default(),
        }
    }

    /// Underline `issues` (of this buffer's file, found in its current text).
    pub fn set(
        &self,
        editor: &Entity<EditorState>,
        issues: &[&Issue],
        colors: (Hsla, Hsla),
        cx: &mut App,
    ) {
        let text = editor.read(cx).text().clone();
        let mut notes = Vec::new();
        let mut decorations = Vec::new();
        for issue in issues {
            let range = issue.range.start.min(text.len())..issue.range.end.min(text.len());
            if range.is_empty()
                || !text.is_char_boundary(range.start)
                || !text.is_char_boundary(range.end)
            {
                continue;
            }
            let color = match issue.level {
                Level::Error => colors.0,
                Level::Warning => colors.1,
            };
            notes.push(Note {
                start: range.start,
                text: text.slice(range.clone()).to_string(),
                message: issue.message.clone(),
            });
            decorations.push(TextDecoration::new(range, underline(color)));
        }
        *self.notes.borrow_mut() = notes;
        self.underlines.set(decorations, cx);
    }

    pub fn clear(&self, cx: &mut App) {
        self.notes.borrow_mut().clear();
        self.underlines.clear(cx);
    }

    /// The message of the issue under an offset, for the hover.
    pub fn hover_note(&self, editor: &Entity<EditorState>) -> HoverNote {
        let underlines = self.underlines.clone();
        let notes = self.notes.clone();
        let editor = editor.downgrade();
        Rc::new(move |offset, cx| {
            let range = underlines
                .get_ranges(cx)
                .into_iter()
                .find(|r| r.start <= offset && offset <= r.end)?;
            let editor = editor.upgrade()?;
            let text = editor.read(cx).text().slice(range.clone()).to_string();
            let notes = notes.borrow();
            let note = notes
                .iter()
                .filter(|n| n.text == text)
                .min_by_key(|n| n.start.abs_diff(range.start))?;
            Some(format!("⚠ {}", note.message))
        })
    }
}

fn underline(color: Hsla) -> HighlightStyle {
    HighlightStyle {
        underline: Some(UnderlineStyle {
            thickness: px(1.),
            color: Some(color),
            wavy: true,
        }),
        ..Default::default()
    }
}

/// An issue as a row of the Problems panel.
pub fn diagnostic(issue: &Issue) -> Diagnostic {
    Diagnostic {
        severity: match issue.level {
            Level::Error => Severity::Error,
            Level::Warning => Severity::Warning,
        },
        file: Some(issue.file.clone()),
        line: Some(issue.line + 1),
        message: issue.message.clone(),
    }
}

/// Whether a log warning says what a check already does
/// (`Reference `x' on page 1 undefined`, `Citation `x' … undefined`).
pub fn duplicates(d: &Diagnostic, keys: &std::collections::HashSet<&str>) -> bool {
    let m = &d.message;
    if !(m.contains("undefined") && (m.starts_with("Reference") || m.starts_with("Citation"))) {
        return false;
    }
    let key = m
        .split_once('`')
        .and_then(|(_, rest)| rest.split_once('\''))
        .map(|(k, _)| k);
    key.is_some_and(|k| keys.contains(k))
}

#[cfg(test)]
mod tests {
    use super::duplicates;
    use tex_log::{Diagnostic, Severity};

    #[test]
    fn tells_duplicate_log_warnings() {
        let keys = std::collections::HashSet::from(["sec:x", "knuth"]);
        let d = |m: &str| Diagnostic {
            severity: Severity::Warning,
            file: None,
            line: None,
            message: m.into(),
        };
        assert!(duplicates(
            &d("Reference `sec:x' on page 1 undefined on input line 5."),
            &keys
        ));
        assert!(duplicates(
            &d("Citation `knuth' on page 2 undefined on input line 9."),
            &keys
        ));
        assert!(!duplicates(
            &d("Reference `other' on page 1 undefined"),
            &keys
        ));
        assert!(!duplicates(&d("There were undefined references."), &keys));
    }
}
