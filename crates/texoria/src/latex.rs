//! LaTeX language registration and the completion provider bridging
//! `tex-complete` to the editor.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, RwLock};

use gpui_kit::component::highlighter::{LanguageConfig, LanguageRegistry};
use gpui_kit::component::input::{CompletionProvider, Rope, RopeExt};
use gpui_kit::*;
use lsp_types::{
    CompletionContext, CompletionItem, CompletionItemKind, CompletionResponse, CompletionTextEdit,
    InsertReplaceEdit,
};
use tex_complete::ItemKind;
use tex_project::Project;

pub const LANGUAGE: &str = "latex";

pub fn register() {
    LanguageRegistry::singleton().register(
        LANGUAGE,
        &LanguageConfig::new(
            LANGUAGE,
            tex_syntax::language(),
            vec![],
            tex_syntax::HIGHLIGHTS,
            "",
            "",
        ),
    );
}

/// A completion that leaves the cursor somewhere other than its end
/// (`\frac{|}{}`). The editor inserts completions as plain text, so the
/// workspace repositions the cursor after the insertion lands.
#[derive(Debug, Clone)]
pub struct CursorFixup {
    pub insert: String,
    /// Bytes to move back from the end of `insert`.
    pub back: usize,
}

pub struct LatexCompletions {
    pub project: Arc<RwLock<Project>>,
    pub fixups: Rc<RefCell<Vec<CursorFixup>>>,
}

impl CompletionProvider for LatexCompletions {
    fn completions(
        &self,
        rope: &Rope,
        offset: usize,
        _trigger: CompletionContext,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Task<Result<CompletionResponse>> {
        // Ranking a few hundred candidates takes microseconds: do it inline.
        // One character past the cursor (to see an auto-closed `}`); it may be
        // multi-byte, and slicing inside it panics.
        let end = rope
            .char_at(offset)
            .map_or(offset, |c| offset + c.len_utf8());
        let text = rope.slice(0..end).to_string();
        let project = self.project.read().ok();
        let items = tex_complete::complete(&text, offset, project.as_deref(), "  ");

        let mut fixups = Vec::new();
        let items = items
            .into_iter()
            .map(|item| {
                let range = lsp_types::Range::new(
                    rope.offset_to_position(item.replace.start),
                    rope.offset_to_position(item.replace.end),
                );
                if item.cursor < item.insert.len() {
                    fixups.push(CursorFixup {
                        insert: item.insert.clone(),
                        back: item.insert.len() - item.cursor,
                    });
                }
                CompletionItem {
                    label: item.label,
                    kind: Some(match item.kind {
                        ItemKind::Command => CompletionItemKind::FUNCTION,
                        ItemKind::UserCommand => CompletionItemKind::METHOD,
                        ItemKind::Environment => CompletionItemKind::MODULE,
                        ItemKind::Label => CompletionItemKind::REFERENCE,
                        ItemKind::Citation => CompletionItemKind::VALUE,
                        ItemKind::Package => CompletionItemKind::FILE,
                    }),
                    detail: item.detail,
                    text_edit: Some(CompletionTextEdit::InsertAndReplace(InsertReplaceEdit {
                        new_text: item.insert,
                        insert: range,
                        replace: range,
                    })),
                    ..Default::default()
                }
            })
            .collect();
        *self.fixups.borrow_mut() = fixups;
        Task::ready(Ok(CompletionResponse::Array(items)))
    }

    fn is_completion_trigger(&self, _offset: usize, new_text: &str, _cx: &mut App) -> bool {
        // Only single typed characters that can be part of a completable
        // token; `complete` itself decides whether the context applies.
        let mut chars = new_text.chars();
        matches!((chars.next(), chars.next()), (Some(c), None)
            if c.is_alphanumeric() || matches!(c, '\\' | '{' | ',' | ':' | '-' | '_' | '*'))
    }
}
