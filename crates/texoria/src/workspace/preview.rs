//! The PDF: exporting it, its own window and forward search.

use super::*;

impl Workspace {
    /// Save a copy of the PDF where the user chooses (by default, where the
    /// last one went).
    pub(super) fn export_pdf(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
                    let name = crate::paths::file_name(&dest);
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

    /// Move the PDF to a window of its own (to put on another display), or
    /// back next to the editor.
    pub(super) fn toggle_preview_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.preview_detached {
            return self.dock_preview(cx);
        }
        self.preview_detached = true;
        // Its bitmaps are uploaded again in the new window.
        let drawn = self.pdf.update(cx, |pdf, cx| {
            pdf.set_detached(true, cx);
            pdf.leave_window(cx)
        });
        for image in drawn {
            let _ = window.drop_image(image);
        }
        let options = preview_window::options(self.settings.preview_window.as_ref(), cx);
        let pane = self.pdf.clone();
        let workspace = cx.entity().downgrade();
        let (main, focus) = (window.window_handle(), self.focus.clone());
        let name = self.root_name();
        // The new window reads the workspace, which is being updated here.
        cx.defer(move |cx| {
            let opened = gpui_kit::open_window(options, cx, {
                let workspace = workspace.clone();
                move |window, cx| {
                    let closing = workspace.clone();
                    window.on_window_should_close(cx, move |window, cx| {
                        let placement = preview_window::placement(window, cx);
                        closing
                            .update(cx, |w, cx| {
                                w.settings.preview_window = Some(placement);
                                w.dock_preview(cx)
                            })
                            .ok();
                        true
                    });
                    cx.new(|cx| PreviewWindow::new(pane, name, workspace, main, focus, window, cx))
                }
            });
            workspace
                .update(cx, |w, cx| match opened {
                    // Docked again meanwhile.
                    Ok((handle, _)) if !w.preview_detached => {
                        handle.update(cx, |_, w, _| w.remove_window()).ok();
                    }
                    Ok((handle, view)) => {
                        w.preview_window = Some(handle);
                        w.preview_view = view.downgrade();
                    }
                    Err(e) => {
                        log::error!("opening the PDF's window: {e}");
                        w.dock_preview(cx);
                    }
                })
                .ok();
        });
        cx.notify();
    }

    /// Put the PDF back next to the editor, and close its window.
    pub(super) fn dock_preview(&mut self, cx: &mut Context<Self>) {
        if !self.preview_detached {
            return;
        }
        self.remember_preview_window(cx);
        self.preview_detached = false;
        if let Some(handle) = self.preview_window.take() {
            // Its bitmaps go with it. Fails when it is closing already.
            handle.update(cx, |_, w, _| w.remove_window()).ok();
        }
        self.pdf.update(cx, |pdf, cx| {
            pdf.set_detached(false, cx);
            pdf.leave_window(cx);
        });
        cx.notify();
    }

    /// Run `f` on the PDF's pane in the window that shows it (its fields
    /// belong there), and bring that window forward.
    pub(super) fn in_pdf_window(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut PdfPane, &mut Window, &mut Context<PdfPane>),
    ) {
        if !self.preview_detached {
            return self.pdf.update(cx, |p, cx| f(p, window, cx));
        }
        // Not open yet, or closing.
        let Some(handle) = self.preview_window else {
            return;
        };
        let pdf = self.pdf.clone();
        handle
            .update(cx, |_, window, cx| {
                window.activate_window();
                pdf.update(cx, |p, cx| f(p, window, cx))
            })
            .ok();
    }

    /// Name the PDF's window after the root file.
    pub(super) fn name_preview_window(&mut self, cx: &mut Context<Self>) {
        let (Some(handle), Some(view)) = (self.preview_window, self.preview_view.upgrade()) else {
            return;
        };
        let name = self.root_name();
        handle
            .update(cx, |_, window, cx| {
                view.update(cx, |v, cx| v.set_root_name(name, window, cx))
            })
            .ok();
    }

    /// Save where the PDF's window is, to open it there next time.
    pub(super) fn remember_preview_window(&mut self, cx: &mut Context<Self>) {
        if let Some(placement) = self.preview_window.and_then(|h| {
            h.update(cx, |_, w, cx| preview_window::placement(w, cx))
                .ok()
        }) {
            self.settings.preview_window = Some(placement);
        }
        if self.preview_detached {
            self.settings.save();
        }
    }

    pub(super) fn forward_search(&mut self, cx: &mut Context<Self>) {
        let Some(b) = self.buffers.get(self.active) else {
            return;
        };
        let line = b.editor.read(cx).cursor_position().line + 1;
        let path = b.path.clone();
        self.pdf.update(cx, |pdf, cx| {
            pdf.forward_search(&path, line, cx);
        });
    }
}
