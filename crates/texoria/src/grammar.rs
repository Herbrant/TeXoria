//! Grammar checking of the editor buffers with LanguageTool
//! (`tex_spell::grammar`): the server (found running, or started by
//! TeXoria), the checks in the background, the underlines, the corrections
//! and the hover. `Workspace` decides when a buffer is checked.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::process::{Child, Command, Stdio};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use gpui_kit::component::input::{
    CodeActionProvider, EditorState, TextDecoration, TextDecorationCollection,
};
use gpui_kit::component::native_menu::NativeMenu;
use gpui_kit::*;
use lsp_types::{CodeAction, CodeActionKind};
use serde_json::json;
use tex_spell::grammar::{self, Problem, Server};

use crate::latex::HoverNote;
use crate::spell::ReplaceMisspelling;

/// Pause after the last keystroke before a buffer is checked again: a check
/// costs the server more than a spelling check.
pub const DELAY: Duration = Duration::from_millis(1500);
/// How long a server TeXoria starts may take to answer (the JVM is slow).
const START_TIMEOUT: Duration = Duration::from_secs(90);
/// Rules that the annotated LaTeX would trip anyway.
const ALWAYS_OFF: &[&str] = &["WHITESPACE_RULE"];

/// Stop reporting a LanguageTool rule.
#[derive(Clone, PartialEq, Action)]
#[action(namespace = texoria, no_json)]
pub struct IgnoreGrammarRule {
    pub rule: String,
}

#[derive(Clone, PartialEq)]
pub enum ServerState {
    Off,
    /// Looking for it, or waiting for the one TeXoria started.
    Connecting,
    /// With the languages it knows.
    Ready(Arc<Vec<String>>),
    Unreachable(SharedString),
}

/// The server, shared by every buffer.
pub struct Grammar {
    pub server: Option<Arc<Server>>,
    pub state: ServerState,
    /// Findings by chunk (hash of its text, language and rules), with
    /// ranges relative to the chunk.
    pub cache: Arc<Mutex<HashMap<u64, Vec<Problem>>>>,
    /// A server TeXoria started, stopped with it.
    child: Option<Child>,
}

impl Default for Grammar {
    fn default() -> Self {
        Self {
            server: None,
            state: ServerState::Off,
            cache: Arc::default(),
            child: None,
        }
    }
}

impl Grammar {
    /// Stop the server TeXoria started, if any.
    pub fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.server = None;
        self.state = ServerState::Off;
    }

    /// Start `languagetool-server` on `port`, if it is installed.
    pub fn spawn(&mut self, port: u16) -> Result<(), String> {
        let program = tex_agent::find_program("languagetool-server").ok_or_else(|| {
            "LanguageTool is not installed (no languagetool-server program)".to_string()
        })?;
        let mut cmd = Command::new(program);
        cmd.args(["--port", &port.to_string()])
            .env("PATH", tex_agent::search_path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let child = cmd
            .spawn()
            .map_err(|e| format!("Cannot start languagetool-server: {e}"))?;
        log::info!("started languagetool-server on port {port}");
        self.child = Some(child);
        Ok(())
    }

    pub fn started_server(&self) -> bool {
        self.child.is_some()
    }
}

impl Drop for Grammar {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Wait until the server at `server` answers, up to [`START_TIMEOUT`].
pub fn wait_for(server: &Server) -> Result<Vec<String>, String> {
    let deadline = std::time::Instant::now() + START_TIMEOUT;
    loop {
        match server.languages() {
            Ok(l) => return Ok(l),
            Err(_) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_secs(1))
            }
            Err(e) => return Err(format!("{e:#}")),
        }
    }
}

/// Check `text` in `language` without the rules in `disabled`, chunk by
/// chunk, asking the server only for chunks not in `cache`.
pub fn check(
    server: &Server,
    cache: &Mutex<HashMap<u64, Vec<Problem>>>,
    text: &str,
    language: &str,
    disabled: &[String],
) -> Result<Vec<Problem>> {
    let Some(tree) = tex_syntax::parse(text, None) else {
        return Ok(Vec::new());
    };
    let mut rules: Vec<String> = ALWAYS_OFF.iter().map(|r| r.to_string()).collect();
    rules.extend(disabled.iter().cloned());
    let mut out = Vec::new();
    for chunk in grammar::chunks(text, grammar::annotate(&tree, text)) {
        let start = chunk.first().map_or(0, |p| p.range.start);
        let end = chunk.last().map_or(start, |p| p.range.end);
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (&text[start..end], language, &rules).hash(&mut h);
        let key = h.finish();
        let cached = cache.lock().unwrap().get(&key).cloned();
        let found = match cached {
            Some(found) => found,
            None => {
                let found = relative(server.check(text, &chunk, language, &rules)?, start..end);
                let mut cache = cache.lock().unwrap();
                if cache.len() >= CACHE_CHUNKS {
                    // Old chunks are mostly text that has been edited since.
                    cache.clear();
                }
                cache.insert(key, found.clone());
                found
            }
        };
        out.extend(found.into_iter().map(|p| Problem {
            range: p.range.start + start..p.range.end + start,
            ..p
        }));
    }
    Ok(out)
}

/// Chunks whose findings are kept, beyond which the cache starts over.
const CACHE_CHUNKS: usize = 4096;

/// `problems` found in the chunk `chunk` of the text, with ranges relative to it. Those
/// outside it (a server answering about another text) are dropped.
fn relative(problems: Vec<Problem>, chunk: std::ops::Range<usize>) -> Vec<Problem> {
    problems
        .into_iter()
        .filter(|p| chunk.start <= p.range.start && p.range.start <= p.range.end)
        .filter(|p| p.range.end <= chunk.end)
        .map(|p| Problem {
            range: p.range.start - chunk.start..p.range.end - chunk.start,
            ..p
        })
        .collect()
}

/// An underlined finding: where it was, the text there, and the finding.
struct Note {
    start: usize,
    text: String,
    problem: Problem,
}

type Notes = Rc<RefCell<Vec<Note>>>;

/// A buffer's grammar checking.
pub struct BufferGrammar {
    underlines: TextDecorationCollection,
    notes: Notes,
    pub task: Task<()>,
}

impl BufferGrammar {
    /// Set up grammar checking in `editor`: its underlines and corrections.
    pub fn new(editor: &Entity<EditorState>, cx: &mut App) -> Self {
        let notes: Notes = Rc::default();
        let underlines = editor.update(cx, |state, cx| {
            let underlines = state.create_decorations_collection(vec![], cx);
            state
                .lsp_mut()
                .code_action_providers
                .push(Rc::new(GrammarActions {
                    underlines: underlines.clone(),
                    notes: notes.clone(),
                }));
            underlines
        });
        Self {
            underlines,
            notes,
            task: Task::ready(()),
        }
    }

    pub fn set(
        &self,
        editor: &Entity<EditorState>,
        problems: Vec<Problem>,
        color: Hsla,
        cx: &mut App,
    ) {
        let text = editor.read(cx).text().clone();
        let style = HighlightStyle {
            underline: Some(UnderlineStyle {
                thickness: px(1.),
                color: Some(color),
                wavy: true,
            }),
            ..Default::default()
        };
        let mut notes = Vec::new();
        let mut decorations = Vec::new();
        for p in problems {
            let r = p.range.clone();
            if r.is_empty()
                || r.end > text.len()
                || !text.is_char_boundary(r.start)
                || !text.is_char_boundary(r.end)
            {
                continue;
            }
            decorations.push(TextDecoration::new(r.clone(), style));
            notes.push(Note {
                start: r.start,
                text: text.slice(r).to_string(),
                problem: p,
            });
        }
        *self.notes.borrow_mut() = notes;
        self.underlines.set(decorations, cx);
    }

    pub fn clear(&mut self, cx: &mut App) {
        self.task = Task::ready(());
        self.notes.borrow_mut().clear();
        self.underlines.clear(cx);
    }

    pub fn hover_note(&self, editor: &Entity<EditorState>) -> HoverNote {
        let (underlines, notes, editor) = (
            self.underlines.clone(),
            self.notes.clone(),
            editor.downgrade(),
        );
        Rc::new(move |offset, cx| {
            let editor = editor.upgrade()?;
            let (_, problem) = problem_at(&editor, &underlines, &notes, offset, cx)?;
            Some(format!("✎ {}", problem.message))
        })
    }

    /// The editor's right-click menu: the corrections of the finding under
    /// the cursor, if any.
    pub fn context_menu(
        &self,
        editor: Entity<EditorState>,
    ) -> impl Fn(NativeMenu, &mut App) -> NativeMenu + 'static {
        let (underlines, notes) = (self.underlines.clone(), self.notes.clone());
        move |mut menu, cx| {
            let cursor = editor.read(cx).cursor();
            let Some((range, problem)) = problem_at(&editor, &underlines, &notes, cursor, cx)
            else {
                return menu;
            };
            for text in &problem.replacements {
                let action = ReplaceMisspelling {
                    range: range.clone(),
                    text: text.clone(),
                };
                menu = menu.menu(display(text), Box::new(action));
            }
            menu.menu(
                format!("Ignore “{}”", problem.rule),
                Box::new(IgnoreGrammarRule {
                    rule: problem.rule.clone(),
                }),
            )
            .separator()
        }
    }
}

/// A replacement as a menu item (a space alone would show nothing).
fn display(text: &str) -> String {
    if text.trim().is_empty() {
        "(remove)".to_string()
    } else {
        text.to_string()
    }
}

/// The finding underlined at `offset`, and where it is now.
fn problem_at(
    editor: &Entity<EditorState>,
    underlines: &TextDecorationCollection,
    notes: &Notes,
    offset: usize,
    cx: &App,
) -> Option<(Range<usize>, Problem)> {
    let range = underlines
        .get_ranges(cx)
        .into_iter()
        .find(|r| r.start <= offset && offset <= r.end)?;
    let text = editor.read(cx).text().slice(range.clone()).to_string();
    let notes = notes.borrow();
    let note = notes
        .iter()
        .filter(|n| n.text == text)
        .min_by_key(|n| n.start.abs_diff(range.start))?;
    Some((range, note.problem.clone()))
}

/// Corrections as code actions (⌘. / Ctrl+.).
struct GrammarActions {
    underlines: TextDecorationCollection,
    notes: Notes,
}

impl CodeActionProvider for GrammarActions {
    fn id(&self) -> SharedString {
        "grammar".into()
    }

    fn code_actions(
        &self,
        state: Entity<EditorState>,
        range: Range<usize>,
        _: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Vec<CodeAction>>> {
        let Some((range, problem)) =
            problem_at(&state, &self.underlines, &self.notes, range.start, cx)
        else {
            return Task::ready(Ok(Vec::new()));
        };
        let mut actions: Vec<CodeAction> = problem
            .replacements
            .iter()
            .map(|text| CodeAction {
                title: format!("Change to “{}”", display(text)),
                kind: Some(CodeActionKind::QUICKFIX),
                data: Some(json!({ "start": range.start, "end": range.end, "text": text })),
                ..Default::default()
            })
            .collect();
        actions.push(CodeAction {
            title: format!("Ignore “{}” ({})", problem.rule, problem.message),
            data: Some(json!({ "ignore": problem.rule })),
            ..Default::default()
        });
        Task::ready(Ok(actions))
    }

    fn perform_code_action(
        &self,
        _: Entity<EditorState>,
        action: CodeAction,
        _: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<()>> {
        // The editor is being updated: the workspace applies it afterwards.
        let data = action.data.unwrap_or_default();
        let field = |k: &str| data.get(k).and_then(|v| v.as_str()).map(str::to_string);
        let offset = |k: &str| data.get(k).and_then(|v| v.as_u64()).map(|v| v as usize);
        if let Some(rule) = field("ignore") {
            window.dispatch_action(Box::new(IgnoreGrammarRule { rule }), cx);
        } else if let (Some(start), Some(end), Some(text)) =
            (offset("start"), offset("end"), field("text"))
        {
            let action = ReplaceMisspelling {
                range: start..end,
                text,
            };
            window.dispatch_action(Box::new(action), cx);
        }
        Task::ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    fn problem(range: std::ops::Range<usize>) -> Problem {
        Problem {
            range,
            message: String::new(),
            replacements: Vec::new(),
            rule: "R".into(),
            issue_type: "grammar".into(),
        }
    }

    #[test]
    fn findings_are_made_relative_to_their_chunk() {
        let found = relative(
            vec![
                problem(10..12),
                problem(3..5),
                problem(18..25),
                problem(19..20),
            ],
            10..20,
        );
        let ranges: Vec<_> = found.iter().map(|p| p.range.clone()).collect();
        // Before the chunk or past its end: not about this chunk.
        assert_eq!(ranges, [0..2, 9..10]);
    }
}
