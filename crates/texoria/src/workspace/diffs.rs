//! The diff tab: Git changes, commits, conflicts and the agent's review.

use super::*;

impl Workspace {
    pub(super) fn open_diff(
        &mut self,
        source: DiffSource,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.save_all(cx);
        let Some(repo) = self.git.read(cx).repo().cloned() else {
            return;
        };
        self.show_diff(Some(repo), source, window, cx);
    }

    pub(super) fn show_diff(
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

    pub(super) fn close_diff(&mut self, cx: &mut Context<Self>) {
        self.diff = None;
        self.diff_active = false;
        self._diff_subscription = None;
        cx.notify();
    }

    pub(super) fn on_diff_event(
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
    pub(super) fn replace_file(
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

    pub(super) fn restore_version(
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
        cx.background_then_in(
            window,
            move || repo.file_at(&sha, &path),
            move |this, contents, window, cx| match contents {
                Ok(bytes) => match this.replace_file(&file, &bytes, window, cx) {
                    Ok(()) => {
                        let name = crate::paths::file_name(&file);
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
            },
        )
        .detach();
    }

    pub(super) fn save_resolution(
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
}
