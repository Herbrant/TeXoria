//! The live checks of `tex_project::check` in the editor: underlines that
//! follow edits (editor diagnostics are cleared on every edit) and their
//! message on hover.

use std::rc::Rc;

use gpui_kit::component::input::EditorState;
use gpui_kit::*;
use tex_log::{Diagnostic, Severity};
use tex_project::check::{Issue, Level};

use crate::latex::HoverNote;
use crate::underlines::Notes;

/// A buffer's underlined issues, with their message.
pub struct BufferChecks {
    notes: Notes<String>,
}

impl BufferChecks {
    pub fn new(editor: &Entity<EditorState>, cx: &mut App) -> Self {
        Self {
            notes: Notes::new(editor, cx),
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
        let len = editor.read(cx).text().len();
        let items = issues.iter().map(|issue| {
            let range = issue.range.start.min(len)..issue.range.end.min(len);
            let color = match issue.level {
                Level::Error => colors.0,
                Level::Warning => colors.1,
            };
            (range, color, issue.message.clone())
        });
        self.notes.set(editor, items, cx);
    }

    pub fn clear(&self, cx: &mut App) {
        self.notes.clear(cx);
    }

    /// The message of the issue under an offset, for the hover.
    pub fn hover_note(&self, editor: &Entity<EditorState>) -> HoverNote {
        let notes = self.notes.clone();
        let editor = editor.downgrade();
        Rc::new(move |offset, cx| {
            let (_, message) = notes.at(&editor.upgrade()?, offset, cx)?;
            Some(format!("⚠ {message}"))
        })
    }
}

/// An issue as a row of the Problems panel.
pub fn diagnostic(issue: &Issue) -> Diagnostic {
    Diagnostic {
        severity: severity(issue.level),
        file: Some(issue.file.clone()),
        line: Some(issue.line + 1),
        message: issue.message.clone(),
    }
}

/// The severity of an issue of `level`.
pub fn severity(level: Level) -> Severity {
    match level {
        Level::Error => Severity::Error,
        Level::Warning => Severity::Warning,
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
mod tests;
