//! LaTeX language registration and the completion provider bridging
//! `tex-complete` to the editor.

use std::cell::RefCell;
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, RwLock};

use gpui_kit::component::highlighter::{LanguageConfig, LanguageRegistry};
use gpui_kit::component::input::{
    CompletionProvider, DefinitionProvider, HoverProvider, Rope, RopeExt, ShowDocumentHandler,
};
use gpui_kit::*;
use lsp_types::{
    CompletionContext, CompletionItem, CompletionItemKind, CompletionResponse, CompletionTextEdit,
    Hover, HoverContents, InsertReplaceEdit, LocationLink, MarkupContent, MarkupKind,
};
use tex_complete::ItemKind;
use tex_complete::nav::{self, Symbol, Target};
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
                        ItemKind::Package => CompletionItemKind::MODULE,
                        ItemKind::File => CompletionItemKind::FILE,
                        ItemKind::Folder => CompletionItemKind::FOLDER,
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
            if c.is_alphanumeric() || matches!(c, '\\' | '{' | ',' | ':' | '-' | '_' | '*' | '/' | '.'))
    }
}

/// Show bytes of a project file (Go to Definition).
#[derive(Clone, PartialEq, Action)]
#[action(namespace = texoria, no_json)]
pub struct OpenLocation {
    pub file: PathBuf,
    pub range: Range<usize>,
}

/// Open a file with its app (a picture of `\includegraphics`).
#[derive(Clone, PartialEq, Action)]
#[action(namespace = texoria, no_json)]
pub struct OpenExternal {
    pub path: PathBuf,
}

/// Extra hover text for an offset of the buffer (a problem found there).
pub type HoverNote = Rc<dyn Fn(usize, &App) -> Option<String>>;

/// Go to Definition and hover, from `tex_complete::nav`.
pub struct LatexNavigation {
    pub project: Arc<RwLock<Project>>,
    /// The targets of the last lookup: links point into it, since a target's
    /// bytes belong to a file the editor does not have.
    targets: Rc<RefCell<Vec<Target>>>,
    pub notes: Vec<HoverNote>,
}

const TARGET_URI: &str = "texoria:definition/";

impl LatexNavigation {
    pub fn new(project: Arc<RwLock<Project>>) -> Self {
        Self {
            project,
            targets: Rc::default(),
            notes: Vec::new(),
        }
    }

    /// Follows the links of `definitions` by asking the workspace.
    pub fn show_document(&self) -> ShowDocumentHandler {
        let targets = self.targets.clone();
        Rc::new(move |params, window, cx| {
            let Some(ix) = params
                .uri
                .as_str()
                .strip_prefix(TARGET_URI)
                .and_then(|i| i.parse::<usize>().ok())
            else {
                return false;
            };
            let Some(target) = targets.borrow().get(ix).cloned() else {
                return true;
            };
            // The editor is being updated: the workspace opens it afterwards.
            match target {
                Target::Source { file, range } => {
                    window.dispatch_action(Box::new(OpenLocation { file, range }), cx)
                }
                Target::External(path) => {
                    window.dispatch_action(Box::new(OpenExternal { path }), cx)
                }
            }
            true
        })
    }
}

/// The symbol at `offset`, looked for on its line only.
fn symbol_at(rope: &Rope, offset: usize) -> Option<Symbol> {
    let row = rope.offset_to_position(offset).line as usize;
    let start = rope.line_start_offset(row);
    let line = rope.slice_line(row).to_string();
    let shift = |r: Range<usize>| r.start + start..r.end + start;
    Some(match nav::symbol_at(&line, offset.checked_sub(start)?)? {
        Symbol::Reference { name, range } => Symbol::Reference {
            name,
            range: shift(range),
        },
        Symbol::Citation { key, range } => Symbol::Citation {
            key,
            range: shift(range),
        },
        Symbol::Label { name, range } => Symbol::Label {
            name,
            range: shift(range),
        },
        Symbol::Path { path, kind, range } => Symbol::Path {
            path,
            kind,
            range: shift(range),
        },
        Symbol::Package { name, range } => Symbol::Package {
            name,
            range: shift(range),
        },
        Symbol::Environment { name, range } => Symbol::Environment {
            name,
            range: shift(range),
        },
        Symbol::Command { name, range } => Symbol::Command {
            name,
            range: shift(range),
        },
    })
}

fn lsp_range(rope: &Rope, range: Range<usize>) -> lsp_types::Range {
    lsp_types::Range::new(
        rope.offset_to_position(range.start),
        rope.offset_to_position(range.end),
    )
}

impl DefinitionProvider for LatexNavigation {
    fn definitions(
        &self,
        rope: &Rope,
        offset: usize,
        _: &mut Window,
        _: &mut App,
    ) -> Task<Result<Vec<LocationLink>>> {
        let found = symbol_at(rope, offset).and_then(|sym| {
            let project = self.project.read().ok()?;
            let target = nav::definition(&project, &sym)?;
            Some((sym.range(), target))
        });
        let Some((range, target)) = found else {
            return Task::ready(Ok(Vec::new()));
        };
        let mut targets = self.targets.borrow_mut();
        targets.clear();
        targets.push(target);
        let Ok(uri) = format!("{TARGET_URI}0").parse::<lsp_types::Uri>() else {
            return Task::ready(Ok(Vec::new()));
        };
        Task::ready(Ok(vec![LocationLink {
            origin_selection_range: Some(lsp_range(rope, range)),
            target_uri: uri,
            target_range: lsp_types::Range::default(),
            target_selection_range: lsp_types::Range::default(),
        }]))
    }
}

impl HoverProvider for LatexNavigation {
    fn hover(
        &self,
        rope: &Rope,
        offset: usize,
        _: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Option<Hover>>> {
        let notes: Vec<String> = self.notes.iter().filter_map(|n| n(offset, cx)).collect();
        let symbol = symbol_at(rope, offset);
        let about = symbol.as_ref().and_then(|sym| {
            let project = self.project.read().ok()?;
            nav::hover(&project, sym)
        });
        let parts: Vec<String> = notes.into_iter().chain(about).collect();
        if parts.is_empty() {
            return Task::ready(Ok(None));
        }
        Task::ready(Ok(Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: parts.join("\n\n---\n\n"),
            }),
            range: symbol.map(|s| lsp_range(rope, s.range())),
        })))
    }
}
