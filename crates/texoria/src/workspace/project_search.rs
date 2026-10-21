//! The Search view: finding in the project and replacing.

use super::*;

impl Workspace {
    /// Show the Search view, searching for the editor's selection if any.
    pub(super) fn find_in_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    pub(super) fn on_search_event(
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
    pub(super) fn replace_targets(&self, found: &[PathBuf], truncated: bool) -> Vec<PathBuf> {
        match self.project_dir() {
            Some(dir) if truncated => tex_project::search::text_files(&dir),
            _ => found.to_vec(),
        }
    }

    pub(super) fn confirm_replace_all(
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
        dialogs::open_form(
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
    pub(super) fn replace_all(
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
}
