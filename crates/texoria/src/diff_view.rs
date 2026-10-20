//! The diff tab: a working-tree change, a conflicted file, a commit or what a
//! coding agent changed, shown with gpui-kit's read-only diff viewer.

use std::path::{Path, PathBuf};

use gpui_kit::component::diff::{
    Diff, DiffChangeIndicator, DiffEvent, DiffFile, DiffFileStatus, DiffHoverHighlight,
    DiffInlineUnit, DiffMode, DiffState,
};
use gpui_kit::component::{
    ActiveTheme, Disableable as _, Icon, Sizable,
    button::{Button, ButtonVariants},
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use tex_git::{Commit, Error, Repo};

use crate::assets::Glyph;
use crate::latex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffSource {
    /// Working tree vs index, or index vs HEAD when `staged`.
    Working {
        path: String,
        staged: bool,
        untracked: bool,
    },
    /// A file with conflict markers, to resolve.
    Conflict {
        path: String,
    },
    Commit(Commit),
    /// A coding agent's changes to the project in `dir`, as a patch.
    Agent {
        dir: PathBuf,
        patch: String,
    },
}

impl DiffSource {
    /// The file this diff is about, if a single one.
    pub fn path(&self) -> Option<&str> {
        match self {
            DiffSource::Working { path, .. } | DiffSource::Conflict { path } => Some(path),
            DiffSource::Commit(_) | DiffSource::Agent { .. } => None,
        }
    }
}

pub enum DiffViewEvent {
    /// Put `path` back to its version in commit `sha`.
    Restore {
        sha: String,
        path: String,
    },
    /// Every conflict in `path` is resolved: write `text` and stage it.
    Resolved {
        path: String,
        text: String,
    },
    Open(PathBuf),
    SplitChanged(bool),
    /// Undo the agent's changes to one file (relative path), or all of them.
    UndoAgent(Option<PathBuf>),
    KeepAgent,
    Close,
}

impl EventEmitter<DiffViewEvent> for DiffView {}

pub struct DiffView {
    /// `None` for an agent's changes.
    repo: Option<Repo>,
    source: DiffSource,
    state: Entity<DiffState>,
    split: bool,
    /// The text the current files were parsed from, to skip no-op reloads.
    loaded: Option<String>,
    error: Option<SharedString>,
    generation: u64,
    _subscription: Subscription,
}

impl DiffView {
    pub fn new(
        repo: Option<Repo>,
        source: DiffSource,
        split: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let state = cx.new(|cx| {
            DiffState::new(Vec::new(), cx)
                .with_mode(mode(split))
                // Prose: highlight changed words, not characters.
                .with_inline_unit(Some(DiffInlineUnit::Word))
        });
        let subscription = cx.subscribe(&state, |_, _, ev: &DiffEvent, cx| {
            if let DiffEvent::ConflictResolved(..) = ev {
                cx.notify();
            }
        });
        let mut this = DiffView {
            repo,
            source,
            state,
            split,
            loaded: None,
            error: None,
            generation: 0,
            _subscription: subscription,
        };
        this.load(cx);
        this
    }

    pub fn show(&mut self, repo: Option<Repo>, source: DiffSource, cx: &mut Context<Self>) {
        let root = |r: &Option<Repo>| r.as_ref().map(|r| r.root().to_path_buf());
        if self.source == source && root(&self.repo) == root(&repo) {
            return self.reload(cx);
        }
        self.repo = repo;
        self.source = source;
        self.loaded = None;
        self.error = None;
        self.state.update(cx, |s, cx| s.set_files(Vec::new(), cx));
        self.load(cx);
    }

    /// Whether it shows an agent's changes.
    pub fn is_agent(&self) -> bool {
        matches!(self.source, DiffSource::Agent { .. })
    }

    /// Re-read a working-tree diff (commits never change, and an agent's
    /// changes come with their patch).
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        if !matches!(
            self.source,
            DiffSource::Commit(_) | DiffSource::Agent { .. }
        ) {
            self.load(cx);
        }
    }

    /// Short label for the tab.
    pub fn title(&self) -> String {
        match &self.source {
            DiffSource::Working { path, staged, .. } => {
                let name = path.rsplit('/').next().unwrap_or(path);
                if *staged {
                    format!("{name} (staged)")
                } else {
                    format!("{name} (changes)")
                }
            }
            DiffSource::Conflict { path } => {
                format!("{} (conflict)", path.rsplit('/').next().unwrap_or(path))
            }
            DiffSource::Commit(c) => format!("Commit {}", c.short),
            DiffSource::Agent { .. } => "Agent changes".into(),
        }
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        let repo = self.repo.clone();
        let source = self.source.clone();
        cx.spawn(async move |this, cx| {
            let text = cx
                .background_spawn(async move { load_text(repo.as_ref(), &source) })
                .await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                match text {
                    Ok(text) if this.loaded.as_ref() == Some(&text) => return,
                    Ok(text) => {
                        match files_for(&this.source, &text) {
                            Ok(files) => {
                                this.error = None;
                                this.state.update(cx, |s, cx| s.set_files(files, cx));
                            }
                            Err(e) => this.error = Some(e.into()),
                        }
                        this.loaded = Some(text);
                    }
                    Err(e) => {
                        this.error = Some(e.to_string().into());
                        // The next load that works must show its text, even if unchanged.
                        this.loaded = None;
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn set_split(&mut self, split: bool, cx: &mut Context<Self>) {
        self.split = split;
        self.state.update(cx, |s, cx| s.set_mode(mode(split), cx));
        cx.emit(DiffViewEvent::SplitChanged(split));
        cx.notify();
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (title, subtitle): (SharedString, Option<SharedString>) = match &self.source {
            DiffSource::Working {
                path,
                staged,
                untracked,
            } => (
                path.clone().into(),
                Some(
                    match (staged, untracked) {
                        (true, _) => "Staged changes",
                        (_, true) => "New file",
                        _ => "Changes not staged",
                    }
                    .into(),
                ),
            ),
            DiffSource::Conflict { path } => (path.clone().into(), Some("Merge conflict".into())),
            DiffSource::Commit(c) => (
                c.subject.clone().into(),
                Some(format!("{} · {} <{}>", c.short, c.author, c.email).into()),
            ),
            DiffSource::Agent { .. } => {
                let n = self.state.read(cx).files().len();
                (
                    "Changes by the agent".into(),
                    Some(format!("{n} file{}", if n == 1 { "" } else { "s" }).into()),
                )
            }
        };
        let file = self
            .source
            .path()
            .zip(self.repo.as_ref())
            .map(|(p, repo)| repo.abs_path(p));
        let agent = self.is_agent();
        let resolved = match &self.source {
            DiffSource::Conflict { path } => self
                .state
                .read(cx)
                .resolved_text(path)
                .map(|text| (path.clone(), text)),
            _ => None,
        };
        let is_conflict = matches!(self.source, DiffSource::Conflict { .. });
        let split = self.split;

        h_flex()
            .h(px(38.))
            .flex_none()
            .px_3()
            .gap_2()
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.tab_bar)
            .child(
                Icon::new(if matches!(self.source, DiffSource::Commit(_)) {
                    Glyph::Commit
                } else if is_conflict {
                    Glyph::Conflict
                } else {
                    Glyph::File
                })
                .xsmall()
                .text_color(theme.muted_foreground),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_2()
                    .overflow_hidden()
                    .child(
                        div()
                            .flex_none()
                            .max_w(relative(0.7))
                            .truncate()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.foreground)
                            .child(title),
                    )
                    .when_some(subtitle, |d, s| {
                        d.child(
                            div()
                                .truncate()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(s),
                        )
                    }),
            )
            .when(agent, |d| {
                d.child(
                    Button::new("diff-agent-undo")
                        .outline()
                        .icon(Glyph::Discard)
                        .label("Undo All")
                        .tooltip("Put the files back as they were before the agent's turn")
                        .on_click(
                            cx.listener(|_, _, _, cx| cx.emit(DiffViewEvent::UndoAgent(None))),
                        ),
                )
                .child(
                    Button::new("diff-agent-keep")
                        .primary()
                        .icon(Glyph::Check)
                        .label("Keep All")
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(DiffViewEvent::KeepAgent))),
                )
            })
            .when(is_conflict, |d| {
                let ready = resolved.is_some();
                d.child(
                    Button::new("diff-save-resolution")
                        .primary()
                        .icon(Glyph::Check)
                        .label("Save Resolution")
                        .tooltip(if ready {
                            "Write the resolved file and mark it resolved"
                        } else {
                            "Accept a side for every conflict first"
                        })
                        .disabled(!ready)
                        .on_click(cx.listener(move |_, _, _, cx| {
                            if let Some((path, text)) = resolved.clone() {
                                cx.emit(DiffViewEvent::Resolved { path, text });
                            }
                        })),
                )
            })
            .when_some(file, |d, file| {
                d.child(
                    Button::new("diff-open")
                        .ghost()
                        .icon(Glyph::File)
                        .tooltip("Open the file")
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.emit(DiffViewEvent::Open(file.clone()))
                        })),
                )
            })
            .child(
                Button::new("diff-prev")
                    .ghost()
                    .icon(Glyph::PrevChange)
                    .tooltip("Previous change")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.state.update(cx, |s, cx| s.previous_change(cx))
                    })),
            )
            .child(
                Button::new("diff-next")
                    .ghost()
                    .icon(Glyph::NextChange)
                    .tooltip("Next change")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.state.update(cx, |s, cx| s.next_change(cx))
                    })),
            )
            .when(!is_conflict, |d| {
                d.child(
                    Button::new("diff-mode")
                        .ghost()
                        .icon(if split { Glyph::Unified } else { Glyph::Split })
                        .tooltip(if split {
                            "Unified view"
                        } else {
                            "Side by side"
                        })
                        .on_click(cx.listener(move |this, _, _, cx| this.set_split(!split, cx))),
                )
            })
            .child(
                Button::new("diff-close")
                    .ghost()
                    .icon(Glyph::Close)
                    .tooltip("Close (⌘W)")
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(DiffViewEvent::Close))),
            )
    }
}

impl Render for DiffView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let mut diff = Diff::new(&self.state)
            .line_number(true)
            .syntax_highlight(true)
            .soft_wrap(true)
            .change_indicator(DiffChangeIndicator::Bars)
            .hover_highlight(DiffHoverHighlight::Line)
            .size_full();
        if let DiffSource::Commit(c) = &self.source {
            let sha = c.sha.clone();
            let weak = cx.entity().downgrade();
            diff = diff.render_header_suffix(move |file: &DiffFile, _, _| {
                let restorable = !file.is_binary() && file.status() != DiffFileStatus::Deleted;
                let (weak, sha, path) = (weak.clone(), sha.clone(), file.path().to_string());
                Button::new(SharedString::from(format!("restore-{path}")))
                    .ghost()
                    .icon(Glyph::Discard)
                    .label("Restore")
                    .tooltip("Put this version of the file back in the project")
                    .when(!restorable, |b| b.disabled(true))
                    .on_click(move |_, _, cx| {
                        let (sha, path) = (sha.clone(), path.clone());
                        weak.update(cx, |_, cx| cx.emit(DiffViewEvent::Restore { sha, path }))
                            .ok();
                    })
            });
        }
        if let DiffSource::Agent { dir, .. } = &self.source {
            let dir = dir.clone();
            let weak = cx.entity().downgrade();
            diff = diff.render_header_suffix(move |file: &DiffFile, _, _| {
                let path = PathBuf::from(file.path().as_str());
                let (weak_open, weak_undo) = (weak.clone(), weak.clone());
                let (open, undo) = (dir.join(&path), path.clone());
                let exists = file.status() != DiffFileStatus::Deleted;
                h_flex()
                    .gap_1()
                    .when(exists, |d| {
                        d.child(
                            Button::new(SharedString::from(format!("open-{}", path.display())))
                                .ghost()
                                .icon(Glyph::File)
                                .tooltip("Open the file")
                                .on_click(move |_, _, cx| {
                                    let open = open.clone();
                                    weak_open
                                        .update(cx, |_, cx| cx.emit(DiffViewEvent::Open(open)))
                                        .ok();
                                }),
                        )
                    })
                    .child(
                        Button::new(SharedString::from(format!("undo-{}", path.display())))
                            .ghost()
                            .icon(Glyph::Discard)
                            .label("Undo")
                            .tooltip("Put this file back as it was")
                            .on_click(move |_, _, cx| {
                                let path = undo.clone();
                                weak_undo
                                    .update(cx, |_, cx| {
                                        cx.emit(DiffViewEvent::UndoAgent(Some(path)))
                                    })
                                    .ok();
                            }),
                    )
            });
        }
        v_flex()
            .size_full()
            .bg(theme.background)
            .child(self.render_toolbar(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .when_some(self.error.clone(), |d, e| {
                        d.p_4().text_sm().text_color(theme.danger).child(e)
                    })
                    .when(self.error.is_none(), |d| d.child(diff)),
            )
    }
}

fn mode(split: bool) -> DiffMode {
    if split {
        DiffMode::Split
    } else {
        DiffMode::Unified
    }
}

fn load_text(repo: Option<&Repo>, source: &DiffSource) -> tex_git::Result<String> {
    let repo = || repo.ok_or_else(|| Error::Invalid("No repository".into()));
    match source {
        DiffSource::Working {
            path,
            staged,
            untracked,
        } => {
            if *untracked {
                repo()?.untracked_diff(path)
            } else {
                repo()?.diff(path, *staged)
            }
        }
        DiffSource::Conflict { path } => std::fs::read(repo()?.abs_path(path))
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .map_err(Error::Spawn),
        DiffSource::Commit(c) => repo()?.show(&c.sha),
        DiffSource::Agent { patch, .. } => Ok(patch.clone()),
    }
}

fn files_for(source: &DiffSource, text: &str) -> Result<Vec<DiffFile>, String> {
    let files = match source {
        DiffSource::Conflict { path } => {
            vec![DiffFile::parse_conflicts(path.clone(), text).map_err(|e| e.to_string())?]
        }
        _ => DiffFile::parse(text).map_err(|e| e.to_string())?,
    };
    Ok(files
        .into_iter()
        .map(|f| {
            let path = Path::new(f.path().as_ref());
            if crate::paths::is_latex(path) || crate::paths::is_bibtex(path) {
                let language = latex::language_for(path);
                f.with_language(language)
            } else {
                f
            }
        })
        .collect())
}

#[cfg(test)]
mod tests;
