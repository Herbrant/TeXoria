//! Spell checking of the editor buffers with `tex-spell`: the dictionaries,
//! the analysis that runs in the background, the underlines and the editor's
//! context menu. `Workspace` decides when a buffer is checked.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use gpui_kit::component::input::{
    CodeActionProvider, Copy, Cut, EditorState, Paste, SelectAll, TextDecoration,
    TextDecorationCollection,
};
use gpui_kit::component::native_menu::NativeMenu;
use gpui_kit::*;
use lsp_types::CodeAction;
use serde_json::json;
use tex_spell::lang::{self, Clues};
use tex_spell::{Detection, Source, Speller, Store, catalog, extract};
use tex_syntax::Index;

use crate::underlines;

/// Pause after the last keystroke before a buffer is checked again.
pub const DELAY: Duration = Duration::from_millis(300);
const SUGGESTIONS: usize = 6;

/// Replace a misspelled word (a byte range of the active buffer).
#[derive(Clone, PartialEq, Action)]
#[action(namespace = texoria, no_json)]
pub struct ReplaceMisspelling {
    pub range: Range<usize>,
    pub text: String,
}

#[derive(Clone, PartialEq, Action)]
#[action(namespace = texoria, no_json)]
pub struct AddToDictionary {
    pub word: String,
}

pub enum Dictionary {
    Loading,
    Ready(Arc<Speller>),
    Failed(SharedString),
}

/// Where the dictionaries come from: the system's, the cache, else a
/// download. Tests never download one.
pub struct Dictionaries {
    store: Store,
    download: bool,
}

impl Dictionaries {
    pub fn new(store: Store) -> Self {
        Self {
            store,
            download: true,
        }
    }

    /// Only the dictionaries of `store` already on this machine.
    pub fn offline(store: Store) -> Self {
        Self {
            store,
            download: false,
        }
    }

    /// Load the dictionary for `code`, downloading it if allowed. Blocking.
    pub fn load(&self, code: &str) -> Result<Speller> {
        if !self.download && self.store.local(code).is_none() {
            anyhow::bail!("no {code} dictionary on this machine");
        }
        self.store.load(code)
    }
}

/// The dictionaries, shared by every buffer.
pub struct Spell {
    pub store: Arc<Dictionaries>,
    pub dictionaries: HashMap<&'static str, Dictionary>,
    /// The system locale's region, for regional variants (`GB` → `en-GB`).
    pub region: Option<String>,
    /// The system language: what a document says nothing about is in it.
    pub fallback: &'static str,
    /// The personal dictionary.
    pub words: Arc<HashSet<String>>,
}

impl Spell {
    pub fn new(words: &[String]) -> Self {
        let locale = sys_locale::get_locale();
        let region = locale.as_deref().and_then(region_of);
        let fallback = locale.as_deref().and_then(catalog::resolve).unwrap_or("en");
        Self {
            store: Arc::new(dictionaries()),
            dictionaries: HashMap::new(),
            region,
            fallback,
            words: Arc::new(words.iter().cloned().collect()),
        }
    }

    pub fn set_words(&mut self, words: &[String]) {
        self.words = Arc::new(words.iter().cloned().collect());
    }
}

/// The dictionaries of this machine, downloaded when missing. Tests get
/// none at all: they neither read the machine's nor download any (they give
/// a workspace their own).
fn dictionaries() -> Dictionaries {
    if cfg!(test) {
        let none = std::env::temp_dir().join("texoria-tests-no-dictionaries");
        return Dictionaries::offline(Store::with_dirs(&none, Vec::new()));
    }
    let cache = directories::ProjectDirs::from("app", "TeXoria", "TeXoria")
        .map(|d| d.cache_dir().join("dictionaries"))
        .unwrap_or_else(|| std::env::temp_dir().join("texoria-dictionaries"));
    Dictionaries::new(Store::new(&cache))
}

/// The region of a BCP 47 locale (`en-GB`, `zh-Hant-TW`), in capitals.
fn region_of(locale: &str) -> Option<String> {
    locale
        .split(['-', '_'])
        .skip(1)
        .find(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_alphabetic()))
        .map(str::to_ascii_uppercase)
}

/// A buffer's spell checking.
pub struct BufferSpell {
    /// `None` until the first check, or when the buffer is not checked.
    pub language: Option<Detection>,
    pub underlines: TextDecorationCollection,
    /// The dictionary of `language`, for the context menu.
    pub speller: Rc<RefCell<Option<Arc<Speller>>>>,
    pub task: Task<()>,
}

impl BufferSpell {
    /// Set up spell checking in `editor`: its underlines and corrections.
    pub fn new(editor: &Entity<EditorState>, cx: &mut App) -> Self {
        let speller = Rc::new(RefCell::new(None));
        let underlines = editor.update(cx, |state, cx| {
            let underlines = state.create_decorations_collection(vec![], cx);
            state
                .lsp_mut()
                .code_action_providers
                .push(Rc::new(SpellActions {
                    underlines: underlines.clone(),
                    speller: speller.clone(),
                }));
            underlines
        });
        Self {
            language: None,
            underlines,
            speller,
            task: Task::ready(()),
        }
    }

    pub fn clear(&mut self, cx: &mut App) {
        self.task = Task::ready(());
        self.underlines.clear(cx);
        *self.speller.borrow_mut() = None;
    }
}

/// What a check needs, gathered on the UI thread.
pub struct Job {
    pub text: String,
    /// The language chosen by the user.
    pub chosen: Option<&'static str>,
    pub previous: Option<Detection>,
    pub index: Option<Index>,
    /// The project's root file, when it is not this one.
    pub root: Option<PathBuf>,
    /// Its text, if it is open (else it is read from disk).
    pub root_text: Option<String>,
    pub root_index: Option<Index>,
    pub region: Option<String>,
    pub fallback: &'static str,
}

pub struct Analysis {
    pub language: Detection,
    pub words: Vec<Range<usize>>,
}

/// Find the words to check and the language. Blocking.
pub fn analyze(job: Job) -> Analysis {
    let words = tex_syntax::parse(&job.text, None)
        .map(|tree| extract::words(&tree, &job.text))
        .unwrap_or_default();
    if let Some(code) = job.chosen {
        return Analysis {
            language: Detection {
                code,
                source: Source::User,
            },
            words,
        };
    }
    let root_text = job
        .root_text
        .or_else(|| job.root.as_deref().and_then(tex_project::read_regular));
    let clues = Clues {
        text: &job.text,
        index: job.index.as_ref(),
        root_text: root_text.as_deref(),
        root_index: job.root_index.as_ref(),
        prose: "",
        root_prose: "",
        region: job.region.as_deref(),
        fallback: job.fallback,
    };
    // What the document declares, then (once) what its text looks like: a
    // recognized language stays put while the text grows.
    let declared = lang::detect(&clues);
    let language = match (declared.source, job.previous) {
        (Source::Fallback, Some(prev)) if prev.source == Source::Text => prev,
        (Source::Fallback, _) => {
            let prose = extract::prose(&job.text, &words);
            let found = lang::detect(&Clues {
                prose: &prose,
                ..clues
            });
            match root_text.as_deref() {
                // Too little text of its own (a short chapter): the document's.
                Some(root) if found.source == Source::Fallback => {
                    let root_prose = tex_syntax::parse(root, None)
                        .map(|tree| extract::prose(root, &extract::words(&tree, root)))
                        .unwrap_or_default();
                    lang::detect(&Clues {
                        prose: &prose,
                        root_prose: &root_prose,
                        ..clues
                    })
                }
                _ => found,
            }
        }
        _ => declared,
    };
    Analysis { language, words }
}

pub fn underlines(ranges: Vec<Range<usize>>, color: Hsla) -> Vec<TextDecoration> {
    let style = underlines::wavy(color);
    ranges
        .into_iter()
        .map(|range| TextDecoration { range, style })
        .collect()
}

/// The misspelled range at `offset`, and its word.
fn misspelling_at(
    editor: &Entity<EditorState>,
    underlines: &TextDecorationCollection,
    offset: usize,
    cx: &App,
) -> Option<(Range<usize>, String)> {
    let range = underlines::range_at(underlines, offset, cx)?;
    let word = editor.read(cx).text().slice(range.clone()).to_string();
    Some((range, word))
}

/// Corrections as code actions (⌘. / Ctrl+.).
pub struct SpellActions {
    pub underlines: TextDecorationCollection,
    pub speller: Rc<RefCell<Option<Arc<Speller>>>>,
}

impl CodeActionProvider for SpellActions {
    fn id(&self) -> SharedString {
        "spelling".into()
    }

    fn code_actions(
        &self,
        state: Entity<EditorState>,
        range: Range<usize>,
        _: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Vec<CodeAction>>> {
        let hit = misspelling_at(&state, &self.underlines, range.start, cx);
        let (Some((range, word)), Some(speller)) = (hit, self.speller.borrow().clone()) else {
            return Task::ready(Ok(Vec::new()));
        };
        cx.background_spawn(async move {
            let mut actions: Vec<CodeAction> = speller
                .suggest(&word, SUGGESTIONS)
                .into_iter()
                .map(|text| underlines::replacement(&range, &text))
                .collect();
            actions.push(CodeAction {
                title: format!("Add “{word}” to Dictionary"),
                data: Some(json!({ "add": word })),
                ..Default::default()
            });
            Ok(actions)
        })
    }

    fn perform_code_action(
        &self,
        _: Entity<EditorState>,
        action: CodeAction,
        _: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<()>> {
        underlines::perform(
            action,
            |data| {
                let word = data.get("add")?.as_str()?.to_string();
                Some(Box::new(AddToDictionary { word }))
            },
            window,
            cx,
        );
        Task::ready(Ok(()))
    }
}

/// The editor's right-click menu: corrections when the click is on a
/// misspelled word, then the usual editing items.
pub fn context_menu(
    editor: Entity<EditorState>,
    underlines: TextDecorationCollection,
    speller: Rc<RefCell<Option<Arc<Speller>>>>,
) -> impl Fn(NativeMenu, &mut Window, &mut App) -> NativeMenu + 'static {
    move |mut menu, _, cx| {
        // A right click moves the cursor to the clicked word first.
        let cursor = editor.read(cx).cursor();
        if let Some((range, word)) = misspelling_at(&editor, &underlines, cursor, cx) {
            let suggestions = speller
                .borrow()
                .as_ref()
                .map(|s| s.suggest(&word, SUGGESTIONS))
                .unwrap_or_default();
            for entry in corrections(range, word, suggestions) {
                menu = match entry {
                    Entry::Item {
                        label,
                        action,
                        disabled,
                    } => menu.menu_with_disabled(label, disabled, action),
                    Entry::Separator => menu.separator(),
                };
            }
        }
        menu.menu("Cut", Box::new(Cut))
            .menu("Copy", Box::new(Copy))
            .menu("Paste", Box::new(Paste))
            .separator()
            .menu("Select All", Box::new(SelectAll))
    }
}

/// An entry of the context menu.
enum Entry {
    Item {
        label: String,
        action: Box<dyn Action>,
        disabled: bool,
    },
    Separator,
}

/// The menu's entries for the misspelled `word` at `range`.
fn corrections(range: Range<usize>, word: String, suggestions: Vec<String>) -> Vec<Entry> {
    let mut entries = Vec::new();
    if suggestions.is_empty() {
        let none = ReplaceMisspelling {
            range: range.clone(),
            text: word.clone(),
        };
        entries.push(Entry::Item {
            label: "No Suggestions".into(),
            action: Box::new(none),
            disabled: true,
        });
    }
    for text in suggestions {
        let action = ReplaceMisspelling {
            range: range.clone(),
            text: text.clone(),
        };
        entries.push(Entry::Item {
            label: text,
            action: Box::new(action),
            disabled: false,
        });
    }
    entries.push(Entry::Separator);
    entries.push(Entry::Item {
        label: format!("Add “{word}” to Dictionary"),
        action: Box::new(AddToDictionary { word }),
        disabled: false,
    });
    entries.push(Entry::Separator);
    entries
}

#[cfg(test)]
mod tests;
