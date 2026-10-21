//! Editing commands: formatting, comments and Vim.

use super::*;

impl Workspace {
    /// Format the active file with tex-fmt (⇧⌥F), in the background.
    pub(super) fn format_document(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
        let input = text.clone();
        cx.background_then_in(
            window,
            move || edit::format_latex(&input, &path, dir.as_deref()),
            move |_, formatted, window, cx| match formatted {
                // Edited meanwhile: formatting it would undo those edits.
                Ok(new) if *editor.read(cx).text() == text => {
                    apply_format(&editor, new, window, cx)
                }
                Ok(_) => {}
                Err(e) => window
                    .push_notification(Notification::error(e).title("Cannot format the file"), cx),
            },
        )
        .detach();
    }

    /// A key typed anywhere in the window: Vim's if it is in the active
    /// editor (it then goes no further).
    pub(super) fn on_vim_keystroke(
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
    pub(super) fn update_block(&mut self, cx: &mut Context<Self>) {
        let color = cx.theme().caret.opacity(0.45);
        let vim = self.vim.as_ref();
        if let Some(b) = self.buffers.get_mut(self.active) {
            b.block.update(vim, &b.editor, color, cx);
        }
    }

    pub(super) fn set_vim_mode(&mut self, on: bool, cx: &mut Context<Self>) {
        self.vim = on.then(tex_vim::Vim::new);
        for b in &mut self.buffers {
            b.block.update(None, &b.editor, Hsla::default(), cx);
        }
        self.update_block(cx);
    }

    /// Comment the selected lines out, or back in (⌘/).
    pub(super) fn toggle_comment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
}
