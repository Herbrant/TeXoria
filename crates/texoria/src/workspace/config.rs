//! Settings: applying them, the font size, the theme and the engine.

use super::*;

impl Workspace {
    /// Change the settings, save them and bring what is running in line.
    pub fn update_settings(
        &mut self,
        change: impl FnOnce(&mut Settings),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let old = self.settings.clone();
        change(&mut self.settings);
        self.settings.save();

        if self.settings.engine != old.engine
            && let Some(ix) = self
                .settings
                .engine
                .as_ref()
                .and_then(|label| self.engines.iter().position(|e| &e.label() == label))
            && ix != self.engine_ix
        {
            self.set_engine(ix, cx);
        }
        if self.settings.compile_timeout_secs != old.compile_timeout_secs
            && let Some(c) = &self.compiler
        {
            c.set_timeout(self.settings.compile_timeout());
        }
        if self.settings.soft_wrap != old.soft_wrap {
            let wrap = self.settings.soft_wrap;
            for b in &mut self.buffers {
                let lines = b.editor.read(cx).text().lines_len();
                b.soft_wrap = wrap && lines <= SOFT_WRAP_MAX_LINES;
                let wrap = b.soft_wrap;
                b.editor
                    .update(cx, |state, cx| state.set_soft_wrap(wrap, window, cx));
            }
        }
        let s = &self.settings;
        if (s.pull_rebase, s.git_timeout_secs, &s.overleaf_servers)
            != (old.pull_rebase, old.git_timeout_secs, &old.overleaf_servers)
        {
            self.configure_git(cx);
        }
        if self.settings.diff_split != old.diff_split
            && let Some(d) = &self.diff
        {
            let split = self.settings.diff_split;
            d.update(cx, |d, cx| d.set_split(split, cx));
        }
        if self.settings.trusted_projects != old.trusted_projects {
            self.sync_git(cx);
        }
        if self.settings.shell_escape_allowed != old.shell_escape_allowed
            && let Some(job) = self.job()
            && job.shell_escape != old.allows_shell_escape(job.work_dir())
        {
            self.build(cx);
        }
        if self.settings.rc_allowed != old.rc_allowed
            && let Some(job) = self.job()
            && job.allow_project_rc != old.allows_rc(job.work_dir())
        {
            self.untrusted_rc = !job.allow_project_rc && job.has_project_rc();
            self.build(cx);
        }
        let s = &self.settings;
        if (&s.agent, &s.custom_agents) != (&old.agent, &old.custom_agents) {
            let (preferred, custom) = (s.agent.clone(), s.custom_agents.clone());
            self.agent
                .update(cx, |a, cx| a.configure(preferred, custom, cx));
        }
        if self.settings.agent_projects != old.agent_projects {
            self.sync_agent(cx);
        }
        let s = &self.settings;
        if (s.grammar_check, &s.languagetool_url, s.languagetool_start)
            != (
                old.grammar_check,
                &old.languagetool_url,
                old.languagetool_start,
            )
        {
            // The URL changes as it is typed.
            self.start_grammar(Duration::from_secs(1), cx);
        } else if s.grammar_rules_off != old.grammar_rules_off {
            for ix in 0..self.buffers.len() {
                self.grammar_check(ix, Duration::ZERO, cx);
            }
        }
        if self.settings.vim_mode != old.vim_mode {
            let on = self.settings.vim_mode;
            self.set_vim_mode(on, cx);
        }
        if self.settings.live_checks != old.live_checks {
            self.reindex(cx);
        }
        if self.settings.theme != old.theme {
            theme::apply(self.settings.theme, Some(window), cx);
            self.sync_theme(cx);
        }
        let s = &self.settings;
        if (s.spell_check, &s.spell_words, &s.spell_languages)
            != (old.spell_check, &old.spell_words, &old.spell_languages)
        {
            self.spell.set_words(&self.settings.spell_words);
            for ix in 0..self.buffers.len() {
                let path = &self.buffers[ix].path;
                if self.settings.spell_language(path) != old.spell_language(path) {
                    // Back to detecting it, or a new choice.
                    self.buffers[ix].spell.language = None;
                }
                self.spellcheck(ix, Duration::ZERO, cx);
            }
        }
        cx.notify();
    }

    /// Resize the editor's text (⌘= / ⌘- / ⌘0), within the allowed range.
    pub(super) fn set_font_size(&mut self, size: f32, window: &mut Window, cx: &mut Context<Self>) {
        let size = size.clamp(Settings::MIN_FONT_SIZE, Settings::MAX_FONT_SIZE);
        if size != self.settings.font_size() {
            self.update_settings(|s| s.editor_font_size = size, window, cx);
        }
    }

    /// Open the Settings window, or bring it to the front.
    pub(super) fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Its Agents page lists them.
        self.agent.update(cx, |a, cx| a.activate(cx));
        if let Some(handle) = self.settings_window
            && handle
                .update(cx, |_, window, _| window.activate_window())
                .is_ok()
        {
            return;
        }
        let workspace = cx.entity().downgrade();
        let main = window.window_handle();
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::centered(size(px(820.), px(600.)), cx)),
            window_min_size: Some(size(px(560.), px(400.))),
            app_id: Some("texoria".into()),
            ..TitleBar::window_options()
        };
        // The new window reads the workspace, which is being updated here.
        cx.defer(move |cx| {
            let opened = gpui_kit::open_window(options, cx, {
                let workspace = workspace.clone();
                move |window, cx| cx.new(|cx| SettingsWindow::new(workspace, main, window, cx))
            });
            match opened {
                Ok((handle, _)) => {
                    let _ = workspace.update(cx, |w, _| w.settings_window = Some(handle));
                }
                Err(e) => log::error!("opening the settings window: {e}"),
            }
        });
    }

    pub(super) fn toggle_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let choice = theme::toggled(cx);
        self.update_settings(|s| s.theme = choice, window, cx);
    }

    pub(super) fn next_engine(&mut self, cx: &mut Context<Self>) {
        if self.engines.is_empty() {
            return;
        }
        self.set_engine((self.engine_ix + 1) % self.engines.len(), cx);
    }

    pub(super) fn set_engine(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.engine_ix = ix;
        let engine = self.engines[ix].clone();
        self.settings.engine = Some(engine.label());
        self.settings.save();
        if let Some(c) = &self.compiler {
            c.set_engine(engine);
        }
        self.build(cx);
        cx.notify();
    }

    pub(super) fn sync_theme(&mut self, cx: &mut Context<Self>) {
        let dark = cx.theme_is_dark();
        if dark != self.was_dark {
            self.was_dark = dark;
            self.pdf.update(cx, |pdf, cx| pdf.set_dark(dark, cx));
            // Underlines take their color from the theme.
            for ix in 0..self.buffers.len() {
                self.spellcheck(ix, Duration::ZERO, cx);
                self.grammar_check(ix, Duration::ZERO, cx);
            }
            self.reindex(cx);
        }
    }

    pub(super) fn toggle_live(&mut self, cx: &mut Context<Self>) {
        self.settings.live_preview = !self.settings.live_preview;
        self.settings.save();
        cx.notify();
    }
}
