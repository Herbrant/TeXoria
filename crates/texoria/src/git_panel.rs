//! The Source Control sidebar: changes, commit, sync, history and branches.
//!
//! git runs on the background executor. Operations that change the
//! repository go through the workspace first ([`GitPanelEvent::Request`]),
//! which saves open buffers, then come back to [`GitPanel::execute`]; one runs
//! at a time.

use std::cell::Cell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::dialog::{DialogAction, DialogClose, DialogFooter};
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{
    ActiveTheme, Disableable as _, Icon, Sizable, WindowExt as _,
    button::{Button, ButtonVariant, ButtonVariants},
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use tex_git::{
    Branch, Cancel, Change, Commit, CommitOptions, Credential, Entry, Error, Git, Operation, Repo,
    Status, SyncOutcome,
};

use crate::assets::Glyph;
use crate::diff_view::DiffSource;
use crate::file_tree::GitMark;
use crate::overleaf;

/// The message of the commit a Sync makes when the message box is empty.
const SYNC_MESSAGE: &str = "Update from TeXoria";
/// Files listed by name before a Sync; the rest are counted.
const SYNC_REVIEW_ROWS: usize = 12;

/// The files a Sync would add to or delete from the project. Reviewed by
/// the user first: once pushed, collaborators see them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncReview {
    added: Vec<String>,
    deleted: Vec<String>,
}

impl SyncReview {
    fn of(status: &Status) -> SyncReview {
        let mut review = SyncReview::default();
        for e in &status.entries {
            let sides = [e.index, e.worktree];
            if e.is_untracked() || sides.contains(&Change::Added) || sides.contains(&Change::Copied)
            {
                review.added.push(e.path.clone());
            }
            if sides.contains(&Change::Deleted) {
                review.deleted.push(e.path.clone());
            }
            if sides.contains(&Change::Renamed) {
                review.added.push(e.path.clone());
                review.deleted.extend(e.orig_path.clone());
            }
        }
        review.added.sort();
        review.added.dedup();
        review.deleted.sort();
        review.deleted.dedup();
        review
    }

    fn is_empty(&self) -> bool {
        self.added.is_empty() && self.deleted.is_empty()
    }

    /// Everything in `self` was reviewed in `seen`.
    fn within(&self, seen: &SyncReview) -> bool {
        self.added.iter().all(|p| seen.added.contains(p))
            && self.deleted.iter().all(|p| seen.deleted.contains(p))
    }
}

const HISTORY_PAGE: usize = 200;
/// Rows rendered per section; a repository full of untracked files would
/// otherwise build thousands of elements every frame.
const MAX_ROWS: usize = 500;

/// A change to the repository, run by [`GitPanel::execute`].
#[derive(Debug, Clone)]
pub enum GitOp {
    Stage(Vec<String>),
    Unstage(Vec<String>),
    Discard {
        tracked: Vec<String>,
        untracked: Vec<String>,
    },
    Commit {
        message: String,
        amend: bool,
        /// Stage everything first.
        all: bool,
    },
    Fetch,
    Pull,
    Push,
    /// Commit everything, pull with rebase and push (Overleaf projects).
    Sync {
        /// From the message box, else [`SYNC_MESSAGE`].
        message: Option<String>,
        /// The files the user agreed to add or delete.
        review: SyncReview,
    },
    Abort(Operation),
    ContinueRebase,
    Switch(Branch),
    CreateBranch(String),
    AddRemote {
        name: String,
        url: String,
    },
    SetIdentity {
        name: String,
        email: String,
        global: bool,
    },
    Init,
}

impl GitOp {
    fn label(&self) -> &'static str {
        match self {
            GitOp::Stage(_) => "Stage",
            GitOp::Unstage(_) => "Unstage",
            GitOp::Discard { .. } => "Discard",
            GitOp::Commit { .. } => "Commit",
            GitOp::Fetch => "Fetch",
            GitOp::Pull => "Pull",
            GitOp::Push => "Push",
            GitOp::Sync { .. } => "Sync",
            GitOp::Abort(_) => "Abort",
            GitOp::ContinueRebase => "Continue rebase",
            GitOp::Switch(_) => "Switch branch",
            GitOp::CreateBranch(_) => "Create branch",
            GitOp::AddRemote { .. } => "Add remote",
            GitOp::SetIdentity { .. } => "Save identity",
            GitOp::Init => "Initialize repository",
        }
    }

    fn is_network(&self) -> bool {
        matches!(
            self,
            GitOp::Fetch | GitOp::Pull | GitOp::Push | GitOp::Sync { .. }
        )
    }

    /// git may rewrite files on disk, so open buffers must be reloaded.
    fn touches_worktree(&self) -> bool {
        matches!(
            self,
            GitOp::Discard { .. }
                | GitOp::Pull
                | GitOp::Sync { .. }
                | GitOp::Abort(_)
                | GitOp::ContinueRebase
                | GitOp::Switch(_)
        )
    }
}

pub enum GitPanelEvent {
    /// Run this once open buffers are saved.
    Request(GitOp),
    OpenDiff(DiffSource),
    /// Files on disk may have changed.
    WorktreeChanged,
    /// A fresh status is available.
    StatusChanged,
    /// The user asked to trust the project (or created its repository).
    Trust,
}

impl EventEmitter<GitPanelEvent> for GitPanel {}

#[derive(Debug, Clone)]
enum RepoState {
    NoProject,
    Searching,
    NoGit,
    NotRepo,
    /// The repository's working tree, which the user has not trusted.
    Untrusted(PathBuf),
    Ready(Repo),
    Failed(SharedString),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum View {
    Changes,
    History,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    Conflicts,
    Staged,
    Changes,
}

pub struct GitPanel {
    /// Located lazily: `Some(None)` once searched for and not found.
    git: Option<Option<Git>>,
    project_dir: Option<PathBuf>,
    trusted: bool,
    state: RepoState,
    status: Status,
    branches: Vec<Branch>,
    remotes: Vec<String>,
    history: Vec<Commit>,
    history_loaded: bool,
    history_done: bool,
    /// The last status refresh failed (e.g. a lock held by another git).
    error: Option<SharedString>,
    view: View,
    message: Entity<TextareaState>,
    /// Text to put in the message box at the next render (needs a window).
    next_message: Option<String>,
    amend: bool,
    busy: Option<&'static str>,
    cancel: Option<Cancel>,
    /// An operation to retry once the user's identity (or a token) is set.
    pending: Option<GitOp>,
    /// The server, when the remote is an Overleaf project.
    overleaf: Option<String>,
    /// The token for `overleaf`, once read from the keychain.
    credential: Option<Credential>,
    /// `credential` was just typed in: saved once Overleaf accepts it.
    credential_unsaved: bool,
    overleaf_servers: Vec<String>,
    pull_rebase: bool,
    timeout: Option<Duration>,
    /// Bumped on project change: results for another project are dropped.
    generation: u64,
    refresh_generation: u64,
    _subscriptions: Vec<Subscription>,
}

impl GitPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let message = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Message (⌘↵ to commit)")
                .auto_grow(2, 8)
        });
        let subscriptions =
            vec![
                cx.subscribe_in(&message, window, |this, _, ev: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter {
                        secondary: true, ..
                    } = ev
                    {
                        this.commit(window, cx);
                    }
                }),
            ];
        GitPanel {
            git: None,
            project_dir: None,
            trusted: false,
            state: RepoState::NoProject,
            status: Status::default(),
            branches: Vec::new(),
            remotes: Vec::new(),
            history: Vec::new(),
            history_loaded: false,
            history_done: false,
            error: None,
            view: View::Changes,
            message,
            next_message: None,
            amend: false,
            busy: None,
            cancel: None,
            pending: None,
            overleaf: None,
            credential: None,
            credential_unsaved: false,
            overleaf_servers: Vec::new(),
            pull_rebase: false,
            timeout: None,
            generation: 0,
            refresh_generation: 0,
            _subscriptions: subscriptions,
        }
    }

    pub fn configure(
        &mut self,
        pull_rebase: bool,
        timeout: Option<Duration>,
        overleaf_servers: Vec<String>,
    ) {
        self.pull_rebase = pull_rebase;
        self.timeout = timeout;
        self.overleaf_servers = overleaf_servers;
    }

    pub fn repo(&self) -> Option<&Repo> {
        match &self.state {
            RepoState::Ready(r) => Some(r),
            _ => None,
        }
    }

    pub fn status(&self) -> &Status {
        &self.status
    }

    /// Files with changes, for a badge.
    pub fn change_count(&self) -> usize {
        self.status.entries.len()
    }

    /// Changed files by absolute path, for the Files view.
    pub fn file_marks(&self) -> HashMap<PathBuf, GitMark> {
        let Some(repo) = self.repo() else {
            return HashMap::new();
        };
        // Project paths are canonical.
        let root = crate::paths::canonical(repo.root());
        self.status
            .entries
            .iter()
            .map(|e| {
                let mark = if e.conflicted {
                    GitMark::Conflict
                } else if e.is_untracked()
                    || matches!(e.index, Change::Added | Change::Renamed | Change::Copied)
                {
                    GitMark::Added
                } else {
                    GitMark::Modified
                };
                (root.join(&e.path), mark)
            })
            .collect()
    }

    pub fn focus_message(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.message.update(cx, |s, cx| s.focus(window, cx));
    }

    /// Track the repository containing `dir` (the project directory).
    pub fn set_project(&mut self, dir: Option<PathBuf>, trusted: bool, cx: &mut Context<Self>) {
        if self.project_dir == dir
            && self.trusted == trusted
            && !matches!(self.state, RepoState::Failed(_) | RepoState::NotRepo)
        {
            self.refresh(cx);
            return;
        }
        self.project_dir = dir.clone();
        self.trusted = trusted;
        self.status = Status::default();
        self.branches.clear();
        self.remotes.clear();
        self.history.clear();
        self.history_loaded = false;
        self.error = None;
        self.overleaf = None;
        self.credential = None;
        self.credential_unsaved = false;
        self.generation += 1;
        let generation = self.generation;
        cx.notify();

        let Some(dir) = dir else {
            self.state = RepoState::NoProject;
            return;
        };
        // Cheap check first: no process (and no macOS developer-tools
        // prompt) for projects that are not in a repository.
        let Some(top) = repo_top(&dir) else {
            self.state = RepoState::NotRepo;
            return;
        };
        if !trusted {
            self.state = RepoState::Untrusted(top);
            return;
        }
        self.state = RepoState::Searching;
        let cached = self.git.clone();
        let timeout = self.timeout;
        cx.spawn(async move |this, cx| {
            let (git, found) = cx
                .background_spawn(async move {
                    let git = cached
                        .unwrap_or_else(Git::find)
                        .map(|g| g.network_timeout(timeout));
                    // Only the trusted working tree, never a repository above it.
                    let found = git.as_ref().map(|g| g.discover_at(&top));
                    (git, found)
                })
                .await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.git = Some(git);
                this.state = match found {
                    None => RepoState::NoGit,
                    Some(Ok(Some(repo))) => RepoState::Ready(repo),
                    Some(Ok(None)) => RepoState::NotRepo,
                    Some(Err(e)) => RepoState::Failed(e.to_string().into()),
                };
                this.refresh(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Re-read status, branches and (if HEAD moved) history.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.repo().cloned() else {
            return;
        };
        self.refresh_generation += 1;
        let (generation, refresh) = (self.generation, self.refresh_generation);
        let head = self.status.oid.clone();
        let history_loaded = self.history_loaded;
        let servers = self.overleaf_servers.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let status = repo.status()?;
                    let branches = repo.branches().unwrap_or_default();
                    let remotes = repo.remotes().unwrap_or_default();
                    let history = (!history_loaded || status.oid != head)
                        .then(|| repo.log(0, HISTORY_PAGE, None).unwrap_or_default());
                    let merge_message = repo.merge_message();
                    let overleaf = main_remote(&status, &remotes)
                        .and_then(|r| repo.remote_url(r).ok().flatten())
                        .and_then(|url| tex_git::overleaf::remote(&url, &servers))
                        .map(|p| p.server);
                    Ok::<_, Error>((status, branches, remotes, history, merge_message, overleaf))
                })
                .await;
            this.update(cx, |this, cx| {
                if this.generation != generation || this.refresh_generation != refresh {
                    return;
                }
                match result {
                    Ok((status, branches, remotes, history, merge_message, overleaf)) => {
                        this.set_overleaf(overleaf, cx);
                        // Offer git's merge message when a merge stops.
                        if status.operation == Operation::Merge
                            && this.status.operation != Operation::Merge
                        {
                            this.next_message = merge_message;
                        }
                        this.status = status;
                        this.branches = branches;
                        this.remotes = remotes;
                        if let Some(history) = history {
                            this.history_done = history.len() < HISTORY_PAGE;
                            this.history = history;
                            this.history_loaded = true;
                        }
                        this.error = None;
                        cx.emit(GitPanelEvent::StatusChanged);
                    }
                    Err(e) => this.error = Some(e.to_string().into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Track the Overleaf server of the remote, and read its token.
    fn set_overleaf(&mut self, server: Option<String>, cx: &mut Context<Self>) {
        if self.overleaf == server {
            return;
        }
        self.overleaf = server.clone();
        self.credential = None;
        self.credential_unsaved = false;
        let Some(server) = server else {
            return;
        };
        // Also for projects cloned outside TeXoria.
        if let Some(repo) = self.repo().cloned() {
            cx.background_spawn(async move {
                if let Err(e) = repo.exclude_local_files() {
                    log::warn!("{e}");
                }
            })
            .detach();
        }
        let token = overleaf::load_token(&server, cx);
        cx.spawn(async move |this, cx| {
            let token = token.await;
            this.update(cx, |this, cx| {
                if this.overleaf.as_ref() == Some(&server) {
                    this.credential = token.map(|t| overleaf::credential(&server, t));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Overleaf has a single branch: the one the project was cloned with.
    fn check_overleaf_branch(&self) -> Result<(), &'static str> {
        if self.overleaf.is_none() {
            return Ok(());
        }
        let tracked = main_remote(&self.status, &self.remotes).and_then(|remote| {
            self.status
                .upstream
                .as_deref()?
                .strip_prefix(remote)?
                .strip_prefix('/')
        });
        if matches!(tracked, Some("main" | "master")) {
            Ok(())
        } else {
            Err("Overleaf projects have a single branch: switch back to it to sync.")
        }
    }

    pub fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.repo().is_none() || self.overleaf.is_none() {
            return;
        }
        if let Err(msg) = self.check_overleaf_branch() {
            window.push_notification(Notification::info(msg), cx);
            return;
        }
        let typed = self.message.read(cx).value().trim().to_string();
        let message = (!typed.is_empty()).then_some(typed);
        let review = SyncReview::of(&self.status);
        if review.is_empty() {
            self.request(GitOp::Sync { message, review }, cx);
        } else {
            self.confirm_sync(message, review, window, cx);
        }
    }

    /// List the files a Sync would add or delete, and ask before going on.
    fn confirm_sync(
        &mut self,
        message: Option<String>,
        review: SyncReview,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let weak = cx.entity().downgrade();
        let body_review = review.clone();
        open_form(
            window,
            cx,
            "Sync with Overleaf?",
            "Sync",
            None,
            move |_, cx| render_sync_review(&body_review, cx),
            move |_, cx| {
                let op = GitOp::Sync {
                    message: message.clone(),
                    review: review.clone(),
                };
                weak.update(cx, |this, cx| this.request(op, cx)).ok();
                true
            },
        );
    }

    /// Ask for a new token, then retry `op` with it. The old one stays in
    /// the keychain until the new one works: Overleaf may have refused this
    /// project only.
    fn prompt_overleaf_token(&mut self, op: GitOp, window: &mut Window, cx: &mut Context<Self>) {
        let Some(server) = self.overleaf.clone() else {
            return;
        };
        let reason = if self.credential.is_some() {
            overleaf::TOKEN_REJECTED
        } else {
            overleaf::TOKEN_NEEDED
        };
        let weak = cx.entity().downgrade();
        let for_token = server.clone();
        overleaf::prompt_token(
            &server,
            reason,
            move |token, _, cx| {
                let credential = overleaf::credential(&for_token, token);
                let op = op.clone();
                weak.update(cx, |this, cx| {
                    if this.overleaf.as_ref() != Some(&credential.url) {
                        return;
                    }
                    this.credential = Some(credential);
                    this.credential_unsaved = true;
                    this.request(op, cx);
                })
                .ok();
            },
            window,
            cx,
        );
    }

    fn load_more_history(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.repo().cloned() else {
            return;
        };
        let (generation, skip) = (self.generation, self.history.len());
        cx.spawn(async move |this, cx| {
            let more = cx
                .background_spawn(async move { repo.log(skip, HISTORY_PAGE, None) })
                .await;
            this.update(cx, |this, cx| {
                if this.generation != generation || this.history.len() != skip {
                    return;
                }
                match more {
                    Ok(more) => {
                        this.history_done = more.len() < HISTORY_PAGE;
                        this.history.extend(more);
                    }
                    Err(e) => this.error = Some(e.to_string().into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn request(&mut self, op: GitOp, cx: &mut Context<Self>) {
        cx.emit(GitPanelEvent::Request(op));
    }

    /// Run `op` in the background (the workspace has saved open buffers).
    pub fn execute(&mut self, op: GitOp, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(busy) = self.busy {
            window.push_notification(Notification::info(format!("{busy} is still running")), cx);
            return;
        }
        let repo = self.repo().cloned();
        if repo.is_none() && !matches!(op, GitOp::Init) {
            return;
        }
        let cancel = Cancel::new();
        self.busy = Some(op.label());
        self.cancel = op.is_network().then(|| cancel.clone());
        let ctx = OpContext {
            repo,
            git: self.git.clone().flatten(),
            dir: self.project_dir.clone(),
            status: self.status.clone(),
            credential: self.credential.clone(),
            rebase: self.pull_rebase,
            timeout: self.timeout,
            cancel,
        };
        let job = op.clone();
        let task = cx.background_spawn(async move { run_op(job, ctx) });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| this.finish(op, result, window, cx))
                .ok();
        })
        .detach();
        cx.notify();
    }

    fn finish(
        &mut self,
        op: GitOp,
        result: tex_git::Result<Outcome>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.busy = None;
        self.cancel = None;
        if op.touches_worktree() {
            cx.emit(GitPanelEvent::WorktreeChanged);
        }
        if result.is_ok()
            && op.is_network()
            && self.credential_unsaved
            && let Some(c) = &self.credential
        {
            overleaf::save_token(&c.url, &c.password, window, cx);
            self.credential_unsaved = false;
        }
        match result {
            Ok(Outcome::Initialized(repo)) => {
                self.git = Some(Some(repo.git().clone()));
                self.state = RepoState::Ready(*repo);
                self.history_loaded = false;
                cx.emit(GitPanelEvent::Trust);
                window.push_notification(Notification::success("Repository created"), cx);
            }
            Ok(Outcome::Done) => match op {
                GitOp::Commit { .. } => {
                    self.message.update(cx, |s, cx| s.set_value("", window, cx));
                    self.amend = false;
                }
                GitOp::SetIdentity { .. } => {
                    if let Some(next) = self.pending.take() {
                        self.request(next, cx);
                    }
                }
                GitOp::Push => {
                    window.push_notification(Notification::success("Changes pushed"), cx)
                }
                GitOp::Pull => window.push_notification(Notification::success("Up to date"), cx),
                _ => {}
            },
            Ok(Outcome::Synced(done)) => {
                if done.committed
                    && matches!(
                        op,
                        GitOp::Sync {
                            message: Some(_),
                            ..
                        }
                    )
                {
                    self.message.update(cx, |s, cx| s.set_value("", window, cx));
                    self.amend = false;
                }
                let note = if done == SyncOutcome::default() {
                    Notification::info("Already up to date")
                } else {
                    Notification::success("Synced with Overleaf")
                };
                window.push_notification(note, cx);
            }
            Err(Error::NoIdentity) => {
                self.pending = Some(op);
                self.prompt_identity(window, cx);
            }
            Err(Error::Auth { .. }) if self.overleaf.is_some() && op.is_network() => {
                self.prompt_overleaf_token(op, window, cx);
            }
            Err(Error::Unreachable { .. } | Error::TimedOut(_))
                if matches!(op, GitOp::Sync { .. }) =>
            {
                window.push_notification(
                    Notification::warning(
                        "Your changes are kept on this computer: sync again when you are back \
                         online.",
                    )
                    .title("Overleaf is out of reach"),
                    cx,
                );
            }
            Err(Error::Cancelled) => {}
            Err(e) => {
                let stopped = self
                    .repo()
                    .map(|r| r.operation())
                    .filter(|o| *o != Operation::None);
                match (&op, stopped) {
                    (GitOp::Pull | GitOp::Sync { .. }, Some(o)) => window.push_notification(
                        Notification::warning(
                            "Resolve the conflicts below, then commit to finish.",
                        )
                        .title(format!("{} stopped on conflicts", o.label())),
                        cx,
                    ),
                    _ => notify_error(window, cx, &format!("{} failed", op.label()), &e),
                }
            }
        }
        self.refresh(cx);
        cx.notify();
    }

    /// Whether committing now would first stage everything (`Some(true)`),
    /// commit the index (`Some(false)`), or is not possible (`None`).
    fn commit_mode(&self) -> Option<bool> {
        if self.status.conflicts().next().is_some() {
            None
        } else if self.status.staged().next().is_some() {
            Some(false)
        } else if self.status.unstaged().next().is_some() {
            Some(true)
        } else if self.amend {
            Some(false)
        } else {
            None
        }
    }

    pub fn commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(all) = self.commit_mode() else {
            return;
        };
        let message = self.message.read(cx).value().to_string();
        if message.trim().is_empty() {
            self.focus_message(window, cx);
            window.push_notification(Notification::info("Write a commit message first"), cx);
            return;
        }
        self.request(
            GitOp::Commit {
                message,
                amend: self.amend,
                all,
            },
            cx,
        );
    }

    fn toggle_amend(&mut self, amend: bool, cx: &mut Context<Self>) {
        self.amend = amend;
        cx.notify();
        if !amend || !self.message.read(cx).value().trim().is_empty() {
            return;
        }
        // Amending usually starts from the previous message.
        let Some(repo) = self.repo().cloned() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let msg = cx
                .background_spawn(async move { repo.head_message() })
                .await;
            this.update(cx, |this, cx| {
                if let Ok(msg) = msg {
                    this.next_message = Some(msg);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    pub fn fetch(&mut self, cx: &mut Context<Self>) {
        if self.repo().is_some() {
            self.request(GitOp::Fetch, cx);
        }
    }

    pub fn pull(&mut self, cx: &mut Context<Self>) {
        if self.status.upstream.is_some() {
            self.request(GitOp::Pull, cx);
        }
    }

    pub fn push(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.repo().is_none() {
            return;
        }
        if self.remotes.is_empty() {
            self.prompt_remote(window, cx);
        } else if let Err(msg) = self.check_overleaf_branch() {
            window.push_notification(Notification::info(msg), cx);
        } else {
            self.request(GitOp::Push, cx);
        }
    }

    // ── Dialogs ──────────────────────────────────────────────────────────

    pub fn prompt_new_branch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.repo().is_none() {
            return;
        }
        if self.overleaf.is_some() {
            window.push_notification(
                Notification::info("Overleaf projects have a single branch"),
                cx,
            );
            return;
        }
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("draft-2"));
        let weak = cx.entity().downgrade();
        let focus = name.focus_handle(cx);
        let body = name.clone();
        open_form(
            window,
            cx,
            "New Branch",
            "Create",
            Some(focus),
            move |_, cx| {
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child("The branch starts from the current commit and is checked out."),
                    )
                    .child(Input::new(&body))
                    .into_any_element()
            },
            move |_, cx| {
                let name = name.read(cx).value().trim().to_string();
                if name.is_empty() {
                    return false;
                }
                weak.update(cx, |this, cx| this.request(GitOp::CreateBranch(name), cx))
                    .ok();
                true
            },
        );
    }

    fn prompt_remote(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder("origin");
            s.set_value("origin", window, cx);
            s
        });
        let url = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("https://git.overleaf.com/… or git@github.com:…")
        });
        let weak = cx.entity().downgrade();
        let focus = url.focus_handle(cx);
        let (body_name, body_url) = (name.clone(), url.clone());
        open_form(
            window,
            cx,
            "Add Remote",
            "Add",
            Some(focus),
            move |_, cx| {
                let muted = cx.theme().muted_foreground;
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .text_color(muted)
                            .child("Where to push this project: GitHub, GitLab, Overleaf…"),
                    )
                    .child(div().text_xs().text_color(muted).child("Name"))
                    .child(Input::new(&body_name))
                    .child(div().text_xs().text_color(muted).child("URL"))
                    .child(Input::new(&body_url))
                    .into_any_element()
            },
            move |_, cx| {
                let name = name.read(cx).value().trim().to_string();
                let url = url.read(cx).value().trim().to_string();
                if name.is_empty() || url.is_empty() {
                    return false;
                }
                weak.update(cx, |this, cx| {
                    this.request(GitOp::AddRemote { name, url }, cx)
                })
                .ok();
                true
            },
        );
    }

    fn prompt_identity(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("Your name"));
        let email = cx.new(|cx| InputState::new(window, cx).placeholder("you@example.com"));
        let global = Rc::new(Cell::new(true));
        let weak = cx.entity().downgrade();
        let focus = name.focus_handle(cx);
        let (body_name, body_email, body_global) = (name.clone(), email.clone(), global.clone());
        open_form(
            window,
            cx,
            "Who Is Committing?",
            "Save",
            Some(focus),
            move |_, cx| {
                let global = body_global.clone();
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child("Git records a name and an email address with every commit."),
                    )
                    .child(Input::new(&body_name))
                    .child(Input::new(&body_email))
                    .child(
                        Checkbox::new("identity-global")
                            .label("Use for all repositories")
                            .checked(global.get())
                            .on_click(move |checked, window, _| {
                                global.set(*checked);
                                window.refresh();
                            }),
                    )
                    .into_any_element()
            },
            move |_, cx| {
                let name = name.read(cx).value().trim().to_string();
                let email = email.read(cx).value().trim().to_string();
                if name.is_empty() || email.is_empty() {
                    return false;
                }
                let global = global.get();
                weak.update(cx, |this, cx| {
                    this.request(
                        GitOp::SetIdentity {
                            name,
                            email,
                            global,
                        },
                        cx,
                    )
                })
                .ok();
                true
            },
        );
    }

    fn confirm_discard(
        &mut self,
        entries: Vec<Entry>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if entries.is_empty() {
            return;
        }
        let (untracked, tracked): (Vec<_>, Vec<_>) =
            entries.into_iter().partition(Entry::is_untracked);
        let tracked: Vec<String> = tracked.into_iter().map(|e| e.path).collect();
        let untracked: Vec<String> = untracked.into_iter().map(|e| e.path).collect();
        let n = tracked.len() + untracked.len();
        let title: SharedString = match (tracked.first(), untracked.first()) {
            _ if n > 1 => format!("Discard changes to {n} files?").into(),
            (Some(p), _) => format!("Discard changes to {}?", file_name(p)).into(),
            (_, Some(p)) => format!("Delete {}?", file_name(p)).into(),
            _ => return,
        };
        let description: SharedString = if untracked.is_empty() {
            "The files go back to their last committed version. This cannot be undone.".into()
        } else {
            "Untracked files are deleted. This cannot be undone.".into()
        };
        let weak = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let (weak, tracked, untracked) = (weak.clone(), tracked.clone(), untracked.clone());
            alert
                .title(title.clone())
                .description(description.clone())
                .confirm()
                .ok_text("Discard")
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, _, cx| {
                    let op = GitOp::Discard {
                        tracked: tracked.clone(),
                        untracked: untracked.clone(),
                    };
                    weak.update(cx, |this, cx| this.request(op, cx)).ok();
                    true
                })
        });
    }

    // ── Rendering ────────────────────────────────────────────────────────

    fn render_message(&self, title: &'static str, body: impl Into<SharedString>, cx: &App) -> Div {
        let theme = cx.theme();
        v_flex()
            .gap_1()
            .px_4()
            .py_2()
            .child(
                div()
                    .text_base()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.foreground)
                    .child(title),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(body.into()),
            )
    }

    fn render_sync(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let st = &self.status;
        let busy = self.busy.is_some();
        let branch: SharedString = st
            .branch
            .clone()
            .or_else(|| {
                st.oid
                    .as_ref()
                    .map(|o| format!("{} (detached)", &o[..o.len().min(7)]))
            })
            .unwrap_or_else(|| "No branch".into())
            .into();
        let branches = self.branches.clone();
        let overleaf = self.overleaf.is_some();
        let weak = cx.entity().downgrade();
        let menu = Button::new("git-branch-menu")
            .ghost()
            .icon(Glyph::Branch)
            .label(branch)
            .dropdown_caret(true)
            .disabled(busy)
            .dropdown_menu(move |mut menu, _, _| {
                let locals: Vec<&Branch> = branches.iter().filter(|b| !b.remote).collect();
                for b in &locals {
                    let (weak, branch) = (weak.clone(), (*b).clone());
                    menu = menu.item(
                        PopupMenuItem::new(b.name.clone())
                            .checked(b.current)
                            .on_click(move |_, _, cx| {
                                if branch.current {
                                    return;
                                }
                                let op = GitOp::Switch(branch.clone());
                                weak.update(cx, |this, cx| this.request(op, cx)).ok();
                            }),
                    );
                }
                // Remote branches not checked out yet.
                let remotes: Vec<&Branch> = branches
                    .iter()
                    .filter(|b| b.remote && !locals.iter().any(|l| l.name == b.local_name()))
                    .collect();
                if !remotes.is_empty() {
                    menu = menu.separator().label("Remote branches");
                    for b in remotes {
                        let (weak, branch) = (weak.clone(), b.clone());
                        menu = menu.item(PopupMenuItem::new(b.name.clone()).on_click(
                            move |_, _, cx| {
                                let op = GitOp::Switch(branch.clone());
                                weak.update(cx, |this, cx| this.request(op, cx)).ok();
                            },
                        ));
                    }
                }
                if overleaf {
                    return menu;
                }
                let (new_branch, add_remote) = (weak.clone(), weak.clone());
                menu.separator()
                    .item(
                        PopupMenuItem::new("New Branch…").on_click(move |_, window, cx| {
                            new_branch
                                .update(cx, |this, cx| this.prompt_new_branch(window, cx))
                                .ok();
                        }),
                    )
                    .item(
                        PopupMenuItem::new("Add Remote…").on_click(move |_, window, cx| {
                            add_remote
                                .update(cx, |this, cx| this.prompt_remote(window, cx))
                                .ok();
                        }),
                    )
            });

        let has_upstream = st.upstream.is_some();
        let pull_label = if st.behind > 0 {
            st.behind.to_string()
        } else {
            String::new()
        };
        let push_label = match (has_upstream, self.remotes.is_empty()) {
            (_, true) => "Add Remote…".to_string(),
            (false, false) => "Publish".to_string(),
            (true, false) if st.ahead > 0 => st.ahead.to_string(),
            _ => String::new(),
        };
        let network_busy = self.cancel.is_some();

        h_flex()
            .px_2()
            .pb_2()
            .gap_0p5()
            .child(div().flex_1().min_w_0().overflow_hidden().child(menu))
            .when(network_busy, |d| {
                d.child(
                    Button::new("git-cancel")
                        .ghost()
                        .label("Cancel")
                        .on_click(cx.listener(|this, _, _, _| {
                            if let Some(c) = &this.cancel {
                                c.cancel();
                            }
                        })),
                )
            })
            .when(overleaf && !network_busy, |d| {
                d.child(
                    Button::new("git-sync")
                        .primary()
                        .icon(Glyph::Refresh)
                        .label("Sync")
                        .tooltip("Commit all changes, then pull from and push to Overleaf")
                        .disabled(
                            busy || st.operation != Operation::None
                                || st.conflicts().next().is_some(),
                        )
                        .on_click(cx.listener(|this, _, window, cx| this.sync(window, cx))),
                )
            })
            .when(!network_busy, |d| {
                d.child(
                    Button::new("git-fetch")
                        .ghost()
                        .icon(Glyph::Fetch)
                        .tooltip("Fetch")
                        .disabled(busy || self.remotes.is_empty())
                        .on_click(cx.listener(|this, _, _, cx| this.fetch(cx))),
                )
                .child(
                    Button::new("git-pull")
                        .ghost()
                        .icon(Glyph::Pull)
                        .when(!pull_label.is_empty(), |b| b.label(pull_label))
                        .tooltip(if has_upstream {
                            "Pull"
                        } else {
                            "Pull (the branch has no upstream yet)"
                        })
                        .disabled(busy || !has_upstream)
                        .on_click(cx.listener(|this, _, _, cx| this.pull(cx))),
                )
                .child(
                    Button::new("git-push")
                        .ghost()
                        .icon(Glyph::Push)
                        .when(!push_label.is_empty(), |b| b.label(push_label))
                        .tooltip("Push")
                        .disabled(busy)
                        .on_click(cx.listener(|this, _, window, cx| this.push(window, cx))),
                )
            })
            .text_color(theme.muted_foreground)
    }

    fn render_view_switch(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let tab = |id: &'static str, label: &'static str, view: View, cx: &mut Context<Self>| {
            let active = self.view == view;
            let theme = cx.theme();
            div()
                .id(id)
                .px_2()
                .py_0p5()
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
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.view = view;
                    cx.notify();
                }))
        };
        h_flex()
            .px_3()
            .pb_2()
            .gap_1()
            .child(tab("git-view-changes", "Changes", View::Changes, cx))
            .child(tab("git-view-history", "History", View::History, cx))
    }

    fn render_operation(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let op = self.status.operation;
        if op == Operation::None {
            return None;
        }
        let theme = cx.theme();
        let conflicts = self.status.conflicts().count();
        let text = if conflicts > 0 {
            format!(
                "{} in progress: {conflicts} conflict{} left.",
                op.label(),
                plural(conflicts)
            )
        } else if op == Operation::Rebase {
            "Rebase in progress: conflicts resolved.".to_string()
        } else {
            format!("{} in progress: commit to finish.", op.label())
        };
        Some(
            v_flex()
                .mx_2()
                .mb_2()
                .p_2()
                .gap_1p5()
                .rounded(theme.radius)
                .bg(theme.warning.opacity(0.12))
                .text_sm()
                .child(
                    h_flex()
                        .gap_1p5()
                        .child(Icon::new(Glyph::Conflict).small().text_color(theme.warning))
                        .child(div().text_color(theme.foreground).child(text)),
                )
                .child(
                    h_flex()
                        .gap_1()
                        .when(op == Operation::Rebase && conflicts == 0, |d| {
                            d.child(
                                Button::new("git-continue")
                                    .primary()
                                    .label("Continue")
                                    .disabled(self.busy.is_some())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.request(GitOp::ContinueRebase, cx)
                                    })),
                            )
                        })
                        .child(
                            Button::new("git-abort")
                                .outline()
                                .label(format!("Abort {}", op.label()))
                                .disabled(self.busy.is_some())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.request(GitOp::Abort(op), cx)
                                })),
                        ),
                ),
        )
    }

    fn render_commit_box(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let mode = self.commit_mode();
        let label = match mode {
            Some(true) => "Commit All",
            _ if self.amend => "Amend",
            _ => "Commit",
        };
        let tooltip = match mode {
            Some(true) => "Nothing is staged: stage every change and commit (⌘↵)",
            Some(false) => "Commit the staged changes (⌘↵)",
            None if self.status.conflicts().next().is_some() => "Resolve the conflicts first",
            None => "Nothing to commit",
        };
        let has_head = self.status.oid.is_some();
        v_flex()
            .px_2()
            .pb_2()
            .gap_1p5()
            .child(Textarea::new(&self.message))
            .child(
                h_flex()
                    .gap_2()
                    .when(has_head, |d| {
                        d.child(
                            Checkbox::new("git-amend")
                                .label("Amend")
                                .tooltip("Replace the last commit")
                                .checked(self.amend)
                                .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                    this.toggle_amend(*checked, cx)
                                })),
                        )
                    })
                    .child(div().flex_1())
                    .child(
                        Button::new("git-commit")
                            .primary()
                            .icon(Glyph::Check)
                            .label(label)
                            .tooltip(tooltip)
                            .loading(self.busy == Some("Commit"))
                            .disabled(mode.is_none() || self.busy.is_some())
                            .on_click(cx.listener(|this, _, window, cx| this.commit(window, cx))),
                    ),
            )
    }

    fn render_section(
        &self,
        section: Section,
        entries: Vec<&Entry>,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        if entries.is_empty() {
            return None;
        }
        let theme = cx.theme();
        let busy = self.busy.is_some();
        let title = match section {
            Section::Conflicts => "MERGE CONFLICTS",
            Section::Staged => "STAGED",
            Section::Changes => "CHANGES",
        };
        let all: Vec<Entry> = entries.iter().map(|e| (*e).clone()).collect();
        let paths: Vec<String> = all.iter().map(|e| e.path.clone()).collect();
        let total = entries.len();

        let mut header = h_flex()
            .px_3()
            .pt_2()
            .pb_1()
            .gap_1()
            .text_sm()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme.muted_foreground)
            .child(div().flex_1().child(format!("{title}  {total}")));
        match section {
            Section::Staged => {
                let paths = paths.clone();
                header = header.child(
                    Button::new("git-unstage-all")
                        .ghost()
                        .icon(Glyph::Minus)
                        .tooltip("Unstage all")
                        .disabled(busy)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.request(GitOp::Unstage(paths.clone()), cx)
                        })),
                );
            }
            Section::Changes => {
                let stage_paths = paths.clone();
                let discard = all.clone();
                header = header
                    .child(
                        Button::new("git-discard-all")
                            .ghost()
                            .icon(Glyph::Discard)
                            .tooltip("Discard all changes")
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.confirm_discard(discard.clone(), window, cx)
                            })),
                    )
                    .child(
                        Button::new("git-stage-all")
                            .ghost()
                            .icon(Glyph::Plus)
                            .tooltip("Stage all")
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.request(GitOp::Stage(stage_paths.clone()), cx)
                            })),
                    );
            }
            Section::Conflicts => {}
        }

        let rows = all
            .into_iter()
            .take(MAX_ROWS)
            .enumerate()
            .map(|(i, e)| self.render_entry(section, i, e, cx))
            .collect::<Vec<_>>();
        Some(
            v_flex()
                .child(header)
                .children(rows)
                .when(total > MAX_ROWS, |d| {
                    d.child(
                        div()
                            .px_4()
                            .py_1()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("… and {} more", total - MAX_ROWS)),
                    )
                }),
        )
    }

    fn render_entry(
        &self,
        section: Section,
        i: usize,
        entry: Entry,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let busy = self.busy.is_some();
        let change = match section {
            Section::Staged => entry.index,
            _ => entry.worktree,
        };
        let (letter, color) = if section == Section::Conflicts {
            ("!", theme.danger)
        } else {
            let color = match change {
                Change::Added | Change::Untracked => theme.success,
                Change::Deleted => theme.danger,
                Change::Renamed | Change::Copied => theme.info,
                _ => theme.warning,
            };
            (change.letter(), color)
        };
        let name = file_name(&entry.path).to_string();
        let dir = entry
            .path
            .rsplit_once('/')
            .map(|(d, _)| d.to_string())
            .unwrap_or_default();
        let deleted = change == Change::Deleted;
        let group: SharedString = format!("git-row-{section:?}-{i}").into();
        let source = match section {
            Section::Conflicts => DiffSource::Conflict {
                path: entry.path.clone(),
            },
            _ => DiffSource::Working {
                path: entry.path.clone(),
                staged: section == Section::Staged,
                untracked: entry.is_untracked(),
            },
        };

        let path = entry.path.clone();
        let mut actions = h_flex()
            .gap_0p5()
            .opacity(0.)
            .group_hover(group.clone(), |s| s.opacity(1.));
        match section {
            Section::Staged => {
                actions = actions.child(
                    Button::new(("git-unstage", i))
                        .ghost()
                        .icon(Glyph::Minus)
                        .tooltip("Unstage")
                        .disabled(busy)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.request(GitOp::Unstage(vec![path.clone()]), cx)
                        })),
                );
            }
            Section::Changes => {
                let stage = path.clone();
                let discard = entry.clone();
                actions = actions
                    .child(
                        Button::new(("git-discard", i))
                            .ghost()
                            .icon(Glyph::Discard)
                            .tooltip(if entry.is_untracked() {
                                "Delete"
                            } else {
                                "Discard changes"
                            })
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.confirm_discard(vec![discard.clone()], window, cx)
                            })),
                    )
                    .child(
                        Button::new(("git-stage", i))
                            .ghost()
                            .icon(Glyph::Plus)
                            .tooltip("Stage")
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.request(GitOp::Stage(vec![stage.clone()]), cx)
                            })),
                    );
            }
            Section::Conflicts => {
                actions = actions.child(
                    Button::new(("git-resolved", i))
                        .ghost()
                        .icon(Glyph::Check)
                        .tooltip("Mark as resolved")
                        .disabled(busy)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.request(GitOp::Stage(vec![path.clone()]), cx)
                        })),
                );
            }
        }

        h_flex()
            .id(group.clone())
            .group(group)
            .gap_1p5()
            .pl_4()
            .pr_2()
            .h(px(34.))
            .mx_1()
            .rounded(theme.radius)
            .text_base()
            .cursor_pointer()
            .text_color(theme.sidebar_foreground)
            .hover(|s| s.bg(theme.sidebar_accent))
            .child(
                Icon::new(Glyph::File)
                    .small()
                    .text_color(theme.muted_foreground.opacity(0.8)),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1p5()
                    .overflow_hidden()
                    .child(
                        div()
                            .flex_none()
                            .when(deleted, |d| d.line_through())
                            .child(name),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(dir),
                    ),
            )
            .child(actions)
            .child(
                div()
                    .w(px(12.))
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(color)
                    .child(letter),
            )
            .on_click(
                cx.listener(move |_, _, _, cx| cx.emit(GitPanelEvent::OpenDiff(source.clone()))),
            )
    }

    fn render_changes(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let conflicts: Vec<&Entry> = self.status.conflicts().collect();
        let staged: Vec<&Entry> = self.status.staged().collect();
        let changes: Vec<&Entry> = self.status.unstaged().collect();
        let clean = self.status.is_clean();
        let operation = self.render_operation(cx);
        let commit_box = self.render_commit_box(cx);
        let sections = [
            self.render_section(Section::Conflicts, conflicts, cx),
            self.render_section(Section::Staged, staged, cx),
            self.render_section(Section::Changes, changes, cx),
        ];
        v_flex()
            .id("git-changes")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .pb_4()
            .children(operation)
            .child(commit_box)
            .when(clean, |d| {
                d.child(self.render_message(
                    "No changes",
                    "Everything is committed. Edits show up here.",
                    cx,
                ))
            })
            .children(sections.into_iter().flatten())
    }

    fn render_history(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let rows = self.history.iter().enumerate().map(|(i, c)| {
            let commit = c.clone();
            let meta = format!("{} · {} · {}", c.short, c.author, relative_time(c.time));
            v_flex()
                .id(("git-commit", i))
                .px_3()
                .py_1()
                .mx_1()
                .rounded(theme.radius)
                .cursor_pointer()
                .hover(|s| s.bg(theme.sidebar_accent))
                .child(
                    div()
                        .text_base()
                        .truncate()
                        .text_color(theme.sidebar_foreground)
                        .child(c.subject.clone()),
                )
                .child(
                    div()
                        .text_sm()
                        .truncate()
                        .text_color(theme.muted_foreground)
                        .child(meta),
                )
                .on_click(cx.listener(move |_, _, _, cx| {
                    cx.emit(GitPanelEvent::OpenDiff(DiffSource::Commit(commit.clone())))
                }))
        });
        let empty = self.history.is_empty();
        let more = !self.history_done && !empty;
        v_flex()
            .id("git-history")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .pb_4()
            .when(empty, |d| {
                d.child(self.render_message(
                    "No commits yet",
                    "Commit your changes to start the history.",
                    cx,
                ))
            })
            .children(rows)
            .when(more, |d| {
                d.child(
                    div().px_3().pt_1().child(
                        Button::new("git-more")
                            .ghost()
                            .label("Load more")
                            .on_click(cx.listener(|this, _, _, cx| this.load_more_history(cx))),
                    ),
                )
            })
    }

    fn render_body(&self, cx: &mut Context<Self>) -> AnyElement {
        match &self.state {
            RepoState::NoProject => self
                .render_message("No project", "Open a document to see its changes.", cx)
                .into_any_element(),
            RepoState::Searching => self
                .render_message("Looking for a repository…", "", cx)
                .into_any_element(),
            RepoState::NoGit => self
                .render_message(
                    "Git is not installed",
                    "Install Git (on macOS: xcode-select --install) to track the history \
                     of this project.",
                    cx,
                )
                .into_any_element(),
            RepoState::NotRepo => v_flex()
                .child(self.render_message(
                    "No repository",
                    "Track every change to this project, go back to any version and \
                     sync it with GitHub or Overleaf.",
                    cx,
                ))
                .child(
                    div().px_4().pt_1().child(
                        Button::new("git-init")
                            .primary()
                            .icon(Glyph::Branch)
                            .label("Initialize Repository")
                            .loading(self.busy.is_some())
                            .on_click(cx.listener(|this, _, _, cx| this.request(GitOp::Init, cx))),
                    ),
                )
                .into_any_element(),
            RepoState::Untrusted(top) => v_flex()
                .child(self.render_message(
                    "Trust this repository to use Git",
                    format!(
                        "A repository's configuration can run programs, so Git stays off \
                         until you trust it. The repository is {}{}.",
                        top.display(),
                        if self.project_dir.as_deref() == Some(top.as_path()) {
                            ""
                        } else {
                            ", a folder above the document's"
                        }
                    ),
                    cx,
                ))
                .child(
                    div().px_4().pt_1().child(
                        Button::new("git-trust")
                            .warning()
                            .label("Trust Repository")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(GitPanelEvent::Trust))),
                    ),
                )
                .into_any_element(),
            RepoState::Failed(msg) => v_flex()
                .gap_2()
                .px_4()
                .py_2()
                .child(
                    div()
                        .text_base()
                        .text_color(cx.theme().danger)
                        .child(msg.clone()),
                )
                .child(
                    Button::new("git-retry")
                        .outline()
                        .label("Retry")
                        .on_click(cx.listener(|this, _, _, cx| {
                            let (dir, trusted) = (this.project_dir.clone(), this.trusted);
                            this.project_dir = None;
                            this.set_project(dir, trusted, cx);
                        })),
                )
                .into_any_element(),
            RepoState::Ready(_) => {
                let sync = self.render_sync(cx);
                let switch = self.render_view_switch(cx);
                let error = self.error.clone();
                let body = match self.view {
                    View::Changes => self.render_changes(cx).into_any_element(),
                    View::History => self.render_history(cx).into_any_element(),
                };
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .child(sync)
                    .child(switch)
                    .when_some(error, |d, e| {
                        d.child(
                            div()
                                .px_4()
                                .pb_2()
                                .text_sm()
                                .text_color(cx.theme().danger)
                                .child(e),
                        )
                    })
                    .child(body)
                    .into_any_element()
            }
        }
    }
}

impl Render for GitPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(msg) = self.next_message.take()
            && self.message.read(cx).value().trim().is_empty()
        {
            self.message
                .update(cx, |s, cx| s.set_value(msg, window, cx));
        }
        let theme = cx.theme();
        let ready = self.repo().is_some();
        let busy = self.busy;
        v_flex()
            .size_full()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.sidebar_border)
            .child(
                h_flex()
                    .px_4()
                    .pt_3()
                    .pb_2()
                    .gap_2()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.muted_foreground)
                    .child(div().flex_1().child("SOURCE CONTROL"))
                    .when_some(busy, |d, b| {
                        d.child(div().font_weight(FontWeight::NORMAL).child(format!("{b}…")))
                    })
                    .when(ready, |d| {
                        d.child(
                            Button::new("git-refresh")
                                .ghost()
                                .icon(Glyph::Refresh)
                                .tooltip("Refresh")
                                .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                        )
                    }),
            )
            .child(self.render_body(cx))
    }
}

struct OpContext {
    repo: Option<Repo>,
    git: Option<Git>,
    dir: Option<PathBuf>,
    status: Status,
    credential: Option<Credential>,
    rebase: bool,
    timeout: Option<Duration>,
    cancel: Cancel,
}

enum Outcome {
    Done,
    Initialized(Box<Repo>),
    Synced(SyncOutcome),
}

fn run_op(op: GitOp, ctx: OpContext) -> tex_git::Result<Outcome> {
    if let GitOp::Init = op {
        let git = ctx
            .git
            .or_else(Git::find)
            .ok_or_else(|| Error::Invalid("Git is not installed".into()))?
            .network_timeout(ctx.timeout);
        let dir = ctx
            .dir
            .ok_or_else(|| Error::Invalid("No project is open".into()))?;
        return Ok(Outcome::Initialized(Box::new(git.init(&dir)?)));
    }
    let repo = ctx
        .repo
        .ok_or_else(|| Error::Invalid("No repository".into()))?
        .with_credential(ctx.credential);
    match op {
        GitOp::Stage(paths) => repo.stage(&paths)?,
        GitOp::Unstage(paths) => repo.unstage(&paths, ctx.status.oid.is_some())?,
        GitOp::Discard { tracked, untracked } => {
            if !tracked.is_empty() {
                repo.discard(&tracked)?;
            }
            repo.delete_untracked(&untracked)?;
        }
        GitOp::Commit {
            message,
            amend,
            all,
        } => {
            if all {
                repo.stage(&[".".to_string()])?;
            }
            repo.commit(&message, CommitOptions { amend, all: false })?;
        }
        GitOp::Fetch => repo.fetch(&ctx.cancel)?,
        GitOp::Pull => repo.pull(ctx.rebase, &ctx.cancel)?,
        GitOp::Push => repo.push(&ctx.status, &ctx.cancel)?,
        GitOp::Sync { message, review } => {
            // Files may have come or gone since the user looked.
            if !SyncReview::of(&repo.status()?).within(&review) {
                return Err(Error::Invalid(
                    "Files were added or deleted in the meantime: Sync again to review them".into(),
                ));
            }
            let message = message.as_deref().unwrap_or(SYNC_MESSAGE);
            return Ok(Outcome::Synced(repo.sync(message, &ctx.cancel)?));
        }
        GitOp::Abort(op) => repo.abort(op)?,
        GitOp::ContinueRebase => repo.continue_rebase()?,
        GitOp::Switch(branch) => repo.switch(&branch)?,
        GitOp::CreateBranch(name) => repo.create_branch(&name)?,
        GitOp::AddRemote { name, url } => repo.add_remote(&name, &url)?,
        GitOp::SetIdentity {
            name,
            email,
            global,
        } => repo.set_identity(&name, &email, global)?,
        GitOp::Init => unreachable!(),
    }
    Ok(Outcome::Done)
}

fn render_sync_review(review: &SyncReview, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let rows = review
        .added
        .iter()
        .map(|p| ("+", p, theme.success))
        .chain(review.deleted.iter().map(|p| ("−", p, theme.danger)));
    let total = review.added.len() + review.deleted.len();
    let mut list = v_flex()
        .id("sync-review")
        .max_h(px(240.))
        .overflow_y_scroll()
        .p_2()
        .rounded(theme.radius)
        .bg(theme.muted)
        .font_family(theme.mono_font_family.clone())
        .text_xs();
    for (sign, path, color) in rows.take(SYNC_REVIEW_ROWS) {
        list = list.child(
            h_flex()
                .gap_2()
                .child(div().text_color(color).child(sign))
                .child(div().truncate().child(path.clone())),
        );
    }
    if total > SYNC_REVIEW_ROWS {
        list = list.child(
            div()
                .text_color(theme.muted_foreground)
                .child(format!("and {} more", total - SYNC_REVIEW_ROWS)),
        );
    }
    let summary = match (review.added.len(), review.deleted.len()) {
        (a, 0) => format!("{a} new file{} will be added", plural(a)),
        (0, d) => format!("{d} file{} will be deleted", plural(d)),
        (a, d) => format!(
            "{a} new file{} will be added and {d} file{} deleted",
            plural(a),
            plural(d)
        ),
    };
    v_flex()
        .gap_2()
        .child(div().text_sm().child(format!(
            "{summary} on Overleaf, for every collaborator of the project."
        )))
        .child(list)
        .into_any_element()
}

/// The remote the branch tracks, else `origin`, else the only one.
fn main_remote<'a>(status: &Status, remotes: &'a [String]) -> Option<&'a str> {
    let tracked = status.upstream.as_deref().and_then(|u| {
        remotes
            .iter()
            .filter(|r| {
                u.strip_prefix(r.as_str())
                    .is_some_and(|b| b.starts_with('/'))
            })
            .max_by_key(|r| r.len())
    });
    tracked
        .or_else(|| remotes.iter().find(|r| *r == "origin"))
        .or_else(|| (remotes.len() == 1).then(|| &remotes[0]))
        .map(String::as_str)
}

/// The working tree of the repository `dir` is in: the nearest of `dir` and
/// its parents with a `.git` (directory, or file for worktrees and
/// submodules). Found without running git, which must not run before the
/// user trusts that folder.
pub fn repo_top(dir: &Path) -> Option<PathBuf> {
    // A relative path would end with the empty one, the current folder's.
    if !dir.is_absolute() {
        return None;
    }
    dir.ancestors()
        .find(|d| d.join(".git").exists())
        .map(Path::to_path_buf)
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// A dialog with a body and Cancel / `ok` buttons; `on_ok` returns whether
/// to close it.
pub fn open_form(
    window: &mut Window,
    cx: &mut App,
    title: &'static str,
    ok: &'static str,
    focus: Option<FocusHandle>,
    body: impl Fn(&mut Window, &mut App) -> AnyElement + 'static,
    on_ok: impl Fn(&mut Window, &mut App) -> bool + 'static,
) {
    let on_ok = Rc::new(on_ok);
    let first = Cell::new(true);
    window.open_dialog(cx, move |dialog, window, cx| {
        if first.replace(false)
            && let Some(f) = &focus
        {
            f.focus(window, cx);
        }
        let on_ok = on_ok.clone();
        dialog
            .title(title)
            .w(px(440.))
            .child(body(window, cx))
            .footer(
                DialogFooter::new()
                    .child(DialogClose::new().trigger(|b| b.outline().label("Cancel")))
                    .child(DialogAction::new().child(Button::new("ok").primary().label(ok))),
            )
            .on_ok(move |_, window, cx| on_ok(window, cx))
    });
}

/// An error toast; git's full output is one click away.
pub fn notify_error(window: &mut Window, cx: &mut App, title: &str, err: &Error) {
    let mut note = Notification::error(err.to_string()).title(title.to_string());
    if let Some(details) = err
        .details()
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(|d| SharedString::from(d.to_string()))
    {
        note = note.action(move |_, _, _| {
            let details = details.clone();
            Button::new("git-details")
                .outline()
                .label("Details")
                .on_click(move |_, window, cx| show_details(window, cx, details.clone()))
        });
    }
    window.push_notification(note, cx);
}

fn show_details(window: &mut Window, cx: &mut App, text: SharedString) {
    window.open_dialog(cx, move |dialog, _, cx| {
        let theme = cx.theme();
        dialog.title("Git Output").w(px(600.)).child(
            div()
                .id("git-output")
                .max_h(px(360.))
                .overflow_y_scroll()
                .p_2()
                .rounded(theme.radius)
                .bg(theme.muted)
                .font_family(theme.mono_font_family.clone())
                .text_xs()
                .child(text.clone()),
        )
    });
}

/// "5 min ago", or a date for anything older than a week.
fn relative_time(ts: i64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(ts);
    let d = (now - ts).max(0);
    match d {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", d / 60),
        3600..86_400 => format!("{} h ago", d / 3600),
        86_400..604_800 => {
            let n = d / 86_400;
            format!("{n} day{} ago", if n == 1 { "" } else { "s" })
        }
        _ => {
            let (y, m, day) = civil_from_days(ts.div_euclid(86_400));
            format!("{y}-{m:02}-{day:02}")
        }
    }
}

/// Days since 1970-01-01 to a proleptic Gregorian date (H. Hinnant).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::{SyncReview, civil_from_days, main_remote, repo_top};
    use tex_git::{Change, Entry, Status};

    fn entry(path: &str, index: Change, worktree: Change) -> Entry {
        Entry {
            path: path.into(),
            orig_path: None,
            index,
            worktree,
            conflicted: false,
        }
    }

    #[test]
    fn sync_review_lists_new_and_deleted_files() {
        use Change::*;
        let mut renamed = entry("new.tex", Renamed, Unmodified);
        renamed.orig_path = Some("old.tex".into());
        let status = Status {
            entries: vec![
                entry("main.tex", Unmodified, Modified),
                entry("fig.png", Unmodified, Untracked),
                entry("staged.tex", Added, Unmodified),
                entry("gone.tex", Unmodified, Deleted),
                entry("removed.tex", Deleted, Unmodified),
                renamed,
            ],
            ..Default::default()
        };
        let review = SyncReview::of(&status);
        assert_eq!(review.added, ["fig.png", "new.tex", "staged.tex"]);
        assert_eq!(review.deleted, ["gone.tex", "old.tex", "removed.tex"]);

        let edits_only = Status {
            entries: vec![entry("main.tex", Modified, Modified)],
            ..Default::default()
        };
        assert!(SyncReview::of(&edits_only).is_empty());

        // A file that appeared after the review was not agreed to.
        assert!(SyncReview::of(&edits_only).within(&review));
        assert!(review.within(&review));
        assert!(!review.within(&SyncReview::default()));
    }

    #[test]
    fn picks_the_tracked_remote() {
        let remotes = |r: &[&str]| r.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let tracking = |u: &str| Status {
            upstream: Some(u.into()),
            ..Default::default()
        };
        let rs = remotes(&["origin", "overleaf", "over"]);
        assert_eq!(
            main_remote(&tracking("overleaf/main"), &rs),
            Some("overleaf")
        );
        assert_eq!(main_remote(&Status::default(), &rs), Some("origin"));
        let rs = remotes(&["overleaf"]);
        assert_eq!(main_remote(&Status::default(), &rs), Some("overleaf"));
        let rs = remotes(&["a", "b"]);
        assert_eq!(main_remote(&Status::default(), &rs), None);
    }

    #[test]
    fn dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
    }

    #[test]
    fn finds_enclosing_repo() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("paper/figures");
        std::fs::create_dir_all(&sub).unwrap();
        assert_eq!(repo_top(&sub), None);
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        assert_eq!(repo_top(&sub).as_deref(), Some(dir.path()));
        std::fs::write(
            dir.path().join("paper/.git"),
            "gitdir: ../.git/modules/paper\n",
        )
        .unwrap();
        assert_eq!(repo_top(&sub), Some(dir.path().join("paper")));
        // Tests run in crates/texoria, two folders below this repository's .git: a relative
        // path is never resolved against the current folder.
        assert_eq!(repo_top(std::path::Path::new("../..")), None);
        assert_eq!(repo_top(std::path::Path::new("")), None);
    }
}
