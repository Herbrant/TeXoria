//! The Search view of the sidebar: find, and replace, in every text file of
//! the project.
//!
//! The search runs on the background executor over the open buffers' text
//! (given by the workspace, [`SearchPanelEvent::Search`]) and the files on
//! disk. Replacing goes through the workspace, which edits open buffers in
//! the editor (so it can be undone) and writes the other files.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{
    ActiveTheme, Disableable as _, Icon, Selectable as _, Sizable,
    button::{Button, ButtonVariants},
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use tex_project::search::{self, Query, Results};

use crate::assets::Glyph;

/// Pause after a keystroke before searching.
const DELAY: Duration = Duration::from_millis(200);

pub enum SearchPanelEvent {
    /// Search again: the workspace answers with [`SearchPanel::run`].
    Search,
    Open {
        path: PathBuf,
        range: Range<usize>,
    },
    /// Replace every match of `query` in `files`.
    ReplaceAll {
        query: Query,
        replacement: String,
        files: Vec<PathBuf>,
    },
}

impl EventEmitter<SearchPanelEvent> for SearchPanel {}

/// A row of the results: a file, or one of its matches.
#[derive(Clone, Copy)]
enum Row {
    File(usize),
    Match(usize, usize),
}

pub struct SearchPanel {
    dir: Option<PathBuf>,
    query: Entity<InputState>,
    replacement: Entity<InputState>,
    case_sensitive: bool,
    whole_word: bool,
    regex: bool,
    show_replace: bool,
    results: Results,
    rows: Vec<Row>,
    collapsed: HashSet<PathBuf>,
    error: Option<SharedString>,
    searching: bool,
    /// Stops the search in progress.
    cancel: Arc<AtomicBool>,
    scroll: UniformListScrollHandle,
    _debounce: Task<()>,
    _search: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl SearchPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query = cx.new(|cx| InputState::new(window, cx).placeholder("Search"));
        let replacement = cx.new(|cx| InputState::new(window, cx).placeholder("Replace"));
        let subscriptions =
            vec![
                cx.subscribe_in(&query, window, |this, _, ev: &InputEvent, _, cx| match ev {
                    InputEvent::Change => this.schedule(DELAY, cx),
                    InputEvent::PressEnter { .. } => this.schedule(Duration::ZERO, cx),
                    _ => {}
                }),
            ];
        Self {
            dir: None,
            query,
            replacement,
            case_sensitive: false,
            whole_word: false,
            regex: false,
            show_replace: false,
            results: Results::default(),
            rows: Vec::new(),
            collapsed: HashSet::new(),
            error: None,
            searching: false,
            cancel: Arc::new(AtomicBool::new(false)),
            scroll: UniformListScrollHandle::new(),
            _debounce: Task::ready(()),
            _search: Task::ready(()),
            _subscriptions: subscriptions,
        }
    }

    /// Search in the folder of the project (none: nothing to search).
    pub fn set_dir(&mut self, dir: Option<PathBuf>, cx: &mut Context<Self>) {
        if dir != self.dir {
            self.dir = dir;
            self.set_results(Results::default());
            self.schedule(Duration::ZERO, cx);
        }
    }

    /// Focus the search field, with its text selected; `text` replaces it.
    pub fn focus(&mut self, text: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        self.query.update(cx, |q, cx| {
            if let Some(text) = text {
                q.set_value(text, window, cx);
            }
            q.focus(window, cx);
            q.select_all(window, cx);
        });
        self.schedule(Duration::ZERO, cx);
    }

    fn query(&self, cx: &App) -> Option<Query> {
        let text = self.query.read(cx).value().to_string();
        (!text.is_empty()).then_some(Query {
            text,
            case_sensitive: self.case_sensitive,
            whole_word: self.whole_word,
            regex: self.regex,
        })
    }

    fn schedule(&mut self, delay: Duration, cx: &mut Context<Self>) {
        self._debounce = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            this.update(cx, |_, cx| cx.emit(SearchPanelEvent::Search))
                .ok();
        });
    }

    /// Search, reading `open` (the open buffers) instead of their files.
    pub fn run(&mut self, open: HashMap<PathBuf, Arc<str>>, cx: &mut Context<Self>) {
        self.cancel.store(true, Ordering::Relaxed);
        let (Some(dir), Some(query)) = (self.dir.clone(), self.query(cx)) else {
            self.error = None;
            self.searching = false;
            self.set_results(Results::default());
            cx.notify();
            return;
        };
        let regex = match query.compile() {
            Ok(r) => r,
            Err(e) => {
                self.error = Some(e.into());
                self.searching = false;
                self.set_results(Results::default());
                cx.notify();
                return;
            }
        };
        self.error = None;
        self.searching = true;
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = cancel.clone();
        self._search = cx.spawn(async move |this, cx| {
            let results = cx
                .background_spawn(async move { search::search(&dir, &regex, &open, &cancel) })
                .await;
            this.update(cx, |this, cx| {
                this.searching = false;
                this.set_results(results);
                cx.notify();
            })
            .ok();
        });
        cx.notify();
    }

    fn set_results(&mut self, results: Results) {
        self.results = results;
        self.update_rows();
    }

    fn update_rows(&mut self) {
        self.rows.clear();
        for (f, file) in self.results.files.iter().enumerate() {
            self.rows.push(Row::File(f));
            if !self.collapsed.contains(&file.path) {
                self.rows
                    .extend((0..file.matches.len()).map(|m| Row::Match(f, m)));
            }
        }
    }

    fn toggle(&mut self, option: fn(&mut Self) -> &mut bool, cx: &mut Context<Self>) {
        let value = option(self);
        *value = !*value;
        self.schedule(Duration::ZERO, cx);
        cx.notify();
    }

    fn replace_all(&mut self, cx: &mut Context<Self>) {
        let Some(query) = self.query(cx) else {
            return;
        };
        if self.results.files.is_empty() {
            return;
        }
        let replacement = self.replacement.read(cx).value().to_string();
        let files = self.results.files.iter().map(|f| f.path.clone()).collect();
        cx.emit(SearchPanelEvent::ReplaceAll {
            query,
            replacement,
            files,
        });
    }

    fn relative<'a>(&self, path: &'a Path) -> &'a Path {
        self.dir
            .as_ref()
            .and_then(|d| path.strip_prefix(d).ok())
            .unwrap_or(path)
    }

    fn render_row(&self, row: Row, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        match row {
            Row::File(f) => {
                let file = &self.results.files[f];
                let path = file.path.clone();
                let collapsed = self.collapsed.contains(&path);
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let folder = self
                    .relative(&path)
                    .parent()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default();
                h_flex()
                    .id(("search-file", ix))
                    .h(px(26.))
                    .px_2()
                    .gap_1p5()
                    .mx_1()
                    .rounded(theme.radius)
                    .text_sm()
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.sidebar_accent))
                    .child(
                        Icon::new(Glyph::Expand)
                            .small()
                            .text_color(theme.muted_foreground)
                            .when(!collapsed, |i| {
                                i.transform(Transformation::rotate(percentage(0.25)))
                            }),
                    )
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.sidebar_foreground)
                            .child(name),
                    )
                    .child(
                        div()
                            .flex_1()
                            .truncate()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(folder),
                    )
                    .child(
                        div()
                            .px_1p5()
                            .rounded_full()
                            .bg(theme.muted)
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(file.matches.len().to_string()),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.collapsed.remove(&path) {
                            this.collapsed.insert(path.clone());
                        }
                        this.update_rows();
                        cx.notify();
                    }))
                    .into_any_element()
            }
            Row::Match(f, m) => {
                let file = &self.results.files[f];
                let found = &file.matches[m];
                let (path, range) = (file.path.clone(), found.range.clone());
                let highlight = HighlightStyle {
                    background_color: Some(theme.warning.opacity(0.35)),
                    font_weight: Some(FontWeight::SEMIBOLD),
                    ..Default::default()
                };
                let text = StyledText::new(found.preview.clone())
                    .with_highlights([(found.highlight.clone(), highlight)]);
                h_flex()
                    .id(("search-match", ix))
                    .h(px(24.))
                    .pl(px(30.))
                    .pr_2()
                    .gap_2()
                    .mx_1()
                    .rounded(theme.radius)
                    .text_sm()
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.sidebar_accent))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(theme.sidebar_foreground)
                            .child(text),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child((found.line + 1).to_string()),
                    )
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(SearchPanelEvent::Open {
                            path: path.clone(),
                            range: range.clone(),
                        })
                    }))
                    .into_any_element()
            }
        }
    }
}

impl Render for SearchPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let option = |id: &'static str, icon: Glyph, tooltip: &'static str, on: bool| {
            Button::new(id)
                .ghost()
                .xsmall()
                .icon(icon)
                .tooltip(tooltip)
                .selected(on)
                .toggled(on)
        };
        let options = h_flex()
            .gap_0p5()
            .child(
                option(
                    "search-case",
                    Glyph::CaseSensitive,
                    "Match Case",
                    self.case_sensitive,
                )
                .on_click(cx.listener(|this, _, _, cx| this.toggle(|t| &mut t.case_sensitive, cx))),
            )
            .child(
                option(
                    "search-word",
                    Glyph::WholeWord,
                    "Match Whole Word",
                    self.whole_word,
                )
                .on_click(cx.listener(|this, _, _, cx| this.toggle(|t| &mut t.whole_word, cx))),
            )
            .child(
                option(
                    "search-regex",
                    Glyph::Regex,
                    "Use Regular Expression",
                    self.regex,
                )
                .on_click(cx.listener(|this, _, _, cx| this.toggle(|t| &mut t.regex, cx))),
            );
        let count = self.results.count();
        let summary: Option<SharedString> = if let Some(e) = &self.error {
            Some(e.clone())
        } else if self.searching {
            Some("Searching…".into())
        } else if self.query(cx).is_none() {
            None
        } else if count == 0 {
            Some("No results".into())
        } else {
            let files = self.results.files.len();
            Some(
                format!(
                    "{count}{} result{} in {files} file{}",
                    if self.results.truncated { "+" } else { "" },
                    if count == 1 { "" } else { "s" },
                    if files == 1 { "" } else { "s" },
                )
                .into(),
            )
        };

        let header = v_flex()
            .px_3()
            .pt_3()
            .pb_2()
            .gap_1p5()
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Button::new("search-toggle-replace")
                            .ghost()
                            .xsmall()
                            .icon(Glyph::Expand)
                            .tooltip("Replace")
                            .when(self.show_replace, |b| {
                                b.icon(
                                    Icon::new(Glyph::Expand)
                                        .transform(Transformation::rotate(percentage(0.25))),
                                )
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.show_replace = !this.show_replace;
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .child(Input::new(&self.query).small().suffix(options)),
                    ),
            )
            .when(self.show_replace, |d| {
                d.child(
                    h_flex()
                        .gap_1()
                        .pl(px(26.))
                        .child(div().flex_1().child(Input::new(&self.replacement).small()))
                        .child(
                            Button::new("search-replace-all")
                                .ghost()
                                .xsmall()
                                .icon(Glyph::ReplaceAll)
                                .tooltip("Replace All")
                                .disabled(count == 0 || self.searching)
                                .on_click(cx.listener(|this, _, _, cx| this.replace_all(cx))),
                        ),
                )
            })
            .when_some(summary, |d, s| {
                d.child(
                    div()
                        .pl(px(26.))
                        .text_xs()
                        .text_color(if self.error.is_some() {
                            theme.danger
                        } else {
                            theme.muted_foreground
                        })
                        .child(s),
                )
            });

        let rows = self.rows.len();
        v_flex().size_full().child(header).child(
            uniform_list(
                "search-results",
                rows,
                cx.processor(|this, range: Range<usize>, _, cx| {
                    range
                        .map(|ix| this.render_row(this.rows[ix], ix, cx))
                        .collect::<Vec<_>>()
                }),
            )
            .track_scroll(&self.scroll)
            .flex_1()
            .pb_2(),
        )
    }
}
