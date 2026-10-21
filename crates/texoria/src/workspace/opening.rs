//! Opening projects and files, and going to places in them.

use super::*;

impl Workspace {
    pub fn open_path(&mut self, file: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let file = crate::paths::canonical(&file);
        // A mistyped path on the command line, or a recent project since moved: not a
        // document to build (its folder would get a build folder).
        if !file.exists() {
            self.state = CompileState::Failed(format!("{} does not exist", file.display()).into());
            if let Some(ix) = self.settings.recent.iter().position(|p| *p == file) {
                self.settings.recent.remove(ix);
                self.settings.save();
            }
            cx.notify();
            return;
        }
        // A project is opened by its main file (or one of its parts): nothing guesses which
        // file of a folder is the document, and a `.bib` or a package is not one.
        if file.is_dir() || !tex_project::has_extension(&file, &["tex"]) {
            // A file of the open project (its bibliography, a package): a tab of it.
            let inside = self.project_dir().is_some_and(|d| file.starts_with(d));
            if inside && file.is_file() && files::looks_like_text(&file) {
                self.activate_file(file, window, cx);
                return;
            }
            let what = if file.is_dir() {
                "a folder"
            } else {
                "not a .tex file"
            };
            self.state = CompileState::Failed(
                format!(
                    "{} is {what}: open the document's main .tex file",
                    file.display()
                )
                .into(),
            );
            cx.notify();
            return;
        }
        let text = tex_project::read_regular(&file).unwrap_or_default();
        let root = tex_project::find_root(&file, &text);
        let root = crate::paths::canonical(&root);
        let new_project = self.root.as_ref() != Some(&root);
        let session = new_project
            .then(|| self.settings.session(&root).cloned())
            .flatten();
        self.set_root(root.clone(), cx);
        match session {
            Some(session) => self.restore_session(session, file, window, cx),
            None => {
                self.activate_file(file, window, cx);
            }
        }
    }

    /// Reopen a project's tabs, then show `file`, or the tab that was
    /// active if `file` is the root (the project was opened, not a file).
    pub(super) fn restore_session(
        &mut self,
        session: Session,
        file: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let dir = self.project_dir();
        for f in &session.files {
            let inside = dir.as_ref().is_some_and(|d| f.starts_with(d));
            if inside && f.is_file() && files::looks_like_text(f) {
                self.activate_file(f.clone(), window, cx);
            }
        }
        let active = session
            .active
            .filter(|a| {
                Some(&file) == self.root.as_ref() && self.buffers.iter().any(|b| &b.path == a)
            })
            .unwrap_or(file);
        self.activate_file(active, window, cx);
    }

    /// Remember the project's open tabs (saved with the settings).
    pub(super) fn remember_session(&mut self) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let dir = self.project_dir();
        let inside = |p: &Path| dir.as_ref().is_some_and(|d| p.starts_with(d));
        let previous = self.settings.session(&root).cloned().unwrap_or_default();
        let session = Session {
            root,
            files: self
                .buffers
                .iter()
                .map(|b| b.path.clone())
                .filter(|p| inside(p))
                .collect(),
            active: self
                .buffers
                .get(self.active)
                .map(|b| b.path.clone())
                .filter(|p| inside(p)),
            export: previous.export,
        };
        if self.settings.set_session(session) {
            self.settings.save();
        }
    }

    /// Make `root` the project's root file (a no-op if it already is).
    pub(super) fn set_root(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        if self.root.as_deref() != Some(&root) {
            self.root = Some(root.clone());
            *self.project.write().unwrap() = Project::open(&root);
            self._reindex = Task::ready(());
            (self.reindexing, self.reindex_again) = (false, false);
            for b in &mut self.buffers {
                b.indexed = None;
            }
            self.outline_fold = Fold::default();
            self.refresh_outline(cx);
            self.issues.clear();
            self.word_counts.clear();
            self.count_words(&[], cx);
            self.run_checks(Vec::new(), cx);
            self.show_old_pdf = self.settings.recent.contains(&root);
            self.settings.push_recent(root.clone());
            self.settings.save();
            // The previous project's run is irrelevant now.
            if let Some(c) = &self.compiler {
                c.cancel();
            }
            self.answer_waiters(ToolResult::error("Another document was opened."));
            let job = self.job().expect("root is set");
            self.untrusted_rc = !job.allow_project_rc && job.has_project_rc();
            self.wants_shell_escape = false;
            self.close_diff(cx);
            self.sync_git(cx);
            self.sync_agent(cx);
            self.files
                .update(cx, |f, cx| f.set_project(Some(root.clone()), cx));
            self.name_preview_window(cx);
            let dir = self.project_dir();
            self.search.update(cx, |s, cx| s.set_dir(dir, cx));
            // Show a previous build right away, if any.
            if self.show_old_pdf && job.artifact("pdf").is_file() {
                let synctex = Some(job.artifact("synctex.gz")).filter(|p| p.is_file());
                self.pdf
                    .update(cx, |pdf, cx| pdf.load(job.artifact("pdf"), synctex, cx));
            }
            self.build(cx);
        }
    }

    /// Focus the buffer for `file`, opening it if needed. Returns its index.
    pub(super) fn activate_file(
        &mut self,
        file: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<usize> {
        self.diff_active = false;
        if let Some(ix) = self.buffers.iter().position(|b| b.path == file) {
            if ix != self.active
                && let Some(v) = &mut self.vim
            {
                v.reset();
            }
            self.active = ix;
            self.remember_session();
            cx.notify();
            return Some(ix);
        }
        // Only regular files: inverse search and diagnostics can point
        // anywhere, and reading a pipe or device would hang.
        // Nor huge ones, which would freeze the editor.
        let read = std::fs::metadata(&file).and_then(|m| {
            if !m.is_file() {
                Err(std::io::Error::other("not a regular file"))
            } else if m.len() > files::MAX_TEXT_BYTES {
                Err(std::io::Error::other(format!(
                    "it is too large to edit ({} MB)",
                    m.len() >> 20
                )))
            } else {
                std::fs::read_to_string(&file)
            }
        });
        let text = match read {
            Ok(t) => t,
            Err(e) => {
                self.state =
                    CompileState::Failed(format!("Cannot open {}: {e}", file.display()).into());
                cx.notify();
                return None;
            }
        };
        // Soft wrap lays out every line up front; past a few thousand lines
        // it dominates memory, so large files start unwrapped (⌥Z toggles).
        let soft_wrap = self.settings.soft_wrap && text.lines().count() <= SOFT_WRAP_MAX_LINES;
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language(latex::language_for(&file))
                .line_number(true)
                .soft_wrap(soft_wrap)
                .tab_size(TabSize {
                    tab_size: 2,
                    hard_tabs: false,
                })
                .default_value(text.clone())
        });
        let provider = Rc::new(LatexCompletions {
            project: self.project.clone(),
            fixups: self.fixups.clone(),
        });
        let checks = BufferChecks::new(&editor, cx);
        let block = crate::vim::Block::new(&editor, cx);
        let grammar = BufferGrammar::new(&editor, cx);
        let mut navigation = LatexNavigation::new(self.project.clone());
        navigation.notes.push(checks.hover_note(&editor));
        navigation.notes.push(grammar.hover_note(&editor));
        let navigation = Rc::new(navigation);
        let busy = self.agent_busy;
        editor.update(cx, |state, cx| {
            state.set_readonly(busy, cx);
            let lsp = state.lsp_mut();
            lsp.completion_provider = Some(provider);
            lsp.show_document = Some(navigation.show_document());
            lsp.definition_provider = Some(navigation.clone());
            lsp.hover_provider = Some(navigation);
            state.set_editor_paddings(Edges {
                top: px(18.),
                right: px(24.),
                bottom: px(18.),
                left: px(8.),
            });
            cx.notify();
        });
        let subscription = cx.subscribe_in(
            &editor,
            window,
            |this, editor, ev: &InputEvent, window, cx| {
                if let InputEvent::Change = ev {
                    this.on_buffer_changed(editor.entity_id(), window, cx);
                }
            },
        );
        let last_len = text.len();
        let spell = BufferSpell::new(&editor, cx);
        self.buffers.push(Buffer {
            path: file,
            editor,
            saved: text,
            dirty: false,
            conflict: false,
            last_len,
            indexed: None,
            soft_wrap,
            spell,
            grammar,
            checks,
            block,
            _subscription: subscription,
        });
        self.active = self.buffers.len() - 1;
        // The project indexed this file from disk: the issues match its text.
        self.underline_issues(self.active, cx);
        self.remember_session();
        self.spellcheck(self.active, Duration::ZERO, cx);
        self.grammar_check(self.active, Duration::ZERO, cx);
        self.apply_diagnostics(cx);
        let focus = self.buffers[self.active].editor.focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
        Some(self.active)
    }

    /// Jump to a 0-based `line` in `file` (outline, diagnostics, inverse search).
    pub(super) fn reveal(
        &mut self,
        file: PathBuf,
        line: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let file = crate::paths::canonical(&file);
        let Some(ix) = self.activate_file(file, window, cx) else {
            return;
        };
        let editor = self.buffers[ix].editor.clone();
        editor.update(cx, |state, cx| {
            state.set_cursor_position(Position::new(line, 0), window, cx);
            state.focus(window, cx);
        });
    }

    /// Select bytes of `file` (a definition, a search result).
    pub(super) fn reveal_range(
        &mut self,
        file: PathBuf,
        range: std::ops::Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let file = crate::paths::canonical(&file);
        let Some(ix) = self.activate_file(file, window, cx) else {
            return;
        };
        let editor = self.buffers[ix].editor.clone();
        editor.update(cx, |state, cx| {
            let len = state.text().len();
            let range = range.start.min(len)..range.end.min(len);
            // Scroll to it: moving the cursor brings it into view.
            let start = state.text().offset_to_position(range.start);
            state.set_cursor_position(start, window, cx);
            state.set_selected_range(range, cx);
            state.focus(window, cx);
        });
    }

    /// Open a file with its app, if it is a document or a picture.
    pub(super) fn open_external(
        &mut self,
        path: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if files::safe_to_open(path) {
            cx.open_with_system(path);
        } else {
            // It could be a program: let the user decide in the file manager
            // rather than start it.
            cx.reveal_path(path);
            window.push_notification(
                Notification::info(
                    "TeXoria opens only documents and pictures with other apps: the file is \
                     shown in its folder instead.",
                ),
                cx,
            );
        }
    }

    /// Open a package's documentation: the local one, else texdoc.org's.
    pub(super) fn package_docs(
        &mut self,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !tex_compile::is_package_name(&name) {
            return;
        }
        let package = name.clone();
        cx.background_then_in(
            window,
            move || tex_compile::texdoc(&package),
            move |this, local, window, cx| match local {
                Some(doc) => this.open_external(&doc, window, cx),
                None => cx.open_url(&format!("https://texdoc.org/pkg/{name}")),
            },
        )
        .detach();
    }

    pub(super) fn prompt_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Open".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Some(path) = paths
                .await
                .ok()
                .and_then(|r| r.ok())
                .flatten()
                .and_then(|p| p.into_iter().next())
            else {
                return;
            };
            this.update_in(cx, |this, window, cx| this.open_path(path, window, cx))
                .ok();
        })
        .detach();
    }
}
