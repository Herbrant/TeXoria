//! What follows an edit: the re-index of the project, the word count and the live checks.

use super::*;

impl Workspace {
    pub(super) fn on_buffer_changed(
        &mut self,
        id: EntityId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ix) = self.buffers.iter().position(|b| b.editor.entity_id() == id) else {
            return;
        };
        let editor = self.buffers[ix].editor.clone();
        let (len, cursor, dirty) = {
            let state = editor.read(cx);
            let text = state.text();
            let len = text.len();
            (
                len,
                state.cursor(),
                len != self.buffers[ix].saved.len() || *text != self.buffers[ix].saved,
            )
        };
        let grew = len.saturating_sub(self.buffers[ix].last_len);
        self.buffers[ix].last_len = len;
        self.buffers[ix].dirty = dirty;

        // A multi-character insertion ending in a completion's text is that
        // completion landing: move the cursor into its first argument.
        if grew > 1 {
            let fix = {
                let state = editor.read(cx);
                let text = state.text();
                self.fixups.borrow().iter().find_map(|f| {
                    let start = cursor.checked_sub(f.insert.len())?;
                    // Slicing inside a multi-byte char would panic.
                    (text.is_char_boundary(start) && text.slice(start..cursor) == f.insert.as_str())
                        .then(|| cursor - f.back)
                })
            };
            if let Some(target) = fix {
                self.fixups.borrow_mut().clear();
                editor.update(cx, |state, cx| state.set_selected_range(target..target, cx));
            }
        }

        self.spellcheck(ix, spell::DELAY, cx);
        self.grammar_check(ix, grammar::DELAY, cx);
        let delay = Duration::from_millis(self.settings.live_delay_ms);
        let live = self.settings.live_preview;
        self._live = cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(delay).await;
            this.update(cx, |this, cx| {
                this.reindex(cx);
                if live {
                    this.build(cx);
                }
            })
            .ok();
        });
        cx.notify();
    }

    /// Re-index open buffers so outline and completions follow unsaved edits. Only the
    /// buffers whose text changed since are parsed again, in the background (a big file
    /// takes a tenth of a second), and the project is locked only to take the results.
    /// One re-index runs at a time: those asked for meanwhile make one more once it ends
    /// (a parse cannot be stopped, and two at once would hold two trees of the same file).
    pub(super) fn reindex(&mut self, cx: &mut Context<Self>) {
        if self.reindexing {
            self.reindex_again = true;
            return;
        }
        let mut texts = Vec::new();
        let mut jobs = Vec::new();
        {
            let project = self.project.read().unwrap();
            for b in &self.buffers {
                let text = Arc::new(b.editor.read(cx).text().to_string());
                let hash = text_hash(&text);
                if b.indexed != Some(hash) {
                    let bib = project.is_bib(&b.path);
                    jobs.push((b.path.clone(), bib, text.clone(), hash));
                }
                texts.push((b.editor.clone(), text));
            }
        }
        if jobs.is_empty() {
            self.reindexed(texts, cx);
            return;
        }
        let cache = self.count_cache.clone();
        self.reindexing = true;
        self._reindex = cx.background_then(
            move || {
                let parse = |path: &PathBuf, bib, text: &str, hash| {
                    if bib {
                        return Parsed::Bib(tex_project::bib::parse(text));
                    }
                    let tree = tex_syntax::parse(text, None);
                    // The word count's too, from the same tree.
                    let counts = (tree.as_ref())
                        .map(|t| tex_spell::extract::count(t, text))
                        .unwrap_or_default();
                    cache.lock().unwrap().insert(path.clone(), (hash, counts));
                    Parsed::Tex(Box::new(
                        tree.map(|t| tex_syntax::Index::build(&t, text))
                            .unwrap_or_default(),
                    ))
                };
                (jobs.into_iter())
                    .map(|(path, bib, text, hash)| {
                        let parsed = parse(&path, bib, &text, hash);
                        (path, hash, parsed)
                    })
                    .collect::<Vec<_>>()
            },
            move |this, parsed, cx| {
                {
                    let mut project = this.project.write().unwrap();
                    for (path, hash, parsed) in parsed {
                        // Parsed as the wrong kind: the file became or stopped being a
                        // bibliography meanwhile, and is parsed again next time.
                        let kind_ok = matches!(parsed, Parsed::Bib(_)) == project.is_bib(&path);
                        project.apply(&path, parsed);
                        if let Some(b) = this.buffers.iter_mut().find(|b| b.path == path) {
                            b.indexed = kind_ok.then_some(hash);
                        }
                    }
                }
                this.reindexing = false;
                this.reindexed(texts, cx);
                if std::mem::take(&mut this.reindex_again) {
                    this.reindex(cx);
                }
            },
        );
    }

    /// What follows a re-index from `texts`, the buffers' text.
    pub(super) fn reindexed(
        &mut self,
        texts: Vec<(Entity<EditorState>, Arc<String>)>,
        cx: &mut Context<Self>,
    ) {
        self.refresh_outline(cx);
        self.count_words(&texts, cx);
        self.run_checks(texts, cx);
    }

    /// Re-index every open buffer, also those that did not change: an unchanged buffer's
    /// `\input` may now name a file that was created since.
    pub(super) fn reindex_all(&mut self, cx: &mut Context<Self>) {
        for b in &mut self.buffers {
            b.indexed = None;
        }
        self.reindex(cx);
    }

    /// Read the project's bibliographies from disk again and re-index. Those open are
    /// read from disk too: parsed again, their unsaved text comes back.
    pub(super) fn reload_bibs(&mut self, cx: &mut Context<Self>) {
        {
            let mut project = self.project.write().unwrap();
            project.reload_bibs();
            for b in &mut self.buffers {
                if project.is_bib(&b.path) {
                    b.indexed = None;
                }
            }
        }
        self.reindex(cx);
    }

    /// Count the words of every file of the project in the background,
    /// reading open files from `texts`.
    pub(super) fn count_words(
        &mut self,
        texts: &[(Entity<EditorState>, Arc<String>)],
        cx: &mut Context<Self>,
    ) {
        let files = self.project.read().unwrap().files();
        let open: std::collections::HashMap<PathBuf, Arc<String>> = texts
            .iter()
            .filter_map(|(editor, text)| {
                let b = self.buffers.iter().find(|b| &b.editor == editor)?;
                Some((b.path.clone(), text.clone()))
            })
            .collect();
        let cache = self.count_cache.clone();
        self._count = cx.background_then(
            move || {
                let mut cache = cache.lock().unwrap();
                let mut out = Vec::new();
                for file in files {
                    let read;
                    let text = match open.get(&file) {
                        Some(text) => text.as_str(),
                        None => match tex_project::read_regular(&file) {
                            Some(text) => {
                                read = text;
                                read.as_str()
                            }
                            None => continue,
                        },
                    };
                    let hash = text_hash(text);
                    let counts = match cache.get(&file) {
                        Some((cached, counts)) if *cached == hash => *counts,
                        _ => {
                            let counts = tex_syntax::parse(text, None)
                                .map(|t| tex_spell::extract::count(&t, text))
                                .unwrap_or_default();
                            cache.insert(file.clone(), (hash, counts));
                            counts
                        }
                    };
                    out.push((file, counts));
                }
                out
            },
            move |this, counts, cx| {
                this.word_counts = counts;
                cx.notify();
            },
        );
    }

    /// The words of the whole project.
    pub(super) fn total_words(&self) -> tex_spell::extract::Counts {
        let mut total = tex_spell::extract::Counts::default();
        for (_, c) in &self.word_counts {
            total.add(c);
        }
        total
    }

    /// The project's words, by kind and by file.
    pub(super) fn show_word_count(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let total = self.total_words();
        let files: Vec<(String, usize)> = self
            .word_counts
            .iter()
            .map(|(f, c)| (self.relative(f).display().to_string(), c.words()))
            .collect();
        window.open_dialog(cx, move |dialog, _, cx| {
            let theme = cx.theme();
            let row = |label: String, value: String| {
                h_flex()
                    .justify_between()
                    .gap_4()
                    .text_sm()
                    .child(div().text_color(theme.muted_foreground).child(label))
                    .child(div().child(value))
            };
            let mut body = v_flex()
                .gap_1()
                .child(row("Words in the text".into(), thousands(total.text)))
                .child(row("Words in headings".into(), thousands(total.headers)))
                .child(row("Words in captions".into(), thousands(total.captions)))
                .child(row("Inline formulas".into(), thousands(total.inline_math)))
                .child(row(
                    "Displayed formulas".into(),
                    thousands(total.display_math),
                ));
            if files.len() > 1 {
                body = body.child(div().h_2()).child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.muted_foreground)
                        .child("WORDS BY FILE"),
                );
                for (name, words) in &files {
                    body = body.child(row(name.clone(), thousands(*words)));
                }
            }
            dialog
                .title(format!("{} words", thousands(total.words())))
                .w(px(420.))
                .child(
                    div()
                        .id("word-count")
                        .max_h(px(420.))
                        .overflow_y_scroll()
                        .child(body),
                )
        });
    }

    /// Check the project (as indexed from `texts`, the buffers' text) in the
    /// background, then underline what was found.
    pub(super) fn run_checks(
        &mut self,
        texts: Vec<(Entity<EditorState>, Arc<String>)>,
        cx: &mut Context<Self>,
    ) {
        if !self.settings.live_checks || self.root.is_none() {
            self._checks = Task::ready(());
            if !self.issues.is_empty() {
                self.issues.clear();
                for b in &self.buffers {
                    b.checks.clear(cx);
                }
                cx.notify();
            }
            return;
        }
        let project = self.project.clone();
        self._checks = cx.background_then(
            move || {
                let project = project.read().unwrap();
                tex_project::check::run(&project)
            },
            move |this, issues, cx| {
                this.issues = issues;
                for ix in 0..this.buffers.len() {
                    let b = &this.buffers[ix];
                    // Edited meanwhile: the next check is on its way. A buffer
                    // not in `texts` was indexed from its file.
                    let checked = texts
                        .iter()
                        .find(|(e, _)| *e == b.editor)
                        .map_or(b.saved.as_str(), |(_, t)| t.as_str());
                    let same = *b.editor.read(cx).text() == *checked;
                    if same {
                        this.underline_issues(ix, cx);
                    }
                }
                cx.notify();
            },
        );
    }

    /// Underline the issues of buffer `ix`.
    pub(super) fn underline_issues(&self, ix: usize, cx: &mut App) {
        let b = &self.buffers[ix];
        let issues: Vec<&tex_project::check::Issue> =
            self.issues.iter().filter(|i| i.file == b.path).collect();
        let colors = (cx.theme().danger, cx.theme().warning);
        b.checks.set(&b.editor, &issues, colors, cx);
    }

    /// The problems to list: the last build's, without those a live check
    /// also reports, then the live checks'.
    pub(super) fn problems(&self) -> Vec<Diagnostic> {
        let keys = self.checked_keys();
        self.diagnostics
            .iter()
            .filter(|d| !checks::duplicates(d, &keys))
            .cloned()
            .chain(self.issues.iter().map(checks::diagnostic))
            .collect()
    }

    /// The errors and the warnings among `problems`, without copying them (the
    /// status bar counts them on every frame).
    pub(super) fn problem_counts(&self) -> (usize, usize) {
        let keys = self.checked_keys();
        let severities = (self.diagnostics.iter())
            .filter(|d| !checks::duplicates(d, &keys))
            .map(|d| d.severity)
            .chain(self.issues.iter().map(|i| checks::severity(i.level)));
        let (mut errors, mut warnings) = (0, 0);
        for s in severities {
            match s {
                Severity::Error => errors += 1,
                Severity::Warning => warnings += 1,
                Severity::Info => {}
            }
        }
        (errors, warnings)
    }

    /// The keys the live checks report on.
    pub(super) fn checked_keys(&self) -> std::collections::HashSet<&str> {
        self.issues
            .iter()
            .filter_map(|i| i.key.as_deref())
            .collect()
    }

    pub(super) fn refresh_outline(&mut self, cx: &mut Context<Self>) {
        self.outline = self.project.read().unwrap().outline();
        cx.notify();
    }
}
