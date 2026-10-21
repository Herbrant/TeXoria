//! Adding citations from a DOI, arXiv or Crossref.

use super::*;

impl Workspace {
    /// Ask for a DOI, an arXiv identifier or a search, then add the entry to
    /// the bibliography and cite it.
    pub(super) fn prompt_citation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.root.is_none() {
            return;
        }
        let weak = cx.entity().downgrade();
        dialogs::prompt_text(
            window,
            cx,
            "Add Citation",
            "Find",
            "10.1145/… · arXiv:2301.01234 · title, authors",
            "A DOI, an arXiv identifier, or words of the title and authors. The entry is added \
             to the project's bibliography and cited where the cursor is. This asks doi.org, \
             arXiv or Crossref.",
            move |text, window, cx| {
                weak.update(cx, |this, cx| this.look_up_citation(text, window, cx))
                    .ok();
                true
            },
        );
    }

    pub(super) fn look_up_citation(
        &mut self,
        input: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let lookup = tex_cite::classify(&input);
        window.push_notification(Notification::info("Looking it up…"), cx);
        cx.background_then_in(
            window,
            move || {
                let client = tex_cite::Client::new();
                match lookup {
                    tex_cite::Lookup::Doi(doi) => client.doi(&doi).map(Ok),
                    tex_cite::Lookup::Arxiv(id) => client.arxiv(&id).map(Ok),
                    tex_cite::Lookup::Search(q) => client.search(&q).map(Err),
                }
            },
            move |this, found, window, cx| match found {
                Ok(Ok(bibtex)) => this.add_citation(&bibtex, window, cx),
                Ok(Err(candidates)) => this.choose_citation(candidates, window, cx),
                Err(e) => window.push_notification(
                    Notification::error(format!("{e:#}")).title("Citation not found"),
                    cx,
                ),
            },
        )
        .detach();
    }

    /// Show the papers a search found; picking one adds it.
    pub(super) fn choose_citation(
        &mut self,
        candidates: Vec<tex_cite::Candidate>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if candidates.is_empty() {
            window.push_notification(Notification::info("No paper matches."), cx);
            return;
        }
        let weak = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let theme = cx.theme();
            let rows = candidates.iter().enumerate().map(|(i, c)| {
                let mut meta: Vec<String> = Vec::new();
                if !c.authors.is_empty() {
                    let mut a = c
                        .authors
                        .iter()
                        .take(3)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ");
                    if c.authors.len() > 3 {
                        a.push_str(" et al.");
                    }
                    meta.push(a);
                }
                meta.extend(c.year.clone());
                meta.extend(c.venue.clone());
                let doi = c.doi.clone();
                let weak = weak.clone();
                v_flex()
                    .id(("citation", i))
                    .px_2()
                    .py_1p5()
                    .rounded(theme.radius)
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.accent))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .child(c.title.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(meta.join(" · ")),
                    )
                    .on_click(move |_, window, cx| {
                        window.close_dialog(cx);
                        let doi = doi.clone();
                        weak.update(cx, |this, cx| this.look_up_citation(doi, window, cx))
                            .ok();
                    })
            });
            dialog.title("Choose the Paper").w(px(560.)).child(
                v_flex()
                    .id("citations")
                    .max_h(px(420.))
                    .overflow_y_scroll()
                    .gap_0p5()
                    .children(rows),
            )
        });
    }

    /// Add an entry to the project's bibliography under a new key, and cite
    /// it at the cursor.
    pub(super) fn add_citation(
        &mut self,
        bibtex: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(dir) = self.project_dir() else {
            return;
        };
        let (taken, bib) = {
            let project = self.project.read().unwrap();
            let taken: std::collections::HashSet<String> =
                project.bib_entries().map(|e| e.key.clone()).collect();
            let mut bibs: Vec<PathBuf> = project
                .bib_files()
                .map(|(p, _)| p.to_path_buf())
                .filter(|p| p.is_file())
                .collect();
            bibs.sort();
            (taken, bibs.into_iter().next())
        };
        let key = tex_cite::make_key(bibtex, &taken);
        let Some(entry) = tex_cite::normalize(bibtex, &key) else {
            window.push_notification(Notification::error("The entry could not be read."), cx);
            return;
        };
        let created = bib.is_none();
        let bib = bib.unwrap_or_else(|| dir.join("references.bib"));
        if let Some(b) = self.buffers.iter().find(|b| b.path == bib) {
            b.editor.update(cx, |state, cx| {
                let len = state.text().len();
                let sep = if state.text().to_string().ends_with('\n') || len == 0 {
                    "\n"
                } else {
                    "\n\n"
                };
                state.set_selected_range(len..len, cx);
                state.insert(format!("{sep}{entry}\n"), window, cx);
            });
        } else {
            let old = tex_project::read_regular(&bib).unwrap_or_default();
            let sep = if old.is_empty() || old.ends_with("\n\n") {
                ""
            } else if old.ends_with('\n') {
                "\n"
            } else {
                "\n\n"
            };
            let new = format!("{old}{sep}{entry}\n");
            if let Err(e) = atomic::write(&bib, new.as_bytes()) {
                window.push_notification(
                    Notification::error(format!("Cannot write {}: {e}", bib.display())),
                    cx,
                );
                return;
            }
            self.project.write().unwrap().update(&bib, &new);
        }
        self.cite_at_cursor(&key, window, cx);
        let name = self.relative(&bib).display().to_string();
        let message = if created {
            format!(
                "Added {key} to {name}, a new file: add \\bibliography{{references}} or \
                 \\addbibresource{{references.bib}} to the document."
            )
        } else {
            format!("Added {key} to {name}.")
        };
        window.push_notification(Notification::success(message).title("Citation added"), cx);
        self.files.update(cx, |f, cx| f.refresh(cx));
        self.reindex_all(cx);
    }

    /// Cite `key` at the cursor of the active file: inside a `\cite{…}`, as
    /// one more key.
    pub(super) fn cite_at_cursor(
        &mut self,
        key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(b) = self
            .buffers
            .get(self.active)
            .filter(|b| !self.diff_active && !self.agent_busy && crate::paths::is_latex(&b.path))
        else {
            return;
        };
        b.editor.update(cx, |state, cx| {
            let text = state.text().to_string();
            let cursor = state.cursor();
            let insert = match tex_complete::context(&text, cursor) {
                Some(tex_complete::Context::Citation { prefix }) => {
                    let before = text[..prefix.start].trim_end();
                    if prefix.is_empty() && (before.ends_with('{') || before.ends_with(',')) {
                        key.to_string()
                    } else {
                        // After the key being typed.
                        state.set_selected_range(prefix.end..prefix.end, cx);
                        format!(",{key}")
                    }
                }
                _ => format!("\\cite{{{key}}}"),
            };
            state.insert(insert, window, cx);
        });
    }
}
