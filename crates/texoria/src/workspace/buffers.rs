//! Buffers and the files behind them: saving, conflicts, reloading and closing.

use super::*;

impl Workspace {
    pub(super) fn save_all(&mut self, cx: &mut Context<Self>) -> bool {
        let mut ok = true;
        let mut wrote = false;
        let mut conflicts = false;
        for b in &mut self.buffers {
            if !b.dirty {
                continue;
            }
            // Changed on disk since (another editor, git in a terminal, a sync service):
            // never overwritten without asking.
            if b.conflict || tex_project::read_regular(&b.path).is_some_and(|disk| disk != b.saved)
            {
                b.conflict = true;
                conflicts = true;
                ok = false;
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
        if conflicts {
            self.state = CompileState::Failed(
                "A file changed on disk while it had unsaved edits: it is not saved until you \
                 choose which version to keep."
                    .into(),
            );
            if !self.asking_conflict {
                // From wherever the save came: the question needs the window.
                let window = self.window;
                cx.spawn(async move |this, cx| {
                    cx.update_window(window, |_, window, cx| {
                        this.update(cx, |this, cx| this.ask_conflicts(window, cx))
                            .ok();
                    })
                    .ok();
                })
                .detach();
            }
        }
        cx.notify();
        ok
    }

    /// Ask which version to keep of the files changed on disk while they had unsaved
    /// edits: the edits (written over the file) or the file (the edits are dropped).
    pub(super) fn ask_conflicts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.asking_conflict {
            return;
        }
        let names: Vec<String> = self
            .buffers
            .iter()
            .filter(|b| b.conflict)
            .map(|b| self.relative(&b.path).display().to_string())
            .collect();
        if names.is_empty() {
            return;
        }
        self.asking_conflict = true;
        let title: SharedString = match names.as_slice() {
            [one] => format!("{one} changed on disk").into(),
            _ => format!("{} files changed on disk", names.len()).into(),
        };
        let description: SharedString = format!(
            "{} changed by another program while you had unsaved edits. Keep your edits \
             (the other changes are lost), or load the file from disk (your edits are lost).",
            match names.as_slice() {
                [one] => format!("{one} was"),
                _ => format!("{} were", names.join(", ")),
            }
        )
        .into();
        let (keep, load) = (cx.entity().downgrade(), cx.entity().downgrade());
        window.open_alert_dialog(cx, move |alert, _, _| {
            let (keep, load) = (keep.clone(), load.clone());
            alert
                .title(title.clone())
                .description(description.clone())
                .confirm()
                .ok_text("Keep My Edits")
                .cancel_text("Load from Disk")
                .cancel_variant(ButtonVariant::Danger)
                // Either choice drops something: no Escape.
                .keyboard(false)
                .on_ok(move |_, window, cx| {
                    keep.update(cx, |this, cx| this.resolve_conflicts(true, window, cx))
                        .ok();
                    true
                })
                .on_cancel(move |_, window, cx| {
                    load.update(cx, |this, cx| this.resolve_conflicts(false, window, cx))
                        .ok();
                    true
                })
        });
    }

    pub(super) fn resolve_conflicts(
        &mut self,
        keep_edits: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.asking_conflict = false;
        for ix in 0..self.buffers.len() {
            if !std::mem::take(&mut self.buffers[ix].conflict) {
                continue;
            }
            // As it is now (it may have changed again meanwhile).
            let Some(disk) = tex_project::read_regular(&self.buffers[ix].path) else {
                continue;
            };
            if keep_edits {
                // Saved below, over it.
                self.buffers[ix].saved = disk;
            } else {
                self.load_text(ix, disk, window, cx);
                self.buffers[ix].dirty = false;
            }
        }
        if keep_edits {
            self.save_all(cx);
        } else {
            self.reindex(cx);
            self.apply_diagnostics(cx);
        }
        self.state = CompileState::Idle;
        self.build(cx);
        cx.notify();
    }

    /// Show `text`, read from the file of `buffers[ix]`, in its editor.
    pub(super) fn load_text(
        &mut self,
        ix: usize,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.buffers[ix].editor.clone().update(cx, |state, cx| {
            let cursor = state.cursor_position();
            state.set_value(text.clone(), window, cx);
            state.set_cursor_position(cursor, window, cx);
        });
        let b = &mut self.buffers[ix];
        b.last_len = text.len();
        b.saved = text;
        // `set_value` emits no change event.
        self.spellcheck(ix, Duration::ZERO, cx);
        self.grammar_check(ix, Duration::ZERO, cx);
    }

    /// Pick up what changed on disk: the buffers, the Files view and the Source Control panel.
    pub(super) fn refresh_from_disk(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.reload_from_disk(window, cx);
        self.files.update(cx, |f, cx| f.refresh(cx));
        self.git.update(cx, |g, cx| g.refresh(cx));
    }

    /// Pick up files changed on disk behind our back (git checkout, pull,
    /// another editor). Tabs of deleted files close. For a buffer with unsaved edits
    /// the user chooses which version to keep.
    pub(super) fn reload_from_disk(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut changed = false;
        let mut conflicts = false;
        let mut ix = 0;
        while ix < self.buffers.len() {
            let b = &mut self.buffers[ix];
            if b.dirty {
                if !b.conflict
                    && tex_project::read_regular(&b.path).is_some_and(|disk| disk != b.saved)
                {
                    b.conflict = true;
                }
                conflicts |= b.conflict;
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
                self.load_text(ix, text, window, cx);
                changed = true;
            }
            ix += 1;
        }
        if conflicts {
            self.ask_conflicts(window, cx);
        }
        if changed {
            self.active = self.active.min(self.buffers.len().saturating_sub(1));
            self.remember_session();
            self.reindex_all(cx);
            self.apply_diagnostics(cx);
            self.build(cx);
        }
    }

    /// ⌘S: format the active file first if asked to, then save everything.
    pub(super) fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    pub(super) fn close_tab(&mut self, cx: &mut Context<Self>) {
        if self.diff_active {
            return self.close_diff(cx);
        }
        self.close_buffer(self.active, cx);
    }

    /// Close the tab of `buffers[ix]`, saving first. The last tab stays; the
    /// active one keeps its tab, or goes to the closed one's neighbor.
    pub(super) fn close_buffer(&mut self, ix: usize, cx: &mut Context<Self>) {
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
}
