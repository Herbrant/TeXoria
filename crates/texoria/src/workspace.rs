//! The main window: outline or files | editor | PDF, with live typesetting.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use gpui_kit::component::highlighter::{Diagnostic as EditorDiagnostic, DiagnosticSeverity};
use gpui_kit::component::input::{
    Editor, EditorState, Input, InputEvent, InputState, Position, RopeExt, TabSize,
};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::resizable::{h_resizable, resizable_panel};
use gpui_kit::component::{
    ActiveTheme, Disableable as _, Icon, Sizable, TitleBar, WindowExt as _,
    button::{Button, ButtonVariants},
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use tex_compile::{Compiler, Engine, Event as CompileEvent, Job, Severity, Status};
use tex_git::{Cancel, Credential, Git};
use tex_log::Diagnostic;
use tex_project::{Project, ProjectOutlineItem, files};
use tex_spell::{Source, Speller, catalog};
use tex_syntax::OutlineKind;

use crate::assets::Glyph;
use crate::atomic;
use crate::diff_view::{DiffSource, DiffView, DiffViewEvent};
use crate::file_tree::{FileOp, FileTree, FileTreeEvent};
use crate::git_panel::{self, GitOp, GitPanel, GitPanelEvent};
use crate::latex::{self, CursorFixup, LatexCompletions};
use crate::overleaf;
use crate::pdf_pane::{PdfPane, PdfPaneEvent};
use crate::settings::{Settings, SpellLanguage, ThemeChoice};
use crate::settings_window::SettingsWindow;
use crate::spell::{self, AddToDictionary, BufferSpell, Dictionary, ReplaceMisspelling, Spell};
use crate::theme::{self, ThemeExt};
use crate::{
    CancelTypeset, CloneRepository, CloseTab, Commit, Fetch, ForwardSearch, NewBranch, Open,
    OpenFromOverleaf, OpenSettings, Pull, Push, Save, ShowSourceControl, SyncOverleaf,
    ToggleDiagnostics, ToggleFiles, ToggleLivePreview, ToggleOutline, ToggleSoftWrap, ToggleTheme,
    Typeset, ZoomIn, ZoomOut, ZoomToFit,
};

const SOFT_WRAP_MAX_LINES: usize = 4000;

#[derive(Debug, Clone)]
enum CompileState {
    Idle,
    Running,
    Done {
        status: Status,
        elapsed: Duration,
        errors: usize,
        warnings: usize,
    },
    Failed(SharedString),
}

struct Buffer {
    path: PathBuf,
    editor: Entity<EditorState>,
    /// Text as last written to disk, to know whether a save is needed.
    saved: String,
    dirty: bool,
    last_len: usize,
    soft_wrap: bool,
    spell: BufferSpell,
    _subscription: Subscription,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sidebar {
    Outline,
    Files,
    SourceControl,
}

pub struct Workspace {
    pub(crate) settings: Settings,
    project: Arc<RwLock<Project>>,
    root: Option<PathBuf>,
    buffers: Vec<Buffer>,
    active: usize,
    pdf: Entity<PdfPane>,
    pub(crate) engines: Vec<Engine>,
    pub(crate) engine_ix: usize,
    compiler: Option<Compiler>,
    state: CompileState,
    diagnostics: Vec<Diagnostic>,
    outline: Vec<ProjectOutlineItem>,
    show_sidebar: bool,
    sidebar: Sidebar,
    /// The last of Outline and Files: what the sidebar button shows.
    navigator: Sidebar,
    files: Entity<FileTree>,
    git: Entity<GitPanel>,
    /// The diff tab, shown instead of the editor while `diff_active`.
    diff: Option<Entity<DiffView>>,
    diff_active: bool,
    /// A clone in progress (URL), started from the welcome screen.
    cloning: Option<(SharedString, Cancel)>,
    show_diagnostics: bool,
    /// The project has a `latexmkrc` that is ignored until the user trusts it.
    untrusted_rc: bool,
    /// A close was already refused once because saving failed.
    refused_close: bool,
    fixups: Rc<RefCell<Vec<CursorFixup>>>,
    focus: FocusHandle,
    was_dark: bool,
    settings_window: Option<AnyWindowHandle>,
    spell: Spell,
    _live: Task<()>,
    _compile_events: Task<()>,
    _diff_subscription: Option<Subscription>,
    _subscriptions: Vec<Subscription>,
}

impl Workspace {
    pub fn new(path: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings = Settings::load();
        let pdf = cx.new(|cx| PdfPane::new(pdfium_dirs(), cx));
        let files = cx.new(FileTree::new);
        let git = cx.new(|cx| GitPanel::new(window, cx));
        theme::apply(settings.theme, Some(window), cx);
        let mut subscriptions = vec![
            cx.subscribe_in(
                &pdf,
                window,
                |this, _, ev: &PdfPaneEvent, window, cx| match ev {
                    PdfPaneEvent::InverseSearch { file, line } => {
                        this.reveal(file.clone(), line.saturating_sub(1), window, cx)
                    }
                },
            ),
            cx.subscribe_in(&git, window, Self::on_git_event),
            cx.subscribe_in(&files, window, Self::on_file_tree_event),
            // Coming back to the window: pick up what changed meanwhile
            // (a `git pull` in a terminal, another editor).
            cx.observe_window_activation(window, |this, window, cx| {
                if window.is_window_active() {
                    this.reload_from_disk(window, cx);
                    this.files.update(cx, |f, cx| f.refresh(cx));
                    this.git.update(cx, |g, cx| g.refresh(cx));
                }
            }),
            cx.observe_window_appearance(window, |this, window, cx| {
                if this.settings.theme == ThemeChoice::System {
                    theme::apply(ThemeChoice::System, Some(window), cx);
                    this.sync_theme(cx);
                }
            }),
        ];
        // Never lose edits made since the last (debounced) save.
        subscriptions.push(cx.on_app_quit(|this, cx| {
            this.save_all(cx);
            async {}
        }));
        let weak = cx.entity().downgrade();
        window.on_window_should_close(cx, move |_, cx| {
            weak.update(cx, |this, cx| {
                // If saving fails, keep the window (and the error) up once.
                let close = this.save_all(cx) || this.refused_close;
                this.refused_close = !close;
                close
            })
            .unwrap_or(true)
        });

        let engines = tex_compile::detect(dev_vendor_dir("tectonic").as_deref());
        let engine_ix = settings
            .engine
            .as_ref()
            .and_then(|label| engines.iter().position(|e| &e.label() == label))
            .unwrap_or(0);

        let spell = Spell::new(&settings.spell_words);
        let mut this = Workspace {
            settings,
            project: Arc::new(RwLock::new(Project::default())),
            root: None,
            buffers: Vec::new(),
            active: 0,
            pdf,
            engines,
            engine_ix,
            compiler: None,
            state: CompileState::Idle,
            diagnostics: Vec::new(),
            outline: Vec::new(),
            show_sidebar: true,
            sidebar: Sidebar::Outline,
            navigator: Sidebar::Outline,
            files,
            git,
            diff: None,
            diff_active: false,
            cloning: None,
            show_diagnostics: false,
            untrusted_rc: false,
            refused_close: false,
            fixups: Rc::new(RefCell::new(Vec::new())),
            focus: cx.focus_handle(),
            was_dark: cx.theme_is_dark(),
            settings_window: None,
            spell,
            _live: Task::ready(()),
            _compile_events: Task::ready(()),
            _diff_subscription: None,
            _subscriptions: subscriptions,
        };
        this.configure_git(cx);
        this.start_compiler(cx);
        if this.engines.is_empty() {
            this.state = CompileState::Failed(
                "No TeX engine found. Install TeX Live or MiKTeX, or place tectonic next to TeXoria.".into(),
            );
        }
        if let Some(p) = path {
            this.open_path(p, window, cx);
        }
        let dark = this.was_dark;
        this.pdf.update(cx, |pdf, cx| pdf.set_dark(dark, cx));
        this
    }

    fn configure_git(&mut self, cx: &mut Context<Self>) {
        let s = &self.settings;
        let (rebase, timeout, servers) =
            (s.pull_rebase, s.git_timeout(), s.overleaf_servers.clone());
        self.git
            .update(cx, |g, _| g.configure(rebase, timeout, servers));
    }

    // ── Settings ─────────────────────────────────────────────────────────

    /// Change the settings, save them and bring what is running in line.
    pub fn update_settings(
        &mut self,
        change: impl FnOnce(&mut Settings),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let old = self.settings.clone();
        change(&mut self.settings);
        self.settings.save();

        if self.settings.engine != old.engine
            && let Some(ix) = self
                .settings
                .engine
                .as_ref()
                .and_then(|label| self.engines.iter().position(|e| &e.label() == label))
            && ix != self.engine_ix
        {
            self.set_engine(ix, cx);
        }
        if self.settings.compile_timeout_secs != old.compile_timeout_secs
            && let Some(c) = &self.compiler
        {
            c.set_timeout(self.settings.compile_timeout());
        }
        if self.settings.soft_wrap != old.soft_wrap {
            let wrap = self.settings.soft_wrap;
            for b in &mut self.buffers {
                let lines = b.editor.read(cx).text().lines_len();
                b.soft_wrap = wrap && lines <= SOFT_WRAP_MAX_LINES;
                let wrap = b.soft_wrap;
                b.editor
                    .update(cx, |state, cx| state.set_soft_wrap(wrap, window, cx));
            }
        }
        let s = &self.settings;
        if (s.pull_rebase, s.git_timeout_secs, &s.overleaf_servers)
            != (old.pull_rebase, old.git_timeout_secs, &old.overleaf_servers)
        {
            self.configure_git(cx);
        }
        if self.settings.diff_split != old.diff_split
            && let Some(d) = &self.diff
        {
            let split = self.settings.diff_split;
            d.update(cx, |d, cx| d.set_split(split, cx));
        }
        if self.settings.trusted_projects != old.trusted_projects {
            self.sync_git(cx);
        }
        if self.settings.rc_allowed != old.rc_allowed
            && let Some(job) = self.job()
            && job.allow_project_rc != old.allows_rc(job.work_dir())
        {
            self.untrusted_rc = !job.allow_project_rc && job.has_project_rc();
            self.typeset(cx);
        }
        if self.settings.theme != old.theme {
            theme::apply(self.settings.theme, Some(window), cx);
            self.sync_theme(cx);
        }
        let s = &self.settings;
        if (s.spell_check, &s.spell_words, &s.spell_languages)
            != (old.spell_check, &old.spell_words, &old.spell_languages)
        {
            self.spell.set_words(&self.settings.spell_words);
            for ix in 0..self.buffers.len() {
                let path = &self.buffers[ix].path;
                if self.settings.spell_language(path) != old.spell_language(path) {
                    // Back to detecting it, or a new choice.
                    self.buffers[ix].spell.language = None;
                }
                self.spellcheck(ix, Duration::ZERO, cx);
            }
        }
        cx.notify();
    }

    /// Open the Settings window, or bring it to the front.
    fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(handle) = self.settings_window
            && handle
                .update(cx, |_, window, _| window.activate_window())
                .is_ok()
        {
            return;
        }
        let workspace = cx.entity().downgrade();
        let main = window.window_handle();
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::centered(size(px(820.), px(600.)), cx)),
            window_min_size: Some(size(px(560.), px(400.))),
            app_id: Some("texoria".into()),
            ..TitleBar::window_options()
        };
        // The new window reads the workspace, which is being updated here.
        cx.defer(move |cx| {
            let opened = gpui_kit::open_window(options, cx, {
                let workspace = workspace.clone();
                move |window, cx| cx.new(|cx| SettingsWindow::new(workspace, main, window, cx))
            });
            match opened {
                Ok((handle, _)) => {
                    let _ = workspace.update(cx, |w, _| w.settings_window = Some(handle));
                }
                Err(e) => log::error!("opening the settings window: {e}"),
            }
        });
    }

    fn toggle_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let choice = theme::toggled(cx);
        self.update_settings(|s| s.theme = choice, window, cx);
    }

    fn start_compiler(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.engines.get(self.engine_ix).cloned() else {
            return;
        };
        let (tx, rx) = smol::channel::unbounded::<CompileEvent>();
        // Live debouncing happens here in the UI (it must save first), so the
        // scheduler runs without its own delay.
        let compiler = Compiler::new(engine, Duration::ZERO, move |e| {
            let _ = tx.send_blocking(e);
        });
        compiler.set_timeout(self.settings.compile_timeout());
        self.compiler = Some(compiler);
        self._compile_events = cx.spawn(async move |this, cx| {
            while let Ok(event) = rx.recv().await {
                if this
                    .update(cx, |this, cx| this.on_compile_event(event, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
    }

    // ── Opening files ────────────────────────────────────────────────────

    pub fn open_path(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        let file = if path.is_dir() {
            find_main_file(&path)
        } else {
            Some(path.clone())
        };
        let Some(file) = file else {
            self.state = CompileState::Failed(
                format!("No .tex file with \\documentclass in {}", path.display()).into(),
            );
            cx.notify();
            return;
        };
        let text = tex_project::read_regular(&file).unwrap_or_default();
        let root = tex_project::find_root(&file, &text);
        let root = std::fs::canonicalize(&root).unwrap_or(root);
        self.set_root(root, cx);
        self.activate_file(file, window, cx);
    }

    /// Make `root` the project's root file (a no-op if it already is).
    fn set_root(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        if self.root.as_deref() != Some(&root) {
            self.root = Some(root.clone());
            *self.project.write().unwrap() = Project::open(&root);
            self.refresh_outline(cx);
            self.settings.push_recent(root.clone());
            self.settings.save();
            // The previous project's run is irrelevant now.
            if let Some(c) = &self.compiler {
                c.cancel();
            }
            let job = self.job().expect("root is set");
            self.untrusted_rc = !job.allow_project_rc && job.has_project_rc();
            self.close_diff(cx);
            self.sync_git(cx);
            self.files
                .update(cx, |f, cx| f.set_project(Some(root.clone()), cx));
            // Show a previous build right away, if any.
            if job.artifact("pdf").is_file() {
                let synctex = Some(job.artifact("synctex.gz")).filter(|p| p.is_file());
                self.pdf
                    .update(cx, |pdf, cx| pdf.load(job.artifact("pdf"), synctex, cx));
            }
            self.typeset(cx);
        }
    }

    /// Focus the buffer for `file`, opening it if needed. Returns its index.
    fn activate_file(
        &mut self,
        file: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<usize> {
        self.diff_active = false;
        if let Some(ix) = self.buffers.iter().position(|b| b.path == file) {
            self.active = ix;
            cx.notify();
            return Some(ix);
        }
        // Only regular files: inverse search and diagnostics can point
        // anywhere, and reading a pipe or device would hang.
        let read = std::fs::metadata(&file).and_then(|m| {
            if m.is_file() {
                std::fs::read_to_string(&file)
            } else {
                Err(std::io::Error::other("not a regular file"))
            }
        });
        let text = match read {
            Ok(t) => t,
            Err(e) => {
                self.state =
                    CompileState::Failed(format!("Cannot open {}: {e}", file.display()).into());
                cx.notify();
                return None;
            }
        };
        // Soft wrap lays out every line up front; past a few thousand lines
        // it dominates memory, so large files start unwrapped (⌥Z toggles).
        let soft_wrap = self.settings.soft_wrap && text.lines().count() <= SOFT_WRAP_MAX_LINES;
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language(latex::LANGUAGE)
                .line_number(true)
                .soft_wrap(soft_wrap)
                .tab_size(TabSize {
                    tab_size: 2,
                    hard_tabs: false,
                })
                .default_value(text.clone())
        });
        let provider = Rc::new(LatexCompletions {
            project: self.project.clone(),
            fixups: self.fixups.clone(),
        });
        editor.update(cx, |state, cx| {
            state.lsp_mut().completion_provider = Some(provider);
            state.set_editor_paddings(Edges {
                top: px(18.),
                right: px(24.),
                bottom: px(18.),
                left: px(8.),
            });
            cx.notify();
        });
        let subscription = cx.subscribe_in(
            &editor,
            window,
            |this, editor, ev: &InputEvent, window, cx| {
                if let InputEvent::Change = ev {
                    this.on_buffer_changed(editor.entity_id(), window, cx);
                }
            },
        );
        let last_len = text.len();
        let spell = BufferSpell::new(&editor, cx);
        self.buffers.push(Buffer {
            path: file,
            editor,
            saved: text,
            dirty: false,
            last_len,
            soft_wrap,
            spell,
            _subscription: subscription,
        });
        self.active = self.buffers.len() - 1;
        self.spellcheck(self.active, Duration::ZERO, cx);
        self.apply_diagnostics(cx);
        let focus = self.buffers[self.active].editor.focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
        Some(self.active)
    }

    /// Jump to a 0-based `line` in `file` (outline, diagnostics, inverse search).
    fn reveal(&mut self, file: PathBuf, line: u32, window: &mut Window, cx: &mut Context<Self>) {
        let file = std::fs::canonicalize(&file).unwrap_or(file);
        let Some(ix) = self.activate_file(file, window, cx) else {
            return;
        };
        let editor = self.buffers[ix].editor.clone();
        editor.update(cx, |state, cx| {
            state.set_cursor_position(Position::new(line, 0), window, cx);
            state.focus(window, cx);
        });
    }

    fn prompt_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: true,
            multiple: false,
            prompt: Some("Open".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Some(path) = paths
                .await
                .ok()
                .and_then(|r| r.ok())
                .flatten()
                .and_then(|p| p.into_iter().next())
            else {
                return;
            };
            this.update_in(cx, |this, window, cx| this.open_path(path, window, cx))
                .ok();
        })
        .detach();
    }

    // ── Editing & live preview ───────────────────────────────────────────

    fn on_buffer_changed(&mut self, id: EntityId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.buffers.iter().position(|b| b.editor.entity_id() == id) else {
            return;
        };
        let editor = self.buffers[ix].editor.clone();
        let (len, cursor, dirty) = {
            let state = editor.read(cx);
            let text = state.text();
            let len = text.len();
            (
                len,
                state.cursor(),
                len != self.buffers[ix].saved.len() || *text != self.buffers[ix].saved,
            )
        };
        let grew = len.saturating_sub(self.buffers[ix].last_len);
        self.buffers[ix].last_len = len;
        self.buffers[ix].dirty = dirty;

        // A multi-character insertion ending in a completion's text is that
        // completion landing: move the cursor into its first argument.
        if grew > 1 {
            let fix = {
                let state = editor.read(cx);
                let text = state.text();
                self.fixups.borrow().iter().find_map(|f| {
                    let start = cursor.checked_sub(f.insert.len())?;
                    // Slicing inside a multi-byte char would panic.
                    (text.is_char_boundary(start) && text.slice(start..cursor) == f.insert.as_str())
                        .then(|| cursor - f.back)
                })
            };
            if let Some(target) = fix {
                self.fixups.borrow_mut().clear();
                editor.update(cx, |state, cx| state.set_selected_range(target..target, cx));
            }
        }

        self.spellcheck(ix, spell::DELAY, cx);
        let delay = Duration::from_millis(self.settings.live_delay_ms);
        let live = self.settings.live_preview;
        self._live = cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(delay).await;
            this.update(cx, |this, cx| {
                this.reindex(cx);
                if live {
                    this.typeset(cx);
                }
            })
            .ok();
        });
        cx.notify();
    }

    /// Re-index open buffers so outline and completions follow unsaved edits.
    fn reindex(&mut self, cx: &mut Context<Self>) {
        {
            let mut project = self.project.write().unwrap();
            for b in &self.buffers {
                let text = b.editor.read(cx).text().to_string();
                project.update(&b.path, &text);
            }
        }
        self.refresh_outline(cx);
    }

    fn refresh_outline(&mut self, cx: &mut Context<Self>) {
        self.outline = self.project.read().unwrap().outline();
        cx.notify();
    }

    fn save_all(&mut self, cx: &mut Context<Self>) -> bool {
        let mut ok = true;
        let mut wrote = false;
        for b in &mut self.buffers {
            if !b.dirty {
                continue;
            }
            let text = b.editor.read(cx).text().to_string();
            match atomic::write(&b.path, text.as_bytes()) {
                Ok(()) => {
                    b.saved = text;
                    b.dirty = false;
                    wrote = true;
                }
                Err(e) => {
                    ok = false;
                    self.state = CompileState::Failed(
                        format!("Cannot save {}: {e}", b.path.display()).into(),
                    );
                }
            }
        }
        if wrote {
            self.git.update(cx, |g, cx| g.refresh(cx));
        }
        cx.notify();
        ok
    }

    /// Pick up files changed on disk behind our back (git checkout, pull,
    /// another editor). Buffers with unsaved edits are left alone; tabs of
    /// deleted files close.
    fn reload_from_disk(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut changed = false;
        let mut ix = 0;
        while ix < self.buffers.len() {
            let b = &self.buffers[ix];
            if b.dirty {
                ix += 1;
                continue;
            }
            if !b.path.exists() {
                if self.buffers.len() > 1 {
                    self.buffers.remove(ix);
                    changed = true;
                    continue;
                }
                ix += 1;
                continue;
            }
            if let Some(text) = tex_project::read_regular(&b.path)
                && text != b.saved
            {
                b.editor.clone().update(cx, |state, cx| {
                    let cursor = state.cursor_position();
                    state.set_value(text.clone(), window, cx);
                    state.set_cursor_position(cursor, window, cx);
                });
                let b = &mut self.buffers[ix];
                b.last_len = text.len();
                b.saved = text;
                changed = true;
                // `set_value` emits no change event.
                self.spellcheck(ix, Duration::ZERO, cx);
            }
            ix += 1;
        }
        if changed {
            self.active = self.active.min(self.buffers.len().saturating_sub(1));
            self.reindex(cx);
            self.apply_diagnostics(cx);
            self.typeset(cx);
        }
    }

    /// Point the Source Control panel at the current project.
    fn sync_git(&mut self, cx: &mut Context<Self>) {
        let dir = self
            .root
            .as_ref()
            .and_then(|r| r.parent())
            .map(Path::to_path_buf);
        let trusted = dir.as_ref().is_some_and(|d| self.settings.is_trusted(d));
        self.git.update(cx, |g, cx| g.set_project(dir, trusted, cx));
    }

    fn on_git_event(
        &mut self,
        _: &Entity<GitPanel>,
        ev: &GitPanelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match ev {
            GitPanelEvent::Request(op) => {
                // git must see what is on screen.
                if self.save_all(cx) {
                    let op = op.clone();
                    self.git.update(cx, |g, cx| g.execute(op, window, cx));
                }
            }
            GitPanelEvent::OpenDiff(source) => self.open_diff(source.clone(), window, cx),
            GitPanelEvent::WorktreeChanged => {
                self.reload_from_disk(window, cx);
                self.files.update(cx, |f, cx| f.refresh(cx));
                if let Some(d) = &self.diff {
                    d.update(cx, |d, cx| d.reload(cx));
                }
            }
            GitPanelEvent::StatusChanged => {
                if let Some(d) = &self.diff {
                    d.update(cx, |d, cx| d.reload(cx));
                }
                let marks = self.git.read(cx).file_marks();
                self.files.update(cx, |f, cx| f.set_marks(marks, cx));
                cx.notify();
            }
            GitPanelEvent::Trust => self.trust_git(cx),
        }
    }

    // ── Files ────────────────────────────────────────────────────────────

    fn on_file_tree_event(
        &mut self,
        _: &Entity<FileTree>,
        ev: &FileTreeEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match ev {
            FileTreeEvent::Open(path) => {
                if files::looks_like_text(path) {
                    self.activate_file(path.clone(), window, cx);
                } else {
                    cx.open_with_system(path);
                }
            }
            FileTreeEvent::Request(op) => self.run_file_op(op.clone(), window, cx),
        }
    }

    /// Carry out a change to the project's files, keeping open tabs in step.
    fn run_file_op(&mut self, op: FileOp, window: &mut Window, cx: &mut Context<Self>) {
        // Unsaved edits would be lost by a delete, or written back to the
        // old path after a rename.
        if !self.save_all(cx) {
            return;
        }
        let (title, result) = match &op {
            FileOp::CreateFile(path) => ("Cannot create the file", create_file(path)),
            FileOp::CreateDir(path) => ("Cannot create the folder", std::fs::create_dir_all(path)),
            FileOp::Rename { from, to } => ("Cannot rename", rename(from, to)),
            FileOp::Delete(path) => ("Cannot delete", remove(path)),
        };
        if let Err(e) = result {
            window.push_notification(Notification::error(e.to_string()).title(title), cx);
            self.files.update(cx, |f, cx| f.refresh(cx));
            return;
        }

        let active = self.buffers.get(self.active).map(|b| b.path.clone());
        match op {
            FileOp::CreateFile(path) => {
                self.activate_file(path, window, cx);
            }
            FileOp::CreateDir(_) => {}
            FileOp::Rename { from, to } => {
                let moved = |p: &Path| {
                    let rest = p.strip_prefix(&from).ok()?;
                    Some(if rest.as_os_str().is_empty() {
                        to.clone()
                    } else {
                        to.join(rest)
                    })
                };
                for b in &mut self.buffers {
                    if let Some(p) = moved(&b.path) {
                        b.path = p;
                    }
                }
                self.settings.move_spell_languages(&from, &to);
                self.settings.save();
                if let Some(root) = self.root.as_deref().and_then(moved) {
                    self.set_root(root, cx);
                }
            }
            FileOp::Delete(path) => {
                self.buffers.retain(|b| !b.path.starts_with(&path));
                self.active = active
                    .and_then(|a| self.buffers.iter().position(|b| b.path == a))
                    .unwrap_or(0);
                if self.buffers.is_empty()
                    && let Some(root) = self.root.clone()
                {
                    self.activate_file(root, window, cx);
                }
            }
        }
        self.files.update(cx, |f, cx| f.refresh(cx));
        self.git.update(cx, |g, cx| g.refresh(cx));
        self.reindex(cx);
    }

    // ── Diff tab ─────────────────────────────────────────────────────────

    fn open_diff(&mut self, source: DiffSource, window: &mut Window, cx: &mut Context<Self>) {
        self.save_all(cx);
        let Some(repo) = self.git.read(cx).repo().cloned() else {
            return;
        };
        match &self.diff {
            Some(d) => d.update(cx, |d, cx| d.show(repo, source, cx)),
            None => {
                let split = self.settings.diff_split;
                let d = cx.new(|cx| DiffView::new(repo, source, split, cx));
                self._diff_subscription = Some(cx.subscribe_in(&d, window, Self::on_diff_event));
                self.diff = Some(d);
            }
        }
        self.diff_active = true;
        cx.notify();
    }

    fn close_diff(&mut self, cx: &mut Context<Self>) {
        self.diff = None;
        self.diff_active = false;
        self._diff_subscription = None;
        cx.notify();
    }

    fn on_diff_event(
        &mut self,
        _: &Entity<DiffView>,
        ev: &DiffViewEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match ev {
            DiffViewEvent::Restore { sha, path } => {
                self.restore_version(sha.clone(), path.clone(), window, cx)
            }
            DiffViewEvent::Resolved { path, text } => {
                self.save_resolution(path.clone(), text.clone(), window, cx)
            }
            DiffViewEvent::Open(file) => {
                let file = std::fs::canonicalize(file).unwrap_or_else(|_| file.clone());
                self.activate_file(file, window, cx);
            }
            DiffViewEvent::SplitChanged(split) => {
                let split = *split;
                self.update_settings(|s| s.diff_split = split, window, cx);
            }
            DiffViewEvent::Close => self.close_diff(cx),
        }
    }

    /// Write `contents` to a project file and show it in its open tab.
    fn replace_file(
        &mut self,
        file: &Path,
        contents: &[u8],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> std::io::Result<()> {
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        atomic::write(file, contents)?;
        self.reload_from_disk(window, cx);
        Ok(())
    }

    fn restore_version(
        &mut self,
        sha: String,
        path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(repo) = self.git.read(cx).repo().cloned() else {
            return;
        };
        // Unsaved edits would be overwritten without a trace.
        if !self.save_all(cx) {
            return;
        }
        let file = repo.abs_path(&path);
        let short: String = sha.chars().take(7).collect();
        cx.spawn_in(window, async move |this, cx| {
            let contents = cx
                .background_spawn(async move { repo.file_at(&sha, &path) })
                .await;
            this.update_in(cx, |this, window, cx| match contents {
                Ok(bytes) => match this.replace_file(&file, &bytes, window, cx) {
                    Ok(()) => {
                        let name = file
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        window.push_notification(
                            Notification::success(format!("{name} restored from {short}"))
                                .title("Version restored"),
                            cx,
                        );
                        this.git.update(cx, |g, cx| g.refresh(cx));
                    }
                    Err(e) => window.push_notification(
                        Notification::error(format!("Cannot write {}: {e}", file.display())),
                        cx,
                    ),
                },
                Err(e) => git_panel::notify_error(window, cx, "Restore failed", &e),
            })
            .ok();
        })
        .detach();
    }

    fn save_resolution(
        &mut self,
        path: String,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(repo) = self.git.read(cx).repo().cloned() else {
            return;
        };
        if !self.save_all(cx) {
            return;
        }
        let file = repo.abs_path(&path);
        match self.replace_file(&file, text.as_bytes(), window, cx) {
            Ok(()) => {
                self.close_diff(cx);
                self.git
                    .update(cx, |g, cx| g.execute(GitOp::Stage(vec![path]), window, cx));
            }
            Err(e) => window.push_notification(
                Notification::error(format!("Cannot write {}: {e}", file.display())),
                cx,
            ),
        }
    }

    // ── Cloning ──────────────────────────────────────────────────────────

    fn prompt_clone(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.cloning.is_some() {
            return;
        }
        let url = cx
            .new(|cx| InputState::new(window, cx).placeholder("https://github.com/you/paper.git"));
        let weak = cx.entity().downgrade();
        let focus = url.focus_handle(cx);
        let body = url.clone();
        git_panel::open_form(
            window,
            cx,
            "Clone Repository",
            "Choose Folder…",
            Some(focus),
            move |_, cx| {
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(
                                "The URL of a Git repository (GitHub, GitLab, Overleaf…). \
                                 Next, pick the folder to put it in.",
                            ),
                    )
                    .child(Input::new(&body))
                    .into_any_element()
            },
            move |window, cx| {
                let url = url.read(cx).value().trim().to_string();
                if url.is_empty() {
                    return false;
                }
                weak.update(cx, |this, cx| {
                    // Overleaf needs a token, which a plain clone cannot ask for.
                    match this.overleaf_project(&url).filter(|_| url.contains("://")) {
                        Some(project) => this.open_overleaf(project, window, cx),
                        None => this.choose_clone_dir(url, window, cx),
                    }
                })
                .ok();
                true
            },
        );
    }

    fn choose_clone_dir(&mut self, url: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = tex_git::clone_dir_name(&url) else {
            window.push_notification(Notification::error("That is not a repository URL"), cx);
            return;
        };
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Clone Here".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Some(parent) = paths
                .await
                .ok()
                .and_then(|r| r.ok())
                .flatten()
                .and_then(|p| p.into_iter().next())
            else {
                return;
            };
            this.update_in(cx, |this, window, cx| {
                this.clone_into(url, parent.join(name), None, window, cx)
            })
            .ok();
        })
        .detach();
    }

    fn clone_into(
        &mut self,
        url: String,
        dest: PathBuf,
        credential: Option<Credential>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.cloning.is_some() {
            return;
        }
        let cancel = Cancel::new();
        self.cloning = Some((url.clone().into(), cancel.clone()));
        let timeout = self.settings.git_timeout();
        let job = (url.clone(), dest.clone(), credential.clone());
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let overleaf = credential.is_some();
                    let git = Git::find()
                        .ok_or_else(|| tex_git::Error::Invalid("Git is not installed".into()))?
                        .network_timeout(timeout)
                        .credential(credential);
                    let repo = git.clone_repo(&url, &dest, &cancel)?;
                    // Overleaf compiles on its side: local build files must not
                    // reach the collaborators.
                    if overleaf && let Err(e) = repo.exclude_local_files() {
                        log::warn!("{e}");
                    }
                    Ok(repo.root().to_path_buf())
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.cloning = None;
                let (url, dest, credential) = job;
                match result {
                    Ok(dir) => {
                        if let Some(c) = &credential {
                            overleaf::save_token(&c.url, &c.password, window, cx);
                        }
                        this.open_path(dir.clone(), window, cx);
                        // We created it, so its configuration is git's default; its
                        // latexmkrc comes from the remote and still needs consent. A
                        // `%!TEX root` may point elsewhere: that is not ours to trust.
                        let dir = std::fs::canonicalize(&dir).unwrap_or(dir);
                        if this.root.as_ref().is_some_and(|r| r.starts_with(&dir)) {
                            this.trust_git(cx);
                        }
                    }
                    Err(tex_git::Error::Cancelled) => {}
                    Err(tex_git::Error::Auth { .. }) if let Some(c) = credential => {
                        // Kept until replaced: the token may be fine, and only this
                        // project out of reach.
                        let weak = cx.entity().downgrade();
                        let server = c.url.clone();
                        overleaf::prompt_token(
                            &c.url,
                            overleaf::TOKEN_REJECTED,
                            move |token, window, cx| {
                                let credential = overleaf::credential(&server, token);
                                weak.update(cx, |this, cx| {
                                    this.clone_into(
                                        url.clone(),
                                        dest.clone(),
                                        Some(credential),
                                        window,
                                        cx,
                                    )
                                })
                                .ok();
                            },
                            window,
                            cx,
                        );
                    }
                    Err(e) => git_panel::notify_error(window, cx, "Clone failed", &e),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    // ── Overleaf ─────────────────────────────────────────────────────────

    fn overleaf_project(&self, input: &str) -> Option<tex_git::overleaf::Project> {
        tex_git::overleaf::project(input, &self.settings.overleaf_servers)
    }

    fn prompt_overleaf(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.cloning.is_some() {
            return;
        }
        let input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("https://www.overleaf.com/project/…")
        });
        let weak = cx.entity().downgrade();
        let focus = input.focus_handle(cx);
        let body = input.clone();
        git_panel::open_form(
            window,
            cx,
            "Open from Overleaf",
            "Continue",
            Some(focus),
            move |_, cx| {
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(
                                "The project's address as shown in your browser, or its \
                                 Git URL. TeXoria keeps a copy you can edit offline, and \
                                 syncs it through Overleaf's Git integration (a premium \
                                 feature).",
                            ),
                    )
                    .child(Input::new(&body))
                    .into_any_element()
            },
            move |window, cx| {
                let text = input.read(cx).value().trim().to_string();
                if text.is_empty() {
                    return false;
                }
                let Ok(project) = weak.update(cx, |this, _| this.overleaf_project(&text)) else {
                    return true;
                };
                let Some(project) = project else {
                    window.push_notification(
                        Notification::error("That is not the address of an Overleaf project"),
                        cx,
                    );
                    return false;
                };
                weak.update(cx, |this, cx| this.open_overleaf(project, window, cx))
                    .ok();
                true
            },
        );
    }

    /// Get a token for the project's server, then a folder, then clone.
    fn open_overleaf(
        &mut self,
        project: tex_git::overleaf::Project,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let token = overleaf::load_token(&project.server, cx);
        cx.spawn_in(window, async move |this, cx| {
            let token = token.await;
            this.update_in(cx, |this, window, cx| match token {
                Some(token) => this.choose_overleaf_dir(project, token, window, cx),
                None => {
                    let weak = cx.entity().downgrade();
                    let server = project.server.clone();
                    overleaf::prompt_token(
                        &server,
                        overleaf::TOKEN_NEEDED,
                        move |token, window, cx| {
                            let project = project.clone();
                            weak.update(cx, |this, cx| {
                                this.choose_overleaf_dir(project, token, window, cx)
                            })
                            .ok();
                        },
                        window,
                        cx,
                    );
                }
            })
            .ok();
        })
        .detach();
    }

    fn choose_overleaf_dir(
        &mut self,
        project: tex_git::overleaf::Project,
        token: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Git cannot tell the project's name: the user picks the folder's.
        let start = directories::UserDirs::new()
            .map(|d| d.document_dir().unwrap_or(d.home_dir()).to_path_buf())
            .unwrap_or_default();
        let path = cx.prompt_for_new_path(&start, Some("Overleaf Project"));
        cx.spawn_in(window, async move |this, cx| {
            let Some(dest) = path.await.ok().and_then(|r| r.ok()).flatten() else {
                return;
            };
            this.update_in(cx, |this, window, cx| {
                let credential = overleaf::credential(&project.server, token);
                this.clone_into(project.git_url, dest, Some(credential), window, cx)
            })
            .ok();
        })
        .detach();
    }

    fn show_sidebar(&mut self, kind: Sidebar, cx: &mut Context<Self>) {
        self.show_sidebar = true;
        self.sidebar = kind;
        match kind {
            Sidebar::Outline => self.navigator = kind,
            Sidebar::Files => {
                self.navigator = kind;
                self.files.update(cx, |f, cx| f.refresh(cx));
            }
            Sidebar::SourceControl => {}
        }
        cx.notify();
    }

    fn toggle_sidebar(&mut self, kind: Sidebar, cx: &mut Context<Self>) {
        if self.show_sidebar && self.sidebar == kind {
            self.show_sidebar = false;
            cx.notify();
        } else {
            self.show_sidebar(kind, cx);
        }
    }

    /// The compile job for the current root. The project's own latexmk rc
    /// file runs arbitrary code, so it is honored only once trusted.
    fn job(&self) -> Option<Job> {
        let mut job = Job::for_root(self.root.as_ref()?);
        job.allow_project_rc = self.settings.allows_rc(job.work_dir());
        Some(job)
    }

    /// Let Git run in the project (its configuration can run programs).
    fn trust_git(&mut self, cx: &mut Context<Self>) {
        let Some(job) = self.job() else {
            return;
        };
        self.settings.trust(job.work_dir().to_path_buf());
        self.settings.save();
        self.sync_git(cx);
        cx.notify();
    }

    fn allow_rc(&mut self, cx: &mut Context<Self>) {
        let Some(job) = self.job() else {
            return;
        };
        self.settings.allow_rc(job.work_dir().to_path_buf());
        self.settings.save();
        self.untrusted_rc = false;
        self.typeset(cx);
    }

    fn typeset(&mut self, cx: &mut Context<Self>) {
        let Some(job) = self.job() else {
            return;
        };
        if !self.save_all(cx) {
            return;
        }
        if let Some(c) = &self.compiler {
            c.request_now(job);
        }
    }

    fn cancel_typeset(&mut self) {
        if let Some(c) = &self.compiler {
            c.cancel();
        }
    }

    fn on_compile_event(&mut self, event: CompileEvent, cx: &mut Context<Self>) {
        match event {
            CompileEvent::Started { .. } => {
                self.state = CompileState::Running;
            }
            CompileEvent::SpawnFailed { error, .. } => {
                self.state = CompileState::Failed(error.into());
            }
            CompileEvent::Finished(out) => {
                // A run for a project that is no longer open.
                if self.root.as_ref() != Some(&out.job.root_file) {
                    return;
                }
                if out.status == Status::Cancelled {
                    self.state = CompileState::Idle;
                    cx.notify();
                    return;
                }
                let errors = out
                    .diagnostics
                    .iter()
                    .filter(|d| d.severity == Severity::Error)
                    .count();
                let warnings = out
                    .diagnostics
                    .iter()
                    .filter(|d| d.severity == Severity::Warning)
                    .count();
                self.state = CompileState::Done {
                    status: out.status,
                    elapsed: out.elapsed,
                    errors,
                    warnings,
                };
                self.diagnostics = out
                    .diagnostics
                    .into_iter()
                    .map(|mut d| {
                        d.file = d.file.map(|f| std::fs::canonicalize(&f).unwrap_or(f));
                        d
                    })
                    .collect();
                self.apply_diagnostics(cx);
                if let Some(pdf) = out.pdf {
                    self.pdf.update(cx, |p, cx| p.load(pdf, out.synctex, cx));
                }
                // .bib files may have been edited outside the app.
                self.project.write().unwrap().reload_bibs();
            }
        }
        cx.notify();
    }

    /// Push compile diagnostics into each open editor as squiggles.
    fn apply_diagnostics(&mut self, cx: &mut Context<Self>) {
        for b in &self.buffers {
            let diags: Vec<&Diagnostic> = self
                .diagnostics
                .iter()
                .filter(|d| d.file.as_deref() == Some(b.path.as_path()))
                .collect();
            b.editor.update(cx, |state, cx| {
                let items: Vec<EditorDiagnostic> = {
                    let text = state.text();
                    diags
                        .iter()
                        .filter_map(|d| {
                            let row = d.line?.saturating_sub(1) as usize;
                            if row >= text.lines_len() {
                                return None;
                            }
                            let line = text.slice_line(row).to_string();
                            // Columns are in chars.
                            let start = line.chars().count() - line.trim_start().chars().count();
                            let end = line.trim_end().chars().count().max(start + 1);
                            let severity = match d.severity {
                                Severity::Error => DiagnosticSeverity::Error,
                                Severity::Warning => DiagnosticSeverity::Warning,
                                Severity::Info => DiagnosticSeverity::Info,
                            };
                            Some(
                                EditorDiagnostic::new(
                                    Position::new(row as u32, start as u32)
                                        ..Position::new(row as u32, end as u32),
                                    d.message.clone(),
                                )
                                .with_severity(severity),
                            )
                        })
                        .collect()
                };
                if let Some(set) = state.diagnostics_mut() {
                    set.clear();
                    set.extend(items);
                }
                cx.notify();
            });
        }
    }

    // ── Spell checking ───────────────────────────────────────────────────

    /// Check buffer `ix` after `delay` (replacing a pending check).
    fn spellcheck(&mut self, ix: usize, delay: Duration, cx: &mut Context<Self>) {
        let Some(b) = self.buffers.get_mut(ix) else {
            return;
        };
        let chosen = self.settings.spell_language(&b.path);
        if !self.settings.spell_check || chosen == Some(SpellLanguage::OFF) {
            b.spell.language = None;
            b.spell.clear(cx);
            return;
        }
        // An unknown code (a hand-edited setting) means detecting it.
        let chosen = chosen.and_then(catalog::get).map(|l| l.code);
        let path = b.path.clone();
        let editor = b.editor.clone();
        let previous = b.spell.language;
        let root = self.root.clone().filter(|r| *r != path);
        let root_text = root
            .as_ref()
            .and_then(|r| self.buffers.iter().find(|b| &b.path == r))
            .map(|b| b.editor.read(cx).text().to_string());
        let (index, root_index) = {
            let project = self.project.read().unwrap();
            let index = project.index(&path).cloned();
            (index, root.as_ref().and_then(|r| project.index(r).cloned()))
        };
        let region = self.spell.region.clone();
        let fallback = self.spell.fallback;
        let words = self.spell.words.clone();
        let color = cx.theme().danger;
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let Ok(text) = this.update(cx, |_, cx| editor.read(cx).text().to_string()) else {
                return;
            };
            let job = spell::Job {
                text: text.clone(),
                chosen,
                previous,
                index,
                root,
                root_text,
                root_index,
                region,
                fallback,
            };
            let analysis = cx
                .background_spawn(async move { spell::analyze(job) })
                .await;
            let speller = this
                .update(cx, |this, cx| {
                    let speller = this.dictionary(analysis.language.code, cx);
                    let b = this.buffers.iter_mut().find(|b| b.editor == editor)?;
                    b.spell.language = Some(analysis.language);
                    *b.spell.speller.borrow_mut() = speller.clone();
                    if speller.is_none() {
                        // Underlines from another language would be wrong;
                        // loading the dictionary checks the buffer again.
                        b.spell.underlines.clear(cx);
                    }
                    cx.notify();
                    speller
                })
                .ok()
                .flatten();
            let Some(speller) = speller else {
                return;
            };
            let (checked, words_in) = (text.clone(), analysis.words);
            let misspelled = cx
                .background_spawn(async move { speller.misspelled(&checked, &words_in, &words) })
                .await;
            this.update(cx, |this, cx| {
                let Some(b) = this.buffers.iter().find(|b| b.editor == editor) else {
                    return;
                };
                // Edited meanwhile: the next check is already on its way.
                if *b.editor.read(cx).text() != text {
                    return;
                }
                b.spell
                    .underlines
                    .set(spell::underlines(misspelled, color), cx);
            })
            .ok();
        });
        self.buffers[ix].spell.task = task;
    }

    /// The dictionary for `code`, or `None` while it loads (and checks the
    /// buffers in that language again once it has).
    fn dictionary(&mut self, code: &'static str, cx: &mut Context<Self>) -> Option<Arc<Speller>> {
        match self.spell.dictionaries.get(code) {
            Some(Dictionary::Ready(s)) => return Some(s.clone()),
            Some(_) => return None,
            None => {}
        }
        self.spell.dictionaries.insert(code, Dictionary::Loading);
        let store = self.spell.store.clone();
        cx.spawn(async move |this, cx| {
            let loaded = cx.background_spawn(async move { store.load(code) }).await;
            this.update(cx, |this, cx| {
                let entry = match loaded {
                    Ok(s) => Dictionary::Ready(Arc::new(s)),
                    Err(e) => {
                        log::warn!("spell checking dictionary {code}: {e:#}");
                        Dictionary::Failed(format!("{e:#}").into())
                    }
                };
                this.spell.dictionaries.insert(code, entry);
                for ix in 0..this.buffers.len() {
                    if this.buffers[ix]
                        .spell
                        .language
                        .is_some_and(|d| d.code == code)
                    {
                        this.spellcheck(ix, Duration::ZERO, cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        None
    }

    /// Try a dictionary that failed to load (offline) again.
    fn retry_dictionary(&mut self, code: &str, cx: &mut Context<Self>) {
        if matches!(
            self.spell.dictionaries.get(code),
            Some(Dictionary::Failed(_))
        ) {
            self.spell.dictionaries.remove(code);
        }
        self.spellcheck(self.active, Duration::ZERO, cx);
    }

    /// Choose the active file's language (`None`: detect it).
    fn set_spell_language(
        &mut self,
        language: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(path) = self.buffers.get(self.active).map(|b| b.path.clone()) else {
            return;
        };
        self.update_settings(|s| s.set_spell_language(&path, language), window, cx);
    }

    fn replace_misspelling(
        &mut self,
        action: &ReplaceMisspelling,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(b) = self.buffers.get(self.active) else {
            return;
        };
        let range = action.range.clone();
        let text = action.text.clone();
        b.editor.update(cx, |state, cx| {
            if range.end > state.text().len() {
                return;
            }
            state.set_selected_range(range, cx);
            state.replace(text, window, cx);
        });
    }

    // ── Actions ──────────────────────────────────────────────────────────

    fn forward_search(&mut self, cx: &mut Context<Self>) {
        let Some(b) = self.buffers.get(self.active) else {
            return;
        };
        let line = b.editor.read(cx).cursor_position().line + 1;
        let path = b.path.clone();
        self.pdf.update(cx, |pdf, cx| {
            pdf.forward_search(&path, line, cx);
        });
    }

    fn next_engine(&mut self, cx: &mut Context<Self>) {
        if self.engines.is_empty() {
            return;
        }
        self.set_engine((self.engine_ix + 1) % self.engines.len(), cx);
    }

    fn set_engine(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.engine_ix = ix;
        let engine = self.engines[ix].clone();
        self.settings.engine = Some(engine.label());
        self.settings.save();
        if let Some(c) = &self.compiler {
            c.set_engine(engine);
        }
        self.typeset(cx);
        cx.notify();
    }

    fn close_tab(&mut self, cx: &mut Context<Self>) {
        if self.diff_active {
            return self.close_diff(cx);
        }
        if self.buffers.len() <= 1 {
            return;
        }
        // Closing after a failed save would drop the edits.
        if !self.save_all(cx) {
            return;
        }
        self.buffers.remove(self.active);
        self.active = self.active.min(self.buffers.len() - 1);
        cx.notify();
    }

    fn sync_theme(&mut self, cx: &mut Context<Self>) {
        let dark = cx.theme_is_dark();
        if dark != self.was_dark {
            self.was_dark = dark;
            self.pdf.update(cx, |pdf, cx| pdf.set_dark(dark, cx));
            // Underlines take their color from the theme.
            for ix in 0..self.buffers.len() {
                self.spellcheck(ix, Duration::ZERO, cx);
            }
        }
    }

    // ── Rendering ────────────────────────────────────────────────────────

    fn render_title_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let title: SharedString = self
            .root
            .as_ref()
            .and_then(|r| r.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "TeXoria".into())
            .into();
        let subtitle: Option<SharedString> = self
            .root
            .as_ref()
            .and_then(|r| r.parent())
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned().into());
        let changes = self.git.read(cx).change_count();
        let has_project = self.root.is_some();

        TitleBar::new().child(
            h_flex()
                .flex_1()
                .h_full()
                .gap_1()
                .child(
                    Button::new("toggle-outline")
                        .ghost()
                        .small()
                        .icon(Glyph::PanelLeft)
                        .tooltip("Outline and files (⌘1, ⇧⌘E)")
                        .on_click(
                            cx.listener(|this, _, _, cx| this.toggle_sidebar(this.navigator, cx)),
                        ),
                )
                .when(has_project, |d| {
                    d.child(
                        Button::new("toggle-git")
                            .ghost()
                            .small()
                            .icon(Glyph::Branch)
                            .when(changes > 0, |b| b.label(changes.to_string()))
                            .tooltip("Source control (⇧⌘G)")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.toggle_sidebar(Sidebar::SourceControl, cx)
                            })),
                    )
                })
                .child(
                    h_flex()
                        .flex_1()
                        .justify_center()
                        .gap_2()
                        .text_sm()
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.foreground)
                                .child(title),
                        )
                        .when_some(subtitle, |el, s| {
                            el.child(
                                div()
                                    .text_color(theme.muted_foreground)
                                    .child(format!("— {s}")),
                            )
                        }),
                )
                .child(self.render_status_pill(cx))
                .child(
                    Button::new("typeset")
                        .primary()
                        .small()
                        .icon(Glyph::Play)
                        .label("Typeset")
                        .tooltip("Typeset now (⌘R)")
                        .on_click(cx.listener(|this, _, _, cx| this.typeset(cx))),
                )
                .child(
                    Button::new("theme")
                        .ghost()
                        .small()
                        .icon(if cx.theme_is_dark() {
                            Glyph::Sun
                        } else {
                            Glyph::Moon
                        })
                        .tooltip("Toggle light/dark")
                        .on_click(cx.listener(|this, _, window, cx| this.toggle_theme(window, cx))),
                )
                .child(
                    Button::new("settings")
                        .ghost()
                        .small()
                        .icon(Glyph::Settings)
                        .tooltip("Settings (⌘,)")
                        .on_click(
                            cx.listener(|this, _, window, cx| this.open_settings(window, cx)),
                        ),
                )
                .child(div().w(px(6.))),
        )
    }

    fn render_status_pill(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (icon, color, label): (Glyph, Hsla, SharedString) = match &self.state {
            CompileState::Idle => (Glyph::Info, theme.muted_foreground, "Ready".into()),
            CompileState::Running => (Glyph::Spinner, theme.primary, "Typesetting…".into()),
            CompileState::Done {
                status: Status::Success,
                elapsed,
                warnings,
                ..
            } => (
                Glyph::Ok,
                theme.success,
                if *warnings > 0 {
                    format!(
                        "{:.1}s · {warnings} warning{}",
                        elapsed.as_secs_f32(),
                        plural(*warnings)
                    )
                    .into()
                } else {
                    format!("Typeset in {:.1}s", elapsed.as_secs_f32()).into()
                },
            ),
            CompileState::Done {
                errors, warnings, ..
            } => (
                Glyph::Error,
                theme.danger,
                if *errors > 0 {
                    format!("{errors} error{}", plural(*errors)).into()
                } else {
                    format!("Failed · {warnings} warning{}", plural(*warnings)).into()
                },
            ),
            CompileState::Failed(msg) => (Glyph::Warning, theme.warning, msg.clone()),
        };
        let spinning = matches!(self.state, CompileState::Running);
        let icon = Icon::new(icon).xsmall().text_color(color);
        h_flex()
            .id("status-pill")
            .gap_1p5()
            .px_2p5()
            .py_0p5()
            .max_w(px(360.))
            .rounded_full()
            .bg(theme.muted)
            .text_xs()
            .text_color(theme.muted_foreground)
            .cursor_pointer()
            .hover(|s| s.bg(theme.accent))
            .child(if spinning {
                icon.with_animation(
                    "spin",
                    Animation::new(Duration::from_millis(900)).repeat(),
                    |icon, t| icon.transform(Transformation::rotate(percentage(t))),
                )
                .into_any_element()
            } else {
                icon.into_any_element()
            })
            .child(div().truncate().child(label))
            .on_click(cx.listener(|this, _, _, cx| {
                this.show_diagnostics = !this.show_diagnostics;
                cx.notify();
            }))
    }

    /// The sidebar's Outline and Files views, with a switch between them.
    fn render_navigator(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let files = self.sidebar == Sidebar::Files;
        let tab = |id: &'static str, label: &'static str, kind: Sidebar, cx: &mut Context<Self>| {
            let active = self.sidebar == kind;
            let theme = cx.theme();
            div()
                .id(id)
                .px_2()
                .py_0p5()
                .rounded(theme.radius)
                .text_xs()
                .cursor_pointer()
                .when(active, |d| {
                    d.bg(theme.sidebar_accent)
                        .text_color(theme.foreground)
                        .font_weight(FontWeight::MEDIUM)
                })
                .when(!active, |d| {
                    d.text_color(theme.muted_foreground)
                        .hover(|s| s.text_color(theme.foreground))
                })
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| this.show_sidebar(kind, cx)))
        };
        let file_button = |id: &'static str, icon: Glyph, tooltip: &'static str| {
            Button::new(id)
                .ghost()
                .xsmall()
                .icon(icon)
                .tooltip(tooltip)
                .disabled(self.root.is_none())
        };
        let header = h_flex()
            .px_3()
            .pt_3()
            .pb_2()
            .gap_1()
            .child(tab("nav-outline", "Outline", Sidebar::Outline, cx))
            .child(tab("nav-files", "Files", Sidebar::Files, cx))
            .child(div().flex_1())
            .when(files, |d| {
                d.child(
                    file_button("files-new-file", Glyph::NewFile, "New File").on_click(
                        cx.listener(|this, _, window, cx| {
                            this.files
                                .update(cx, |f, cx| f.prompt_new(false, window, cx))
                        }),
                    ),
                )
                .child(
                    file_button("files-new-folder", Glyph::NewFolder, "New Folder").on_click(
                        cx.listener(|this, _, window, cx| {
                            this.files
                                .update(cx, |f, cx| f.prompt_new(true, window, cx))
                        }),
                    ),
                )
                .child(
                    file_button("files-refresh", Glyph::Refresh, "Refresh").on_click(
                        cx.listener(|this, _, _, cx| this.files.update(cx, |f, cx| f.refresh(cx))),
                    ),
                )
            });
        let body = if files {
            div()
                .flex_1()
                .min_h_0()
                .pb_2()
                .child(self.files.clone())
                .into_any_element()
        } else {
            self.render_outline(cx).into_any_element()
        };

        let theme = cx.theme();
        v_flex()
            .size_full()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.sidebar_border)
            .child(header)
            .child(body)
    }

    fn render_outline(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        // The section containing the cursor is highlighted.
        let current = self.buffers.get(self.active).and_then(|b| {
            let line = b.editor.read(cx).cursor_position().line;
            self.outline
                .iter()
                .enumerate()
                .filter(|(_, o)| o.file == b.path && o.item.line <= line)
                .map(|(i, _)| i)
                .next_back()
        });

        let items = self.outline.iter().enumerate().map(|(i, o)| {
            let kind = o.item.kind;
            let depth = match kind {
                OutlineKind::Figure | OutlineKind::Table => {
                    // Indent floats under the preceding heading.
                    self.outline[..i]
                        .iter()
                        .rev()
                        .find(|p| !p.item.kind.is_float())
                        .map_or(0, |p| p.item.kind.depth() + 1)
                }
                k => k.depth(),
            };
            let min_depth = self
                .outline
                .iter()
                .map(|o| o.item.kind.depth())
                .min()
                .unwrap_or(0);
            let indent = (depth.saturating_sub(min_depth) as f32) * 12.0;
            let icon = match kind {
                OutlineKind::Figure => Glyph::Figure,
                OutlineKind::Table => Glyph::Table,
                _ => Glyph::Section,
            };
            let active = current == Some(i);
            let title: SharedString = if o.item.title.is_empty() {
                "Untitled".into()
            } else {
                o.item.title.clone().into()
            };
            let (file, line) = (o.file.clone(), o.item.line);
            h_flex()
                .id(("outline", i))
                .gap_2()
                .pl(px(10.0 + indent))
                .pr_2()
                .py_1()
                .mx_1()
                .rounded(theme.radius)
                .text_sm()
                .cursor_pointer()
                .when(active, |d| {
                    d.bg(theme.sidebar_accent).text_color(theme.foreground)
                })
                .when(!active, |d| d.text_color(theme.sidebar_foreground))
                .when(kind.depth() <= 2, |d| d.font_weight(FontWeight::MEDIUM))
                .hover(|s| s.bg(theme.sidebar_accent))
                .child(
                    Icon::new(icon)
                        .xsmall()
                        .text_color(theme.muted_foreground.opacity(0.8)),
                )
                .child(div().truncate().child(title))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.reveal(file.clone(), line, window, cx)
                }))
        });

        v_flex()
            .id("outline-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .pb_4()
            .when(self.outline.is_empty(), |d| {
                d.child(
                    div()
                        .px_4()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child("No sections yet"),
                )
            })
            .children(items)
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let root_dir = self
            .root
            .as_ref()
            .and_then(|r| r.parent())
            .map(Path::to_path_buf);
        let diff_title: Option<SharedString> =
            self.diff.as_ref().map(|d| d.read(cx).title().into());
        h_flex()
            .h(px(34.))
            .px_2()
            .gap_1()
            .bg(theme.tab_bar)
            .border_b_1()
            .border_color(theme.border)
            .children(self.buffers.iter().enumerate().map(|(i, b)| {
                let name = root_dir
                    .as_ref()
                    .and_then(|d| b.path.strip_prefix(d).ok())
                    .unwrap_or(&b.path)
                    .to_string_lossy()
                    .into_owned();
                let active = i == self.active && !self.diff_active;
                h_flex()
                    .id(("tab", i))
                    .gap_1p5()
                    .px_3()
                    .h(px(26.))
                    .rounded(theme.radius)
                    .text_xs()
                    .cursor_pointer()
                    .when(active, |d| {
                        d.bg(theme.tab_active)
                            .text_color(theme.tab_active_foreground)
                            .shadow_xs()
                    })
                    .when(!active, |d| {
                        d.text_color(theme.tab_foreground)
                            .hover(|s| s.bg(theme.accent))
                    })
                    .child(Icon::new(Glyph::File).xsmall())
                    .child(name)
                    .when(b.dirty, |d| {
                        d.child(div().size(px(6.)).rounded_full().bg(theme.muted_foreground))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.active = i;
                        this.diff_active = false;
                        cx.notify();
                    }))
            }))
            .when_some(diff_title, |d, title| {
                let active = self.diff_active;
                d.child(
                    h_flex()
                        .id("tab-diff")
                        .gap_1p5()
                        .pl_3()
                        .pr_1()
                        .h(px(26.))
                        .rounded(theme.radius)
                        .text_xs()
                        .italic()
                        .cursor_pointer()
                        .when(active, |d| {
                            d.bg(theme.tab_active)
                                .text_color(theme.tab_active_foreground)
                                .shadow_xs()
                        })
                        .when(!active, |d| {
                            d.text_color(theme.tab_foreground)
                                .hover(|s| s.bg(theme.accent))
                        })
                        .child(Icon::new(Glyph::Branch).xsmall())
                        .child(title)
                        .child(
                            div()
                                .id("tab-diff-close")
                                .p_0p5()
                                .rounded(theme.radius)
                                .hover(|s| s.bg(theme.muted))
                                .child(Icon::new(Glyph::Close).xsmall())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.close_diff(cx);
                                })),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.diff_active = true;
                            cx.notify();
                        })),
                )
            })
    }

    fn render_trust_banner(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        h_flex()
            .gap_3()
            .px_3()
            .py_1p5()
            .text_sm()
            .bg(theme.warning.opacity(0.12))
            .border_b_1()
            .border_color(theme.border)
            .child(Icon::new(Glyph::Warning).xsmall().text_color(theme.warning))
            .child(div().flex_1().text_color(theme.foreground).child(
                "This project's latexmkrc can run arbitrary commands and is ignored \
                         until you allow it.",
            ))
            .child(
                Button::new("allow-rc")
                    .warning()
                    .xsmall()
                    .label("Allow latexmkrc")
                    .on_click(cx.listener(|this, _, _, cx| this.allow_rc(cx))),
            )
    }

    fn render_diagnostics(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let root_dir = self
            .root
            .as_ref()
            .and_then(|r| r.parent())
            .map(Path::to_path_buf);
        let rows = self
            .diagnostics
            .iter()
            .filter(|d| d.severity != Severity::Info)
            .enumerate()
            .map(|(i, d)| {
                let (icon, color) = match d.severity {
                    Severity::Error => (Glyph::Error, theme.danger),
                    Severity::Warning => (Glyph::Warning, theme.warning),
                    Severity::Info => (Glyph::Info, theme.muted_foreground),
                };
                let location = match (&d.file, d.line) {
                    (Some(f), Some(l)) => {
                        let rel = root_dir
                            .as_ref()
                            .and_then(|r| f.strip_prefix(r).ok())
                            .unwrap_or(f);
                        format!("{}:{l}", rel.display())
                    }
                    (Some(f), None) => f
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    _ => String::new(),
                };
                let target = d.file.clone().zip(d.line);
                h_flex()
                    .id(("diag", i))
                    .gap_2()
                    .px_3()
                    .py_1()
                    .text_sm()
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.accent))
                    .child(Icon::new(icon).xsmall().text_color(color))
                    .child(
                        div()
                            .flex_1()
                            .truncate()
                            .text_color(theme.foreground)
                            .child(d.message.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(location),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if let Some((file, line)) = target.clone() {
                            this.reveal(file, line.saturating_sub(1), window, cx);
                        }
                    }))
            })
            .collect::<Vec<_>>();
        let empty = rows.is_empty();

        v_flex()
            .h(px(170.))
            .border_t_1()
            .border_color(theme.border)
            .bg(theme.sidebar)
            .child(
                h_flex()
                    .px_3()
                    .py_1p5()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.muted_foreground)
                    .child(div().flex_1().child("PROBLEMS"))
                    .child(
                        Button::new("close-diag")
                            .ghost()
                            .xsmall()
                            .icon(Glyph::Close)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.show_diagnostics = false;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                v_flex()
                    .id("diag-list")
                    .flex_1()
                    .overflow_y_scroll()
                    .when(empty, |d| {
                        d.child(
                            div()
                                .px_3()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child("No problems — nicely done."),
                        )
                    })
                    .children(rows),
            )
    }

    fn render_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let language =
            (self.settings.spell_check && !self.diff_active).then(|| self.render_language(cx));
        let theme = cx.theme();
        let cursor = self
            .buffers
            .get(self.active)
            .map(|b| b.editor.read(cx).cursor_position());
        let errors = self
            .diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .count();
        let warnings = self
            .diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Warning)
            .count();
        let engine: SharedString = self
            .engines
            .get(self.engine_ix)
            .map(|e| e.label())
            .unwrap_or_else(|| "No engine".into())
            .into();
        let live = self.settings.live_preview;
        let git = self.git.read(cx);
        let branch: Option<SharedString> = git.repo().map(|_| {
            let st = git.status();
            let mut label = st
                .branch
                .clone()
                .or_else(|| st.oid.as_ref().map(|o| o.chars().take(7).collect()))
                .unwrap_or_else(|| "No branch".into());
            if !st.is_clean() {
                label.push('*');
            }
            if st.ahead > 0 {
                label.push_str(&format!(" ↑{}", st.ahead));
            }
            if st.behind > 0 {
                label.push_str(&format!(" ↓{}", st.behind));
            }
            label.into()
        });

        h_flex()
            .h(px(26.))
            .px_3()
            .gap_4()
            .text_xs()
            .text_color(theme.muted_foreground)
            .bg(theme.status_bar)
            .border_t_1()
            .border_color(theme.status_bar_border)
            .child(
                h_flex()
                    .id("problems")
                    .gap_3()
                    .cursor_pointer()
                    .hover(|s| s.text_color(theme.foreground))
                    .child(
                        h_flex()
                            .gap_1()
                            .child(Icon::new(Glyph::Error).xsmall())
                            .child(errors.to_string()),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .child(Icon::new(Glyph::Warning).xsmall())
                            .child(warnings.to_string()),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_diagnostics = !this.show_diagnostics;
                        cx.notify();
                    })),
            )
            .when_some(cursor, |el, c| {
                el.child(format!("Ln {}, Col {}", c.line + 1, c.character + 1))
            })
            .when_some(branch, |el, branch| {
                el.child(
                    h_flex()
                        .id("git-status")
                        .gap_1()
                        .cursor_pointer()
                        .hover(|s| s.text_color(theme.foreground))
                        .child(Icon::new(Glyph::Branch).xsmall())
                        .child(branch)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.show_sidebar(Sidebar::SourceControl, cx)
                        })),
                )
            })
            .child(div().flex_1())
            .children(language)
            .child(
                div()
                    .id("live")
                    .cursor_pointer()
                    .hover(|s| s.text_color(theme.foreground))
                    .when(live, |d| d.text_color(theme.primary))
                    .child(if live { "● Live" } else { "○ Live" })
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_live(cx))),
            )
            .child(
                div()
                    .id("engine")
                    .cursor_pointer()
                    .hover(|s| s.text_color(theme.foreground))
                    .child(engine)
                    .on_click(cx.listener(|this, _, _, cx| this.next_engine(cx))),
            )
    }

    /// The active file's language, with the menu to change it.
    fn render_language(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let b = &self.buffers[self.active];
        let chosen = self.settings.spell_language(&b.path).map(str::to_string);
        let language = b.spell.language;
        let dictionary = language.and_then(|d| self.spell.dictionaries.get(d.code));
        let (icon, tooltip): (Glyph, SharedString) = match (language, dictionary) {
            (None, _) if chosen.as_deref() == Some(SpellLanguage::OFF) => (
                Glyph::Languages,
                "Spelling is not checked in this file".into(),
            ),
            (None, _) => (Glyph::Spinner, "Checking spelling…".into()),
            (Some(d), Some(Dictionary::Loading)) => (
                Glyph::Spinner,
                format!("Getting the {} dictionary…", catalog::name(d.code)).into(),
            ),
            (Some(_), Some(Dictionary::Failed(e))) => (
                Glyph::Warning,
                format!("The dictionary could not be loaded: {e}").into(),
            ),
            (Some(d), _) => {
                let why = match d.source {
                    Source::User => "chosen for this file",
                    Source::Magic => "set by % !TeX spellcheck",
                    Source::Package => "set with babel/polyglossia",
                    Source::Text => "recognized from the text",
                    Source::Fallback => "the system language",
                };
                (
                    Glyph::Languages,
                    format!("Spelling: {}, {why}", catalog::name(d.code)).into(),
                )
            }
        };
        let label = match (&chosen, language) {
            (Some(c), _) if c == SpellLanguage::OFF => "Spelling Off".into(),
            (_, Some(d)) => SharedString::from(catalog::name(d.code).to_string()),
            _ => "Spelling".into(),
        };
        let failed = match dictionary {
            Some(Dictionary::Failed(_)) => language.map(|d| d.code),
            _ => None,
        };
        let current = language.map(|d| d.code);
        let weak = cx.entity().downgrade();
        Button::new("spell-language")
            .ghost()
            .xsmall()
            .icon(icon)
            .label(label)
            .tooltip(tooltip)
            .dropdown_menu_with_anchor(Anchor::BottomRight, move |menu, window, _| {
                let detected = chosen.is_none();
                let off = chosen.as_deref() == Some(SpellLanguage::OFF);
                let pick = |language: Option<String>| {
                    let weak = weak.clone();
                    move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                        let language = language.clone();
                        weak.update(cx, |this, cx| this.set_spell_language(language, window, cx))
                            .ok();
                    }
                };
                let mut menu = menu
                    .scrollable(true)
                    .max_h(window.viewport_size().height * 0.7)
                    .item(
                        PopupMenuItem::new("Detect Automatically")
                            .checked(detected)
                            .on_click(pick(None)),
                    )
                    .item(
                        PopupMenuItem::new("Don't Check This File")
                            .checked(off)
                            .on_click(pick(Some(SpellLanguage::OFF.to_string()))),
                    );
                if let Some(code) = failed {
                    let weak = weak.clone();
                    menu = menu.item(PopupMenuItem::new("Retry Loading the Dictionary").on_click(
                        move |_, _, cx| {
                            weak.update(cx, |this, cx| this.retry_dictionary(code, cx))
                                .ok();
                        },
                    ));
                }
                menu = menu.separator();
                for l in catalog::LANGUAGES {
                    menu = menu.item(
                        PopupMenuItem::new(l.name)
                            .checked(!off && current == Some(l.code))
                            .on_click(pick(Some(l.code.to_string()))),
                    );
                }
                menu
            })
    }

    fn toggle_live(&mut self, cx: &mut Context<Self>) {
        self.settings.live_preview = !self.settings.live_preview;
        self.settings.save();
        cx.notify();
    }

    fn render_welcome(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let recent =
            self.settings
                .recent
                .iter()
                .take(6)
                .enumerate()
                .map(|(i, p)| {
                    let p = p.clone();
                    let name = p
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let dir = p
                        .parent()
                        .map(|d| d.display().to_string())
                        .unwrap_or_default();
                    h_flex()
                        .id(("recent", i))
                        .w(px(420.))
                        .gap_3()
                        .px_3()
                        .py_2()
                        .rounded(theme.radius)
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.accent))
                        .child(
                            Icon::new(Glyph::File)
                                .small()
                                .text_color(theme.muted_foreground),
                        )
                        .child(
                            v_flex().child(div().text_sm().child(name)).child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .truncate()
                                    .child(dir),
                            ),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_path(p.clone(), window, cx)
                        }))
                });
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_6()
            .bg(theme.background)
            .child(
                v_flex()
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .text_3xl()
                            .font_weight(FontWeight::LIGHT)
                            .text_color(theme.foreground)
                            .child("TeXoria"),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child("Beautiful documents, beautifully written."),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("open")
                            .primary()
                            .icon(Glyph::Open)
                            .label("Open a document…")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.prompt_open(window, cx)),
                            ),
                    )
                    .child(
                        Button::new("clone")
                            .outline()
                            .icon(Glyph::Branch)
                            .label("Clone a repository…")
                            .disabled(self.cloning.is_some())
                            .on_click(
                                cx.listener(|this, _, window, cx| this.prompt_clone(window, cx)),
                            ),
                    )
                    .child(
                        Button::new("overleaf")
                            .outline()
                            .icon(Glyph::Branch)
                            .label("Open from Overleaf…")
                            .disabled(self.cloning.is_some())
                            .on_click(
                                cx.listener(|this, _, window, cx| this.prompt_overleaf(window, cx)),
                            ),
                    ),
            )
            .when_some(self.cloning.clone(), |d, (url, cancel)| {
                d.child(
                    h_flex()
                        .gap_2()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(Icon::new(Glyph::Spinner).xsmall().with_animation(
                            "clone-spin",
                            Animation::new(Duration::from_millis(900)).repeat(),
                            |icon, t| icon.transform(Transformation::rotate(percentage(t))),
                        ))
                        .child(
                            div()
                                .max_w(px(360.))
                                .truncate()
                                .child(format!("Cloning {url}…")),
                        )
                        .child(
                            Button::new("clone-cancel")
                                .ghost()
                                .xsmall()
                                .label("Cancel")
                                .on_click(move |_, _, _| cancel.cancel()),
                        ),
                )
            })
            .child(v_flex().gap_0p5().children(recent))
    }
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_theme(cx);
        let theme = cx.theme().clone();

        let body = if self.buffers.is_empty() {
            self.render_welcome(cx).into_any_element()
        } else {
            let center = match self.diff.clone().filter(|_| self.diff_active) {
                Some(diff) => diff.into_any_element(),
                None => Editor::new(&self.buffers[self.active].editor)
                    .context_menu({
                        let b = &self.buffers[self.active];
                        spell::context_menu(
                            b.editor.clone(),
                            b.spell.underlines.clone(),
                            b.spell.speller.clone(),
                        )
                    })
                    .bordered(false)
                    .h(relative(1.))
                    .font_family(theme.mono_font_family.clone())
                    .text_size(px(14.5))
                    .line_height(relative(1.6))
                    .into_any_element(),
            };
            let editor_column = v_flex()
                .size_full()
                .bg(theme.background)
                .when(self.untrusted_rc, |d| d.child(self.render_trust_banner(cx)))
                .when(self.buffers.len() > 1 || self.diff.is_some(), |d| {
                    d.child(self.render_tabs(cx))
                })
                .child(div().flex_1().min_h_0().child(center))
                .when(self.show_diagnostics, |d| {
                    d.child(self.render_diagnostics(cx))
                });
            let active = (!self.diff_active).then(|| self.buffers[self.active].path.clone());
            self.files.update(cx, |f, cx| f.set_active(active, cx));
            let sidebar = match self.sidebar {
                Sidebar::Outline | Sidebar::Files => self.render_navigator(cx).into_any_element(),
                Sidebar::SourceControl => self.git.clone().into_any_element(),
            };

            h_resizable("main")
                .child(
                    resizable_panel()
                        .size(px(260.))
                        .size_range(px(160.)..px(460.))
                        .visible(self.show_sidebar)
                        .child(sidebar),
                )
                .child(resizable_panel().child(editor_column))
                .child(
                    resizable_panel().size(px(620.)).child(
                        div()
                            .size_full()
                            .border_l_1()
                            .border_color(theme.border)
                            .child(self.pdf.clone()),
                    ),
                )
                .into_any_element()
        };

        v_flex()
            .id("workspace")
            .key_context("Workspace")
            .track_focus(&self.focus)
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .font_family(theme.font_family.clone())
            .on_action(cx.listener(|this, _: &Open, window, cx| this.prompt_open(window, cx)))
            .on_action(cx.listener(|this, _: &Save, _, cx| {
                this.save_all(cx);
            }))
            .on_action(cx.listener(|this, _: &Typeset, _, cx| this.typeset(cx)))
            .on_action(cx.listener(|this, _: &CancelTypeset, _, _| this.cancel_typeset()))
            .on_action(cx.listener(|this, _: &ForwardSearch, _, cx| this.forward_search(cx)))
            .on_action(
                cx.listener(|this, _: &ToggleTheme, window, cx| this.toggle_theme(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &OpenSettings, window, cx| this.open_settings(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &ZoomIn, _, cx| this.pdf.update(cx, |p, cx| p.zoom_in(cx))),
            )
            .on_action(
                cx.listener(|this, _: &ZoomOut, _, cx| this.pdf.update(cx, |p, cx| p.zoom_out(cx))),
            )
            .on_action(cx.listener(|this, _: &ZoomToFit, _, cx| {
                this.pdf.update(cx, |p, cx| p.zoom_to_fit(cx))
            }))
            .on_action(cx.listener(|this, _: &ToggleOutline, _, cx| {
                this.toggle_sidebar(Sidebar::Outline, cx)
            }))
            .on_action(
                cx.listener(|this, _: &ToggleFiles, _, cx| this.toggle_sidebar(Sidebar::Files, cx)),
            )
            .on_action(cx.listener(|this, _: &ShowSourceControl, window, cx| {
                this.show_sidebar(Sidebar::SourceControl, cx);
                this.git.update(cx, |g, cx| g.focus_message(window, cx));
            }))
            .on_action(cx.listener(|this, _: &Commit, window, cx| {
                this.show_sidebar(Sidebar::SourceControl, cx);
                this.git.update(cx, |g, cx| g.commit(window, cx));
            }))
            .on_action(
                cx.listener(|this, _: &Fetch, _, cx| this.git.update(cx, |g, cx| g.fetch(cx))),
            )
            .on_action(cx.listener(|this, _: &Pull, _, cx| this.git.update(cx, |g, cx| g.pull(cx))))
            .on_action(cx.listener(|this, _: &Push, window, cx| {
                this.git.update(cx, |g, cx| g.push(window, cx))
            }))
            .on_action(cx.listener(|this, _: &SyncOverleaf, window, cx| {
                this.git.update(cx, |g, cx| g.sync(window, cx))
            }))
            .on_action(cx.listener(|this, _: &NewBranch, window, cx| {
                this.git.update(cx, |g, cx| g.prompt_new_branch(window, cx))
            }))
            .on_action(
                cx.listener(|this, _: &CloneRepository, window, cx| this.prompt_clone(window, cx)),
            )
            .on_action(cx.listener(|this, _: &OpenFromOverleaf, window, cx| {
                this.prompt_overleaf(window, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleDiagnostics, _, cx| {
                this.show_diagnostics = !this.show_diagnostics;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ToggleLivePreview, _, cx| this.toggle_live(cx)))
            .on_action(cx.listener(|this, _: &ToggleSoftWrap, window, cx| {
                if let Some(b) = this.buffers.get_mut(this.active) {
                    b.soft_wrap = !b.soft_wrap;
                    let wrap = b.soft_wrap;
                    b.editor
                        .update(cx, |state, cx| state.set_soft_wrap(wrap, window, cx));
                }
            }))
            .on_action(cx.listener(|this, _: &CloseTab, _, cx| this.close_tab(cx)))
            .on_action(cx.listener(|this, a: &ReplaceMisspelling, window, cx| {
                this.replace_misspelling(a, window, cx)
            }))
            .on_action(cx.listener(|this, a: &AddToDictionary, window, cx| {
                this.update_settings(|s| s.add_spell_word(&a.word), window, cx)
            }))
            .child(self.render_title_bar(cx))
            .child(div().flex_1().min_h_0().child(body))
            .when(!self.buffers.is_empty(), |d| {
                d.child(self.render_status_bar(cx))
            })
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// An empty file, failing if `path` exists (and creating its directories).
fn create_file(path: &Path) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map(drop)
}

/// `rename`, refusing to replace another file (it replaces silently on Unix).
fn rename(from: &Path, to: &Path) -> std::io::Result<()> {
    if to.symlink_metadata().is_ok() && !files::is_same_file(from, to) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("{} already exists", to.display()),
        ));
    }
    std::fs::rename(from, to)
}

/// Delete a file, or a directory with its contents (a link, not its target).
fn remove(path: &Path) -> std::io::Result<()> {
    if path.symlink_metadata()?.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

/// In a directory, the `.tex` file that has `\documentclass` (preferring
/// `main.tex`).
fn find_main_file(dir: &Path) -> Option<PathBuf> {
    let main = dir.join("main.tex");
    if main.is_file() {
        return Some(main);
    }
    let mut candidates: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "tex"))
        .filter(|p| tex_project::read_regular(p).is_some_and(|s| s.contains("\\documentclass")))
        .collect();
    candidates.sort();
    candidates.into_iter().next()
}

/// Where to look for the PDFium shared library: next to the executable,
/// inside a macOS bundle, and the repository's `vendor/` for development.
fn pdfium_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(exe) = std::env::current_exe()
        && let Some(d) = exe.parent()
    {
        dirs.push(d.to_path_buf());
        dirs.push(d.join("../Frameworks"));
        dirs.push(d.join("../Resources"));
        dirs.push(d.join("lib"));
        // deb / AppImage layout
        dirs.push(d.join("../lib/texoria"));
    }
    dirs.extend(dev_vendor_dir("pdfium"));
    dirs
}

/// `vendor/<name>` of the source checkout, but only for a binary built in it
/// (`cargo run`). The path is the build machine's: on a user's machine it may
/// be creatable by anyone, and a library or engine planted there would run.
fn dev_vendor_dir(name: &str) -> Option<PathBuf> {
    let repo = std::fs::canonicalize(concat!(env!("CARGO_MANIFEST_DIR"), "/../..")).ok()?;
    let exe = std::fs::canonicalize(std::env::current_exe().ok()?).ok()?;
    exe.starts_with(repo.join("target"))
        .then(|| repo.join("vendor").join(name))
}
