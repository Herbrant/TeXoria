//! The main window: outline or files | editor | PDF, with live preview.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, RwLock, mpsc};
use std::time::Duration;

use gpui_kit::component::highlighter::{Diagnostic as EditorDiagnostic, DiagnosticSeverity};
use gpui_kit::component::input::{
    Editor, EditorState, Input, InputEvent, InputState, Position, RopeExt, TabSize,
};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::native_menu::NativeMenu;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::resizable::{h_resizable, resizable_panel};
use gpui_kit::component::{
    ActiveTheme, Disableable as _, Icon, Sizable, TitleBar, WindowExt as _,
    button::{Button, ButtonVariants},
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use tex_agent::mcp::{self, ToolResult};
use tex_agent::{acp, prompt::Rewrite};
use tex_compile::{Compiler, Engine, Event as CompileEvent, Job, Severity, Status};
use tex_git::{Cancel, Credential, Git};
use tex_log::Diagnostic;
use tex_project::{Project, ProjectOutlineItem, files};
use tex_spell::{Source, Speller, catalog};
use tex_syntax::OutlineKind;

use crate::agent_panel::{
    AgentOp, AgentPanel, AgentPanelEvent, Prompt, RewriteSelection, text_block,
};
use crate::agent_tools::{AppTools, ToolCall};
use crate::assets::Glyph;
use crate::atomic;
use crate::checks::{self, BufferChecks};
use crate::diff_view::{DiffSource, DiffView, DiffViewEvent};
use crate::edit;
use crate::file_tree::{FileOp, FileTree, FileTreeEvent};
use crate::git_panel::{self, GitOp, GitPanel, GitPanelEvent};
use crate::grammar::{self, BufferGrammar, Grammar, IgnoreGrammarRule, ServerState};
use crate::latex::{
    self, CursorFixup, LatexCompletions, LatexNavigation, OpenExternal, OpenLocation, PackageDocs,
};
use crate::outline::{self, Fold};
use crate::overleaf;
use crate::pdf_pane::{PdfPane, PdfPaneEvent};
use crate::presentation::Presentation;
use crate::search_panel::{SearchPanel, SearchPanelEvent};
use crate::settings::{Session, Settings, SpellLanguage, ThemeChoice};
use crate::settings_window::SettingsWindow;
use crate::spell::{self, AddToDictionary, BufferSpell, Dictionary, ReplaceMisspelling, Spell};
use crate::theme::{self, ThemeExt};
use crate::updater::{self, UpdateState, Updater, UpdaterEvent};
use crate::{
    AddCitation, Build, CancelBuild, CheckForUpdates, CloneRepository, CloseTab, Commit,
    DecreaseFontSize, EditWithAgent, ExportPdf, Fetch, FindInProject, FixWithAgent, FormatDocument,
    ForwardSearch, IncreaseFontSize, NewBranch, Open, OpenFolder, OpenFromOverleaf, OpenSettings,
    Present, Pull, Push, ResetFontSize, Save, ShowSourceControl, StopAgent, SyncOverleaf,
    ToggleAgent, ToggleComment, ToggleDiagnostics, ToggleFiles, ToggleLivePreview, ToggleOutline,
    ToggleSoftWrap, ToggleTheme, ToggleVimMode, ZoomIn, ZoomOut, ZoomToFit,
};

const SOFT_WRAP_MAX_LINES: usize = 4000;

/// Whether the system's open dialog can pick a file or a folder at once. Only macOS's can: the
/// XDG portal (Linux) and Windows' dialog asked for folders show folders only, and the portal
/// hands back the current folder when a file is chosen anyway.
const OPEN_PICKS_FOLDERS: bool = cfg!(target_os = "macos");

/// The open dialog of *Open…* (`folder: false`) or *Open Folder…*.
fn open_prompt(folder: bool) -> PathPromptOptions {
    PathPromptOptions {
        files: !folder,
        directories: folder || OPEN_PICKS_FOLDERS,
        multiple: false,
        prompt: Some("Open".into()),
    }
}

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
    grammar: BufferGrammar,
    checks: BufferChecks,
    block: crate::vim::Block,
    _subscription: Subscription,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sidebar {
    Outline,
    Files,
    Search,
    SourceControl,
    Agent,
}

/// An agent's `build` call, answered by the first run that starts after it.
struct ToolWaiter {
    reply: mpsc::Sender<ToolResult>,
    after: u64,
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
    /// The live checks' findings, for the whole project.
    issues: Vec<tex_project::check::Issue>,
    /// The words of the project's files, in document order.
    word_counts: Vec<(PathBuf, tex_spell::extract::Counts)>,
    /// Counts of files by the hash of the text they were counted in.
    count_cache: Arc<
        std::sync::Mutex<std::collections::HashMap<PathBuf, (u64, tex_spell::extract::Counts)>>,
    >,
    outline: Vec<ProjectOutlineItem>,
    /// Which of the outline's groups are open.
    outline_fold: Fold,
    show_sidebar: bool,
    sidebar: Sidebar,
    /// The last of Outline and Files: what the sidebar button shows.
    navigator: Sidebar,
    files: Entity<FileTree>,
    search: Entity<SearchPanel>,
    git: Entity<GitPanel>,
    agent: Entity<AgentPanel>,
    /// An agent is working: the editors are read-only meanwhile.
    agent_busy: bool,
    /// Serves TeXoria's tools to the agents.
    _mcp: Option<mcp::Server>,
    tool_waiters: Vec<ToolWaiter>,
    /// The id of the last build started.
    last_run: u64,
    /// The diff tab, shown instead of the editor while `diff_active`.
    diff: Option<Entity<DiffView>>,
    diff_active: bool,
    /// A clone in progress (URL), started from the welcome screen.
    cloning: Option<(SharedString, Cancel)>,
    show_diagnostics: bool,
    /// The project has a `latexmkrc` that is ignored until the user trusts it.
    untrusted_rc: bool,
    /// The last run wanted shell escape, which the project does not allow.
    wants_shell_escape: bool,
    /// A PDF already in the build folder may be shown: TeXoria built it, or
    /// the project was opened before. One that came with the project (a
    /// clone, an archive) could be crafted for the PDF renderer.
    show_old_pdf: bool,
    /// A close was already refused once because saving failed.
    refused_close: bool,
    fixups: Rc<RefCell<Vec<CursorFixup>>>,
    focus: FocusHandle,
    was_dark: bool,
    settings_window: Option<AnyWindowHandle>,
    spell: Spell,
    grammar: Grammar,
    _grammar_start: Task<()>,
    /// Vim's state, when Vim mode is on.
    vim: Option<tex_vim::Vim>,
    updater: Entity<Updater>,
    _live: Task<()>,
    _checks: Task<()>,
    _count: Task<()>,
    _update_checks: Task<()>,
    _compile_events: Task<()>,
    _tool_calls: Task<()>,
    _diff_subscription: Option<Subscription>,
    _subscriptions: Vec<Subscription>,
}

/// What a workspace starts from: the user's settings, the TeX engines found and the folders
/// that may hold PDFium. Tests build their own instead of reading the machine's.
pub struct Startup {
    pub settings: Settings,
    pub engines: Vec<tex_compile::Engine>,
    pub pdfium_dirs: Vec<PathBuf>,
    /// Serve TeXoria's tools to coding agents (a loopback port and its own threads).
    pub tool_server: bool,
}

impl Startup {
    pub fn detect() -> Self {
        Startup {
            settings: Settings::load(),
            engines: tex_compile::detect(dev_vendor_dir("tectonic").as_deref()),
            pdfium_dirs: pdfium_dirs(),
            tool_server: true,
        }
    }
}

impl Workspace {
    pub fn new(path: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::with_startup(path, Startup::detect(), window, cx)
    }

    pub fn with_startup(
        path: Option<PathBuf>,
        startup: Startup,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let Startup {
            mut settings,
            engines,
            pdfium_dirs,
            tool_server,
        } = startup;
        if let Some(error) = settings.load_error.take() {
            window.defer(cx, move |window, cx| {
                window.push_notification(
                    Notification::warning(error).title("Settings could not be read in full"),
                    cx,
                )
            });
        }
        let pdf = cx.new(|cx| PdfPane::new(pdfium_dirs, cx));
        let files = cx.new(FileTree::new);
        let search = cx.new(|cx| SearchPanel::new(window, cx));
        let git = cx.new(|cx| GitPanel::new(window, cx));
        let agent = cx.new(|cx| AgentPanel::new(window, cx));
        let updater = cx.new(Updater::new);
        theme::apply(settings.theme, Some(window), cx);
        let mut subscriptions = vec![
            cx.subscribe_in(
                &pdf,
                window,
                |this, _, ev: &PdfPaneEvent, window, cx| match ev {
                    PdfPaneEvent::InverseSearch { file, line } => {
                        this.reveal(file.clone(), line.saturating_sub(1), window, cx)
                    }
                    PdfPaneEvent::Export => this.export_pdf(window, cx),
                    PdfPaneEvent::Present => this.present(cx),
                },
            ),
            cx.subscribe_in(&git, window, Self::on_git_event),
            cx.subscribe_in(&files, window, Self::on_file_tree_event),
            cx.subscribe_in(&search, window, Self::on_search_event),
            cx.subscribe_in(&agent, window, Self::on_agent_event),
            cx.subscribe_in(&updater, window, Self::on_updater_event),
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
        // Vim mode takes the keys typed in the active editor first.
        let vim_window = window.window_handle();
        let weak_vim = cx.entity().downgrade();
        subscriptions.push(cx.intercept_keystrokes(move |ev, window, cx| {
            if window.window_handle() != vim_window {
                return;
            }
            weak_vim
                .update(cx, |this, cx| this.on_vim_keystroke(ev, window, cx))
                .ok();
        }));
        // Never lose edits made since the last (debounced) save.
        subscriptions.push(cx.on_app_quit(|this, cx| {
            this.save_all(cx);
            this.grammar.stop();
            async {}
        }));
        let weak = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            let close = weak
                .update(cx, |this, cx| {
                    // If saving fails, keep the window (and the error) up once.
                    let close = this.save_all(cx) || this.refused_close;
                    this.refused_close = !close;
                    close
                })
                .unwrap_or(true);
            if close {
                // The Settings and presentation windows belong to this one. Left open,
                // they would keep the app running elsewhere than on macOS, and there
                // keep its Dock icon from opening a new window.
                let main = window.window_handle();
                for other in cx.windows().into_iter().filter(|w| *w != main) {
                    other.update(cx, |_, w, _| w.remove_window()).ok();
                }
            }
            close
        });

        let engine_ix = settings
            .engine
            .as_ref()
            .and_then(|label| engines.iter().position(|e| &e.label() == label))
            .unwrap_or(0);

        // The agents reach TeXoria's tools through `texoria --mcp-bridge`.
        let (tools, tool_calls) = AppTools::new();
        let server = tool_server
            .then(|| {
                mcp::Server::start(Arc::new(tools))
                    .inspect_err(|e| log::warn!("starting the tool server for agents: {e}"))
                    .ok()
            })
            .flatten();
        let bridge = std::env::current_exe().ok();
        let configs = server
            .iter()
            .zip(bridge)
            .map(|(s, exe)| s.stdio_config(&exe))
            .collect();
        let (preferred, custom) = (settings.agent.clone(), settings.custom_agents.clone());
        agent.update(cx, |a, cx| {
            a.set_mcp(configs);
            a.configure(preferred, custom, cx);
        });
        let tool_task = cx.spawn(async move |this, cx| {
            while let Ok(call) = tool_calls.recv().await {
                if this
                    .update(cx, |this, cx| this.on_tool_call(call, cx))
                    .is_err()
                {
                    break;
                }
            }
        });

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
            issues: Vec::new(),
            word_counts: Vec::new(),
            count_cache: Arc::default(),
            outline: Vec::new(),
            outline_fold: Fold::default(),
            show_sidebar: true,
            sidebar: Sidebar::Outline,
            navigator: Sidebar::Outline,
            files,
            search,
            git,
            agent,
            agent_busy: false,
            _mcp: server,
            tool_waiters: Vec::new(),
            last_run: 0,
            diff: None,
            diff_active: false,
            cloning: None,
            show_diagnostics: false,
            untrusted_rc: false,
            wants_shell_escape: false,
            show_old_pdf: false,
            refused_close: false,
            fixups: Rc::new(RefCell::new(Vec::new())),
            focus: cx.focus_handle(),
            was_dark: cx.theme_is_dark(),
            settings_window: None,
            spell,
            grammar: Grammar::default(),
            _grammar_start: Task::ready(()),
            vim: None,
            updater,
            _live: Task::ready(()),
            _checks: Task::ready(()),
            _count: Task::ready(()),
            _update_checks: Task::ready(()),
            _compile_events: Task::ready(()),
            _tool_calls: tool_task,
            _diff_subscription: None,
            _subscriptions: subscriptions,
        };
        this.configure_git(cx);
        this.start_compiler(cx);
        this.start_grammar(Duration::ZERO, cx);
        this.vim = this.settings.vim_mode.then(tex_vim::Vim::new);
        this.schedule_update_checks(cx);
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

    pub(crate) fn agent_panel(&self) -> &Entity<AgentPanel> {
        &self.agent
    }

    pub(crate) fn updater(&self) -> &Entity<Updater> {
        &self.updater
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
        if self.settings.shell_escape_allowed != old.shell_escape_allowed
            && let Some(job) = self.job()
            && job.shell_escape != old.allows_shell_escape(job.work_dir())
        {
            self.build(cx);
        }
        if self.settings.rc_allowed != old.rc_allowed
            && let Some(job) = self.job()
            && job.allow_project_rc != old.allows_rc(job.work_dir())
        {
            self.untrusted_rc = !job.allow_project_rc && job.has_project_rc();
            self.build(cx);
        }
        let s = &self.settings;
        if (&s.agent, &s.custom_agents) != (&old.agent, &old.custom_agents) {
            let (preferred, custom) = (s.agent.clone(), s.custom_agents.clone());
            self.agent
                .update(cx, |a, cx| a.configure(preferred, custom, cx));
        }
        if self.settings.agent_projects != old.agent_projects {
            self.sync_agent(cx);
        }
        let s = &self.settings;
        if (s.grammar_check, &s.languagetool_url, s.languagetool_start)
            != (
                old.grammar_check,
                &old.languagetool_url,
                old.languagetool_start,
            )
        {
            // The URL changes as it is typed.
            self.start_grammar(Duration::from_secs(1), cx);
        } else if s.grammar_rules_off != old.grammar_rules_off {
            for ix in 0..self.buffers.len() {
                self.grammar_check(ix, Duration::ZERO, cx);
            }
        }
        if self.settings.vim_mode != old.vim_mode {
            let on = self.settings.vim_mode;
            self.set_vim_mode(on, cx);
        }
        if self.settings.live_checks != old.live_checks {
            self.reindex(cx);
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

    /// Resize the editor's text (⌘= / ⌘- / ⌘0), within the allowed range.
    fn set_font_size(&mut self, size: f32, window: &mut Window, cx: &mut Context<Self>) {
        let size = size.clamp(Settings::MIN_FONT_SIZE, Settings::MAX_FONT_SIZE);
        if size != self.settings.font_size() {
            self.update_settings(|s| s.editor_font_size = size, window, cx);
        }
    }

    /// Open the Settings window, or bring it to the front.
    fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Its Agents page lists them.
        self.agent.update(cx, |a, cx| a.activate(cx));
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
        let path = crate::paths::canonical(&path);
        let file = if path.is_dir() {
            find_main_file(&path, &self.settings.recent)
        } else {
            Some(path.clone())
        };
        let Some(file) = file else {
            self.state = CompileState::Failed(
                format!(
                    "No LaTeX document (a .tex file with \\documentclass) in {}",
                    path.display()
                )
                .into(),
            );
            cx.notify();
            return;
        };
        let text = tex_project::read_regular(&file).unwrap_or_default();
        let root = tex_project::find_root(&file, &text);
        let root = crate::paths::canonical(&root);
        let new_project = self.root.as_ref() != Some(&root);
        let session = new_project
            .then(|| self.settings.session(&root).cloned())
            .flatten();
        self.set_root(root.clone(), cx);
        match session {
            Some(session) => self.restore_session(session, file, window, cx),
            None => {
                self.activate_file(file, window, cx);
            }
        }
    }

    /// Reopen a project's tabs, then show `file`, or the tab that was
    /// active if `file` is the root (the project was opened, not a file).
    fn restore_session(
        &mut self,
        session: Session,
        file: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let dir = self.project_dir();
        for f in &session.files {
            let inside = dir.as_ref().is_some_and(|d| f.starts_with(d));
            if inside && f.is_file() && files::looks_like_text(f) {
                self.activate_file(f.clone(), window, cx);
            }
        }
        let active = session
            .active
            .filter(|a| {
                Some(&file) == self.root.as_ref() && self.buffers.iter().any(|b| &b.path == a)
            })
            .unwrap_or(file);
        self.activate_file(active, window, cx);
    }

    /// Remember the project's open tabs (saved with the settings).
    fn remember_session(&mut self) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let dir = self.project_dir();
        let inside = |p: &Path| dir.as_ref().is_some_and(|d| p.starts_with(d));
        let previous = self.settings.session(&root).cloned().unwrap_or_default();
        let session = Session {
            root,
            files: self
                .buffers
                .iter()
                .map(|b| b.path.clone())
                .filter(|p| inside(p))
                .collect(),
            active: self
                .buffers
                .get(self.active)
                .map(|b| b.path.clone())
                .filter(|p| inside(p)),
            export: previous.export,
        };
        if self.settings.set_session(session) {
            self.settings.save();
        }
    }

    /// Make `root` the project's root file (a no-op if it already is).
    fn set_root(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        if self.root.as_deref() != Some(&root) {
            self.root = Some(root.clone());
            *self.project.write().unwrap() = Project::open(&root);
            self.outline_fold = Fold::default();
            self.refresh_outline(cx);
            self.issues.clear();
            self.word_counts.clear();
            self.count_words(&[], cx);
            self.run_checks(Vec::new(), cx);
            self.show_old_pdf = self.settings.recent.contains(&root);
            self.settings.push_recent(root.clone());
            self.settings.save();
            // The previous project's run is irrelevant now.
            if let Some(c) = &self.compiler {
                c.cancel();
            }
            self.answer_waiters(ToolResult::error("Another document was opened."));
            let job = self.job().expect("root is set");
            self.untrusted_rc = !job.allow_project_rc && job.has_project_rc();
            self.wants_shell_escape = false;
            self.close_diff(cx);
            self.sync_git(cx);
            self.sync_agent(cx);
            self.files
                .update(cx, |f, cx| f.set_project(Some(root.clone()), cx));
            let dir = self.project_dir();
            self.search.update(cx, |s, cx| s.set_dir(dir, cx));
            // Show a previous build right away, if any.
            if self.show_old_pdf && job.artifact("pdf").is_file() {
                let synctex = Some(job.artifact("synctex.gz")).filter(|p| p.is_file());
                self.pdf
                    .update(cx, |pdf, cx| pdf.load(job.artifact("pdf"), synctex, cx));
            }
            self.build(cx);
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
            if ix != self.active
                && let Some(v) = &mut self.vim
            {
                v.reset();
            }
            self.active = ix;
            self.remember_session();
            cx.notify();
            return Some(ix);
        }
        // Only regular files: inverse search and diagnostics can point
        // anywhere, and reading a pipe or device would hang.
        // Nor huge ones, which would freeze the editor.
        let read = std::fs::metadata(&file).and_then(|m| {
            if !m.is_file() {
                Err(std::io::Error::other("not a regular file"))
            } else if m.len() > files::MAX_TEXT_BYTES {
                Err(std::io::Error::other(format!(
                    "it is too large to edit ({} MB)",
                    m.len() >> 20
                )))
            } else {
                std::fs::read_to_string(&file)
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
        let checks = BufferChecks::new(&editor, cx);
        let block = crate::vim::Block::new(&editor, cx);
        let grammar = BufferGrammar::new(&editor, cx);
        let mut navigation = LatexNavigation::new(self.project.clone());
        navigation.notes.push(checks.hover_note(&editor));
        navigation.notes.push(grammar.hover_note(&editor));
        let navigation = Rc::new(navigation);
        let busy = self.agent_busy;
        editor.update(cx, |state, cx| {
            state.set_readonly(busy, cx);
            let lsp = state.lsp_mut();
            lsp.completion_provider = Some(provider);
            lsp.show_document = Some(navigation.show_document());
            lsp.definition_provider = Some(navigation.clone());
            lsp.hover_provider = Some(navigation);
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
            grammar,
            checks,
            block,
            _subscription: subscription,
        });
        self.active = self.buffers.len() - 1;
        // The project indexed this file from disk: the issues match its text.
        self.underline_issues(self.active, cx);
        self.remember_session();
        self.spellcheck(self.active, Duration::ZERO, cx);
        self.grammar_check(self.active, Duration::ZERO, cx);
        self.apply_diagnostics(cx);
        let focus = self.buffers[self.active].editor.focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
        Some(self.active)
    }

    /// Jump to a 0-based `line` in `file` (outline, diagnostics, inverse search).
    fn reveal(&mut self, file: PathBuf, line: u32, window: &mut Window, cx: &mut Context<Self>) {
        let file = crate::paths::canonical(&file);
        let Some(ix) = self.activate_file(file, window, cx) else {
            return;
        };
        let editor = self.buffers[ix].editor.clone();
        editor.update(cx, |state, cx| {
            state.set_cursor_position(Position::new(line, 0), window, cx);
            state.focus(window, cx);
        });
    }

    /// Select bytes of `file` (a definition, a search result).
    fn reveal_range(
        &mut self,
        file: PathBuf,
        range: std::ops::Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let file = crate::paths::canonical(&file);
        let Some(ix) = self.activate_file(file, window, cx) else {
            return;
        };
        let editor = self.buffers[ix].editor.clone();
        editor.update(cx, |state, cx| {
            let len = state.text().len();
            let range = range.start.min(len)..range.end.min(len);
            // Scroll to it: moving the cursor brings it into view.
            let start = state.text().offset_to_position(range.start);
            state.set_cursor_position(start, window, cx);
            state.set_selected_range(range, cx);
            state.focus(window, cx);
        });
    }

    /// Open a file with its app, if it is a document or a picture.
    fn open_external(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        if files::safe_to_open(path) {
            cx.open_with_system(path);
        } else {
            // It could be a program: let the user decide in the file manager
            // rather than start it.
            cx.reveal_path(path);
            window.push_notification(
                Notification::info(
                    "TeXoria opens only documents and pictures with other apps: the file is \
                     shown in its folder instead.",
                ),
                cx,
            );
        }
    }

    /// Open a package's documentation: the local one, else texdoc.org's.
    fn package_docs(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        if !is_package_name(&name) {
            return;
        }
        cx.spawn_in(window, async move |this, cx| {
            let package = name.clone();
            let local = cx
                .background_spawn(async move { tex_compile::texdoc(&package) })
                .await;
            this.update_in(cx, |this, window, cx| match local {
                Some(doc) => this.open_external(&doc, window, cx),
                None => cx.open_url(&format!("https://texdoc.org/pkg/{name}")),
            })
            .ok();
        })
        .detach();
    }

    fn prompt_open(&mut self, folder: bool, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(open_prompt(folder));
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
        self.grammar_check(ix, grammar::DELAY, cx);
        let delay = Duration::from_millis(self.settings.live_delay_ms);
        let live = self.settings.live_preview;
        self._live = cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(delay).await;
            this.update(cx, |this, cx| {
                this.reindex(cx);
                if live {
                    this.build(cx);
                }
            })
            .ok();
        });
        cx.notify();
    }

    /// Re-index open buffers so outline and completions follow unsaved edits.
    fn reindex(&mut self, cx: &mut Context<Self>) {
        let mut texts = Vec::new();
        {
            let mut project = self.project.write().unwrap();
            for b in &self.buffers {
                let text = b.editor.read(cx).text().to_string();
                project.update(&b.path, &text);
                texts.push((b.editor.clone(), text));
            }
        }
        self.refresh_outline(cx);
        self.count_words(&texts, cx);
        self.run_checks(texts, cx);
    }

    /// Count the words of every file of the project in the background,
    /// reading open files from `texts`.
    fn count_words(&mut self, texts: &[(Entity<EditorState>, String)], cx: &mut Context<Self>) {
        let files = self.project.read().unwrap().files();
        let open: std::collections::HashMap<PathBuf, String> = texts
            .iter()
            .filter_map(|(editor, text)| {
                let b = self.buffers.iter().find(|b| &b.editor == editor)?;
                Some((b.path.clone(), text.clone()))
            })
            .collect();
        let cache = self.count_cache.clone();
        self._count = cx.spawn(async move |this, cx| {
            let counts = cx
                .background_spawn(async move {
                    use std::hash::{Hash, Hasher};
                    let mut cache = cache.lock().unwrap();
                    let mut out = Vec::new();
                    for file in files {
                        let Some(text) = open
                            .get(&file)
                            .cloned()
                            .or_else(|| tex_project::read_regular(&file))
                        else {
                            continue;
                        };
                        let mut h = std::collections::hash_map::DefaultHasher::new();
                        text.hash(&mut h);
                        let hash = h.finish();
                        let counts = match cache.get(&file) {
                            Some((cached, counts)) if *cached == hash => *counts,
                            _ => {
                                let counts = tex_syntax::parse(&text, None)
                                    .map(|t| tex_spell::extract::count(&t, &text))
                                    .unwrap_or_default();
                                cache.insert(file.clone(), (hash, counts));
                                counts
                            }
                        };
                        out.push((file, counts));
                    }
                    out
                })
                .await;
            this.update(cx, |this, cx| {
                this.word_counts = counts;
                cx.notify();
            })
            .ok();
        });
    }

    /// The words of the whole project.
    fn total_words(&self) -> tex_spell::extract::Counts {
        let mut total = tex_spell::extract::Counts::default();
        for (_, c) in &self.word_counts {
            total.add(c);
        }
        total
    }

    /// The project's words, by kind and by file.
    fn show_word_count(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let total = self.total_words();
        let files: Vec<(String, usize)> = self
            .word_counts
            .iter()
            .map(|(f, c)| (self.relative(f).display().to_string(), c.words()))
            .collect();
        window.open_dialog(cx, move |dialog, _, cx| {
            let theme = cx.theme();
            let row = |label: String, value: String| {
                h_flex()
                    .justify_between()
                    .gap_4()
                    .text_sm()
                    .child(div().text_color(theme.muted_foreground).child(label))
                    .child(div().child(value))
            };
            let mut body = v_flex()
                .gap_1()
                .child(row("Words in the text".into(), thousands(total.text)))
                .child(row("Words in headings".into(), thousands(total.headers)))
                .child(row("Words in captions".into(), thousands(total.captions)))
                .child(row("Inline formulas".into(), thousands(total.inline_math)))
                .child(row(
                    "Displayed formulas".into(),
                    thousands(total.display_math),
                ));
            if files.len() > 1 {
                body = body.child(div().h_2()).child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.muted_foreground)
                        .child("WORDS BY FILE"),
                );
                for (name, words) in &files {
                    body = body.child(row(name.clone(), thousands(*words)));
                }
            }
            dialog
                .title(format!("{} words", thousands(total.words())))
                .w(px(420.))
                .child(
                    div()
                        .id("word-count")
                        .max_h(px(420.))
                        .overflow_y_scroll()
                        .child(body),
                )
        });
    }

    /// Check the project (as indexed from `texts`, the buffers' text) in the
    /// background, then underline what was found.
    fn run_checks(&mut self, texts: Vec<(Entity<EditorState>, String)>, cx: &mut Context<Self>) {
        if !self.settings.live_checks || self.root.is_none() {
            self._checks = Task::ready(());
            if !self.issues.is_empty() {
                self.issues.clear();
                for b in &self.buffers {
                    b.checks.clear(cx);
                }
                cx.notify();
            }
            return;
        }
        let project = self.project.clone();
        self._checks = cx.spawn(async move |this, cx| {
            let issues = cx
                .background_spawn(async move {
                    let project = project.read().unwrap();
                    tex_project::check::run(&project)
                })
                .await;
            this.update(cx, |this, cx| {
                this.issues = issues;
                for ix in 0..this.buffers.len() {
                    let b = &this.buffers[ix];
                    // Edited meanwhile: the next check is on its way. A buffer
                    // not in `texts` was indexed from its file.
                    let checked = texts
                        .iter()
                        .find(|(e, _)| *e == b.editor)
                        .map_or(&b.saved, |(_, t)| t);
                    let same = *b.editor.read(cx).text() == **checked;
                    if same {
                        this.underline_issues(ix, cx);
                    }
                }
                cx.notify();
            })
            .ok();
        });
    }

    /// Underline the issues of buffer `ix`.
    fn underline_issues(&self, ix: usize, cx: &mut App) {
        let b = &self.buffers[ix];
        let issues: Vec<&tex_project::check::Issue> =
            self.issues.iter().filter(|i| i.file == b.path).collect();
        let colors = (cx.theme().danger, cx.theme().warning);
        b.checks.set(&b.editor, &issues, colors, cx);
    }

    /// The problems to list: the last build's, without those a live check
    /// also reports, then the live checks'.
    fn problems(&self) -> Vec<Diagnostic> {
        let keys: std::collections::HashSet<&str> = self
            .issues
            .iter()
            .filter_map(|i| i.key.as_deref())
            .collect();
        self.diagnostics
            .iter()
            .filter(|d| !checks::duplicates(d, &keys))
            .cloned()
            .chain(self.issues.iter().map(checks::diagnostic))
            .collect()
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
                self.grammar_check(ix, Duration::ZERO, cx);
            }
            ix += 1;
        }
        if changed {
            self.active = self.active.min(self.buffers.len().saturating_sub(1));
            self.remember_session();
            self.reindex(cx);
            self.apply_diagnostics(cx);
            self.build(cx);
        }
    }

    /// Point the Source Control panel at the current project.
    fn sync_git(&mut self, cx: &mut Context<Self>) {
        let dir = self
            .root
            .as_ref()
            .and_then(|r| r.parent())
            .map(Path::to_path_buf);
        // Trust is the repository's: its configuration is what runs.
        let trusted = dir
            .as_deref()
            .and_then(git_panel::repo_top)
            .is_some_and(|top| self.settings.is_trusted(&top));
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
                    self.open_external(path, window, cx);
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
        self.remember_session();
    }

    // ── Search ───────────────────────────────────────────────────────────

    /// Show the Search view, searching for the editor's selection if any.
    fn find_in_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let selected = self.buffers.get(self.active).and_then(|b| {
            let state = b.editor.read(cx);
            let range = state.selected_range();
            let text = state.text().slice(range).to_string();
            (!text.is_empty() && !text.contains('\n')).then_some(text)
        });
        self.show_sidebar(Sidebar::Search, cx);
        self.search
            .update(cx, |s, cx| s.focus(selected, window, cx));
    }

    fn on_search_event(
        &mut self,
        _: &Entity<SearchPanel>,
        ev: &SearchPanelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match ev {
            SearchPanelEvent::Search => {
                let open = self
                    .buffers
                    .iter()
                    .map(|b| {
                        (
                            b.path.clone(),
                            Arc::from(b.editor.read(cx).text().to_string()),
                        )
                    })
                    .collect();
                self.search.update(cx, |s, cx| s.run(open, cx));
            }
            SearchPanelEvent::Open { path, range } => {
                self.reveal_range(path.clone(), range.clone(), window, cx)
            }
            SearchPanelEvent::ReplaceAll {
                query,
                replacement,
                files,
                truncated,
            } => {
                let files = self.replace_targets(files, *truncated);
                let (query, replacement) = (query.clone(), replacement.clone());
                self.confirm_replace_all(query, replacement, files, *truncated, window, cx)
            }
        }
    }

    /// The files Replace All goes through: the search's, unless it stopped before the end
    /// (there are matches it does not list), then every text file of the project.
    fn replace_targets(&self, found: &[PathBuf], truncated: bool) -> Vec<PathBuf> {
        match self.project_dir() {
            Some(dir) if truncated => tex_project::search::text_files(&dir),
            _ => found.to_vec(),
        }
    }

    fn confirm_replace_all(
        &mut self,
        query: tex_project::search::Query,
        replacement: String,
        files: Vec<PathBuf>,
        truncated: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.agent_busy {
            window.push_notification(
                Notification::info("Wait for the agent to finish before replacing."),
                cx,
            );
            return;
        }
        let closed = files
            .iter()
            .filter(|f| !self.buffers.iter().any(|b| &b.path == *f))
            .count();
        let place = if truncated {
            // The search stopped early: Replace All goes through every file.
            format!(
                "the project (there are more matches than the search lists: its {} text file{} \
                 are all searched)",
                files.len(),
                plural(files.len())
            )
        } else {
            format!("{} file{}", files.len(), plural(files.len()))
        };
        let message = format!(
            "Replace every match of “{}” with “{}” in {place}?{}",
            query.text,
            replacement,
            if closed > 0 {
                " Files that are not open are written directly: only Git can undo that."
            } else {
                ""
            }
        );
        let weak = cx.entity().downgrade();
        git_panel::open_form(
            window,
            cx,
            "Replace All",
            "Replace",
            None,
            move |_, cx| {
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(message.clone())
                    .into_any_element()
            },
            move |window, cx| {
                weak.update(cx, |this, cx| {
                    this.replace_all(&query, &replacement, &files, window, cx)
                })
                .ok();
                true
            },
        );
    }

    /// Replace every match of `query` in `files`: in the editor for open ones
    /// (so that it can be undone), on disk for the others.
    fn replace_all(
        &mut self,
        query: &tex_project::search::Query,
        replacement: &str,
        files: &[PathBuf],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Ok(regex) = query.compile() else {
            return;
        };
        let (mut total, mut changed) = (0, 0);
        for path in files {
            if let Some(b) = self.buffers.iter().find(|b| &b.path == path) {
                let n = b.editor.update(cx, |state, cx| {
                    let text = state.text().to_string();
                    let (new, n) = query.replace_all(&regex, &text, replacement);
                    if n > 0 {
                        let cursor = state.cursor().min(new.len());
                        state.set_selected_range(0..text.len(), cx);
                        state.replace(new, window, cx);
                        state.set_selected_range(cursor..cursor, cx);
                    }
                    n
                });
                total += n;
                changed += (n > 0) as usize;
                continue;
            }
            let Some(text) = tex_project::read_regular(path) else {
                continue;
            };
            let (new, n) = query.replace_all(&regex, &text, replacement);
            if n == 0 {
                continue;
            }
            match atomic::write(path, new.as_bytes()) {
                Ok(()) => {
                    total += n;
                    changed += 1;
                    self.project.write().unwrap().update(path, &new);
                }
                Err(e) => window.push_notification(
                    Notification::error(format!("Cannot write {}: {e}", path.display())),
                    cx,
                ),
            }
        }
        if total > 0 {
            window.push_notification(
                Notification::success(format!(
                    "Replaced {total} match{} in {changed} file{}.",
                    if total == 1 { "" } else { "es" },
                    plural(changed)
                )),
                cx,
            );
        }
        self.reindex(cx);
        self.git.update(cx, |g, cx| g.refresh(cx));
        self.on_search_event(&self.search.clone(), &SearchPanelEvent::Search, window, cx);
        self.build(cx);
    }

    // ── Diff tab ─────────────────────────────────────────────────────────

    fn open_diff(&mut self, source: DiffSource, window: &mut Window, cx: &mut Context<Self>) {
        self.save_all(cx);
        let Some(repo) = self.git.read(cx).repo().cloned() else {
            return;
        };
        self.show_diff(Some(repo), source, window, cx);
    }

    fn show_diff(
        &mut self,
        repo: Option<tex_git::Repo>,
        source: DiffSource,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
                let file = crate::paths::canonical(file);
                self.activate_file(file, window, cx);
            }
            DiffViewEvent::SplitChanged(split) => {
                let split = *split;
                self.update_settings(|s| s.diff_split = split, window, cx);
            }
            DiffViewEvent::UndoAgent(path) => {
                let paths = path.clone().map(|p| vec![p]);
                self.run_agent_op(AgentOp::Undo(paths), window, cx)
            }
            DiffViewEvent::KeepAgent => self.agent.update(cx, |a, cx| a.keep(cx)),
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
                        let dir = crate::paths::canonical(&dir);
                        let top = this
                            .root
                            .as_deref()
                            .and_then(Path::parent)
                            .and_then(git_panel::repo_top);
                        if top.as_deref() == Some(dir.as_path()) {
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
            Sidebar::Search | Sidebar::SourceControl => {}
            Sidebar::Agent => self.agent.update(cx, |a, cx| a.activate(cx)),
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
        job.shell_escape = self.settings.allows_shell_escape(job.work_dir());
        Some(job)
    }

    /// Let Git run in the project (its configuration can run programs).
    /// Trust the repository the project is in (its working tree).
    fn trust_git(&mut self, cx: &mut Context<Self>) {
        let Some(top) = self
            .job()
            .and_then(|job| git_panel::repo_top(job.work_dir()))
        else {
            return;
        };
        self.settings.trust(top);
        self.settings.save();
        self.sync_git(cx);
        cx.notify();
    }

    // ── Citations ────────────────────────────────────────────────────────

    /// Ask for a DOI, an arXiv identifier or a search, then add the entry to
    /// the bibliography and cite it.
    fn prompt_citation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.root.is_none() {
            return;
        }
        let input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("10.1145/… · arXiv:2301.01234 · title, authors")
        });
        let weak = cx.entity().downgrade();
        let focus = input.focus_handle(cx);
        let body = input.clone();
        git_panel::open_form(
            window,
            cx,
            "Add Citation",
            "Find",
            Some(focus),
            move |_, cx| {
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(
                                "A DOI, an arXiv identifier, or words of the title and authors. \
                                 The entry is added to the project's bibliography and cited \
                                 where the cursor is. This asks doi.org, arXiv or Crossref.",
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
                weak.update(cx, |this, cx| this.look_up_citation(text, window, cx))
                    .ok();
                true
            },
        );
    }

    fn look_up_citation(&mut self, input: String, window: &mut Window, cx: &mut Context<Self>) {
        let lookup = tex_cite::classify(&input);
        window.push_notification(Notification::info("Looking it up…"), cx);
        cx.spawn_in(window, async move |this, cx| {
            let found = cx
                .background_spawn(async move {
                    let client = tex_cite::Client::new();
                    match lookup {
                        tex_cite::Lookup::Doi(doi) => client.doi(&doi).map(Ok),
                        tex_cite::Lookup::Arxiv(id) => client.arxiv(&id).map(Ok),
                        tex_cite::Lookup::Search(q) => client.search(&q).map(Err),
                    }
                })
                .await;
            this.update_in(cx, |this, window, cx| match found {
                Ok(Ok(bibtex)) => this.add_citation(&bibtex, window, cx),
                Ok(Err(candidates)) => this.choose_citation(candidates, window, cx),
                Err(e) => window.push_notification(
                    Notification::error(format!("{e:#}")).title("Citation not found"),
                    cx,
                ),
            })
            .ok();
        })
        .detach();
    }

    /// Show the papers a search found; picking one adds it.
    fn choose_citation(
        &mut self,
        candidates: Vec<tex_cite::Candidate>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if candidates.is_empty() {
            window.push_notification(Notification::info("No paper matches."), cx);
            return;
        }
        let weak = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let theme = cx.theme();
            let rows = candidates.iter().enumerate().map(|(i, c)| {
                let mut meta: Vec<String> = Vec::new();
                if !c.authors.is_empty() {
                    let mut a = c
                        .authors
                        .iter()
                        .take(3)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ");
                    if c.authors.len() > 3 {
                        a.push_str(" et al.");
                    }
                    meta.push(a);
                }
                meta.extend(c.year.clone());
                meta.extend(c.venue.clone());
                let doi = c.doi.clone();
                let weak = weak.clone();
                v_flex()
                    .id(("citation", i))
                    .px_2()
                    .py_1p5()
                    .rounded(theme.radius)
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.accent))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .child(c.title.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(meta.join(" · ")),
                    )
                    .on_click(move |_, window, cx| {
                        window.close_dialog(cx);
                        let doi = doi.clone();
                        weak.update(cx, |this, cx| this.look_up_citation(doi, window, cx))
                            .ok();
                    })
            });
            dialog.title("Choose the Paper").w(px(560.)).child(
                v_flex()
                    .id("citations")
                    .max_h(px(420.))
                    .overflow_y_scroll()
                    .gap_0p5()
                    .children(rows),
            )
        });
    }

    /// Add an entry to the project's bibliography under a new key, and cite
    /// it at the cursor.
    fn add_citation(&mut self, bibtex: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dir) = self.project_dir() else {
            return;
        };
        let (taken, bib) = {
            let project = self.project.read().unwrap();
            let taken: std::collections::HashSet<String> =
                project.bib_entries().map(|e| e.key.clone()).collect();
            let mut bibs: Vec<PathBuf> = project
                .bib_files()
                .map(|(p, _)| p.to_path_buf())
                .filter(|p| p.is_file())
                .collect();
            bibs.sort();
            (taken, bibs.into_iter().next())
        };
        let key = tex_cite::make_key(bibtex, &taken);
        let Some(entry) = tex_cite::normalize(bibtex, &key) else {
            window.push_notification(Notification::error("The entry could not be read."), cx);
            return;
        };
        let created = bib.is_none();
        let bib = bib.unwrap_or_else(|| dir.join("references.bib"));
        if let Some(b) = self.buffers.iter().find(|b| b.path == bib) {
            b.editor.update(cx, |state, cx| {
                let len = state.text().len();
                let sep = if state.text().to_string().ends_with('\n') || len == 0 {
                    "\n"
                } else {
                    "\n\n"
                };
                state.set_selected_range(len..len, cx);
                state.insert(format!("{sep}{entry}\n"), window, cx);
            });
        } else {
            let old = tex_project::read_regular(&bib).unwrap_or_default();
            let sep = if old.is_empty() || old.ends_with("\n\n") {
                ""
            } else if old.ends_with('\n') {
                "\n"
            } else {
                "\n\n"
            };
            let new = format!("{old}{sep}{entry}\n");
            if let Err(e) = atomic::write(&bib, new.as_bytes()) {
                window.push_notification(
                    Notification::error(format!("Cannot write {}: {e}", bib.display())),
                    cx,
                );
                return;
            }
            self.project.write().unwrap().update(&bib, &new);
        }
        self.cite_at_cursor(&key, window, cx);
        let name = self.relative(&bib).display().to_string();
        let message = if created {
            format!(
                "Added {key} to {name}, a new file: add \\bibliography{{references}} or \
                 \\addbibresource{{references.bib}} to the document."
            )
        } else {
            format!("Added {key} to {name}.")
        };
        window.push_notification(Notification::success(message).title("Citation added"), cx);
        self.files.update(cx, |f, cx| f.refresh(cx));
        self.reindex(cx);
    }

    /// Cite `key` at the cursor of the active file: inside a `\cite{…}`, as
    /// one more key.
    fn cite_at_cursor(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(b) = self
            .buffers
            .get(self.active)
            .filter(|b| !self.diff_active && !self.agent_busy && crate::paths::is_latex(&b.path))
        else {
            return;
        };
        b.editor.update(cx, |state, cx| {
            let text = state.text().to_string();
            let cursor = state.cursor();
            let insert = match tex_complete::context(&text, cursor) {
                Some(tex_complete::Context::Citation { prefix }) => {
                    let before = text[..prefix.start].trim_end();
                    if prefix.is_empty() && (before.ends_with('{') || before.ends_with(',')) {
                        key.to_string()
                    } else {
                        // After the key being typed.
                        state.set_selected_range(prefix.end..prefix.end, cx);
                        format!(",{key}")
                    }
                }
                _ => format!("\\cite{{{key}}}"),
            };
            state.insert(insert, window, cx);
        });
    }

    /// Save a copy of the PDF where the user chooses (by default, where the
    /// last one went).
    fn export_pdf(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(job) = self.job() else {
            return;
        };
        let pdf = job.artifact("pdf");
        if !self.show_old_pdf || !pdf.is_file() {
            window.push_notification(Notification::info("Build the document first."), cx);
            return;
        }
        let root = job.root_file.clone();
        let last = self.settings.session(&root).and_then(|s| s.export.clone());
        let dir = last
            .as_deref()
            .and_then(Path::parent)
            .filter(|d| d.is_dir())
            .unwrap_or(job.work_dir())
            .to_path_buf();
        let name = last
            .as_deref()
            .and_then(Path::file_name)
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| format!("{}.pdf", job.stem()));
        let path = cx.prompt_for_new_path(&dir, Some(&name));
        cx.spawn_in(window, async move |this, cx| {
            let Some(dest) = path.await.ok().and_then(|r| r.ok()).flatten() else {
                return;
            };
            let copy = (pdf.clone(), dest.clone());
            let copied = cx
                .background_spawn(async move { std::fs::copy(&copy.0, &copy.1) })
                .await;
            this.update_in(cx, |this, window, cx| match copied {
                Ok(_) => {
                    let mut session = this.settings.session(&root).cloned().unwrap_or(Session {
                        root: root.clone(),
                        ..Default::default()
                    });
                    session.export = Some(dest.clone());
                    if this.settings.set_session(session) {
                        this.settings.save();
                    }
                    let name = dest
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    window.push_notification(
                        Notification::success(format!("Saved {name}"))
                            .title("PDF exported")
                            .action(move |_, _, _| {
                                let dest = dest.clone();
                                Button::new("reveal-export")
                                    .outline()
                                    .label("Show in Folder")
                                    .on_click(move |_, _, cx| cx.reveal_path(&dest))
                            }),
                        cx,
                    );
                }
                Err(e) => window.push_notification(
                    Notification::error(format!("Cannot save {}: {e}", dest.display()))
                        .title("Export failed"),
                    cx,
                ),
            })
            .ok();
        })
        .detach();
    }

    /// Show the PDF full screen in a window of its own, from the page the
    /// preview is on.
    fn present(&mut self, cx: &mut Context<Self>) {
        let (page, pages) = {
            let pdf = self.pdf.read(cx);
            (pdf.current_page(), pdf.presentation().1)
        };
        if pages == 0 {
            return;
        }
        let pane = self.pdf.clone();
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::centered(size(px(1280.), px(800.)), cx)),
            app_id: Some("texoria".into()),
            titlebar: None,
            ..Default::default()
        };
        // The new window reads the preview, which may be being updated.
        cx.defer(move |cx| {
            let opened = gpui_kit::open_window(options, cx, move |window, cx| {
                window.defer(cx, |window, _| window.toggle_fullscreen());
                cx.new(|cx| Presentation::new(pane, page, window, cx))
            });
            if let Err(e) = opened {
                log::error!("opening the presentation: {e}");
            }
        });
    }

    /// Let the project's documents run programs, and build again.
    fn allow_shell_escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dir) = self.project_dir() else {
            return;
        };
        self.wants_shell_escape = false;
        self.update_settings(|s| s.allow_shell_escape(dir), window, cx);
    }

    fn allow_rc(&mut self, cx: &mut Context<Self>) {
        let Some(job) = self.job() else {
            return;
        };
        self.settings.allow_rc(job.work_dir().to_path_buf());
        self.settings.save();
        self.untrusted_rc = false;
        self.build(cx);
    }

    fn build(&mut self, cx: &mut Context<Self>) {
        let Some(mut job) = self.job() else {
            return;
        };
        // Build files that came with the project would make latexmk skip
        // the run, and leave their PDF in place of ours.
        job.force = !self.show_old_pdf;
        if !self.save_all(cx) {
            return;
        }
        if let Some(c) = &self.compiler {
            c.request_now(job);
        }
    }

    fn cancel_build(&mut self) {
        if let Some(c) = &self.compiler {
            c.cancel();
        }
        // A request still waiting to start is dropped without a run to
        // report it: the agent would wait for one until its timeout.
        self.answer_waiters(ToolResult::error("The build was stopped."));
    }

    fn on_compile_event(&mut self, event: CompileEvent, cx: &mut Context<Self>) {
        match event {
            CompileEvent::Started { id, .. } => {
                self.state = CompileState::Running;
                self.last_run = id;
            }
            CompileEvent::SpawnFailed { id, error } => {
                self.answer_build(id, ToolResult::error(format!("The engine failed: {error}")));
                self.state = CompileState::Failed(error.into());
            }
            CompileEvent::Finished(out) => {
                // A run for a project that is no longer open.
                if self.root.as_ref() != Some(&out.job.root_file) {
                    self.answer_build(out.id, ToolResult::error("Another document was opened."));
                    return;
                }
                if out.status == Status::Cancelled {
                    self.answer_build(out.id, ToolResult::error("The build was stopped."));
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
                        d.file = d.file.map(|f| crate::paths::canonical(&f));
                        d
                    })
                    .collect();
                self.apply_diagnostics(cx);
                let report = self.build_report(out.status, out.elapsed);
                self.answer_build(out.id, ToolResult::ok(report));
                self.agent.update(cx, |a, cx| a.set_errors(errors, cx));
                self.wants_shell_escape = out.needs_shell_escape;
                self.show_old_pdf |= out.pdf_written;
                if let Some(pdf) = out.pdf.filter(|_| self.show_old_pdf) {
                    self.pdf.update(cx, |p, cx| p.load(pdf, out.synctex, cx));
                }
                // .bib files may have been edited outside the app.
                self.project.write().unwrap().reload_bibs();
                self.reindex(cx);
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

    // ── Coding agents ────────────────────────────────────────────────────

    fn project_dir(&self) -> Option<PathBuf> {
        self.root
            .as_ref()
            .and_then(|r| r.parent())
            .map(Path::to_path_buf)
    }

    /// Point the Agent panel at the current project.
    fn sync_agent(&mut self, cx: &mut Context<Self>) {
        let dir = self.project_dir();
        let allowed = dir.as_ref().is_some_and(|d| self.settings.allows_agents(d));
        let root = self.root.clone();
        self.agent
            .update(cx, |a, cx| a.set_project(dir, root, allowed, cx));
    }

    fn on_agent_event(
        &mut self,
        _: &Entity<AgentPanel>,
        ev: &AgentPanelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match ev {
            AgentPanelEvent::Request(op) => self.run_agent_op(op.clone(), window, cx),
            AgentPanelEvent::Busy(busy) => self.set_agent_busy(*busy, window, cx),
            AgentPanelEvent::FilesChanged => {
                self.reload_from_disk(window, cx);
                self.files.update(cx, |f, cx| f.refresh(cx));
                self.git.update(cx, |g, cx| g.refresh(cx));
            }
            AgentPanelEvent::Review { patch, open } => {
                self.show_agent_review(patch.clone(), *open, window, cx)
            }
            AgentPanelEvent::Choose(id) => {
                let id = id.clone();
                self.update_settings(|s| s.agent = Some(id), window, cx);
            }
            AgentPanelEvent::Allow => {
                if let Some(dir) = self.project_dir() {
                    self.update_settings(|s| s.allow_agents(dir), window, cx);
                }
            }
            AgentPanelEvent::FixErrors => self.fix_with_agent(window, cx),
            AgentPanelEvent::Open { path, line } => self.reveal(
                path.clone(),
                line.unwrap_or(1).saturating_sub(1),
                window,
                cx,
            ),
        }
    }

    /// The agent works on files: they must be saved first.
    fn run_agent_op(&mut self, op: AgentOp, window: &mut Window, cx: &mut Context<Self>) {
        if !self.save_all(cx) {
            return;
        }
        match op {
            AgentOp::Chat(text) => {
                let prompt = self.chat_prompt(text, cx);
                self.agent.update(cx, |a, cx| a.send(prompt, cx));
            }
            AgentOp::Send(prompt) => self.agent.update(cx, |a, cx| a.send(prompt, cx)),
            AgentOp::Undo(paths) => {
                self.agent.update(cx, |a, cx| a.undo(paths, cx));
                self.reload_from_disk(window, cx);
            }
        }
    }

    /// A chat message, with the open file and its selection.
    fn chat_prompt(&self, text: String, cx: &App) -> Prompt {
        let mut prompt = Prompt::new(text.clone(), text);
        if self.diff_active {
            return prompt;
        }
        let Some(b) = self.buffers.get(self.active) else {
            return prompt;
        };
        let rel = self.relative(&b.path);
        prompt
            .blocks
            .push(acp::ContentBlock::ResourceLink(acp::ResourceLink::new(
                rel.display().to_string(),
                file_uri(&b.path),
            )));
        if let Some((first, last, selected)) = selection(&b.editor, cx) {
            prompt.blocks.push(text_block(tex_agent::prompt::selection(
                &rel, first, last, &selected,
            )));
            prompt.caption =
                Some(format!("With the selection in {}", lines_label(&rel, first, last)).into());
        }
        prompt
    }

    /// `path` relative to the project, as the agents' tools show it: with `/` on every system,
    /// like the paths of `\input` and `\includegraphics`.
    fn tool_path(&self, path: &Path) -> String {
        let rel = self.relative(path);
        if rel.is_absolute() {
            // Outside the project: as the system writes it.
            return rel.display().to_string();
        }
        let parts: Vec<_> = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect();
        parts.join("/")
    }

    fn relative(&self, path: &Path) -> PathBuf {
        self.project_dir()
            .and_then(|d| path.strip_prefix(d).ok().map(Path::to_path_buf))
            .unwrap_or_else(|| path.to_path_buf())
    }

    fn set_agent_busy(&mut self, busy: bool, window: &mut Window, cx: &mut Context<Self>) {
        if busy {
            // The agent reads the files from disk.
            self.save_all(cx);
        }
        self.agent_busy = busy;
        for b in &self.buffers {
            b.editor
                .update(cx, |state, cx| state.set_readonly(busy, cx));
        }
        if !busy {
            self.reload_from_disk(window, cx);
            self.files.update(cx, |f, cx| f.refresh(cx));
            self.git.update(cx, |g, cx| g.refresh(cx));
        }
        cx.notify();
    }

    fn show_agent_review(
        &mut self,
        patch: String,
        open: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let showing = self.diff.as_ref().is_some_and(|d| d.read(cx).is_agent());
        if patch.is_empty() {
            if showing {
                self.close_diff(cx);
            }
            return;
        }
        let Some(dir) = self.project_dir() else {
            return;
        };
        if open || showing {
            let active = self.diff_active;
            self.show_diff(None, DiffSource::Agent { dir, patch }, window, cx);
            // A refresh does not bring the tab to the front.
            self.diff_active = open || active;
        }
    }

    /// Ask the agent to rewrite the selection.
    fn rewrite_selection(&mut self, how: Rewrite, window: &mut Window, cx: &mut Context<Self>) {
        let Some(b) = self.buffers.get(self.active).filter(|_| !self.diff_active) else {
            return;
        };
        let Some((first, last, text)) = selection(&b.editor, cx) else {
            window.push_notification(Notification::info("Select the passage to edit first."), cx);
            return;
        };
        let rel = self.relative(&b.path);
        let place = lines_label(&rel, first, last);
        let (display, caption) = match &how {
            Rewrite::Custom(text) => (text.clone(), Some(place)),
            preset => (format!("{} — {place}", preset.label()), None),
        };
        let prompt = Prompt {
            display: display.into(),
            caption: caption.map(Into::into),
            blocks: vec![text_block(tex_agent::prompt::rewrite(
                &rel, first, last, &text, &how,
            ))],
        };
        self.show_agent(window, cx);
        self.run_agent_op(AgentOp::Send(prompt), window, cx);
    }

    /// Ask for the instruction, then rewrite the selection with it.
    fn prompt_edit_with_agent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let has_selection = self
            .buffers
            .get(self.active)
            .is_some_and(|b| selection(&b.editor, cx).is_some());
        if self.diff_active || !has_selection {
            window.push_notification(Notification::info("Select the passage to edit first."), cx);
            return;
        }
        let input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Translate into English, add an example…")
        });
        let weak = cx.entity().downgrade();
        let focus = input.focus_handle(cx);
        let body = input.clone();
        git_panel::open_form(
            window,
            cx,
            "Edit with the Agent",
            "Edit",
            Some(focus),
            move |_, cx| {
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child("What should the agent do with the selected passage?"),
                    )
                    .child(Input::new(&body))
                    .into_any_element()
            },
            move |window, cx| {
                let text = input.read(cx).value().trim().to_string();
                if text.is_empty() {
                    return false;
                }
                weak.update(cx, |this, cx| {
                    this.rewrite_selection(Rewrite::Custom(text), window, cx)
                })
                .ok();
                true
            },
        );
    }

    /// Ask the agent to fix the last run's errors (or warnings).
    fn fix_with_agent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dir) = self.project_dir() else {
            return;
        };
        let problems: Vec<Diagnostic> = self
            .problems()
            .into_iter()
            .filter(|d| d.severity != Severity::Info)
            .collect();
        if problems.is_empty() {
            return;
        }
        let errors = problems
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .count();
        let display = if errors > 0 {
            format!("Fix the {errors} build error{}", plural(errors))
        } else {
            format!(
                "Fix the {} warning{}",
                problems.len(),
                plural(problems.len())
            )
        };
        let prompt = Prompt::new(display, tex_agent::prompt::fix_errors(&dir, &problems));
        self.show_agent(window, cx);
        self.run_agent_op(AgentOp::Send(prompt), window, cx);
    }

    fn show_agent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_sidebar(Sidebar::Agent, cx);
        self.agent.update(cx, |a, cx| a.focus_composer(window, cx));
    }

    /// One of TeXoria's tools, called by an agent.
    fn on_tool_call(&mut self, call: ToolCall, cx: &mut Context<Self>) {
        let reply = |r: ToolResult| {
            let _ = call.reply.send(r);
        };
        match call.name.as_str() {
            "build" => {
                if self.root.is_none() {
                    return reply(ToolResult::error("No document is open."));
                }
                if self.compiler.is_none() {
                    return reply(ToolResult::error(
                        "No TeX engine is installed, so the document cannot be built.",
                    ));
                }
                if !self.save_all(cx) {
                    return reply(ToolResult::error("TeXoria could not save the files."));
                }
                self.tool_waiters.push(ToolWaiter {
                    reply: call.reply.clone(),
                    after: self.last_run,
                });
                self.build(cx);
            }
            "problems" => reply(ToolResult::ok(self.problems_report())),
            "outline" => reply(ToolResult::ok(self.outline_report())),
            "pdf_page" => self.tool_pdf_page(call, cx),
            "pdf_text" => self.tool_pdf_text(call, cx),
            "bib_search" => {
                let query = call.arguments["query"].as_str().unwrap_or("").to_string();
                reply(ToolResult::ok(self.bib_report(&query)))
            }
            "word_count" => reply(ToolResult::ok(self.word_count_report())),
            other => reply(ToolResult::error(format!("Unknown tool: {other}"))),
        }
    }

    /// `pdf_page`: a PNG of a page of the PDF.
    fn tool_pdf_page(&mut self, call: ToolCall, cx: &mut Context<Self>) {
        let pages = self.pdf.read(cx).page_count();
        let page = call.arguments["page"].as_u64().unwrap_or(0) as usize;
        if pages == 0 {
            let _ = call
                .reply
                .send(ToolResult::error("There is no PDF yet: build first."));
            return;
        }
        if page == 0 || page > pages {
            let _ = call.reply.send(ToolResult::error(format!(
                "The PDF has pages 1 to {pages}."
            )));
            return;
        }
        let scale = call.arguments["scale"]
            .as_f64()
            .unwrap_or(1.5)
            .clamp(0.5, 3.0) as f32;
        let bitmap = self.pdf.update(cx, |p, _| p.snapshot(page - 1, scale));
        cx.spawn(async move |_, cx| {
            let result = match bitmap.recv().await.ok().flatten() {
                Some(b) => cx
                    .background_spawn(async move { png_of(b) })
                    .await
                    .map(|data| {
                        ToolResult::ok(format!("Page {page} of {pages}.")).with_image(
                            mcp::ToolImage {
                                data,
                                mime_type: "image/png".into(),
                            },
                        )
                    })
                    .unwrap_or_else(|e| ToolResult::error(format!("Cannot encode the page: {e}"))),
                None => ToolResult::error("The page could not be rendered."),
            };
            let _ = call.reply.send(result);
        })
        .detach();
    }

    /// `pdf_text`: the text of a page, or of the whole PDF.
    fn tool_pdf_text(&mut self, call: ToolCall, cx: &mut Context<Self>) {
        let pages = self.pdf.read(cx).page_count();
        if pages == 0 {
            let _ = call
                .reply
                .send(ToolResult::error("There is no PDF yet: build first."));
            return;
        }
        let page = call.arguments["page"].as_u64().map(|p| p as usize);
        if page.is_some_and(|p| p == 0 || p > pages) {
            let _ = call.reply.send(ToolResult::error(format!(
                "The PDF has pages 1 to {pages}."
            )));
            return;
        }
        let text = self.pdf.update(cx, |p, _| p.text(page.map(|p| p - 1)));
        cx.spawn(async move |_, _| {
            let result = match text.recv().await.ok().flatten() {
                Some(t) => ToolResult::ok(t),
                None => ToolResult::error("The text could not be read."),
            };
            let _ = call.reply.send(result);
        })
        .detach();
    }

    /// `bib_search`: the bibliography entries matching every word of `query`.
    fn bib_report(&self, query: &str) -> String {
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        let project = self.project.read().unwrap();
        let mut lines = Vec::new();
        for (file, entries) in project.bib_files() {
            for e in entries {
                let hay = format!(
                    "{} {} {} {}",
                    e.key,
                    e.title.as_deref().unwrap_or(""),
                    e.author.as_deref().unwrap_or(""),
                    e.year.as_deref().unwrap_or("")
                )
                .to_lowercase();
                if words.iter().all(|w| hay.contains(w)) {
                    lines.push(format!(
                        "{} — {} ({}) {} [{}]",
                        e.key,
                        e.author.as_deref().unwrap_or("?"),
                        e.year.as_deref().unwrap_or("n.d."),
                        e.title.as_deref().unwrap_or(""),
                        self.tool_path(file)
                    ));
                }
            }
        }
        for (file, index) in project.indexes() {
            for b in &index.bibitems {
                if words.iter().all(|w| b.name.to_lowercase().contains(w)) {
                    lines.push(format!("{} — \\bibitem [{}]", b.name, self.tool_path(file)));
                }
            }
        }
        if lines.is_empty() {
            "No entry matches.".to_string()
        } else {
            lines.join("\n")
        }
    }

    /// `word_count`: the document's words, in total and per file.
    fn word_count_report(&self) -> String {
        let t = self.total_words();
        let mut out = format!(
            "{} words: {} in the text, {} in headings, {} in captions; {} inline and {} \
             displayed formulas.",
            t.words(),
            t.text,
            t.headers,
            t.captions,
            t.inline_math,
            t.display_math
        );
        for (file, c) in &self.word_counts {
            out.push_str(&format!("\n{}: {} words", self.tool_path(file), c.words()));
        }
        out
    }

    /// Answer every pending `build` call with `result`.
    fn answer_waiters(&mut self, result: ToolResult) {
        for w in std::mem::take(&mut self.tool_waiters) {
            let _ = w.reply.send(result.clone());
        }
    }

    /// Answer the `build` calls made before run `id` started.
    fn answer_build(&mut self, id: u64, result: ToolResult) {
        let (done, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut self.tool_waiters)
            .into_iter()
            .partition(|w| id > w.after);
        self.tool_waiters = waiting;
        for w in done {
            let _ = w.reply.send(result.clone());
        }
    }

    fn build_report(&self, status: Status, elapsed: Duration) -> String {
        let head = match status {
            Status::Success => format!("Built in {:.1} s.", elapsed.as_secs_f32()),
            Status::Failed => "The build failed.".to_string(),
            Status::Cancelled => "The build was stopped.".to_string(),
        };
        format!("{head}\n{}", self.problems_text())
    }

    fn problems_report(&self) -> String {
        format!("{}\n{}", run_summary(&self.state), self.problems_text())
    }

    fn problems_text(&self) -> String {
        let dir = self.project_dir().unwrap_or_default();
        let problems: Vec<Diagnostic> = self
            .problems()
            .into_iter()
            .filter(|d| d.severity != Severity::Info)
            .collect();
        tex_agent::prompt::format_diagnostics(&dir, &problems)
    }

    fn outline_report(&self) -> String {
        if self.outline.is_empty() {
            return "The document has no sections, figures or tables yet.".to_string();
        }
        self.outline
            .iter()
            .map(|o| {
                let indent = "  ".repeat(o.item.kind.depth().min(6) as usize);
                format!(
                    "{indent}{:?}: {} ({}:{})",
                    o.item.kind,
                    o.item.title,
                    self.tool_path(&o.file),
                    o.item.line + 1
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
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
                    let ix = this.buffers.iter().position(|b| b.editor == editor)?;
                    let changed = this.buffers[ix].spell.language.map(|d| d.code)
                        != Some(analysis.language.code);
                    if changed {
                        // Grammar is checked in the spelling's language.
                        this.buffers[ix].spell.language = Some(analysis.language);
                        this.grammar_check(ix, Duration::ZERO, cx);
                    }
                    let b = &mut this.buffers[ix];
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

    /// Format the active file with tex-fmt (⇧⌥F), in the background.
    fn format_document(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(b) = self.buffers.get(self.active).filter(|_| !self.diff_active) else {
            return;
        };
        if self.agent_busy || !crate::paths::is_latex(&b.path) {
            return;
        }
        let editor = b.editor.clone();
        let path = b.path.clone();
        let dir = self.project_dir();
        let text = editor.read(cx).text().to_string();
        cx.spawn_in(window, async move |this, cx| {
            let input = text.clone();
            let formatted = cx
                .background_spawn(async move { edit::format_latex(&input, &path, dir.as_deref()) })
                .await;
            this.update_in(cx, |_, window, cx| match formatted {
                // Edited meanwhile: formatting it would undo those edits.
                Ok(new) if *editor.read(cx).text() == text => {
                    apply_format(&editor, new, window, cx)
                }
                Ok(_) => {}
                Err(e) => window
                    .push_notification(Notification::error(e).title("Cannot format the file"), cx),
            })
            .ok();
        })
        .detach();
    }

    /// ⌘S: format the active file first if asked to, then save everything.
    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings.format_on_save
            && !self.agent_busy
            && let Some(b) = self
                .buffers
                .get(self.active)
                .filter(|b| crate::paths::is_latex(&b.path))
        {
            let text = b.editor.read(cx).text().to_string();
            match edit::format_latex(&text, &b.path, self.project_dir().as_deref()) {
                Ok(new) => apply_format(&b.editor.clone(), new, window, cx),
                Err(e) => log::warn!("formatting {}: {e}", b.path.display()),
            }
        }
        self.save_all(cx);
    }

    // ── Vim ──────────────────────────────────────────────────────────────

    /// A key typed anywhere in the window: Vim's if it is in the active
    /// editor (it then goes no further).
    fn on_vim_keystroke(
        &mut self,
        ev: &KeystrokeEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(vim) = self.vim.as_mut() else {
            return;
        };
        if self.diff_active || self.agent_busy {
            return;
        }
        let Some(b) = self.buffers.get(self.active) else {
            return;
        };
        if !b.editor.focus_handle(cx).is_focused(window) {
            return;
        }
        let Some(key) = crate::vim::key_of(&ev.keystroke, vim.mode()) else {
            return;
        };
        // Escape with nothing to cancel closes the editor's popups.
        if key == tex_vim::Key::Escape
            && vim.mode() == tex_vim::Mode::Normal
            && vim.status().is_empty()
        {
            return;
        }
        let editor = b.editor.clone();
        let (taken, requests) = crate::vim::handle(vim, &editor, key, window, cx);
        if taken {
            cx.stop_propagation();
        }
        for r in requests {
            match r {
                crate::vim::Request::Save => {
                    self.save(window, cx);
                }
                crate::vim::Request::CloseTab => self.close_tab(cx),
            }
        }
        self.update_block(cx);
        cx.notify();
    }

    /// Draw Vim's block cursor in the active editor.
    fn update_block(&mut self, cx: &mut Context<Self>) {
        let color = cx.theme().caret.opacity(0.45);
        let vim = self.vim.as_ref();
        if let Some(b) = self.buffers.get_mut(self.active) {
            b.block.update(vim, &b.editor, color, cx);
        }
    }

    fn set_vim_mode(&mut self, on: bool, cx: &mut Context<Self>) {
        self.vim = on.then(tex_vim::Vim::new);
        for b in &mut self.buffers {
            b.block.update(None, &b.editor, Hsla::default(), cx);
        }
        self.update_block(cx);
    }

    /// Comment the selected lines out, or back in (⌘/).
    fn toggle_comment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(b) = self.buffers.get(self.active).filter(|_| !self.diff_active) else {
            return;
        };
        // Other text fields (a commit message) take the shortcut too.
        if self.agent_busy || !b.editor.focus_handle(cx).is_focused(window) {
            return;
        }
        b.editor.update(cx, |state, cx| {
            let e = edit::toggle_comment(&state.text().to_string(), state.selected_range());
            state.set_selected_range(e.range, cx);
            state.replace(e.text, window, cx);
            state.set_selected_range(e.selection, cx);
        });
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

    // ── Grammar ──────────────────────────────────────────────────────────

    /// Connect to the LanguageTool server (after `delay`), starting it if
    /// allowed, then check every buffer.
    fn start_grammar(&mut self, delay: Duration, cx: &mut Context<Self>) {
        self.grammar.stop();
        for b in &mut self.buffers {
            b.grammar.clear(cx);
        }
        if !self.settings.grammar_check {
            self._grammar_start = Task::ready(());
            cx.notify();
            return;
        }
        let url = self.settings.languagetool_url.trim().to_string();
        if let Err(e) = tex_spell::grammar::validate_url(&url) {
            self.grammar.state = ServerState::Unreachable(e.to_string().into());
            cx.notify();
            return;
        }
        let server = Arc::new(tex_spell::grammar::Server::new(&url));
        self.grammar.server = Some(server.clone());
        self.grammar.state = ServerState::Connecting;
        let start = self.settings.languagetool_start;
        self._grammar_start = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let probe = server.clone();
            let running = cx.background_spawn(async move { probe.languages() }).await;
            let languages = match running {
                Ok(l) => Ok(l),
                Err(e) => {
                    let port = tex_spell::grammar::local_port(&url).filter(|_| start);
                    let spawned = this.update(cx, |this, _| match port {
                        Some(port) => this.grammar.spawn(port),
                        None => Err(format!("{e:#}")),
                    });
                    match spawned {
                        Ok(Ok(())) => {
                            cx.background_spawn(async move { grammar::wait_for(&server) })
                                .await
                        }
                        Ok(Err(e)) => Err(e),
                        Err(_) => return,
                    }
                }
            };
            this.update(cx, |this, cx| {
                this.grammar.state = match languages {
                    Ok(l) => ServerState::Ready(Arc::new(l)),
                    Err(e) => {
                        log::warn!("LanguageTool: {e}");
                        ServerState::Unreachable(e.into())
                    }
                };
                for ix in 0..this.buffers.len() {
                    this.grammar_check(ix, Duration::ZERO, cx);
                }
                cx.notify();
            })
            .ok();
        });
        cx.notify();
    }

    /// Check buffer `ix`'s grammar after `delay` (replacing a pending check).
    fn grammar_check(&mut self, ix: usize, delay: Duration, cx: &mut Context<Self>) {
        let ServerState::Ready(languages) = &self.grammar.state else {
            return;
        };
        let Some(server) = self.grammar.server.clone() else {
            return;
        };
        let Some(b) = self.buffers.get_mut(ix) else {
            return;
        };
        let off = self.settings.spell_language(&b.path) == Some(SpellLanguage::OFF);
        if !self.settings.grammar_check || off || !crate::paths::is_latex(&b.path) {
            b.grammar.clear(cx);
            return;
        }
        let languages = languages.clone();
        let cache = self.grammar.cache.clone();
        let disabled = self.settings.grammar_rules_off.clone();
        let editor = b.editor.clone();
        let color = cx.theme().info;
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            // The spelling's language, once known.
            let Ok((text, code)) = this.update(cx, |this, cx| {
                let b = this.buffers.iter().find(|b| b.editor == editor);
                let code = b.and_then(|b| b.spell.language).map(|d| d.code);
                (editor.read(cx).text().to_string(), code)
            }) else {
                return;
            };
            let language = code
                .and_then(|c| tex_spell::grammar::pick_language(c, &languages))
                .unwrap_or_else(|| "auto".to_string());
            let checked = text.clone();
            let found = cx
                .background_spawn(async move {
                    grammar::check(&server, &cache, &checked, &language, &disabled)
                })
                .await;
            this.update(cx, |this, cx| {
                let Some(b) = this.buffers.iter().find(|b| b.editor == editor) else {
                    return;
                };
                // Edited meanwhile: the next check is already on its way.
                if *b.editor.read(cx).text() != text {
                    return;
                }
                match found {
                    Ok(problems) => b.grammar.set(&b.editor, problems, color, cx),
                    Err(e) => {
                        log::warn!("LanguageTool: {e:#}");
                        this.grammar.state = ServerState::Unreachable(format!("{e:#}").into());
                        cx.notify();
                    }
                }
            })
            .ok();
        });
        self.buffers[ix].grammar.task = task;
    }

    /// The status bar item of grammar checking, when it is on.
    fn render_grammar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.settings.grammar_check {
            return None;
        }
        let (icon, tooltip): (Glyph, SharedString) = match &self.grammar.state {
            ServerState::Off => return None,
            ServerState::Connecting if self.grammar.started_server() => {
                (Glyph::Spinner, "Starting LanguageTool…".into())
            }
            ServerState::Connecting => (Glyph::Spinner, "Connecting to LanguageTool…".into()),
            ServerState::Ready(_) => (Glyph::Check, "Grammar checked by LanguageTool".into()),
            ServerState::Unreachable(e) => (
                Glyph::Warning,
                format!("LanguageTool is not available: {e}. Click to try again.").into(),
            ),
        };
        Some(
            Button::new("grammar")
                .ghost()
                .icon(icon)
                .label("Grammar")
                .tooltip(tooltip)
                .on_click(cx.listener(|this, _, _, cx| {
                    if matches!(this.grammar.state, ServerState::Unreachable(_)) {
                        this.start_grammar(Duration::ZERO, cx);
                        this.vim = this.settings.vim_mode.then(tex_vim::Vim::new);
                    }
                }))
                .into_any_element(),
        )
    }

    // ── Updates ──────────────────────────────────────────────────────────

    /// Look for a new release now if one is due, then every hour whether one
    /// is (the app may stay open for days).
    fn schedule_update_checks(&mut self, cx: &mut Context<Self>) {
        if !self.updater.read(cx).enabled() {
            return;
        }
        self._update_checks = cx.spawn(async move |this, cx| {
            loop {
                let due = this.update(cx, |this, cx| {
                    let s = &this.settings;
                    if s.check_updates && Updater::check_due(s.last_update_check) {
                        this.updater.update(cx, |u, cx| u.check(false, cx));
                    }
                });
                if due.is_err() {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_secs(60 * 60))
                    .await;
            }
        });
    }

    fn on_updater_event(
        &mut self,
        _: &Entity<Updater>,
        ev: &UpdaterEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match ev {
            UpdaterEvent::Notify {
                error,
                title,
                message,
                version,
            } => {
                let mut n = if *error {
                    Notification::error(message.clone())
                } else {
                    Notification::info(message.clone())
                }
                .title(title.clone());
                if let Some(v) = version {
                    let url = updater::release_page(v);
                    n = n.action(move |_, _, _| {
                        let url = url.clone();
                        Button::new("release-notes")
                            .label("Release Notes")
                            .on_click(move |_, _, cx| cx.open_url(&url))
                    });
                }
                window.push_notification(n, cx);
            }
            UpdaterEvent::UpToDate => {
                self.settings.last_update_check = Some(updater::now());
                self.settings.save();
            }
            UpdaterEvent::Installed(relaunch) => {
                // The new version opens the same project.
                let args = self.root.iter().map(|p| p.clone().into_os_string());
                match relaunch.spawn(args) {
                    Ok(()) => cx.quit(),
                    Err(e) => {
                        let (title, message) = if relaunch.installs() {
                            ("Could not install the update", format!("{e:#}"))
                        } else {
                            (
                                "Restart TeXoria",
                                format!(
                                    "The new version is installed, but could not be started: \
                                     {e:#}. Quit and open TeXoria again."
                                ),
                            )
                        };
                        self.updater
                            .update(cx, |u, cx| u.relaunch_failed(relaunch, cx));
                        window.push_notification(Notification::error(message).title(title), cx);
                    }
                }
            }
        }
    }

    /// Save everything, install the downloaded release and restart into it.
    fn restart_to_update(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.agent_busy {
            window.push_notification(
                Notification::info("Wait for the agent to finish, or stop it, before updating."),
                cx,
            );
            return;
        }
        if !self.save_all(cx) {
            return;
        }
        self.cancel_build();
        self.updater.update(cx, |u, cx| u.install(cx));
    }

    /// The status bar item of an update, if there is one.
    fn render_update(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let updater = self.updater.read(cx);
        let self_updating = updater.kind().self_updating();
        let (label, tooltip): (SharedString, SharedString) = match updater.state() {
            UpdateState::Idle | UpdateState::Checking => return None,
            UpdateState::Available(v) if self_updating => (
                format!("Update to {v}").into(),
                "Download the new version; TeXoria restarts into it when you are ready".into(),
            ),
            UpdateState::Available(v) => (
                format!("TeXoria {v} available").into(),
                "How to update".into(),
            ),
            UpdateState::Downloading {
                version,
                done,
                total,
            } => {
                let progress = match total {
                    Some(t) if *t > 0 => format!(" {}%", done * 100 / t),
                    _ => format!(" {} MB", done >> 20),
                };
                (
                    format!("Downloading {version}…{progress}").into(),
                    "Click to cancel".into(),
                )
            }
            UpdateState::Ready(v) => (
                "Restart to Update".into(),
                format!("Save everything and restart into TeXoria {v}").into(),
            ),
            UpdateState::Installing(v) => (format!("Installing {v}…").into(), "".into()),
            UpdateState::Elsewhere(v) => (
                "Updating in another window".into(),
                format!("Another TeXoria window is downloading or installing {v}").into(),
            ),
        };
        let icon = match updater.state() {
            UpdateState::Downloading { .. } | UpdateState::Installing(_) => Glyph::Spinner,
            _ => Glyph::Fetch,
        };
        let state = updater.state().clone();
        Some(
            Button::new("update")
                .ghost()
                .icon(icon)
                .label(label)
                .when(!tooltip.is_empty(), |b| b.tooltip(tooltip))
                .on_click(cx.listener(move |this, _, window, cx| match &state {
                    UpdateState::Available(_) => this.updater.update(cx, |u, cx| u.download(cx)),
                    UpdateState::Downloading { .. } => {
                        this.updater.update(cx, |u, cx| u.cancel_download(cx))
                    }
                    UpdateState::Ready(_) => this.restart_to_update(window, cx),
                    UpdateState::Elsewhere(v) => {
                        let v = v.clone();
                        this.updater.update(cx, |u, cx| u.notify_available(v, cx))
                    }
                    _ => {}
                }))
                .into_any_element(),
        )
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
        self.build(cx);
        cx.notify();
    }

    fn close_tab(&mut self, cx: &mut Context<Self>) {
        if self.diff_active {
            return self.close_diff(cx);
        }
        self.close_buffer(self.active, cx);
    }

    /// Close the tab of `buffers[ix]`, saving first. The last tab stays; the
    /// active one keeps its tab, or goes to the closed one's neighbor.
    fn close_buffer(&mut self, ix: usize, cx: &mut Context<Self>) {
        if self.buffers.len() <= 1 || ix >= self.buffers.len() {
            return;
        }
        // Closing after a failed save would drop the edits.
        if !self.save_all(cx) {
            return;
        }
        self.buffers.remove(ix);
        if ix < self.active || self.active == self.buffers.len() {
            self.active -= 1;
        }
        self.remember_session();
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
                self.grammar_check(ix, Duration::ZERO, cx);
            }
            self.reindex(cx);
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

        // On Windows the whole title bar is a drag area: a press there is the
        // system's (it moves the window) unless something on top blocks it, so
        // the buttons are grouped in boxes that do. This also keeps a drag that
        // starts on a button from moving the window elsewhere.
        let buttons = || h_flex().gap_1().h_full().occlude();
        TitleBar::new().child(
            h_flex()
                .flex_1()
                .h_full()
                .gap_1()
                .child(
                    buttons()
                        .child(
                            Button::new("toggle-outline")
                                .ghost()
                                .icon(Glyph::PanelLeft)
                                .tooltip("Outline and files (⌘1, ⇧⌘E)")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.toggle_sidebar(this.navigator, cx)
                                })),
                        )
                        .when(has_project, |d| {
                            d.child(
                                Button::new("toggle-search")
                                    .ghost()
                                    .icon(Glyph::Search)
                                    .tooltip("Find in project (⇧⌘F)")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        if this.show_sidebar && this.sidebar == Sidebar::Search {
                                            this.toggle_sidebar(Sidebar::Search, cx);
                                        } else {
                                            this.find_in_project(window, cx);
                                        }
                                    })),
                            )
                        })
                        .when(has_project, |d| {
                            d.child(
                                Button::new("toggle-git")
                                    .ghost()
                                    .icon(Glyph::Branch)
                                    .when(changes > 0, |b| b.label(changes.to_string()))
                                    .tooltip("Source control (⇧⌘G)")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.toggle_sidebar(Sidebar::SourceControl, cx)
                                    })),
                            )
                        })
                        .when(has_project, |d| {
                            d.child(
                                Button::new("toggle-agent")
                                    .ghost()
                                    .icon(Glyph::Sparkles)
                                    .tooltip("Agent (⇧⌘K)")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        if this.show_sidebar && this.sidebar == Sidebar::Agent {
                                            this.toggle_sidebar(Sidebar::Agent, cx);
                                        } else {
                                            this.show_agent(window, cx);
                                        }
                                    })),
                            )
                        }),
                )
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
                .child(
                    buttons()
                        .child(self.render_status_pill(cx))
                        .child(
                            Button::new("build")
                                .primary()
                                .icon(Glyph::Play)
                                .label("Build")
                                .tooltip("Build now (⌘R)")
                                .on_click(cx.listener(|this, _, _, cx| this.build(cx))),
                        )
                        .child(
                            Button::new("theme")
                                .ghost()
                                .icon(if cx.theme_is_dark() {
                                    Glyph::Sun
                                } else {
                                    Glyph::Moon
                                })
                                .tooltip("Toggle light/dark")
                                .on_click(
                                    cx.listener(|this, _, window, cx| {
                                        this.toggle_theme(window, cx)
                                    }),
                                ),
                        )
                        .child(
                            Button::new("settings")
                                .ghost()
                                .icon(Glyph::Settings)
                                .tooltip("Settings (⌘,)")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_settings(window, cx)
                                })),
                        ),
                )
                .child(div().w(px(6.))),
        )
    }

    fn render_status_pill(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (icon, color, label): (Glyph, Hsla, SharedString) = match &self.state {
            CompileState::Idle => (Glyph::Info, theme.muted_foreground, "Ready".into()),
            CompileState::Running => (Glyph::Spinner, theme.primary, "Building…".into()),
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
                    format!("Built in {:.1}s", elapsed.as_secs_f32()).into()
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
                .px_2p5()
                .py_1()
                .rounded(theme.radius)
                .text_sm()
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
                .icon(icon)
                .tooltip(tooltip)
                .disabled(self.root.is_none())
        };
        let rows = outline::layout(&self.outline, &self.outline_fold);
        let open = outline::any_open(&rows);
        let header = h_flex()
            .px_3()
            .pt_3()
            .pb_2()
            .gap_1()
            .child(tab("nav-outline", "Outline", Sidebar::Outline, cx))
            .child(tab("nav-files", "Files", Sidebar::Files, cx))
            .child(div().flex_1())
            .when(!files, |d| {
                d.child(
                    Button::new("outline-fold-all")
                        .ghost()
                        .icon(if open {
                            Glyph::CollapseAll
                        } else {
                            Glyph::ExpandAll
                        })
                        .tooltip(if open { "Collapse All" } else { "Expand All" })
                        .disabled(!rows.iter().any(|r| r.group.is_some()))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.outline_fold.set_all(!open);
                            cx.notify();
                        })),
                )
            })
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
            self.render_outline(rows, cx).into_any_element()
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

    fn render_outline(&self, rows: Vec<outline::Row>, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        // The section containing the cursor is highlighted, or the
        // collapsed group it is in.
        let current = self.buffers.get(self.active).and_then(|b| {
            let line = b.editor.read(cx).cursor_position().line;
            let i = self
                .outline
                .iter()
                .enumerate()
                .filter(|(_, o)| o.file == b.path && o.item.line <= line)
                .map(|(i, _)| i)
                .next_back()?;
            outline::shown(&rows, i)
        });

        let items = rows.into_iter().filter(|r| r.visible).map(|row| {
            let i = row.index;
            let o = &self.outline[i];
            let kind = o.item.kind;
            let indent = row.depth as f32 * 12.0;
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
            // A fixed slot, so that titles line up with or without a chevron.
            let chevron = div()
                .id(("outline-fold", i))
                .debug_selector(|| format!("outline-fold {title}"))
                .flex_none()
                .size(px(14.))
                .flex()
                .items_center()
                .justify_center()
                .when_some(row.group, |d, key| {
                    d.child(
                        Icon::new(Glyph::Expand)
                            .small()
                            .text_color(theme.muted_foreground)
                            .when(row.open, |i| {
                                i.transform(Transformation::rotate(percentage(0.25)))
                            }),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.outline_fold.toggle(key.clone());
                        cx.notify();
                    }))
                });
            let (file, line) = (o.file.clone(), o.item.line);
            h_flex()
                .id(("outline", i))
                .debug_selector(|| format!("outline {title}"))
                .gap_1p5()
                .pl(px(4.0 + indent))
                .pr_2()
                .py_1()
                .mx_1()
                .rounded(theme.radius)
                .text_base()
                .cursor_pointer()
                .when(active, |d| {
                    d.bg(theme.sidebar_accent).text_color(theme.foreground)
                })
                .when(!active, |d| d.text_color(theme.sidebar_foreground))
                .when(kind.depth() <= 2, |d| d.font_weight(FontWeight::MEDIUM))
                .hover(|s| s.bg(theme.sidebar_accent))
                .child(chevron)
                .child(
                    Icon::new(icon)
                        .small()
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
                        .text_base()
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
        // The last tab stays, so it has no close button.
        let closable = self.buffers.len() > 1;
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
                    .pl_3()
                    .when(closable, |d| d.pr_1())
                    .when(!closable, |d| d.pr_3())
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
                    .when(closable, |d| {
                        d.child(
                            div()
                                .id(("tab-close", i))
                                .p_0p5()
                                .rounded(theme.radius)
                                .hover(|s| s.bg(theme.muted))
                                .child(Icon::new(Glyph::Close).xsmall())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.close_buffer(i, cx);
                                })),
                        )
                        // A middle click closes the tab, as in most editors.
                        .on_aux_click(cx.listener(
                            move |this, e: &ClickEvent, _, cx| {
                                if e.is_middle_click() {
                                    this.close_buffer(i, cx);
                                }
                            },
                        ))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.active = i;
                        this.diff_active = false;
                        this.remember_session();
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
                    .label("Allow latexmkrc")
                    .on_click(cx.listener(|this, _, _, cx| this.allow_rc(cx))),
            )
    }

    fn render_shell_escape_banner(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
                "This document runs programs while it builds (shell escape, for minted, svg, \
                 pythontex…). Allow it only if you trust where the project comes from.",
            ))
            .child(
                Button::new("allow-shell-escape")
                    .warning()
                    .label("Allow Shell Escape")
                    .on_click(
                        cx.listener(|this, _, window, cx| this.allow_shell_escape(window, cx)),
                    ),
            )
    }

    /// Editing is paused while the agent works.
    fn render_agent_banner(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let name = self
            .agent
            .read(cx)
            .agent_name()
            .unwrap_or_else(|| "The agent".into());
        h_flex()
            .gap_3()
            .px_3()
            .py_1p5()
            .text_sm()
            .bg(theme.primary.opacity(0.10))
            .border_b_1()
            .border_color(theme.border)
            .child(
                Icon::new(Glyph::Sparkles)
                    .xsmall()
                    .text_color(theme.primary),
            )
            .child(div().flex_1().text_color(theme.foreground).child(format!(
                "{name} is working: editing is paused until it is done."
            )))
            .child(
                Button::new("agent-banner-stop")
                    .outline()
                    .icon(Glyph::Stop)
                    .label("Stop")
                    .on_click(
                        cx.listener(|this, _, _, cx| this.agent.update(cx, |a, cx| a.cancel(cx))),
                    ),
            )
    }

    fn render_diagnostics(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let root_dir = self
            .root
            .as_ref()
            .and_then(|r| r.parent())
            .map(Path::to_path_buf);
        let problems = self.problems();
        let rows = problems
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
        let fixable = !empty && self.project_dir().is_some() && !self.agent_busy;

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
                    .when(fixable, |d| {
                        d.child(
                            Button::new("fix-with-agent")
                                .ghost()
                                .icon(Glyph::Sparkles)
                                .label("Fix with Agent")
                                .tooltip("Ask the coding agent to fix these problems")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.fix_with_agent(window, cx)
                                })),
                        )
                    })
                    .child(
                        Button::new("close-diag")
                            .ghost()
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
        let update = self.render_update(cx);
        let grammar = self.render_grammar(cx);
        let language =
            (self.settings.spell_check && !self.diff_active).then(|| self.render_language(cx));
        let theme = cx.theme();
        let cursor = self
            .buffers
            .get(self.active)
            .map(|b| b.editor.read(cx).cursor_position());
        let problems = self.problems();
        let errors = problems
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .count();
        let warnings = problems
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
        let vim_status = self
            .vim
            .as_ref()
            .filter(|_| !self.diff_active)
            .map(|v| (format!("-- {} --", v.mode().label()), v.status()));
        let words = (!self.word_counts.is_empty()).then(|| {
            let total = thousands(self.total_words().words());
            let selected = self
                .buffers
                .get(self.active)
                .and_then(|b| selection(&b.editor, cx))
                .map(|(_, _, text)| selected_words(&text));
            match selected {
                Some(n) => format!("{} of {total} words", thousands(n)),
                None => format!("{total} words"),
            }
        });
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

        // Clickable items, as big as the buttons next to them.
        let item = |id: &'static str| {
            h_flex()
                .id(id)
                .h_8()
                .px_2()
                .gap_1p5()
                .rounded(theme.radius)
                .cursor_pointer()
                .hover(|s| s.bg(theme.accent).text_color(theme.foreground))
        };
        h_flex()
            .h(px(36.))
            .px_1()
            .gap_1()
            .text_sm()
            .text_color(theme.muted_foreground)
            .bg(theme.status_bar)
            .border_t_1()
            .border_color(theme.status_bar_border)
            .child(
                item("problems")
                    .gap_3()
                    .child(
                        h_flex()
                            .gap_1()
                            .child(Icon::new(Glyph::Error).size_4())
                            .child(errors.to_string()),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .child(Icon::new(Glyph::Warning).size_4())
                            .child(warnings.to_string()),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_diagnostics = !this.show_diagnostics;
                        cx.notify();
                    })),
            )
            .when_some(cursor, |el, c| {
                el.child(
                    div()
                        .px_2()
                        .child(format!("Ln {}, Col {}", c.line + 1, c.character + 1)),
                )
            })
            .when_some(vim_status, |el, (mode, typed)| {
                el.child(
                    h_flex()
                        .px_2()
                        .gap_2()
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.primary)
                                .child(mode),
                        )
                        .child(
                            div()
                                .font_family(theme.mono_font_family.clone())
                                .child(typed),
                        ),
                )
            })
            .when_some(branch, |el, branch| {
                el.child(
                    item("git-status")
                        .child(Icon::new(Glyph::Branch).size_4())
                        .child(branch)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.show_sidebar(Sidebar::SourceControl, cx)
                        })),
                )
            })
            .child(div().flex_1())
            .when_some(words, |el, words| {
                el.child(
                    item("words").child(words).on_click(
                        cx.listener(|this, _, window, cx| this.show_word_count(window, cx)),
                    ),
                )
            })
            .children(update)
            .children(grammar)
            .children(language)
            .child(
                item("live")
                    .when(live, |d| d.text_color(theme.primary))
                    .child(if live { "● Live" } else { "○ Live" })
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_live(cx))),
            )
            .child(
                item("engine")
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
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.prompt_open(false, window, cx)
                            })),
                    )
                    .when(!OPEN_PICKS_FOLDERS, |d| {
                        d.child(
                            Button::new("open-folder")
                                .outline()
                                .icon(Glyph::Open)
                                .label("Open a folder…")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.prompt_open(true, window, cx)
                                })),
                        )
                    })
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
        if self.vim.is_some() {
            self.update_block(cx);
        }
        let theme = cx.theme().clone();

        let body = if self.buffers.is_empty() {
            self.render_welcome(cx).into_any_element()
        } else {
            let center = match self.diff.clone().filter(|_| self.diff_active) {
                Some(diff) => diff.into_any_element(),
                None => Editor::new(&self.buffers[self.active].editor)
                    .context_menu({
                        let b = &self.buffers[self.active];
                        let spelling = spell::context_menu(
                            b.editor.clone(),
                            b.spell.underlines.clone(),
                            b.spell.speller.clone(),
                        );
                        let editor = b.editor.clone();
                        let grammar = b.grammar.context_menu(b.editor.clone());
                        move |menu, window, cx| {
                            let selected = !editor.read(cx).selected_range().is_empty();
                            let mut menu = if selected { agent_menu(menu) } else { menu };
                            menu = grammar(menu, cx);
                            if let Some(name) = latex::package_at_cursor(&editor, cx) {
                                menu = menu
                                    .menu(
                                        format!("Documentation of {name}"),
                                        Box::new(PackageDocs { name }),
                                    )
                                    .separator();
                            }
                            spelling(menu, window, cx)
                        }
                    })
                    // The element sets the editor's mode on every frame: what
                    // `set_agent_busy` set would not last.
                    .readonly(self.agent_busy)
                    .bordered(false)
                    .h(relative(1.))
                    .font_family(theme.mono_font_family.clone())
                    .text_size(px(self.settings.font_size()))
                    .line_height(relative(1.6))
                    .into_any_element(),
            };
            let editor_column = v_flex()
                .size_full()
                .bg(theme.background)
                .when(self.untrusted_rc, |d| d.child(self.render_trust_banner(cx)))
                .when(self.wants_shell_escape, |d| {
                    d.child(self.render_shell_escape_banner(cx))
                })
                .when(self.agent_busy, |d| d.child(self.render_agent_banner(cx)))
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
                Sidebar::Search => div()
                    .size_full()
                    .bg(theme.sidebar)
                    .border_r_1()
                    .border_color(theme.sidebar_border)
                    .child(self.search.clone())
                    .into_any_element(),
                Sidebar::SourceControl => self.git.clone().into_any_element(),
                Sidebar::Agent => self.agent.clone().into_any_element(),
            };

            h_resizable("main")
                .child(
                    resizable_panel()
                        .size(px(320.))
                        .size_range(px(200.)..px(640.))
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
            .on_action(
                cx.listener(|this, _: &Open, window, cx| this.prompt_open(false, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &OpenFolder, window, cx| this.prompt_open(true, window, cx)),
            )
            .on_action(cx.listener(|this, _: &Save, window, cx| this.save(window, cx)))
            .on_action(
                cx.listener(|this, _: &FormatDocument, window, cx| {
                    this.format_document(window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &Build, _, cx| this.build(cx)))
            .on_action(cx.listener(|this, _: &CheckForUpdates, _, cx| {
                this.updater.update(cx, |u, cx| u.check(true, cx))
            }))
            .on_action(cx.listener(|this, _: &CancelBuild, _, _| this.cancel_build()))
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
            .on_action(cx.listener(|this, _: &crate::pdf_pane::FitPage, _, cx| {
                this.pdf.update(cx, |p, cx| p.fit_page(cx))
            }))
            .on_action(
                cx.listener(|this, _: &crate::pdf_pane::FindInPdf, window, cx| {
                    this.pdf.update(cx, |p, cx| p.open_find(window, cx))
                }),
            )
            .on_action(cx.listener(|this, _: &Present, _, cx| this.present(cx)))
            .on_action(
                cx.listener(|this, _: &AddCitation, window, cx| this.prompt_citation(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &crate::pdf_pane::GoToPage, window, cx| {
                    this.pdf.update(cx, |p, cx| p.prompt_page(window, cx))
                }),
            )
            .on_action(cx.listener(|this, _: &ZoomToFit, _, cx| {
                this.pdf.update(cx, |p, cx| p.zoom_to_fit(cx))
            }))
            .on_action(cx.listener(|this, _: &IncreaseFontSize, window, cx| {
                this.set_font_size(this.settings.font_size() + 1., window, cx)
            }))
            .on_action(cx.listener(|this, _: &DecreaseFontSize, window, cx| {
                this.set_font_size(this.settings.font_size() - 1., window, cx)
            }))
            .on_action(cx.listener(|this, _: &ResetFontSize, window, cx| {
                this.set_font_size(Settings::DEFAULT_FONT_SIZE, window, cx)
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
            .on_action(cx.listener(|this, _: &ToggleAgent, window, cx| {
                if this.show_sidebar && this.sidebar == Sidebar::Agent {
                    this.toggle_sidebar(Sidebar::Agent, cx);
                } else {
                    this.show_agent(window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &EditWithAgent, window, cx| {
                this.prompt_edit_with_agent(window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &FixWithAgent, window, cx| this.fix_with_agent(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &StopAgent, _, cx| {
                    this.agent.update(cx, |a, cx| a.cancel(cx))
                }),
            )
            .on_action(cx.listener(|this, a: &RewriteSelection, window, cx| {
                this.rewrite_selection(a.how.clone(), window, cx)
            }))
            .on_action(cx.listener(|this, a: &OpenLocation, window, cx| {
                this.reveal_range(a.file.clone(), a.range.clone(), window, cx)
            }))
            .on_action(cx.listener(|this, _: &ExportPdf, window, cx| this.export_pdf(window, cx)))
            .on_action(
                cx.listener(|this, _: &FindInProject, window, cx| this.find_in_project(window, cx)),
            )
            .on_action(cx.listener(|this, a: &PackageDocs, window, cx| {
                this.package_docs(a.name.clone(), window, cx)
            }))
            .on_action(cx.listener(|this, a: &OpenExternal, window, cx| {
                this.open_external(&a.path, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleVimMode, window, cx| {
                let on = !this.settings.vim_mode;
                this.update_settings(|s| s.vim_mode = on, window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &ToggleComment, window, cx| this.toggle_comment(window, cx)),
            )
            .on_action(cx.listener(|this, a: &ReplaceMisspelling, window, cx| {
                this.replace_misspelling(a, window, cx)
            }))
            .on_action(cx.listener(|this, a: &IgnoreGrammarRule, window, cx| {
                let rule = a.rule.clone();
                this.update_settings(
                    |s| {
                        if !s.grammar_rules_off.contains(&rule) {
                            s.grammar_rules_off.push(rule);
                        }
                    },
                    window,
                    cx,
                )
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

/// A LaTeX source (not a `.bib`, `.cls` is fine).
/// Replace a buffer's text with its formatted version as one undoable edit,
/// keeping the cursor on the same line.
fn apply_format(editor: &Entity<EditorState>, new: String, window: &mut Window, cx: &mut App) {
    editor.update(cx, |state, cx| {
        if *state.text() == new {
            return;
        }
        let cursor = state.cursor_position();
        let len = state.text().len();
        state.set_selected_range(0..len, cx);
        state.replace(new, window, cx);
        state.set_cursor_position(cursor, window, cx);
    });
}

/// A BGRA bitmap as a base64 PNG.
fn png_of(bitmap: pdf_view::Bitmap) -> Result<String, image::ImageError> {
    use base64::Engine as _;
    let mut rgba = bitmap.bgra;
    for px in rgba.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
    }
    let image = image::RgbaImage::from_raw(bitmap.width, bitmap.height, rgba).ok_or_else(|| {
        image::ImageError::Parameter(image::error::ParameterError::from_kind(
            image::error::ParameterErrorKind::DimensionMismatch,
        ))
    })?;
    let mut png = std::io::Cursor::new(Vec::new());
    image.write_to(&mut png, image::ImageFormat::Png)?;
    Ok(base64::engine::general_purpose::STANDARD.encode(png.into_inner()))
}

/// `12,345`.
fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The words of a selection, roughly: what has a letter or digit between
/// spaces, LaTeX commands left out.
fn selected_words(text: &str) -> usize {
    text.split_whitespace()
        .filter(|w| !w.starts_with('\\') && w.chars().any(char::is_alphanumeric))
        .count()
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// The editor's right-click menu gets the agent's edits when text is selected.
fn agent_menu(menu: NativeMenu) -> NativeMenu {
    let mut edits = NativeMenu::new();
    for how in Rewrite::PRESETS {
        let label = how.label().to_string();
        edits = edits.menu(label, Box::new(RewriteSelection { how }));
    }
    let edits = edits
        .separator()
        .menu("Edit with Instructions…", Box::new(EditWithAgent));
    menu.submenu("Agent", edits).separator()
}

/// The selection of `editor`: its first and last lines (1-based) and text.
fn selection(editor: &Entity<EditorState>, cx: &App) -> Option<(u32, u32, String)> {
    let state = editor.read(cx);
    let range = state.selected_range();
    if range.is_empty() {
        return None;
    }
    let text = state.text();
    let first = text.offset_to_position(range.start).line + 1;
    let end = text.offset_to_position(range.end);
    // A selection of whole lines ends at the start of the next one.
    let last = if end.character == 0 && end.line + 1 > first {
        end.line
    } else {
        end.line + 1
    };
    Some((first, last, text.slice(range).to_string()))
}

/// `main.tex, lines 4–9`.
fn lines_label(file: &Path, first: u32, last: u32) -> String {
    if first == last {
        format!("{}, line {first}", file.display())
    } else {
        format!("{}, lines {first}–{last}", file.display())
    }
}

/// A `file://` URI, with what must be escaped in a path escaped.
fn file_uri(path: &Path) -> String {
    let p = path.to_string_lossy().replace('\\', "/");
    // `/home/x`, `C:/x`, or `//server/share/x` (a Windows share: the server is the host).
    let mut out = String::from(if p.starts_with("//") {
        "file:"
    } else if p.starts_with('/') {
        "file://"
    } else {
        "file:///"
    });
    for b in p.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
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
    let meta = path.symlink_metadata()?;
    // A link to a folder (or a junction) is removed as a folder on Windows, and never what
    // it leads to.
    #[cfg(windows)]
    if std::os::windows::fs::FileTypeExt::is_symlink_dir(&meta.file_type()) {
        return std::fs::remove_dir(path);
    }
    if meta.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

/// Whether `name` can be a LaTeX package's: it goes to `texdoc` as an argument and into a
/// texdoc.org URL.
fn is_package_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with(['-', '.'])
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// How the last build went, for an agent's `problems` call.
fn run_summary(state: &CompileState) -> String {
    match state {
        CompileState::Idle => "The document has not been built yet.".to_string(),
        CompileState::Running => {
            "A build is running; the problems of the previous run:".to_string()
        }
        CompileState::Done { status, .. } => match status {
            Status::Success => "The last run succeeded.".to_string(),
            Status::Failed => "The last run failed.".to_string(),
            Status::Cancelled => "The last run was stopped before it finished.".to_string(),
        },
        CompileState::Failed(msg) => msg.to_string(),
    }
}

/// The document to open for the folder `dir`: the one opened last (`recent`, most recent
/// first), or else the most likely main file.
fn find_main_file(dir: &Path, recent: &[PathBuf]) -> Option<PathBuf> {
    let docs = tex_project::main_file::main_files(dir);
    recent
        .iter()
        .find(|r| docs.contains(r))
        .or(docs.first())
        .cloned()
}

/// Where to look for the PDFium shared library: next to the executable,
/// inside a macOS bundle, and the repository's `vendor/` for development.
fn pdfium_dirs() -> Vec<PathBuf> {
    let exe = std::env::current_exe().ok();
    pdfium_dirs_of(exe.as_deref(), dev_vendor_dir("pdfium"))
}

/// Where PDFium may be for the executable `exe`, then the checkout's `vendor` folder.
fn pdfium_dirs_of(exe: Option<&Path>, vendor: Option<PathBuf>) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(d) = exe.and_then(Path::parent) {
        dirs.push(d.to_path_buf());
        dirs.push(d.join("../Frameworks"));
        dirs.push(d.join("../Resources"));
        dirs.push(d.join("lib"));
        // deb / AppImage layout
        dirs.push(d.join("../lib/texoria"));
    }
    dirs.extend(vendor);
    dirs
}

/// `vendor/<name>` of the source checkout, but only for a binary built in it
/// (`cargo run`). The path is the build machine's: on a user's machine it may
/// be creatable by anyone, and a library or engine planted there would run.
fn dev_vendor_dir(name: &str) -> Option<PathBuf> {
    let repo = std::fs::canonicalize(concat!(env!("CARGO_MANIFEST_DIR"), "/../..")).ok()?;
    let exe = std::fs::canonicalize(std::env::current_exe().ok()?).ok()?;
    vendor_dir_of(&repo, &exe, name)
}

fn vendor_dir_of(repo: &Path, exe: &Path, name: &str) -> Option<PathBuf> {
    exe.starts_with(repo.join("target"))
        .then(|| repo.join("vendor").join(name))
}

/// What the other modules' tests look at.
#[cfg(test)]
impl Workspace {
    pub(crate) fn search_panel(&self) -> &Entity<SearchPanel> {
        &self.search
    }

    pub(crate) fn spell_mut(&mut self) -> &mut Spell {
        &mut self.spell
    }

    pub(crate) fn issues(&self) -> &[tex_project::check::Issue] {
        &self.issues
    }

    /// The editor, spelling and checks of the open file `path`.
    pub(crate) fn buffer_parts(
        &self,
        path: &Path,
    ) -> Option<(&Entity<EditorState>, &BufferSpell, &BufferChecks)> {
        let b = self.buffers.iter().find(|b| b.path == path)?;
        Some((&b.editor, &b.spell, &b.checks))
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod app_tests;
