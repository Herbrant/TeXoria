//! The Source Control panel and the Files view: their events and the file operations.

use super::*;

impl Workspace {
    /// Point the Source Control panel at the current project.
    pub(super) fn sync_git(&mut self, cx: &mut Context<Self>) {
        let dir = self.project_dir();
        // Trust is the repository's: its configuration is what runs.
        let trusted = dir
            .as_deref()
            .and_then(git_panel::repo_top)
            .is_some_and(|top| self.settings.is_trusted(&top));
        self.git.update(cx, |g, cx| g.set_project(dir, trusted, cx));
    }

    pub(super) fn on_git_event(
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

    pub(super) fn on_file_tree_event(
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
    pub(super) fn run_file_op(&mut self, op: FileOp, window: &mut Window, cx: &mut Context<Self>) {
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
        self.reindex_all(cx);
        self.remember_session();
    }

    /// Let Git run in the project (its configuration can run programs).
    /// Trust the repository the project is in (its working tree).
    pub(super) fn trust_git(&mut self, cx: &mut Context<Self>) {
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
}
