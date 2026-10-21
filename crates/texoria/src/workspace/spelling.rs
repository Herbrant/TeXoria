//! Spell checking: the checks, the dictionaries and the language picker.

use super::*;

impl Workspace {
    /// Check buffer `ix` after `delay` (replacing a pending check).
    pub(super) fn spellcheck(&mut self, ix: usize, delay: Duration, cx: &mut Context<Self>) {
        let Some(b) = self.buffers.get_mut(ix) else {
            return;
        };
        let chosen = self.settings.spell_language(&b.path);
        if !self.settings.spell_check || chosen == Some(SpellLanguage::OFF) {
            b.spell.language = None;
            b.spell.clear(cx);
            return;
        }
        // An unknown code (a hand-edited setting) means detecting it.
        let chosen = chosen.and_then(catalog::get).map(|l| l.code);
        let path = b.path.clone();
        let editor = b.editor.clone();
        let previous = b.spell.language;
        let root = self.root.clone().filter(|r| *r != path);
        let region = self.spell.region.clone();
        let fallback = self.spell.fallback;
        let words = self.spell.words.clone();
        let color = cx.theme().danger;
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            // Read once the typing pauses, not on every keystroke: the texts and
            // indexes are as big as the files.
            let Ok(job) = this.update(cx, |this, cx| {
                // Only detecting the language looks at the root and the indexes.
                let (root_text, index, root_index) = match chosen {
                    Some(_) => (None, None, None),
                    None => {
                        let root_text = root
                            .as_ref()
                            .and_then(|r| this.buffers.iter().find(|b| &b.path == r))
                            .map(|b| b.editor.read(cx).text().to_string());
                        let project = this.project.read().unwrap();
                        let index = project.index(&path).cloned();
                        let root_index = root.as_ref().and_then(|r| project.index(r).cloned());
                        (root_text, index, root_index)
                    }
                };
                spell::Job {
                    text: editor.read(cx).text().to_string(),
                    chosen,
                    previous,
                    index,
                    root,
                    root_text,
                    root_index,
                    region,
                    fallback,
                }
            }) else {
                return;
            };
            let text = job.text.clone();
            let analysis = cx
                .background_spawn(async move { spell::analyze(job) })
                .await;
            let speller = this
                .update(cx, |this, cx| {
                    let speller = this.dictionary(analysis.language.code, cx);
                    let ix = this.buffers.iter().position(|b| b.editor == editor)?;
                    let changed = this.buffers[ix].spell.language.map(|d| d.code)
                        != Some(analysis.language.code);
                    if changed {
                        // Grammar is checked in the spelling's language.
                        this.buffers[ix].spell.language = Some(analysis.language);
                        this.grammar_check(ix, Duration::ZERO, cx);
                    }
                    let b = &mut this.buffers[ix];
                    b.spell.language = Some(analysis.language);
                    *b.spell.speller.borrow_mut() = speller.clone();
                    if speller.is_none() {
                        // Underlines from another language would be wrong;
                        // loading the dictionary checks the buffer again.
                        b.spell.underlines.clear(cx);
                    }
                    cx.notify();
                    speller
                })
                .ok()
                .flatten();
            let Some(speller) = speller else {
                return;
            };
            let (checked, words_in) = (text.clone(), analysis.words);
            let misspelled = cx
                .background_spawn(async move { speller.misspelled(&checked, &words_in, &words) })
                .await;
            this.update(cx, |this, cx| {
                let Some(b) = this.buffers.iter().find(|b| b.editor == editor) else {
                    return;
                };
                // Edited meanwhile: the next check is already on its way.
                if *b.editor.read(cx).text() != text {
                    return;
                }
                b.spell
                    .underlines
                    .set(spell::underlines(misspelled, color), cx);
            })
            .ok();
        });
        self.buffers[ix].spell.task = task;
    }

    /// The dictionary for `code`, or `None` while it loads (and checks the
    /// buffers in that language again once it has).
    pub(super) fn dictionary(
        &mut self,
        code: &'static str,
        cx: &mut Context<Self>,
    ) -> Option<Arc<Speller>> {
        match self.spell.dictionaries.get(code) {
            Some(Dictionary::Ready(s)) => return Some(s.clone()),
            Some(_) => return None,
            None => {}
        }
        self.spell.dictionaries.insert(code, Dictionary::Loading);
        let store = self.spell.store.clone();
        cx.background_then(
            move || store.load(code),
            move |this, loaded, cx| {
                let entry = match loaded {
                    Ok(s) => Dictionary::Ready(Arc::new(s)),
                    Err(e) => {
                        log::warn!("spell checking dictionary {code}: {e:#}");
                        Dictionary::Failed(format!("{e:#}").into())
                    }
                };
                this.spell.dictionaries.insert(code, entry);
                for ix in 0..this.buffers.len() {
                    if this.buffers[ix]
                        .spell
                        .language
                        .is_some_and(|d| d.code == code)
                    {
                        this.spellcheck(ix, Duration::ZERO, cx);
                    }
                }
                cx.notify();
            },
        )
        .detach();
        None
    }

    /// Try a dictionary that failed to load (offline) again.
    pub(super) fn retry_dictionary(&mut self, code: &str, cx: &mut Context<Self>) {
        if matches!(
            self.spell.dictionaries.get(code),
            Some(Dictionary::Failed(_))
        ) {
            self.spell.dictionaries.remove(code);
        }
        self.spellcheck(self.active, Duration::ZERO, cx);
    }

    /// Choose the active file's language (`None`: detect it).
    pub(super) fn set_spell_language(
        &mut self,
        language: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(path) = self.buffers.get(self.active).map(|b| b.path.clone()) else {
            return;
        };
        self.update_settings(|s| s.set_spell_language(&path, language), window, cx);
    }

    pub(super) fn replace_misspelling(
        &mut self,
        action: &ReplaceMisspelling,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(b) = self.buffers.get(self.active) else {
            return;
        };
        let range = action.range.clone();
        let text = action.text.clone();
        b.editor.update(cx, |state, cx| {
            if range.end > state.text().len() {
                return;
            }
            state.set_selected_range(range, cx);
            state.replace(text, window, cx);
        });
    }

    /// The active file's language, with the menu to change it.
    pub(super) fn render_language(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let b = &self.buffers[self.active];
        let chosen = self.settings.spell_language(&b.path).map(str::to_string);
        let language = b.spell.language;
        let dictionary = language.and_then(|d| self.spell.dictionaries.get(d.code));
        let (icon, tooltip): (Glyph, SharedString) = match (language, dictionary) {
            (None, _) if chosen.as_deref() == Some(SpellLanguage::OFF) => (
                Glyph::Languages,
                "Spelling is not checked in this file".into(),
            ),
            (None, _) => (Glyph::Spinner, "Checking spelling…".into()),
            (Some(d), Some(Dictionary::Loading)) => (
                Glyph::Spinner,
                format!("Getting the {} dictionary…", catalog::name(d.code)).into(),
            ),
            (Some(_), Some(Dictionary::Failed(e))) => (
                Glyph::Warning,
                format!("The dictionary could not be loaded: {e}").into(),
            ),
            (Some(d), _) => {
                let why = match d.source {
                    Source::User => "chosen for this file",
                    Source::Magic => "set by % !TeX spellcheck",
                    Source::Package => "set with babel/polyglossia",
                    Source::Text => "recognized from the text",
                    Source::Fallback => "the system language",
                };
                (
                    Glyph::Languages,
                    format!("Spelling: {}, {why}", catalog::name(d.code)).into(),
                )
            }
        };
        let label = match (&chosen, language) {
            (Some(c), _) if c == SpellLanguage::OFF => "Spelling Off".into(),
            (_, Some(d)) => SharedString::from(catalog::name(d.code).to_string()),
            _ => "Spelling".into(),
        };
        let failed = match dictionary {
            Some(Dictionary::Failed(_)) => language.map(|d| d.code),
            _ => None,
        };
        let current = language.map(|d| d.code);
        let weak = cx.entity().downgrade();
        Button::new("spell-language")
            .ghost()
            .icon(icon)
            .label(label)
            .tooltip(tooltip)
            .dropdown_menu_with_anchor(Anchor::BottomRight, move |menu, window, _| {
                let detected = chosen.is_none();
                let off = chosen.as_deref() == Some(SpellLanguage::OFF);
                let pick = |language: Option<String>| {
                    let weak = weak.clone();
                    move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                        let language = language.clone();
                        weak.update(cx, |this, cx| this.set_spell_language(language, window, cx))
                            .ok();
                    }
                };
                let mut menu = menu
                    .scrollable(true)
                    .max_h(window.viewport_size().height * 0.7)
                    .item(
                        PopupMenuItem::new("Detect Automatically")
                            .checked(detected)
                            .on_click(pick(None)),
                    )
                    .item(
                        PopupMenuItem::new("Don't Check This File")
                            .checked(off)
                            .on_click(pick(Some(SpellLanguage::OFF.to_string()))),
                    );
                if let Some(code) = failed {
                    let weak = weak.clone();
                    menu = menu.item(PopupMenuItem::new("Retry Loading the Dictionary").on_click(
                        move |_, _, cx| {
                            weak.update(cx, |this, cx| this.retry_dictionary(code, cx))
                                .ok();
                        },
                    ));
                }
                menu = menu.separator();
                for l in catalog::LANGUAGES {
                    menu = menu.item(
                        PopupMenuItem::new(l.name)
                            .checked(!off && current == Some(l.code))
                            .on_click(pick(Some(l.code.to_string()))),
                    );
                }
                menu
            })
    }
}
