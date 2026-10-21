//! Building the document: the compiler, its events and their diagnostics.

use super::*;

impl Workspace {
    pub(super) fn start_compiler(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.engines.get(self.engine_ix).cloned() else {
            return;
        };
        let (tx, rx) = smol::channel::unbounded::<CompileEvent>();
        // Live debouncing happens here in the UI (it must save first), so the
        // scheduler runs without its own delay.
        let compiler = Compiler::new(engine, Duration::ZERO, move |e| {
            let _ = tx.send_blocking(e);
        });
        compiler.set_timeout(self.settings.compile_timeout());
        self.compiler = Some(compiler);
        self._compile_events = cx.forward(rx, |this, event, cx| this.on_compile_event(event, cx));
    }

    /// The compile job for the current root. The project's own latexmk rc
    /// file runs arbitrary code, so it is honored only once trusted.
    pub(super) fn job(&self) -> Option<Job> {
        let mut job = Job::for_root(self.root.as_ref()?);
        job.allow_project_rc = self.settings.allows_rc(job.work_dir());
        job.shell_escape = self.settings.allows_shell_escape(job.work_dir());
        Some(job)
    }

    /// Let the project's documents run programs, and build again.
    pub(super) fn allow_shell_escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dir) = self.project_dir() else {
            return;
        };
        self.wants_shell_escape = false;
        self.update_settings(|s| s.allow_shell_escape(dir), window, cx);
    }

    pub(super) fn allow_rc(&mut self, cx: &mut Context<Self>) {
        let Some(job) = self.job() else {
            return;
        };
        self.settings.allow_rc(job.work_dir().to_path_buf());
        self.settings.save();
        self.untrusted_rc = false;
        self.build(cx);
    }

    pub(super) fn build(&mut self, cx: &mut Context<Self>) {
        let Some(mut job) = self.job() else {
            return;
        };
        // Build files that came with the project would make latexmk skip
        // the run, and leave their PDF in place of ours.
        job.force = !self.show_old_pdf;
        if !self.save_all(cx) {
            return;
        }
        if let Some(c) = &self.compiler {
            c.request_now(job);
        }
    }

    pub(super) fn cancel_build(&mut self) {
        if let Some(c) = &self.compiler {
            c.cancel();
        }
        // A request still waiting to start is dropped without a run to
        // report it: the agent would wait for one until its timeout.
        self.answer_waiters(ToolResult::error("The build was stopped."));
    }

    pub(super) fn on_compile_event(&mut self, event: CompileEvent, cx: &mut Context<Self>) {
        match event {
            CompileEvent::Started { id, .. } => {
                self.state = CompileState::Running;
                self.last_run = id;
            }
            CompileEvent::SpawnFailed { id, error } => {
                self.answer_build(id, ToolResult::error(format!("The engine failed: {error}")));
                self.state = CompileState::Failed(error.into());
            }
            CompileEvent::Finished(out) => {
                // A run for a project that is no longer open.
                if self.root.as_ref() != Some(&out.job.root_file) {
                    self.answer_build(out.id, ToolResult::error("Another document was opened."));
                    return;
                }
                if out.status == Status::Cancelled {
                    self.answer_build(out.id, ToolResult::error("The build was stopped."));
                    self.state = CompileState::Idle;
                    cx.notify();
                    return;
                }
                let errors = out
                    .diagnostics
                    .iter()
                    .filter(|d| d.severity == Severity::Error)
                    .count();
                let warnings = out
                    .diagnostics
                    .iter()
                    .filter(|d| d.severity == Severity::Warning)
                    .count();
                self.state = CompileState::Done {
                    status: out.status,
                    elapsed: out.elapsed,
                    errors,
                    warnings,
                };
                self.diagnostics = out
                    .diagnostics
                    .into_iter()
                    .map(|mut d| {
                        d.file = d.file.map(|f| crate::paths::canonical(&f));
                        d
                    })
                    .collect();
                self.apply_diagnostics(cx);
                let report = self.build_report(out.status, out.elapsed);
                self.answer_build(out.id, ToolResult::ok(report));
                self.agent.update(cx, |a, cx| a.set_errors(errors, cx));
                self.wants_shell_escape = out.needs_shell_escape;
                self.show_old_pdf |= out.pdf_written;
                if let Some(pdf) = out.pdf.filter(|_| self.show_old_pdf) {
                    self.pdf.update(cx, |p, cx| p.load(pdf, out.synctex, cx));
                }
                // .bib files may have been edited outside the app.
                self.reload_bibs(cx);
            }
        }
        cx.notify();
    }

    /// Push compile diagnostics into each open editor as squiggles.
    pub(super) fn apply_diagnostics(&mut self, cx: &mut Context<Self>) {
        for b in &self.buffers {
            let diags: Vec<&Diagnostic> = self
                .diagnostics
                .iter()
                .filter(|d| d.file.as_deref() == Some(b.path.as_path()))
                .collect();
            b.editor.update(cx, |state, cx| {
                let items: Vec<EditorDiagnostic> = {
                    let text = state.text();
                    diags
                        .iter()
                        .filter_map(|d| {
                            let row = d.line?.saturating_sub(1) as usize;
                            if row >= text.lines_len() {
                                return None;
                            }
                            let line = text.slice_line(row).to_string();
                            // Columns are in chars.
                            let start = line.chars().count() - line.trim_start().chars().count();
                            let end = line.trim_end().chars().count().max(start + 1);
                            let severity = match d.severity {
                                Severity::Error => DiagnosticSeverity::Error,
                                Severity::Warning => DiagnosticSeverity::Warning,
                                Severity::Info => DiagnosticSeverity::Info,
                            };
                            Some(
                                EditorDiagnostic::new(
                                    Position::new(row as u32, start as u32)
                                        ..Position::new(row as u32, end as u32),
                                    d.message.clone(),
                                )
                                .with_severity(severity),
                            )
                        })
                        .collect()
                };
                if let Some(set) = state.diagnostics_mut() {
                    set.clear();
                    set.extend(items);
                }
                cx.notify();
            });
        }
    }
}
