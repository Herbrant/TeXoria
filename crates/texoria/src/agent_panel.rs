//! The Agent sidebar: a chat with a coding agent installed on the system
//! (Claude Code, Codex, Gemini CLI, …) through `tex-agent`.
//!
//! Agents write the project's files on their own, so the editors are
//! read-only during a turn (`AgentPanelEvent::Busy`) and what changed is found
//! by comparing the project with a checkpoint taken before it: the review and
//! the undo work the same whichever way the agent wrote. Operations that need
//! the buffers saved first are emitted as `AgentPanelEvent::Request`, like the
//! Git panel's.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::text::{TextView, TextViewState};
use gpui_kit::component::{
    ActiveTheme, Disableable as _, Icon, Sizable,
    button::{Button, ButtonVariants},
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use tex_agent::prompt::Rewrite;
use tex_agent::{
    Agent, AgentSpec, Change, ChangeKind, Checkpoint, CustomAgent, Event, Reply, RpcError, acp,
    checkpoint, prompt,
};
use tex_project::files;

use crate::assets::Glyph;
use crate::atomic;

/// How long a starting agent may take to answer (`npx` may download it).
const START_TIMEOUT: Duration = Duration::from_secs(180);
/// Tool output shown under a tool call, at most.
const DETAIL_CHARS: usize = 1500;

/// Ask the agent to rewrite the editor's selection.
#[derive(Clone, PartialEq, Action)]
#[action(namespace = texoria, no_json)]
pub struct RewriteSelection {
    pub how: Rewrite,
}

/// A message for the agent.
#[derive(Clone)]
pub struct Prompt {
    /// What the chat shows.
    pub display: SharedString,
    /// A note under it (the attached selection).
    pub caption: Option<SharedString>,
    pub blocks: Vec<acp::ContentBlock>,
}

impl Prompt {
    pub fn new(display: impl Into<SharedString>, text: impl Into<String>) -> Self {
        Self {
            display: display.into(),
            caption: None,
            blocks: vec![text_block(text)],
        }
    }
}

pub fn text_block(text: impl Into<String>) -> acp::ContentBlock {
    acp::ContentBlock::Text(acp::TextContent::new(text.into()))
}

#[derive(Clone)]
pub enum AgentOp {
    /// Typed in the chat: the workspace adds the editor's context.
    Chat(String),
    Send(Prompt),
    /// Put back the files (relative paths; all of them if `None`).
    Undo(Option<Vec<PathBuf>>),
}

pub enum AgentPanelEvent {
    /// Save the buffers, then call `AgentPanel::send` or `AgentPanel::undo`.
    Request(AgentOp),
    /// A turn started or ended: editing is paused meanwhile.
    Busy(bool),
    /// The agent may have changed files on disk.
    FilesChanged,
    /// The agent's changes as a patch (empty: none). `open` asks to show
    /// them; otherwise only a review already on screen follows.
    Review {
        patch: String,
        open: bool,
    },
    /// The user picked another agent.
    Choose(String),
    /// The user lets agents work in this project.
    Allow,
    FixErrors,
    Open {
        path: PathBuf,
        line: Option<u32>,
    },
}

impl EventEmitter<AgentPanelEvent> for AgentPanel {}

enum Entry {
    User {
        text: SharedString,
        caption: Option<SharedString>,
    },
    Agent(Entity<TextViewState>),
    Thought {
        text: String,
        open: bool,
    },
    Tool(ToolEntry),
    Plan(Vec<acp::PlanEntry>),
    Permission(PermissionEntry),
    Notice(SharedString),
    Error(SharedString),
}

struct ToolEntry {
    id: acp::ToolCallId,
    title: String,
    kind: acp::ToolKind,
    status: acp::ToolCallStatus,
    locations: Vec<acp::ToolCallLocation>,
    detail: Option<String>,
    open: bool,
}

struct PermissionEntry {
    title: String,
    options: Vec<acp::PermissionOption>,
    reply: Option<Reply<acp::RequestPermissionResponse>>,
    chosen: Option<String>,
}

enum Conn {
    Idle,
    Starting,
    Ready(Box<Live>),
    Failed {
        message: SharedString,
        details: Option<SharedString>,
    },
}

struct Live {
    agent: Arc<Agent>,
    init: acp::InitializeResponse,
    session: Option<Session>,
    _events: Task<()>,
}

struct Session {
    id: acp::SessionId,
    modes: Option<acp::SessionModeState>,
    /// The project's context was sent.
    primed: bool,
}

enum TurnError {
    /// The agent wants the user to sign in.
    Auth,
    Failed(String, Option<String>),
    /// The panel moved on (another agent, another project).
    Stale,
}

pub struct AgentPanel {
    dir: Option<PathBuf>,
    root: Option<PathBuf>,
    allowed: bool,
    /// `None` while looking for them.
    agents: Option<Vec<AgentSpec>>,
    preferred: Option<String>,
    custom: Vec<CustomAgent>,
    mcp: Vec<acp::McpServer>,
    conn: Conn,
    entries: Vec<Entry>,
    composer: Entity<TextareaState>,
    running: bool,
    turn: Task<()>,
    checkpoint: Option<Arc<Checkpoint>>,
    changes: Vec<Change>,
    /// Git configuration and hooks changed since the checkpoint, once told.
    git_changes: Vec<PathBuf>,
    /// Sign-in methods, while the agent asks for one.
    auth: Option<Vec<acp::AuthMethod>>,
    signing_in: bool,
    /// Sent again once signed in.
    pending: Option<Prompt>,
    /// Sent once the agents are found and allowed in the project.
    queued: Option<Prompt>,
    /// The agents were asked for (they are looked for on first use).
    wanted: bool,
    /// Errors of the last build, to offer fixing them.
    errors: usize,
    scroll: ScrollHandle,
    /// Bumped when the agent or the project changes: late results are dropped.
    generation: u64,
    _detect: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl AgentPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let composer = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Ask the agent…")
                .auto_grow(2, 10)
        });
        let subscriptions =
            vec![
                cx.subscribe_in(&composer, window, |this, _, ev: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { shift: false, .. } = ev {
                        this.submit(window, cx);
                    }
                }),
            ];
        AgentPanel {
            dir: None,
            root: None,
            allowed: false,
            agents: None,
            preferred: None,
            custom: Vec::new(),
            mcp: Vec::new(),
            conn: Conn::Idle,
            entries: Vec::new(),
            composer,
            running: false,
            turn: Task::ready(()),
            checkpoint: None,
            changes: Vec::new(),
            git_changes: Vec::new(),
            auth: None,
            signing_in: false,
            pending: None,
            queued: None,
            wanted: false,
            errors: 0,
            scroll: ScrollHandle::new(),
            generation: 0,
            _detect: Task::ready(()),
            _subscriptions: subscriptions,
        }
    }

    // ── Configuration ────────────────────────────────────────────────────

    /// The chosen agent and the user's own; looks for agents the first time.
    pub fn configure(
        &mut self,
        preferred: Option<String>,
        custom: Vec<CustomAgent>,
        cx: &mut Context<Self>,
    ) {
        let before = self.current().map(|a| a.id.clone());
        self.preferred = preferred;
        if custom != self.custom {
            self.custom = custom;
            if self.wanted {
                self.detect(cx);
            }
        }
        if self.agents.is_some() && self.current().map(|a| a.id.clone()) != before {
            self.reset(cx);
        }
        cx.notify();
    }

    /// The panel is used: look for the agents if not done yet.
    pub fn activate(&mut self, cx: &mut Context<Self>) {
        if !self.wanted {
            self.wanted = true;
            self.detect(cx);
        }
    }

    /// Look for installed agents (again).
    pub fn detect(&mut self, cx: &mut Context<Self>) {
        self.wanted = true;
        let custom = self.custom.clone();
        let before = self.current().map(|a| a.id.clone());
        self.agents = None;
        self._detect = cx.spawn(async move |this, cx| {
            let found = cx
                .background_spawn(async move { tex_agent::catalog::detect(&custom) })
                .await;
            this.update(cx, |this, cx| {
                this.agents = Some(found);
                if this.current().map(|a| a.id.clone()) != before {
                    this.reset(cx);
                }
                this.flush_queued(cx);
                cx.notify();
            })
            .ok();
        });
        cx.notify();
    }

    /// TeXoria's tools, offered to every session.
    pub fn set_mcp(&mut self, servers: Vec<acp::McpServer>) {
        self.mcp = servers;
    }

    /// The project's directory and root file, and whether agents may work in it.
    pub fn set_project(
        &mut self,
        dir: Option<PathBuf>,
        root: Option<PathBuf>,
        allowed: bool,
        cx: &mut Context<Self>,
    ) {
        if dir != self.dir {
            self.reset(cx);
            self.entries.clear();
            self.checkpoint = None;
            self.changes.clear();
            self.dir = dir;
        }
        self.root = root;
        self.allowed = allowed;
        if !allowed {
            self.queued = None;
        }
        self.flush_queued(cx);
        cx.notify();
    }

    /// Send what waited for the agents or the user's consent.
    fn flush_queued(&mut self, cx: &mut Context<Self>) {
        if self.can_send()
            && let Some(prompt) = self.queued.take()
        {
            self.send(prompt, cx);
        }
    }

    pub fn set_errors(&mut self, errors: usize, cx: &mut Context<Self>) {
        if errors != self.errors {
            self.errors = errors;
            cx.notify();
        }
    }

    /// The agent in use.
    pub fn current(&self) -> Option<&AgentSpec> {
        let agents = self.agents.as_ref()?;
        self.preferred
            .as_ref()
            .and_then(|id| agents.iter().find(|a| &a.id == id))
            .or(agents.first())
    }

    pub fn agent_name(&self) -> Option<SharedString> {
        self.current().map(|a| a.name.clone().into())
    }

    /// The installed agents, once looked for.
    pub fn agents(&self) -> Option<&[AgentSpec]> {
        self.agents.as_deref()
    }

    pub fn focus_composer(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.composer.update(cx, |c, cx| c.focus(window, cx));
    }

    /// Stop the agent: another one was chosen, or the project changed.
    fn reset(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        self.cancel_permissions();
        // Dropping the turn and the connection stops the agent's processes.
        self.turn = Task::ready(());
        self.conn = Conn::Idle;
        self.auth = None;
        self.signing_in = false;
        self.pending = None;
        self.queued = None;
        if !self.entries.is_empty() {
            self.entries.clear();
        }
        if self.running {
            self.running = false;
            cx.emit(AgentPanelEvent::Busy(false));
        }
        cx.notify();
    }

    /// Start over with a new session of the same agent.
    fn new_chat(&mut self, cx: &mut Context<Self>) {
        if self.running {
            return;
        }
        if let Conn::Ready(live) = &mut self.conn {
            live.session = None;
        }
        if matches!(self.conn, Conn::Failed { .. }) {
            self.conn = Conn::Idle;
        }
        self.entries.clear();
        self.auth = None;
        self.pending = None;
        cx.notify();
    }

    // ── Turns ────────────────────────────────────────────────────────────

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.composer.read(cx).value().trim().to_string();
        if text.is_empty() || !self.can_send() {
            return;
        }
        self.composer
            .update(cx, |c, cx| c.set_value("", window, cx));
        cx.emit(AgentPanelEvent::Request(AgentOp::Chat(text)));
    }

    fn can_send(&self) -> bool {
        self.allowed
            && !self.running
            && self.dir.is_some()
            && self.current().is_some()
            && !matches!(self.conn, Conn::Starting)
    }

    /// Start a turn (the buffers are saved).
    pub fn send(&mut self, prompt: Prompt, cx: &mut Context<Self>) {
        if !self.can_send() {
            // Not ready yet: the agents are being looked for, or the user
            // has to allow them first.
            let waiting = self.agents.is_none() || (!self.allowed && self.current().is_some());
            if waiting && !self.running && self.dir.is_some() {
                self.activate(cx);
                self.queued = Some(prompt);
                cx.notify();
            }
            return;
        }
        let (Some(dir), Some(spec)) = (self.dir.clone(), self.current().cloned()) else {
            return;
        };
        let root = self.root.clone();
        self.entries.push(Entry::User {
            text: prompt.display.clone(),
            caption: prompt.caption.clone(),
        });
        self.auth = None;
        self.running = true;
        cx.emit(AgentPanelEvent::Busy(true));
        self.scroll.scroll_to_bottom();
        cx.notify();

        let generation = self.generation;
        self.turn = cx.spawn(async move |this, cx| {
            let (agent, session, primed) =
                match Self::ensure_session(this.clone(), spec, dir.clone(), generation, cx).await {
                    Ok(ready) => ready,
                    Err(e) => {
                        this.update(cx, |this, cx| {
                            if this.generation != generation {
                                return;
                            }
                            match e {
                                TurnError::Auth => this.pending = Some(prompt),
                                TurnError::Failed(message, details) => {
                                    this.conn = Conn::Failed {
                                        message: message.into(),
                                        details: details.map(Into::into),
                                    }
                                }
                                TurnError::Stale => {}
                            }
                            this.end_turn(cx);
                        })
                        .ok();
                        return;
                    }
                };
            // The project as it is before the agent touches it.
            let snapshot_dir = dir.clone();
            let snapshot = cx
                .background_spawn(async move { Checkpoint::take(&snapshot_dir) })
                .await;
            let blocks = this.update(cx, |this, cx| {
                if this.generation != generation {
                    return None;
                }
                this.checkpoint = Some(Arc::new(snapshot));
                this.git_changes.clear();
                if !this.changes.is_empty() {
                    // The previous turn's changes are kept.
                    this.changes.clear();
                    cx.emit(AgentPanelEvent::Review {
                        patch: String::new(),
                        open: false,
                    });
                }
                let mut blocks = Vec::new();
                if !primed {
                    if let Conn::Ready(live) = &mut this.conn
                        && let Some(s) = &mut live.session
                    {
                        s.primed = true;
                    }
                    let root = root.clone().unwrap_or_else(|| dir.clone());
                    blocks.push(text_block(prompt::preamble(&root, &dir)));
                }
                blocks.extend(prompt.blocks.clone());
                Some(blocks)
            });
            let Ok(Some(blocks)) = blocks else { return };
            let result = agent.prompt(session, blocks).await;
            this.update(cx, |this, cx| {
                if this.generation == generation {
                    this.finish(result, cx);
                }
            })
            .ok();
        });
    }

    /// The agent running, with a session for the project.
    async fn ensure_session(
        this: WeakEntity<Self>,
        spec: AgentSpec,
        dir: PathBuf,
        generation: u64,
        cx: &mut AsyncApp,
    ) -> Result<(Arc<Agent>, acp::SessionId, bool), TurnError> {
        let stale = |_| TurnError::Stale;
        let existing = this
            .update(cx, |this, cx| match &this.conn {
                Conn::Ready(live) if live.agent.spec.id == spec.id => Some((
                    live.agent.clone(),
                    live.session.as_ref().map(|s| (s.id.clone(), s.primed)),
                    live.init.auth_methods.clone(),
                )),
                _ => {
                    this.conn = Conn::Starting;
                    cx.notify();
                    None
                }
            })
            .map_err(stale)?;
        let (agent, auth_methods) = match existing {
            Some((agent, Some((id, primed)), _)) => return Ok((agent, id, primed)),
            Some((agent, None, methods)) => (agent, methods),
            None => {
                let (tx, rx) = smol::channel::unbounded::<Event>();
                let agent = Agent::start(&spec, &dir, move |e| {
                    let _ = tx.send_blocking(e);
                })
                .map_err(|e| {
                    TurnError::Failed(
                        format!("{} could not be started: {e}", spec.name),
                        Some(spec.program.display().to_string()),
                    )
                })?;
                let agent = Arc::new(agent);
                let weak = this.clone();
                let events = cx.spawn(async move |cx| {
                    while let Ok(event) = rx.recv().await {
                        if weak
                            .update(cx, |this, cx| {
                                if this.generation == generation {
                                    this.on_event(event, cx)
                                }
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                });
                let timer = cx.background_executor().timer(START_TIMEOUT);
                let init = smol::future::or(async { Some(agent.initialize().await) }, async {
                    timer.await;
                    None
                })
                .await;
                let init = match init {
                    Some(Ok(init)) => init,
                    Some(Err(e)) => {
                        return Err(TurnError::Failed(
                            format!("{} did not start: {e}", spec.name),
                            Some(agent.stderr_tail()).filter(|s| !s.trim().is_empty()),
                        ));
                    }
                    None => {
                        return Err(TurnError::Failed(
                            format!("{} did not answer.", spec.name),
                            Some(agent.stderr_tail()).filter(|s| !s.trim().is_empty()),
                        ));
                    }
                };
                let methods = init.auth_methods.clone();
                this.update(cx, |this, cx| {
                    if this.generation == generation {
                        this.conn = Conn::Ready(Box::new(Live {
                            agent: agent.clone(),
                            init,
                            session: None,
                            _events: events,
                        }));
                        cx.notify();
                    }
                })
                .map_err(stale)?;
                (agent, methods)
            }
        };
        let mcp = this
            .read_with(cx, |this, _| this.mcp.clone())
            .map_err(stale)?;
        match agent.new_session(&dir, mcp).await {
            Ok(resp) => {
                let id = resp.session_id.clone();
                this.update(cx, |this, cx| {
                    if let Conn::Ready(live) = &mut this.conn {
                        live.session = Some(Session {
                            id: resp.session_id,
                            modes: resp.modes,
                            primed: false,
                        });
                    }
                    cx.notify();
                })
                .map_err(stale)?;
                Ok((agent, id, false))
            }
            Err(e) if e.code == RpcError::AUTH_REQUIRED => {
                this.update(cx, |this, cx| {
                    this.auth = Some(auth_methods);
                    cx.notify();
                })
                .map_err(stale)?;
                Err(TurnError::Auth)
            }
            Err(e) => Err(TurnError::Failed(
                format!("{} could not start a session: {e}", spec.name),
                Some(agent.stderr_tail()).filter(|s| !s.trim().is_empty()),
            )),
        }
    }

    fn finish(&mut self, result: Result<acp::PromptResponse, RpcError>, cx: &mut Context<Self>) {
        match result {
            Ok(r) => {
                let note = match r.stop_reason {
                    acp::StopReason::Cancelled => Some("Stopped."),
                    acp::StopReason::MaxTokens => Some("The agent reached its output limit."),
                    acp::StopReason::MaxTurnRequests => {
                        Some("The agent reached its limit of steps for one request.")
                    }
                    acp::StopReason::Refusal => Some("The agent refused to go on."),
                    _ => None,
                };
                if let Some(note) = note {
                    self.entries.push(Entry::Notice(note.into()));
                }
            }
            // The agent's exit is reported on its own.
            Err(e) if e.is_closed() => {}
            Err(e) => self.entries.push(Entry::Error(e.to_string().into())),
        }
        self.end_turn(cx);
    }

    fn end_turn(&mut self, cx: &mut Context<Self>) {
        self.cancel_permissions();
        if self.running {
            self.running = false;
            cx.emit(AgentPanelEvent::Busy(false));
        }
        self.refresh_changes(false, cx);
        cx.notify();
    }

    /// Stop the current turn.
    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        if !self.running {
            return;
        }
        self.cancel_permissions();
        match self.live_session() {
            Some((agent, s)) => agent.cancel(&s.id),
            // Still starting: give up on it.
            _ => {
                self.turn = Task::ready(());
                self.conn = Conn::Idle;
                self.end_turn(cx);
            }
        }
        cx.notify();
    }

    fn cancel_permissions(&mut self) {
        for e in &mut self.entries {
            if let Entry::Permission(p) = e
                && let Some(reply) = p.reply.take()
            {
                reply.ok(acp::RequestPermissionResponse::new(
                    acp::RequestPermissionOutcome::Cancelled,
                ));
                p.chosen = Some("Cancelled".into());
            }
        }
    }

    // ── Review ───────────────────────────────────────────────────────────

    /// Compare the project with the checkpoint again.
    fn refresh_changes(&mut self, open: bool, cx: &mut Context<Self>) {
        let Some(cp) = self.checkpoint.clone() else {
            return;
        };
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            let (changes, git) = cx
                .background_spawn(async move { (cp.changes(), cp.git_changes()) })
                .await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                if !git.is_empty() && git != this.git_changes {
                    let list: Vec<String> = git.iter().map(|p| p.display().to_string()).collect();
                    this.entries.push(Entry::Error(
                        format!(
                            "The agent changed Git's configuration or hooks, which can run \
                             programs on the next Git command. Check them before using Git \
                             here:\n{}",
                            list.join("\n")
                        )
                        .into(),
                    ));
                    this.git_changes = git;
                }
                let patch = checkpoint::patch(&changes);
                this.changes = changes;
                cx.emit(AgentPanelEvent::Review { patch, open });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Put files back as they were before the turn (the buffers are saved).
    pub fn undo(&mut self, paths: Option<Vec<PathBuf>>, cx: &mut Context<Self>) {
        let (Some(cp), Some(dir)) = (self.checkpoint.clone(), self.dir.clone()) else {
            return;
        };
        let mut failed = Vec::new();
        for c in &self.changes {
            if paths.as_ref().is_some_and(|p| !p.contains(&c.path)) || !c.restorable() {
                continue;
            }
            let abs = dir.join(&c.path);
            let result = match (c.kind, cp.original(&c.path)) {
                (ChangeKind::Added, _) => remove_added(&dir, &abs),
                (_, Some(bytes)) => restore(&dir, &abs, bytes),
                (_, None) => continue,
            };
            if let Err(e) = result {
                failed.push(format!("{}: {e}", c.path.display()));
            }
        }
        if !failed.is_empty() {
            self.entries.push(Entry::Error(
                format!("Some files could not be restored:\n{}", failed.join("\n")).into(),
            ));
        }
        cx.emit(AgentPanelEvent::FilesChanged);
        self.refresh_changes(false, cx);
    }

    /// Accept every change: it is no longer offered for review.
    pub fn keep(&mut self, cx: &mut Context<Self>) {
        if let Some(cp) = &mut self.checkpoint {
            let cp = Arc::make_mut(cp);
            for c in &self.changes {
                cp.keep(&c.path);
            }
        }
        self.changes.clear();
        cx.emit(AgentPanelEvent::Review {
            patch: String::new(),
            open: false,
        });
        cx.notify();
    }

    // ── Agent events ─────────────────────────────────────────────────────

    fn on_event(&mut self, event: Event, cx: &mut Context<Self>) {
        match event {
            Event::Update(n) => {
                let current = self
                    .live_session()
                    .is_some_and(|(_, s)| s.id == n.session_id);
                if current {
                    self.apply(n.update, cx);
                }
            }
            Event::Permission(req, reply) => {
                let title = req
                    .tool_call
                    .fields
                    .title
                    .clone()
                    .or_else(|| {
                        self.tool(&req.tool_call.tool_call_id)
                            .map(|t| t.title.clone())
                    })
                    .unwrap_or_else(|| "Use a tool".into());
                self.entries.push(Entry::Permission(PermissionEntry {
                    title,
                    options: req.options,
                    reply: Some(reply),
                    chosen: None,
                }));
                self.scroll.scroll_to_bottom();
            }
            Event::ReadFile(req, reply) => match self.dir.clone() {
                Some(dir) => cx
                    .background_spawn(async move { read_file(&dir, req, reply) })
                    .detach(),
                None => reply.err(RpcError::internal("No project is open")),
            },
            Event::WriteFile(req, reply) => self.write_file(req, reply, cx),
            Event::Exited(stderr) => {
                let name = self.agent_name().unwrap_or_else(|| "The agent".into());
                self.conn = Conn::Failed {
                    message: format!("{name} stopped.").into(),
                    details: Some(stderr.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .map(Into::into),
                };
                if self.running {
                    self.turn = Task::ready(());
                    self.end_turn(cx);
                }
            }
        }
        cx.notify();
    }

    fn apply(&mut self, update: acp::SessionUpdate, cx: &mut Context<Self>) {
        match update {
            acp::SessionUpdate::AgentMessageChunk(chunk) => {
                if let acp::ContentBlock::Text(t) = chunk.content {
                    match self.entries.last() {
                        Some(Entry::Agent(state)) => {
                            state.update(cx, |s, cx| s.push_str(&t.text, cx));
                        }
                        _ => {
                            let state = cx.new(|cx| TextViewState::markdown(&t.text, cx));
                            self.entries.push(Entry::Agent(state));
                        }
                    }
                    self.scroll.scroll_to_bottom();
                }
            }
            acp::SessionUpdate::AgentThoughtChunk(chunk) => {
                if let acp::ContentBlock::Text(t) = chunk.content {
                    match self.entries.last_mut() {
                        Some(Entry::Thought { text, .. }) => text.push_str(&t.text),
                        _ => self.entries.push(Entry::Thought {
                            text: t.text,
                            open: false,
                        }),
                    }
                }
            }
            acp::SessionUpdate::ToolCall(call) => {
                let detail = detail(&call.content);
                match self.tool_mut(&call.tool_call_id) {
                    Some(t) => {
                        t.title = call.title;
                        t.kind = call.kind;
                        t.status = call.status;
                        t.locations = call.locations;
                        t.detail = detail.or(t.detail.take());
                    }
                    None => self.entries.push(Entry::Tool(ToolEntry {
                        id: call.tool_call_id,
                        title: call.title,
                        kind: call.kind,
                        status: call.status,
                        locations: call.locations,
                        detail,
                        open: false,
                    })),
                }
                self.scroll.scroll_to_bottom();
            }
            acp::SessionUpdate::ToolCallUpdate(update) => {
                let f = update.fields;
                if self.tool(&update.tool_call_id).is_none() {
                    self.entries.push(Entry::Tool(ToolEntry {
                        id: update.tool_call_id.clone(),
                        title: String::new(),
                        kind: acp::ToolKind::Other,
                        status: acp::ToolCallStatus::Pending,
                        locations: Vec::new(),
                        detail: None,
                        open: false,
                    }));
                }
                let Some(t) = self.tool_mut(&update.tool_call_id) else {
                    return;
                };
                if let Some(title) = f.title {
                    t.title = title;
                }
                if let Some(kind) = f.kind {
                    t.kind = kind;
                }
                if let Some(locations) = f.locations {
                    t.locations = locations;
                }
                if let Some(content) = &f.content
                    && let Some(d) = detail(content)
                {
                    t.detail = Some(d);
                }
                if let Some(status) = f.status {
                    let done = matches!(
                        status,
                        acp::ToolCallStatus::Completed | acp::ToolCallStatus::Failed
                    );
                    let reads = matches!(
                        t.kind,
                        acp::ToolKind::Read
                            | acp::ToolKind::Search
                            | acp::ToolKind::Think
                            | acp::ToolKind::Fetch
                    );
                    t.status = status;
                    if done && !reads {
                        cx.emit(AgentPanelEvent::FilesChanged);
                    }
                }
            }
            acp::SessionUpdate::Plan(plan) => {
                let last_user = self
                    .entries
                    .iter()
                    .rposition(|e| matches!(e, Entry::User { .. }))
                    .unwrap_or(0);
                match self.entries[last_user..]
                    .iter_mut()
                    .find(|e| matches!(e, Entry::Plan(_)))
                {
                    Some(Entry::Plan(entries)) => *entries = plan.entries,
                    _ => self.entries.push(Entry::Plan(plan.entries)),
                }
            }
            acp::SessionUpdate::CurrentModeUpdate(m) => {
                if let Some(s) = self.session_mut()
                    && let Some(modes) = &mut s.modes
                {
                    modes.current_mode_id = m.current_mode_id;
                }
            }
            _ => {}
        }
    }

    /// The running agent and its session, if both exist.
    fn live_session(&self) -> Option<(&Arc<Agent>, &Session)> {
        match &self.conn {
            Conn::Ready(live) => live.session.as_ref().map(|s| (&live.agent, s)),
            _ => None,
        }
    }

    fn session_mut(&mut self) -> Option<&mut Session> {
        match &mut self.conn {
            Conn::Ready(live) => live.session.as_mut(),
            _ => None,
        }
    }

    fn tool(&self, id: &acp::ToolCallId) -> Option<&ToolEntry> {
        self.entries.iter().rev().find_map(|e| match e {
            Entry::Tool(t) if &t.id == id => Some(t),
            _ => None,
        })
    }

    fn tool_mut(&mut self, id: &acp::ToolCallId) -> Option<&mut ToolEntry> {
        self.entries.iter_mut().rev().find_map(|e| match e {
            Entry::Tool(t) if &t.id == id => Some(t),
            _ => None,
        })
    }

    fn write_file(
        &mut self,
        req: acp::WriteTextFileRequest,
        reply: Reply<acp::WriteTextFileResponse>,
        cx: &mut Context<Self>,
    ) {
        let Some(dir) = self.dir.clone() else {
            return reply.err(RpcError::internal("No project is open"));
        };
        if let Err(e) = writable(&dir, &req.path) {
            return reply.err(RpcError::internal(e));
        }
        let result = req
            .path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| atomic::write(&req.path, req.content.as_bytes()));
        match result {
            Ok(()) => {
                reply.ok(acp::WriteTextFileResponse::new());
                cx.emit(AgentPanelEvent::FilesChanged);
            }
            Err(e) => reply.err(RpcError::internal(e.to_string())),
        }
    }

    fn choose_permission(&mut self, ix: usize, option: usize, cx: &mut Context<Self>) {
        if let Some(Entry::Permission(p)) = self.entries.get_mut(ix)
            && let Some(o) = p.options.get(option).cloned()
            && let Some(reply) = p.reply.take()
        {
            reply.ok(acp::RequestPermissionResponse::new(
                acp::RequestPermissionOutcome::Selected(acp::SelectedPermissionOutcome::new(
                    o.option_id,
                )),
            ));
            p.chosen = Some(o.name);
            cx.notify();
        }
    }

    fn sign_in(&mut self, method: acp::AuthMethodId, cx: &mut Context<Self>) {
        let Conn::Ready(live) = &self.conn else {
            return;
        };
        let agent = live.agent.clone();
        let generation = self.generation;
        self.signing_in = true;
        cx.spawn(async move |this, cx| {
            let result = agent.authenticate(method).await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.signing_in = false;
                match result {
                    Ok(_) => {
                        this.auth = None;
                        if let Some(prompt) = this.pending.take() {
                            // The user message is shown again by `send`.
                            if let Some(ix) = this
                                .entries
                                .iter()
                                .rposition(|e| matches!(e, Entry::User { .. }))
                            {
                                this.entries.remove(ix);
                            }
                            this.send(prompt, cx);
                        }
                    }
                    Err(e) => this
                        .entries
                        .push(Entry::Error(format!("Signing in failed: {e}").into())),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn set_mode(&mut self, mode: acp::SessionModeId, cx: &mut Context<Self>) {
        let Some((agent, s)) = self.live_session() else {
            return;
        };
        let (agent, session) = (agent.clone(), s.id.clone());
        cx.spawn(async move |this, cx| {
            let result = agent.set_mode(session, mode.clone()).await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(_) => {
                        if let Some(s) = this.session_mut()
                            && let Some(modes) = &mut s.modes
                        {
                            modes.current_mode_id = mode;
                        }
                    }
                    Err(e) => this.entries.push(Entry::Error(
                        format!("The mode was not changed: {e}").into(),
                    )),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // ── Rendering ────────────────────────────────────────────────────────

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let agents = self.agents.clone().unwrap_or_default();
        let current = self.current().map(|a| a.id.clone());
        let label: SharedString = self.agent_name().unwrap_or_else(|| match self.agents {
            None => "Looking for agents…".into(),
            Some(_) => "No agent".into(),
        });
        let weak = cx.entity().downgrade();
        let picker = Button::new("agent-picker")
            .ghost()
            .icon(Glyph::Sparkles)
            .label(label)
            .disabled(agents.is_empty() || self.running)
            .dropdown_menu_with_anchor(Anchor::TopLeft, move |mut menu, _, _| {
                for a in &agents {
                    let (weak, id) = (weak.clone(), a.id.clone());
                    menu = menu.item(
                        PopupMenuItem::new(a.name.clone())
                            .checked(current.as_deref() == Some(a.id.as_str()))
                            .on_click(move |_, _, cx| {
                                let id = id.clone();
                                weak.update(cx, |_, cx| cx.emit(AgentPanelEvent::Choose(id)))
                                    .ok();
                            }),
                    );
                }
                let weak = weak.clone();
                menu.separator()
                    .item(
                        PopupMenuItem::new("Look for Agents Again").on_click(move |_, _, cx| {
                            weak.update(cx, |this, cx| this.detect(cx)).ok();
                        }),
                    )
            });
        let modes = self
            .live_session()
            .and_then(|(_, s)| s.modes.clone())
            .filter(|m| m.available_modes.len() > 1);
        let mode_picker = modes.map(|m| {
            let current = m
                .available_modes
                .iter()
                .find(|x| x.id == m.current_mode_id)
                .map(|x| x.name.clone())
                .unwrap_or_else(|| m.current_mode_id.to_string());
            let weak = cx.entity().downgrade();
            Button::new("agent-mode")
                .ghost()
                .label(current)
                .tooltip("How the agent works: what it may do without asking")
                .dropdown_menu_with_anchor(Anchor::TopRight, move |mut menu, _, _| {
                    for mode in &m.available_modes {
                        let (weak, id) = (weak.clone(), mode.id.clone());
                        menu = menu.item(
                            PopupMenuItem::new(mode.name.clone())
                                .checked(mode.id == m.current_mode_id)
                                .on_click(move |_, _, cx| {
                                    let id = id.clone();
                                    weak.update(cx, |this, cx| this.set_mode(id, cx)).ok();
                                }),
                        );
                    }
                    menu
                })
        });
        h_flex()
            .px_2()
            .pt_2()
            .pb_1()
            .gap_1()
            .border_b_1()
            .border_color(theme.sidebar_border)
            .child(div().flex_1().min_w_0().child(picker))
            .children(mode_picker)
            .child(
                Button::new("agent-new-chat")
                    .ghost()
                    .icon(Glyph::NewChat)
                    .tooltip("New chat")
                    .disabled(self.running || self.entries.is_empty())
                    .on_click(cx.listener(|this, _, _, cx| this.new_chat(cx))),
            )
    }

    /// Instead of the chat, when there is nothing to chat with.
    fn render_placeholder(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = cx.theme();
        let text = |t: SharedString| {
            div()
                .text_base()
                .text_color(theme.muted_foreground)
                .child(t)
        };
        if self.dir.is_none() {
            return Some(
                text("Open a document to work on it with an agent.".into()).into_any_element(),
            );
        }
        match &self.agents {
            None => return Some(text("Looking for agents…".into()).into_any_element()),
            Some(a) if a.is_empty() => {
                let names = tex_agent::catalog::KNOWN
                    .iter()
                    .map(|k| k.name)
                    .collect::<Vec<_>>()
                    .join(", ");
                return Some(
                    v_flex()
                        .gap_3()
                        .child(text("No coding agent was found on this computer.".into()))
                        .child(text(
                            format!(
                                "TeXoria works with the agents you already use: {names}. \
                                 Install one, sign in from a terminal, then look again."
                            )
                            .into(),
                        ))
                        .child(
                            h_flex().child(
                                Button::new("agent-detect")
                                    .icon(Glyph::Refresh)
                                    .label("Look Again")
                                    .on_click(cx.listener(|this, _, _, cx| this.detect(cx))),
                            ),
                        )
                        .into_any_element(),
                );
            }
            Some(_) => {}
        }
        if !self.allowed {
            let name = self.agent_name().unwrap_or_default();
            let dir = self
                .dir
                .as_ref()
                .map(|d| d.display().to_string())
                .unwrap_or_default();
            return Some(
                v_flex()
                    .gap_3()
                    .child(
                        div()
                            .text_base()
                            .font_weight(FontWeight::MEDIUM)
                            .child(format!("Let {name} work on this project?")),
                    )
                    .child(text(
                        format!(
                            "The agent reads and changes the files in {dir}, and can run \
                             commands (it asks first, depending on its mode). What it reads \
                             is sent to its provider. It also follows its own settings found \
                             in the project, which can run programs: only allow projects you \
                             trust."
                        )
                        .into(),
                    ))
                    .child(
                        h_flex().child(
                            Button::new("agent-allow")
                                .primary()
                                .label("Allow Agents")
                                .on_click(
                                    cx.listener(|_, _, _, cx| cx.emit(AgentPanelEvent::Allow)),
                                ),
                        ),
                    )
                    .into_any_element(),
            );
        }
        None
    }

    fn render_entry(&self, ix: usize, entry: &Entry, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        match entry {
            Entry::User { text, caption } => v_flex()
                .items_end()
                .gap_0p5()
                .child(
                    div()
                        .max_w(relative(0.92))
                        .px_2p5()
                        .py_1p5()
                        .rounded(theme.radius)
                        .bg(theme.sidebar_accent)
                        .text_base()
                        .text_color(theme.foreground)
                        .child(text.clone()),
                )
                .when_some(caption.clone(), |d, c| {
                    d.child(div().text_sm().text_color(theme.muted_foreground).child(c))
                })
                .into_any_element(),
            Entry::Agent(state) => div()
                .text_base()
                .text_color(theme.foreground)
                .child(TextView::new(state).selectable(true))
                .into_any_element(),
            Entry::Thought { text, open } => {
                let open = *open;
                v_flex()
                    .gap_1()
                    .child(
                        h_flex()
                            .id(("thought", ix))
                            .gap_1p5()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .cursor_pointer()
                            .hover(|s| s.text_color(theme.foreground))
                            .child(Icon::new(Glyph::Thought).small())
                            .child("Thinking")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(Entry::Thought { open, .. }) = this.entries.get_mut(ix)
                                {
                                    *open = !*open;
                                    cx.notify();
                                }
                            })),
                    )
                    .when(open, |d| {
                        d.child(
                            div()
                                .pl_4()
                                .text_sm()
                                .italic()
                                .text_color(theme.muted_foreground)
                                .child(text.clone()),
                        )
                    })
                    .into_any_element()
            }
            Entry::Tool(t) => self.render_tool(ix, t, cx),
            Entry::Plan(entries) => v_flex()
                .gap_1()
                .p_2()
                .rounded(theme.radius)
                .border_1()
                .border_color(theme.sidebar_border)
                .child(
                    h_flex()
                        .gap_1p5()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.muted_foreground)
                        .child(Icon::new(Glyph::Plan).small())
                        .child("Plan"),
                )
                .children(entries.iter().map(|e| {
                    let (mark, color) = match e.status {
                        acp::PlanEntryStatus::Completed => ("✓", theme.success),
                        acp::PlanEntryStatus::InProgress => ("•", theme.primary),
                        _ => ("○", theme.muted_foreground),
                    };
                    h_flex()
                        .gap_1p5()
                        .items_start()
                        .text_sm()
                        .child(div().text_color(color).child(mark))
                        .child(
                            div()
                                .flex_1()
                                .text_color(theme.foreground)
                                .when(e.status == acp::PlanEntryStatus::Completed, |d| {
                                    d.text_color(theme.muted_foreground)
                                })
                                .child(e.content.clone()),
                        )
                }))
                .into_any_element(),
            Entry::Permission(p) => {
                let pending = p.reply.is_some();
                v_flex()
                    .gap_2()
                    .p_2()
                    .rounded(theme.radius)
                    .border_1()
                    .border_color(if pending {
                        theme.warning
                    } else {
                        theme.sidebar_border
                    })
                    .child(
                        h_flex()
                            .gap_1p5()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(Icon::new(Glyph::Warning).small())
                            .child("Permission"),
                    )
                    .child(div().text_base().child(p.title.clone()))
                    .when(pending, |d| {
                        d.child(h_flex().flex_wrap().gap_1().children(
                            p.options.iter().enumerate().map(|(o, opt)| {
                                let allow = matches!(
                                    opt.kind,
                                    acp::PermissionOptionKind::AllowOnce
                                        | acp::PermissionOptionKind::AllowAlways
                                );
                                Button::new(SharedString::from(format!("perm-{ix}-{o}")))
                                    .when(allow, |b| b.primary())
                                    .when(!allow, |b| b.outline())
                                    .label(opt.name.clone())
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.choose_permission(ix, o, cx)
                                    }))
                            }),
                        ))
                    })
                    .when_some(p.chosen.clone(), |d, c| {
                        d.child(div().text_sm().text_color(theme.muted_foreground).child(c))
                    })
                    .into_any_element()
            }
            Entry::Notice(text) => div()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(text.clone())
                .into_any_element(),
            Entry::Error(text) => div()
                .text_sm()
                .text_color(theme.danger)
                .child(text.clone())
                .into_any_element(),
        }
    }

    fn render_tool(&self, ix: usize, t: &ToolEntry, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let icon = match t.kind {
            acp::ToolKind::Read => Glyph::Read,
            acp::ToolKind::Edit | acp::ToolKind::Move => Glyph::Edit,
            acp::ToolKind::Delete => Glyph::Close,
            acp::ToolKind::Search => Glyph::Search,
            acp::ToolKind::Execute => Glyph::Terminal,
            acp::ToolKind::Think => Glyph::Thought,
            acp::ToolKind::Fetch => Glyph::Web,
            _ => Glyph::Tool,
        };
        let (status, color) = match t.status {
            acp::ToolCallStatus::Completed => (Some(Glyph::Check), theme.success),
            acp::ToolCallStatus::Failed => (Some(Glyph::Error), theme.danger),
            _ => (Some(Glyph::Spinner), theme.muted_foreground),
        };
        let running = matches!(
            t.status,
            acp::ToolCallStatus::Pending | acp::ToolCallStatus::InProgress
        ) && self.running;
        let title: SharedString = if t.title.is_empty() {
            "Tool".into()
        } else {
            t.title.clone().into()
        };
        let location = t.locations.first().map(|l| {
            let rel = self
                .dir
                .as_ref()
                .and_then(|d| l.path.strip_prefix(d).ok())
                .unwrap_or(&l.path)
                .display()
                .to_string();
            let label = match l.line {
                Some(n) => format!("{rel}:{n}"),
                None => rel,
            };
            (label, l.path.clone(), l.line)
        });
        let open = t.open;
        let detail = t.detail.clone();
        v_flex()
            .gap_1()
            .child(
                h_flex()
                    .id(("tool", ix))
                    .gap_1p5()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .when(detail.is_some(), |d| {
                        d.cursor_pointer()
                            .hover(|s| s.text_color(theme.foreground))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(Entry::Tool(t)) = this.entries.get_mut(ix) {
                                    t.open = !t.open;
                                    cx.notify();
                                }
                            }))
                    })
                    .child(Icon::new(icon).small())
                    .child(div().flex_1().min_w_0().truncate().child(title))
                    .when_some(
                        status.filter(|_| running || t.status != acp::ToolCallStatus::Pending),
                        |d, s| {
                            let icon = Icon::new(s).small().text_color(color);
                            d.child(if running {
                                icon.with_animation(
                                    ("tool-spin", ix),
                                    Animation::new(Duration::from_millis(900)).repeat(),
                                    |icon, t| icon.transform(Transformation::rotate(percentage(t))),
                                )
                                .into_any_element()
                            } else {
                                icon.into_any_element()
                            })
                        },
                    ),
            )
            .when_some(location, |d, (label, path, line)| {
                d.child(
                    div()
                        .id(("tool-location", ix))
                        .pl_5()
                        .text_sm()
                        .text_color(theme.link)
                        .cursor_pointer()
                        .truncate()
                        .child(label)
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.emit(AgentPanelEvent::Open {
                                path: path.clone(),
                                line,
                            })
                        })),
                )
            })
            .when(open, |d| {
                d.when_some(detail, |d, text| {
                    d.child(
                        div()
                            .ml_5()
                            .p_1p5()
                            .rounded(theme.radius)
                            .bg(theme.muted)
                            .font_family(theme.mono_font_family.clone())
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(text),
                    )
                })
            })
            .into_any_element()
    }

    fn render_auth(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let methods = self.auth.as_ref()?;
        let theme = cx.theme();
        let name = self.agent_name().unwrap_or_default();
        let hint = self.current().and_then(|a| a.login_hint.clone());
        let buttons = methods.iter().enumerate().filter_map(|(i, m)| match m {
            acp::AuthMethod::Agent(a) => {
                let id = a.id.clone();
                Some(
                    Button::new(("agent-auth", i))
                        .primary()
                        .label(a.name.clone())
                        .when_some(a.description.clone(), |b, d| b.tooltip(d))
                        .disabled(self.signing_in)
                        .on_click(cx.listener(move |this, _, _, cx| this.sign_in(id.clone(), cx))),
                )
            }
            _ => None,
        });
        Some(
            v_flex()
                .gap_2()
                .p_2()
                .rounded(theme.radius)
                .border_1()
                .border_color(theme.warning)
                .child(
                    div()
                        .text_base()
                        .font_weight(FontWeight::MEDIUM)
                        .child(format!("Sign in to {name}")),
                )
                .when_some(hint, |d, h| {
                    d.child(div().text_sm().text_color(theme.muted_foreground).child(h))
                })
                .child(h_flex().flex_wrap().gap_1().children(buttons))
                .into_any_element(),
        )
    }

    fn render_failure(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let Conn::Failed { message, details } = &self.conn else {
            return None;
        };
        let theme = cx.theme();
        Some(
            v_flex()
                .gap_2()
                .p_2()
                .rounded(theme.radius)
                .border_1()
                .border_color(theme.danger)
                .child(
                    div()
                        .text_base()
                        .text_color(theme.danger)
                        .child(message.clone()),
                )
                .when_some(details.clone(), |d, details| {
                    d.child(
                        div()
                            .id("agent-failure-details")
                            .max_h(px(160.))
                            .overflow_y_scroll()
                            .font_family(theme.mono_font_family.clone())
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(details),
                    )
                })
                .child(
                    h_flex().child(Button::new("agent-retry").label("Start Again").on_click(
                        cx.listener(|this, _, _, cx| {
                            this.conn = Conn::Idle;
                            cx.notify();
                        }),
                    )),
                )
                .into_any_element(),
        )
    }

    fn render_review(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.running || self.changes.is_empty() {
            return None;
        }
        let theme = cx.theme();
        let n = self.changes.len();
        let undoable = self.changes.iter().any(Change::restorable);
        Some(
            h_flex()
                .px_2()
                .py_1p5()
                .gap_1()
                .border_t_1()
                .border_color(theme.sidebar_border)
                .child(
                    Icon::new(Glyph::Changes)
                        .small()
                        .text_color(theme.muted_foreground),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_sm()
                        .child(format!("{n} file{} changed", if n == 1 { "" } else { "s" })),
                )
                .child(
                    Button::new("agent-review")
                        .outline()
                        .label("Review")
                        .on_click(cx.listener(|this, _, _, cx| this.refresh_changes(true, cx))),
                )
                .child(
                    Button::new("agent-undo")
                        .outline()
                        .label("Undo")
                        .tooltip("Put the files back as they were before the agent's turn")
                        .disabled(!undoable)
                        .on_click(cx.listener(|_, _, _, cx| {
                            cx.emit(AgentPanelEvent::Request(AgentOp::Undo(None)))
                        })),
                )
                .child(
                    Button::new("agent-keep")
                        .primary()
                        .label("Keep")
                        .on_click(cx.listener(|this, _, _, cx| this.keep(cx))),
                )
                .into_any_element(),
        )
    }

    fn render_composer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let enabled = self.allowed && self.current().is_some() && self.dir.is_some();
        let running = self.running;
        v_flex()
            .p_2()
            .gap_1p5()
            .border_t_1()
            .border_color(theme.sidebar_border)
            .child(Textarea::new(&self.composer).small().disabled(!enabled))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child("↵ to send, ⇧↵ for a new line"),
                    )
                    .child(if running {
                        Button::new("agent-stop")
                            .warning()
                            .icon(Glyph::Stop)
                            .label("Stop")
                            .on_click(cx.listener(|this, _, _, cx| this.cancel(cx)))
                    } else {
                        Button::new("agent-send")
                            .primary()
                            .icon(Glyph::Send)
                            .label("Send")
                            .disabled(!self.can_send())
                            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx)))
                    }),
            )
    }

    fn render_intro(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let name = self.agent_name().unwrap_or_default();
        let errors = self.errors;
        v_flex()
            .gap_3()
            .child(
                div()
                    .text_base()
                    .text_color(theme.muted_foreground)
                    .child(format!(
                        "Ask {name} to write, rewrite or check parts of your document. To edit \
                         a passage, select it and right-click. You review what it changed \
                         when it is done."
                    )),
            )
            .when(errors > 0, |d| {
                d.child(
                    h_flex().child(
                        Button::new("agent-fix-intro")
                            .outline()
                            .icon(Glyph::Error)
                            .label(format!(
                                "Fix {errors} Build Error{}",
                                if errors == 1 { "" } else { "s" }
                            ))
                            .disabled(!self.can_send())
                            .on_click(
                                cx.listener(|_, _, _, cx| cx.emit(AgentPanelEvent::FixErrors)),
                            ),
                    ),
                )
            })
    }
}

impl Render for AgentPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let placeholder = self.render_placeholder(cx);
        let body = match placeholder {
            Some(p) => div().p_3().child(p).into_any_element(),
            None => {
                let entries: Vec<AnyElement> = self
                    .entries
                    .iter()
                    .enumerate()
                    .map(|(ix, e)| self.render_entry(ix, e, cx))
                    .collect();
                let starting = matches!(self.conn, Conn::Starting);
                v_flex()
                    .id("agent-chat")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .p_3()
                    .gap_3()
                    .when(entries.is_empty() && !self.running, |d| {
                        d.child(self.render_intro(cx))
                    })
                    .children(entries)
                    .children(self.render_auth(cx))
                    .children(self.render_failure(cx))
                    .when(self.running, |d| {
                        let label = if starting {
                            format!("Starting {}…", self.agent_name().unwrap_or_default())
                        } else {
                            "Working…".to_string()
                        };
                        d.child(
                            h_flex()
                                .gap_1p5()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child(Icon::new(Glyph::Spinner).small().with_animation(
                                    "agent-spin",
                                    Animation::new(Duration::from_millis(900)).repeat(),
                                    |icon, t| icon.transform(Transformation::rotate(percentage(t))),
                                ))
                                .child(label),
                        )
                    })
                    .into_any_element()
            }
        };
        let chat = self.allowed && self.dir.is_some() && self.current().is_some();
        v_flex()
            .size_full()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.sidebar_border)
            .child(self.render_header(cx))
            .child(div().flex_1().min_h_0().flex().flex_col().child(body))
            .children(self.render_review(cx))
            .when(chat, |d| d.child(self.render_composer(cx)))
    }
}

/// Files that run programs when building (latexmk's Perl rc files): an
/// agent's edits are often approved as edits, not as commands, so these are
/// left to the user.
const RUNS_PROGRAMS: [&str; 2] = ["latexmkrc", ".latexmkrc"];

/// Whether an agent may write `path` through TeXoria: inside the project,
/// not in `.git`, not a file that runs programs.
fn writable(dir: &Path, path: &Path) -> Result<(), String> {
    if !inside(dir, path) {
        return Err(format!(
            "TeXoria only lets agents write inside the project ({}), outside .git",
            dir.display()
        ));
    }
    // The file a link leads to is the one written.
    let resolved = std::fs::canonicalize(path).ok();
    for name in [Some(path), resolved.as_deref()]
        .into_iter()
        .flatten()
        .filter_map(Path::file_name)
    {
        if runs_programs(&name.to_string_lossy()) {
            return Err(format!(
                "TeXoria does not let agents write {}, which runs programs when building: \
                 ask the user to change it",
                name.to_string_lossy()
            ));
        }
    }
    Ok(())
}

/// Whether writing the file `name` writes one of [`RUNS_PROGRAMS`], also as Windows reads
/// names: without trailing dots and spaces, and `name:stream` for `name`.
fn runs_programs(name: &str) -> bool {
    let name = name.split(':').next().unwrap_or_default();
    let name = name.trim_end_matches(['.', ' ']);
    RUNS_PROGRAMS.iter().any(|n| name.eq_ignore_ascii_case(n))
}

/// Put `bytes` back at `abs`, a file of the project. A link the agent put
/// in its place is removed, not followed: it may point outside.
fn restore(dir: &Path, abs: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if abs
        .symlink_metadata()
        .is_ok_and(|m| m.file_type().is_symlink())
    {
        std::fs::remove_file(abs)?;
    }
    if !inside(dir, abs) {
        return Err(std::io::Error::other("it is no longer inside the project"));
    }
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent)?;
    }
    atomic::write(abs, bytes)
}

/// Remove `abs`, a file an agent added to the project in `dir`: a link itself, not what it
/// points to, and nothing through a folder that now leads out of the project.
fn remove_added(dir: &Path, abs: &Path) -> std::io::Result<()> {
    if !abs.parent().is_some_and(|parent| inside(dir, parent)) {
        return Err(std::io::Error::other("it is no longer inside the project"));
    }
    std::fs::remove_file(abs)
}

/// Whether `path` is inside `dir` (once resolved, for what exists of it),
/// and not in its `.git` (in any case: disks are often case-insensitive).
fn inside(dir: &Path, path: &Path) -> bool {
    if !path.is_absolute() {
        return false;
    }
    let mut existing = path;
    let mut rest = Vec::new();
    while !existing.exists() {
        match (existing.parent(), existing.file_name()) {
            (Some(p), Some(name)) => {
                rest.push(name.to_os_string());
                existing = p;
            }
            _ => return false,
        }
    }
    let Ok(mut resolved) = std::fs::canonicalize(existing) else {
        return false;
    };
    for part in rest.iter().rev() {
        if part == ".." {
            return false;
        }
        resolved.push(part);
    }
    let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    resolved.strip_prefix(&dir).is_ok_and(|rel| {
        !rel.components()
            .any(|c| c.as_os_str().eq_ignore_ascii_case(".git"))
    })
}

/// Agents read through TeXoria what is in the project, like they write.
fn read_file(dir: &Path, req: acp::ReadTextFileRequest, reply: Reply<acp::ReadTextFileResponse>) {
    match read_text(dir, &req.path, req.line, req.limit) {
        Ok(content) => reply.ok(acp::ReadTextFileResponse::new(content)),
        Err(e) => reply.err(RpcError::new(RpcError::NOT_FOUND, e)),
    }
}

/// The text of `path`, a file of the project in `dir`, from its 1-based `line` and at most
/// `limit` lines.
fn read_text(
    dir: &Path,
    path: &Path,
    line: Option<u32>,
    limit: Option<u32>,
) -> Result<String, String> {
    if !inside(dir, path) {
        return Err(format!(
            "TeXoria only lets agents read files of the project ({})",
            dir.display()
        ));
    }
    let bytes = match std::fs::metadata(path) {
        Ok(m) if m.is_file() && m.len() > files::MAX_TEXT_BYTES => Err(std::io::Error::other(
            format!("the file is too large ({} MB)", m.len() >> 20),
        )),
        Ok(m) if m.is_file() => std::fs::read(path),
        Ok(_) => Err(std::io::Error::other("not a regular file")),
        Err(e) => Err(e),
    }
    .map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&bytes);
    Ok(match (line, limit) {
        (None, None) => text.into_owned(),
        (line, limit) => {
            let skip = line.unwrap_or(1).saturating_sub(1) as usize;
            let take = limit.map_or(usize::MAX, |l| l as usize);
            text.split_inclusive('\n').skip(skip).take(take).collect()
        }
    })
}

/// What a tool produced, shortened for the chat.
fn detail(content: &[acp::ToolCallContent]) -> Option<String> {
    let mut out = String::new();
    for c in content {
        match c {
            acp::ToolCallContent::Content(c) => {
                if let acp::ContentBlock::Text(t) = &c.content {
                    out.push_str(t.text.trim());
                    out.push('\n');
                }
            }
            acp::ToolCallContent::Diff(d) => {
                let old = d.old_text.as_deref().unwrap_or("");
                let diff = similar::TextDiff::from_lines(old, &d.new_text);
                let (mut added, mut removed) = (0, 0);
                for change in diff.iter_all_changes() {
                    match change.tag() {
                        similar::ChangeTag::Insert => added += 1,
                        similar::ChangeTag::Delete => removed += 1,
                        similar::ChangeTag::Equal => {}
                    }
                }
                let name = d
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                out.push_str(&format!("{name}: +{added} −{removed}\n"));
            }
            _ => {}
        }
    }
    let out = out.trim_end();
    if out.is_empty() {
        return None;
    }
    let mut cut = out.len().min(DETAIL_CHARS);
    while !out.is_char_boundary(cut) {
        cut -= 1;
    }
    Some(if cut < out.len() {
        format!("{}…", &out[..cut])
    } else {
        out.to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::{detail, inside, read_text, writable};
    use tex_agent::acp;

    #[test]
    fn rc_files_are_refused_under_any_spelling() {
        let dir = tempfile::tempdir().unwrap();
        let d = std::fs::canonicalize(dir.path()).unwrap();
        // Windows drops trailing dots and spaces, and `name:stream` writes to `name`.
        for name in [
            "latexmkrc.",
            "latexmkrc ",
            "LatexMkRc. .",
            "latexmkrc::$DATA",
            ".latexmkrc:x",
        ] {
            assert!(writable(&d, &d.join(name)).is_err(), "{name:?}");
        }
        assert!(writable(&d, &d.join("latexmkrc.tex")).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn rc_files_are_refused_through_a_link() {
        let dir = tempfile::tempdir().unwrap();
        let d = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::write(d.join("latexmkrc"), "").unwrap();
        std::os::unix::fs::symlink(d.join("latexmkrc"), d.join("notes.tex")).unwrap();
        assert!(writable(&d, &d.join("notes.tex")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn undo_removes_added_files_only_inside_the_project() {
        use super::remove_added;
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let d = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::write(outside.path().join("new.tex"), "theirs").unwrap();
        // The folder of an added file replaced by a link that leads out of the project.
        std::os::unix::fs::symlink(outside.path(), d.join("ch")).unwrap();
        assert!(remove_added(&d, &d.join("ch/new.tex")).is_err());
        assert!(outside.path().join("new.tex").exists());

        // An added link is removed itself, never what it points to.
        std::os::unix::fs::symlink(outside.path().join("new.tex"), d.join("link.tex")).unwrap();
        remove_added(&d, &d.join("link.tex")).unwrap();
        assert!(outside.path().join("new.tex").exists());
        std::fs::write(d.join("added.tex"), "").unwrap();
        remove_added(&d, &d.join("added.tex")).unwrap();
        assert!(!d.join("added.tex").exists());
    }

    #[test]
    fn reads_only_project_text_and_slices_lines() {
        let dir = tempfile::tempdir().unwrap();
        let d = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::write(d.join("main.tex"), "one\ntwo\nthree\n").unwrap();
        let main = d.join("main.tex");
        assert_eq!(
            read_text(&d, &main, None, None).unwrap(),
            "one\ntwo\nthree\n"
        );
        assert_eq!(read_text(&d, &main, Some(2), None).unwrap(), "two\nthree\n");
        assert_eq!(read_text(&d, &main, Some(2), Some(1)).unwrap(), "two\n");
        // Lines count from 1; 0 reads from the start too.
        assert_eq!(read_text(&d, &main, Some(0), Some(1)).unwrap(), "one\n");
        assert_eq!(read_text(&d, &main, Some(9), None).unwrap(), "");
        assert!(read_text(&d, &d.join("missing.tex"), None, None).is_err());
        assert!(read_text(&d, &d, None, None).is_err(), "a folder");
        std::fs::create_dir(d.join(".git")).unwrap();
        std::fs::write(d.join(".git/config"), "secret").unwrap();
        assert!(read_text(&d, &d.join(".git/config"), None, None).is_err());
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("x.tex"), "theirs").unwrap();
        assert!(read_text(&d, &outside.path().join("x.tex"), None, None).is_err());
        // Not UTF-8: read anyway, with replacement characters.
        std::fs::write(d.join("latin1.tex"), b"caf\xe9").unwrap();
        assert_eq!(
            read_text(&d, &d.join("latin1.tex"), None, None).unwrap(),
            "caf\u{fffd}"
        );
    }

    #[test]
    fn large_files_are_not_read() {
        let dir = tempfile::tempdir().unwrap();
        let d = std::fs::canonicalize(dir.path()).unwrap();
        let big = std::fs::File::create(d.join("big.tex")).unwrap();
        big.set_len(tex_project::files::MAX_TEXT_BYTES + 1).unwrap();
        let e = read_text(&d, &d.join("big.tex"), None, None).unwrap_err();
        assert!(e.contains("too large"), "{e}");
    }

    #[test]
    fn writes_stay_in_the_project() {
        let dir = tempfile::tempdir().unwrap();
        let d = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::create_dir(d.join("ch")).unwrap();
        assert!(inside(&d, &d.join("main.tex")));
        assert!(inside(&d, &d.join("ch/new/intro.tex")));
        assert!(!inside(&d, &d.join("../elsewhere.tex")));
        assert!(!inside(&d, &d.join("ch/../../x.tex")));
        assert!(!inside(&d, &d.join(".git/config")));
        assert!(!inside(&d, &d.join(".GIT/hooks/pre-commit")));
        assert!(!inside(&d, &d.join("ch/.Git/config")));
        assert!(!inside(&d, std::path::Path::new("main.tex")));
        assert!(writable(&d, &d.join("ch/intro.tex")).is_ok());
        assert!(writable(&d, &d.join("latexmkrc")).is_err());
        assert!(writable(&d, &d.join("ch/.LatexMkRc")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn undo_replaces_links_instead_of_following_them() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let d = std::fs::canonicalize(dir.path()).unwrap();
        let target = outside.path().join("victim.txt");
        std::fs::write(&target, "theirs").unwrap();
        std::os::unix::fs::symlink(&target, d.join("main.tex")).unwrap();
        super::restore(&d, &d.join("main.tex"), b"mine").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"theirs");
        assert_eq!(std::fs::read(d.join("main.tex")).unwrap(), b"mine");
        assert!(!d.join("main.tex").symlink_metadata().unwrap().is_symlink());
    }

    #[test]
    fn tool_details_are_short() {
        let text = acp::ToolCallContent::Content(acp::Content::new(acp::ContentBlock::Text(
            acp::TextContent::new("x".repeat(5000)),
        )));
        let d = detail(&[text]).unwrap();
        assert!(d.chars().count() <= super::DETAIL_CHARS + 1);
        let diff = acp::ToolCallContent::Diff(
            acp::Diff::new("/p/main.tex", "a\nc\n").old_text(Some("a\nb\n".to_string())),
        );
        assert_eq!(detail(&[diff]).unwrap(), "main.tex: +1 −1");
        assert_eq!(detail(&[]), None);
    }
}
