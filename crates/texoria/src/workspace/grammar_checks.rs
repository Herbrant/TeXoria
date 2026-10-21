//! Grammar checking with LanguageTool.

use super::*;

impl Workspace {
    /// Connect to the LanguageTool server (after `delay`), starting it if
    /// allowed, then check every buffer.
    pub(super) fn start_grammar(&mut self, delay: Duration, cx: &mut Context<Self>) {
        self.grammar.stop();
        for b in &mut self.buffers {
            b.grammar.clear(cx);
        }
        if !self.settings.grammar_check {
            self._grammar_start = Task::ready(());
            cx.notify();
            return;
        }
        let url = self.settings.languagetool_url.trim().to_string();
        if let Err(e) = tex_spell::grammar::validate_url(&url) {
            self.grammar.state = ServerState::Unreachable(e.to_string().into());
            cx.notify();
            return;
        }
        let server = Arc::new(tex_spell::grammar::Server::new(&url));
        self.grammar.server = Some(server.clone());
        self.grammar.state = ServerState::Connecting;
        let start = self.settings.languagetool_start;
        self._grammar_start = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let probe = server.clone();
            let running = cx.background_spawn(async move { probe.languages() }).await;
            let languages = match running {
                Ok(l) => Ok(l),
                Err(e) => {
                    let port = tex_spell::grammar::local_port(&url).filter(|_| start);
                    let spawned = this.update(cx, |this, _| match port {
                        Some(port) => this.grammar.spawn(port),
                        None => Err(format!("{e:#}")),
                    });
                    match spawned {
                        Ok(Ok(())) => {
                            cx.background_spawn(async move { grammar::wait_for(&server) })
                                .await
                        }
                        Ok(Err(e)) => Err(e),
                        Err(_) => return,
                    }
                }
            };
            this.update(cx, |this, cx| {
                this.grammar.state = match languages {
                    Ok(l) => ServerState::Ready(Arc::new(l)),
                    Err(e) => {
                        log::warn!("LanguageTool: {e}");
                        ServerState::Unreachable(e.into())
                    }
                };
                for ix in 0..this.buffers.len() {
                    this.grammar_check(ix, Duration::ZERO, cx);
                }
                cx.notify();
            })
            .ok();
        });
        cx.notify();
    }

    /// Check buffer `ix`'s grammar after `delay` (replacing a pending check).
    pub(super) fn grammar_check(&mut self, ix: usize, delay: Duration, cx: &mut Context<Self>) {
        let ServerState::Ready(languages) = &self.grammar.state else {
            return;
        };
        let Some(server) = self.grammar.server.clone() else {
            return;
        };
        let Some(b) = self.buffers.get_mut(ix) else {
            return;
        };
        let off = self.settings.spell_language(&b.path) == Some(SpellLanguage::OFF);
        if !self.settings.grammar_check || off || !crate::paths::is_latex(&b.path) {
            b.grammar.clear(cx);
            return;
        }
        let languages = languages.clone();
        let cache = self.grammar.cache.clone();
        let disabled = self.settings.grammar_rules_off.clone();
        let editor = b.editor.clone();
        let color = cx.theme().info;
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            // The spelling's language, once known.
            let Ok((text, code)) = this.update(cx, |this, cx| {
                let b = this.buffers.iter().find(|b| b.editor == editor);
                let code = b.and_then(|b| b.spell.language).map(|d| d.code);
                (editor.read(cx).text().to_string(), code)
            }) else {
                return;
            };
            let language = code
                .and_then(|c| tex_spell::grammar::pick_language(c, &languages))
                .unwrap_or_else(|| "auto".to_string());
            let checked = text.clone();
            let found = cx
                .background_spawn(async move {
                    grammar::check(&server, &cache, &checked, &language, &disabled)
                })
                .await;
            this.update(cx, |this, cx| {
                let Some(b) = this.buffers.iter().find(|b| b.editor == editor) else {
                    return;
                };
                // Edited meanwhile: the next check is already on its way.
                if *b.editor.read(cx).text() != text {
                    return;
                }
                match found {
                    Ok(problems) => b.grammar.set(&b.editor, problems, color, cx),
                    Err(e) => {
                        log::warn!("LanguageTool: {e:#}");
                        this.grammar.state = ServerState::Unreachable(format!("{e:#}").into());
                        cx.notify();
                    }
                }
            })
            .ok();
        });
        self.buffers[ix].grammar.task = task;
    }

    /// The status bar item of grammar checking, when it is on.
    pub(super) fn render_grammar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.settings.grammar_check {
            return None;
        }
        let (icon, tooltip): (Glyph, SharedString) = match &self.grammar.state {
            ServerState::Off => return None,
            ServerState::Connecting if self.grammar.started_server() => {
                (Glyph::Spinner, "Starting LanguageTool…".into())
            }
            ServerState::Connecting => (Glyph::Spinner, "Connecting to LanguageTool…".into()),
            ServerState::Ready(_) => (Glyph::Check, "Grammar checked by LanguageTool".into()),
            ServerState::Unreachable(e) => (
                Glyph::Warning,
                format!("LanguageTool is not available: {e}. Click to try again.").into(),
            ),
        };
        Some(
            Button::new("grammar")
                .ghost()
                .icon(icon)
                .label("Grammar")
                .tooltip(tooltip)
                .on_click(cx.listener(|this, _, _, cx| {
                    if matches!(this.grammar.state, ServerState::Unreachable(_)) {
                        this.start_grammar(Duration::ZERO, cx);
                    }
                }))
                .into_any_element(),
        )
    }
}
