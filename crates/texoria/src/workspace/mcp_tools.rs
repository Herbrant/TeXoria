//! TeXoria's MCP tools, answered for the agents.

use super::*;

impl Workspace {
    /// One of TeXoria's tools, called by an agent.
    pub(super) fn on_tool_call(&mut self, call: ToolCall, cx: &mut Context<Self>) {
        let reply = |r: ToolResult| {
            let _ = call.reply.send(r);
        };
        match call.name.as_str() {
            "build" => {
                if self.root.is_none() {
                    return reply(ToolResult::error("No document is open."));
                }
                if self.compiler.is_none() {
                    return reply(ToolResult::error(
                        "No TeX engine is installed, so the document cannot be built.",
                    ));
                }
                if !self.save_all(cx) {
                    return reply(ToolResult::error("TeXoria could not save the files."));
                }
                self.tool_waiters.push(ToolWaiter {
                    reply: call.reply.clone(),
                    after: self.last_run,
                });
                self.build(cx);
            }
            "problems" => reply(ToolResult::ok(self.problems_report())),
            "outline" => reply(ToolResult::ok(self.outline_report())),
            "pdf_page" => self.tool_pdf_page(call, cx),
            "pdf_text" => self.tool_pdf_text(call, cx),
            "bib_search" => {
                let query = call.arguments["query"].as_str().unwrap_or("").to_string();
                reply(ToolResult::ok(self.bib_report(&query)))
            }
            "word_count" => reply(ToolResult::ok(self.word_count_report())),
            other => reply(ToolResult::error(format!("Unknown tool: {other}"))),
        }
    }

    /// `pdf_page`: a PNG of a page of the PDF.
    pub(super) fn tool_pdf_page(&mut self, call: ToolCall, cx: &mut Context<Self>) {
        let (pages, page) = match self.page_arg(&call, true, cx) {
            Ok((pages, page)) => (pages, page.unwrap_or_default()),
            Err(e) => {
                let _ = call.reply.send(e);
                return;
            }
        };
        let scale = call.arguments["scale"]
            .as_f64()
            .unwrap_or(1.5)
            .clamp(0.5, 3.0) as f32;
        let bitmap = self.pdf.update(cx, |p, _| p.snapshot(page - 1, scale));
        cx.spawn(async move |_, cx| {
            let result = match bitmap.recv().await.ok().flatten() {
                Some(b) => cx
                    .background_spawn(async move { png_of(b) })
                    .await
                    .map(|data| {
                        ToolResult::ok(format!("Page {page} of {pages}.")).with_image(
                            mcp::ToolImage {
                                data,
                                mime_type: "image/png".into(),
                            },
                        )
                    })
                    .unwrap_or_else(|e| ToolResult::error(format!("Cannot encode the page: {e}"))),
                None => ToolResult::error("The page could not be rendered."),
            };
            let _ = call.reply.send(result);
        })
        .detach();
    }

    /// `pdf_text`: the text of a page, or of the whole PDF.
    pub(super) fn tool_pdf_text(&mut self, call: ToolCall, cx: &mut Context<Self>) {
        let page = match self.page_arg(&call, false, cx) {
            Ok((_, page)) => page,
            Err(e) => {
                let _ = call.reply.send(e);
                return;
            }
        };
        let text = self.pdf.update(cx, |p, _| p.text(page.map(|p| p - 1)));
        cx.spawn(async move |_, _| {
            let result = match text.recv().await.ok().flatten() {
                Some(t) => ToolResult::ok(t),
                None => ToolResult::error("The text could not be read."),
            };
            let _ = call.reply.send(result);
        })
        .detach();
    }

    /// The PDF's page count, and the page `call` asks for (from 1; it may give none unless
    /// `required`); else what to answer.
    pub(super) fn page_arg(
        &self,
        call: &ToolCall,
        required: bool,
        cx: &App,
    ) -> Result<(usize, Option<usize>), ToolResult> {
        let pages = self.pdf.read(cx).page_count();
        if pages == 0 {
            return Err(ToolResult::error("There is no PDF yet: build first."));
        }
        let page = call.arguments["page"].as_u64().map(|p| p as usize);
        let page = if required {
            Some(page.unwrap_or(0))
        } else {
            page
        };
        if page.is_some_and(|p| p == 0 || p > pages) {
            return Err(ToolResult::error(format!(
                "The PDF has pages 1 to {pages}."
            )));
        }
        Ok((pages, page))
    }

    /// `bib_search`: the bibliography entries matching every word of `query`.
    pub(super) fn bib_report(&self, query: &str) -> String {
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        let project = self.project.read().unwrap();
        let mut lines = Vec::new();
        for (file, entries) in project.bib_files() {
            for e in entries {
                let hay = format!(
                    "{} {} {} {}",
                    e.key,
                    e.title.as_deref().unwrap_or(""),
                    e.author.as_deref().unwrap_or(""),
                    e.year.as_deref().unwrap_or("")
                )
                .to_lowercase();
                if words.iter().all(|w| hay.contains(w)) {
                    lines.push(format!(
                        "{} — {} ({}) {} [{}]",
                        e.key,
                        e.author.as_deref().unwrap_or("?"),
                        e.year.as_deref().unwrap_or("n.d."),
                        e.title.as_deref().unwrap_or(""),
                        self.tool_path(file)
                    ));
                }
            }
        }
        for (file, index) in project.indexes() {
            for b in &index.bibitems {
                if words.iter().all(|w| b.name.to_lowercase().contains(w)) {
                    lines.push(format!("{} — \\bibitem [{}]", b.name, self.tool_path(file)));
                }
            }
        }
        if lines.is_empty() {
            "No entry matches.".to_string()
        } else {
            lines.join("\n")
        }
    }

    /// `word_count`: the document's words, in total and per file.
    pub(super) fn word_count_report(&self) -> String {
        let t = self.total_words();
        let mut out = format!(
            "{} words: {} in the text, {} in headings, {} in captions; {} inline and {} \
             displayed formulas.",
            t.words(),
            t.text,
            t.headers,
            t.captions,
            t.inline_math,
            t.display_math
        );
        for (file, c) in &self.word_counts {
            out.push_str(&format!("\n{}: {} words", self.tool_path(file), c.words()));
        }
        out
    }

    /// Answer every pending `build` call with `result`.
    pub(super) fn answer_waiters(&mut self, result: ToolResult) {
        for w in std::mem::take(&mut self.tool_waiters) {
            let _ = w.reply.send(result.clone());
        }
    }

    /// Answer the `build` calls made before run `id` started.
    pub(super) fn answer_build(&mut self, id: u64, result: ToolResult) {
        let (done, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut self.tool_waiters)
            .into_iter()
            .partition(|w| id > w.after);
        self.tool_waiters = waiting;
        for w in done {
            let _ = w.reply.send(result.clone());
        }
    }

    pub(super) fn build_report(&self, status: Status, elapsed: Duration) -> String {
        let head = match status {
            Status::Success => format!("Built in {:.1} s.", elapsed.as_secs_f32()),
            Status::Failed => "The build failed.".to_string(),
            Status::Cancelled => "The build was stopped.".to_string(),
        };
        format!("{head}\n{}", self.problems_text())
    }

    pub(super) fn problems_report(&self) -> String {
        format!("{}\n{}", run_summary(&self.state), self.problems_text())
    }

    pub(super) fn problems_text(&self) -> String {
        let dir = self.project_dir().unwrap_or_default();
        let problems: Vec<Diagnostic> = self
            .problems()
            .into_iter()
            .filter(|d| d.severity != Severity::Info)
            .collect();
        tex_agent::prompt::format_diagnostics(&dir, &problems)
    }

    pub(super) fn outline_report(&self) -> String {
        if self.outline.is_empty() {
            return "The document has no sections, figures or tables yet.".to_string();
        }
        self.outline
            .iter()
            .map(|o| {
                let indent = "  ".repeat(o.item.kind.depth().min(6) as usize);
                format!(
                    "{indent}{:?}: {} ({}:{})",
                    o.item.kind,
                    o.item.title,
                    self.tool_path(&o.file),
                    o.item.line + 1
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}
