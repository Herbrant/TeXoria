//! The main window: outline or files | editor | PDF, with live preview.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, RwLock, mpsc};
use std::time::Duration;

use gpui_kit::component::highlighter::{Diagnostic as EditorDiagnostic, DiagnosticSeverity};
use gpui_kit::component::input::{Editor, EditorState, InputEvent, Position, RopeExt, TabSize};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::native_menu::NativeMenu;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::resizable::{h_resizable, resizable_panel};
use gpui_kit::component::{
    ActiveTheme, Disableable as _, Icon, Sizable, TitleBar, WindowExt as _,
    button::{Button, ButtonVariant, ButtonVariants},
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use tex_agent::mcp::{self, ToolResult};
use tex_agent::{acp, prompt::Rewrite};
use tex_compile::{Compiler, Engine, Event as CompileEvent, Job, Severity, Status};
use tex_git::{Cancel, Credential, Git};
use tex_log::Diagnostic;
use tex_project::{Parsed, Project, ProjectOutlineItem, files};
use tex_spell::{Source, Speller, catalog};
use tex_syntax::OutlineKind;

use crate::agent_panel::{
    AgentOp, AgentPanel, AgentPanelEvent, Prompt, RewriteSelection, text_block,
};
use crate::agent_tools::{AppTools, ToolCall};
use crate::assets::Glyph;
use crate::atomic;
use crate::checks::{self, BufferChecks};
use crate::dialogs;
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
use crate::preview_window::{self, PreviewWindow};
use crate::search_panel::{SearchPanel, SearchPanelEvent};
use crate::settings::{Session, Settings, SpellLanguage, ThemeChoice};
use crate::settings_window::SettingsWindow;
use crate::spell::{self, AddToDictionary, BufferSpell, Dictionary, ReplaceMisspelling, Spell};
use crate::tasks::Background as _;
use crate::text::{plural, thousands};
use crate::theme::{self, ThemeExt};
use crate::ui;
use crate::updater::{self, UpdateState, Updater, UpdaterEvent};
use crate::{
    AddCitation, Build, CancelBuild, CheckForUpdates, CloneRepository, CloseTab, Commit,
    DecreaseFontSize, EditWithAgent, ExportPdf, Fetch, FindInProject, FixWithAgent, FormatDocument,
    ForwardSearch, IncreaseFontSize, NewBranch, Open, OpenFolder, OpenFromOverleaf, OpenSettings,
    Pull, Push, ResetFontSize, Save, ShowSourceControl, StopAgent, SyncOverleaf, ToggleAgent,
    ToggleComment, ToggleDiagnostics, ToggleFiles, ToggleLivePreview, ToggleOutline,
    TogglePreviewWindow, ToggleSoftWrap, ToggleTheme, ToggleVimMode, ZoomIn, ZoomOut, ZoomToFit,
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
    /// Text as last read from or written to disk, to know whether a save is needed (and
    /// whether the file changed on disk since).
    saved: String,
    dirty: bool,
    /// The file changed on disk while it had unsaved edits, and the user has not chosen
    /// which version to keep: it is not saved until then.
    conflict: bool,
    last_len: usize,
    /// The hash of the text the project last indexed from this buffer (`None`: not yet).
    indexed: Option<u64>,
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
    /// The tab bar's scroll position.
    tab_scroll: ScrollHandle,
    /// The `(active, diff_active, buffers.len())` the tab bar was last scrolled to.
    tab_shown: Option<(usize, bool, usize)>,
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
    /// The main window, for the question asked when a file changed on disk.
    window: AnyWindowHandle,
    /// That question is on screen.
    asking_conflict: bool,
    fixups: Rc<RefCell<Vec<CursorFixup>>>,
    focus: FocusHandle,
    was_dark: bool,
    settings_window: Option<AnyWindowHandle>,
    /// The PDF is in a window of its own, not next to the editor.
    preview_detached: bool,
    /// That window, once open.
    preview_window: Option<AnyWindowHandle>,
    preview_view: WeakEntity<PreviewWindow>,
    spell: Spell,
    grammar: Grammar,
    _grammar_start: Task<()>,
    /// Vim's state, when Vim mode is on.
    vim: Option<tex_vim::Vim>,
    updater: Entity<Updater>,
    _live: Task<()>,
    _checks: Task<()>,
    _count: Task<()>,
    _reindex: Task<()>,
    /// A re-index is parsing in the background; another was asked for meanwhile.
    reindexing: bool,
    reindex_again: bool,
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
                        this.reveal(file.clone(), line.saturating_sub(1), window, cx);
                        // From the PDF's own window: the editor's comes forward.
                        if this.preview_detached {
                            window.activate_window();
                        }
                    }
                    PdfPaneEvent::Export => this.export_pdf(window, cx),
                    PdfPaneEvent::ToggleWindow => this.toggle_preview_window(window, cx),
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
                    this.refresh_from_disk(window, cx);
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
            this.remember_preview_window(cx);
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
                weak.update(cx, |this, cx| this.remember_preview_window(cx))
                    .ok();
                // The Settings and preview windows belong to this one. Left open,
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
        let tool_task = cx.forward(tool_calls, |this, call, cx| this.on_tool_call(call, cx));

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
            tab_scroll: ScrollHandle::new(),
            tab_shown: None,
            cloning: None,
            show_diagnostics: false,
            untrusted_rc: false,
            wants_shell_escape: false,
            show_old_pdf: false,
            refused_close: false,
            window: window.window_handle(),
            asking_conflict: false,
            fixups: Rc::new(RefCell::new(Vec::new())),
            focus: cx.focus_handle(),
            was_dark: cx.theme_is_dark(),
            settings_window: None,
            preview_detached: false,
            preview_window: None,
            preview_view: WeakEntity::new_invalid(),
            spell,
            grammar: Grammar::default(),
            _grammar_start: Task::ready(()),
            vim: None,
            updater,
            _live: Task::ready(()),
            _checks: Task::ready(()),
            _count: Task::ready(()),
            _reindex: Task::ready(()),
            reindexing: false,
            reindex_again: false,
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

    /// The root file's name, for window titles.
    pub(crate) fn root_name(&self) -> Option<String> {
        let name = self.root.as_ref()?.file_name()?;
        Some(name.to_string_lossy().into_owned())
    }

    fn project_dir(&self) -> Option<PathBuf> {
        self.root
            .as_ref()
            .and_then(|r| r.parent())
            .map(Path::to_path_buf)
    }

    fn relative(&self, path: &Path) -> PathBuf {
        crate::paths::relative(path, self.project_dir().as_deref()).to_path_buf()
    }
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

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

/// A hash of `text`, to know whether it changed without keeping a copy.
fn text_hash(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

/// The words of a selection, roughly: what has a letter or digit between
/// spaces, LaTeX commands left out.
fn selected_words(text: &str) -> usize {
    text.split_whitespace()
        .filter(|w| !w.starts_with('\\') && w.chars().any(char::is_alphanumeric))
        .count()
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
mod agent;
mod buffers;
mod building;
mod chrome;
mod citations;
mod cloning;
mod config;
mod diffs;
mod editing;
mod grammar_checks;
mod indexing;
mod mcp_tools;
mod opening;
mod preview;
mod project_files;
mod project_search;
mod sidebar;
mod spelling;
mod tabs;
mod updates;
mod view;
mod welcome;

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
