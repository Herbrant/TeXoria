//! The coding agent: its panel, its turns and the edits asked of it.

use super::*;

impl Workspace {
    /// Point the Agent panel at the current project.
    pub(super) fn sync_agent(&mut self, cx: &mut Context<Self>) {
        let dir = self.project_dir();
        let allowed = dir.as_ref().is_some_and(|d| self.settings.allows_agents(d));
        let root = self.root.clone();
        self.agent
            .update(cx, |a, cx| a.set_project(dir, root, allowed, cx));
    }

    pub(super) fn on_agent_event(
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
                self.refresh_from_disk(window, cx);
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
    pub(super) fn run_agent_op(
        &mut self,
        op: AgentOp,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
    pub(super) fn chat_prompt(&self, text: String, cx: &App) -> Prompt {
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
    pub(super) fn tool_path(&self, path: &Path) -> String {
        match self.project_dir() {
            Some(dir) => tex_project::source_path(&dir, path),
            None => path.display().to_string(),
        }
    }

    pub(super) fn set_agent_busy(
        &mut self,
        busy: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
            self.refresh_from_disk(window, cx);
        }
        cx.notify();
    }

    pub(super) fn show_agent_review(
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
    pub(super) fn rewrite_selection(
        &mut self,
        how: Rewrite,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
    pub(super) fn prompt_edit_with_agent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let has_selection = self
            .buffers
            .get(self.active)
            .is_some_and(|b| selection(&b.editor, cx).is_some());
        if self.diff_active || !has_selection {
            window.push_notification(Notification::info("Select the passage to edit first."), cx);
            return;
        }
        let weak = cx.entity().downgrade();
        dialogs::prompt_text(
            window,
            cx,
            "Edit with the Agent",
            "Edit",
            "Translate into English, add an example…",
            "What should the agent do with the selected passage?",
            move |text, window, cx| {
                weak.update(cx, |this, cx| {
                    this.rewrite_selection(Rewrite::Custom(text), window, cx)
                })
                .ok();
                true
            },
        );
    }

    /// Ask the agent to fix the last run's errors (or warnings).
    pub(super) fn fix_with_agent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    /// ⇧⌘K: hide the Agent panel if it shows, else show it.
    pub(super) fn toggle_agent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.show_sidebar && self.sidebar == Sidebar::Agent {
            self.toggle_sidebar(Sidebar::Agent, cx);
        } else {
            self.show_agent(window, cx);
        }
    }

    pub(super) fn show_agent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_sidebar(Sidebar::Agent, cx);
        self.agent.update(cx, |a, cx| a.focus_composer(window, cx));
    }
}
