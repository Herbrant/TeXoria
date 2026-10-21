//! Cloning a repository, or an Overleaf project.

use std::cell::Cell;

use gpui_kit::component::radio::RadioGroup;

use super::*;

impl Workspace {
    pub(super) fn prompt_clone(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.cloning.is_some() {
            return;
        }
        let weak = cx.entity().downgrade();
        dialogs::prompt_text(
            window,
            cx,
            "Clone Repository",
            "Choose Folder…",
            "https://github.com/you/paper.git",
            "The URL of a Git repository (GitHub, GitLab, Overleaf…). Next, pick the folder to \
             put it in.",
            move |url, window, cx| {
                weak.update(cx, |this, cx| {
                    // Overleaf needs a token, which a plain clone cannot ask for.
                    match this.overleaf_project(&url).filter(|_| url.contains("://")) {
                        Some(project) => this.open_overleaf(project, window, cx),
                        None => this.choose_clone_dir(url, window, cx),
                    }
                })
                .ok();
                true
            },
        );
    }

    pub(super) fn choose_clone_dir(
        &mut self,
        url: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(name) = tex_git::clone_dir_name(&url) else {
            window.push_notification(Notification::error("That is not a repository URL"), cx);
            return;
        };
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Clone Here".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Some(parent) = paths
                .await
                .ok()
                .and_then(|r| r.ok())
                .flatten()
                .and_then(|p| p.into_iter().next())
            else {
                return;
            };
            this.update_in(cx, |this, window, cx| {
                this.clone_into(url, parent.join(name), None, window, cx)
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn clone_into(
        &mut self,
        url: String,
        dest: PathBuf,
        credential: Option<Credential>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.cloning.is_some() {
            return;
        }
        let cancel = Cancel::new();
        self.cloning = Some((url.clone().into(), cancel.clone()));
        let timeout = self.settings.git_timeout();
        let job = (url.clone(), dest.clone(), credential.clone());
        cx.background_then_in(
            window,
            move || {
                let overleaf = credential.is_some();
                let git = Git::find()
                    .ok_or_else(|| tex_git::Error::Invalid("Git is not installed".into()))?
                    .network_timeout(timeout)
                    .credential(credential);
                let repo = git.clone_repo(&url, &dest, &cancel)?;
                // Overleaf compiles on its side: local build files must not
                // reach the collaborators.
                if overleaf && let Err(e) = repo.exclude_local_files() {
                    log::warn!("{e}");
                }
                let dir = repo.root().to_path_buf();
                let sources = tex_files(&dir);
                Ok((dir, sources))
            },
            move |this, result, window, cx| {
                this.cloning = None;
                let (url, dest, credential) = job;
                match result {
                    Ok((dir, sources)) => {
                        if let Some(c) = &credential {
                            overleaf::save_token(&c.url, &c.password, window, cx);
                        }
                        this.choose_main_file(dir, sources, window, cx);
                    }
                    Err(tex_git::Error::Cancelled) => {}
                    Err(tex_git::Error::Auth { .. }) if let Some(c) = credential => {
                        // Kept until replaced: the token may be fine, and only this
                        // project out of reach.
                        let weak = cx.entity().downgrade();
                        let server = c.url.clone();
                        overleaf::prompt_token(
                            &c.url,
                            overleaf::TOKEN_REJECTED,
                            move |token, window, cx| {
                                let credential = overleaf::credential(&server, token);
                                weak.update(cx, |this, cx| {
                                    this.clone_into(
                                        url.clone(),
                                        dest.clone(),
                                        Some(credential),
                                        window,
                                        cx,
                                    )
                                })
                                .ok();
                            },
                            window,
                            cx,
                        );
                    }
                    Err(e) => git_panel::notify_error(window, cx, "Clone failed", &e),
                }
                cx.notify();
            },
        )
        .detach();
        cx.notify();
    }

    /// Open the clone in `dir` on the one of its `.tex` files (`sources`) that the user picks
    /// as the main file, or on the only one: no file is assumed to be the document.
    pub(super) fn choose_main_file(
        &mut self,
        dir: PathBuf,
        sources: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match sources.as_slice() {
            [] => {
                self.state =
                    CompileState::Failed(format!("No .tex file in {}", dir.display()).into());
                cx.notify();
            }
            [file] => self.open_clone(&dir, file.clone(), window, cx),
            _ => {
                let names: Vec<SharedString> = sources
                    .iter()
                    .map(|f| {
                        f.strip_prefix(&dir)
                            .unwrap_or(f)
                            .display()
                            .to_string()
                            .into()
                    })
                    .collect();
                let chosen = Rc::new(Cell::new(0));
                let shown = chosen.clone();
                let weak = cx.entity().downgrade();
                dialogs::open_form(
                    window,
                    cx,
                    "Choose the Main File",
                    "Open",
                    None,
                    move |_, cx| {
                        let shown = shown.clone();
                        v_flex()
                            .gap_2()
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(
                                        "The file to build: the one with \\documentclass, or the \
                                         one that inputs the file that has it.",
                                    ),
                            )
                            .child(
                                div()
                                    .id("main-file-list")
                                    .max_h(px(320.))
                                    .overflow_y_scroll()
                                    .child(
                                        RadioGroup::vertical("main-file")
                                            .children(names.clone())
                                            .selected_index(Some(shown.get()))
                                            .on_click(move |ix, window, _| {
                                                shown.set(*ix);
                                                window.refresh();
                                            }),
                                    ),
                            )
                            .into_any_element()
                    },
                    move |window, cx| {
                        let file = sources[chosen.get()].clone();
                        weak.update(cx, |this, cx| this.open_clone(&dir, file, window, cx))
                            .ok();
                        true
                    },
                );
            }
        }
    }

    /// Open `file` of the clone in `dir`, and trust the clone.
    fn open_clone(
        &mut self,
        dir: &Path,
        file: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_path(file, window, cx);
        // We created it, so its configuration is git's default; its latexmkrc comes from the
        // remote and still needs consent. A `%!TEX root` may point elsewhere: that is not ours
        // to trust.
        let dir = crate::paths::canonical(dir);
        let top = self
            .root
            .as_deref()
            .and_then(Path::parent)
            .and_then(git_panel::repo_top);
        if top.as_deref() == Some(dir.as_path()) {
            self.trust_git(cx);
        }
    }

    pub(super) fn overleaf_project(&self, input: &str) -> Option<tex_git::overleaf::Project> {
        tex_git::overleaf::project(input, &self.settings.overleaf_servers)
    }

    pub(super) fn prompt_overleaf(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.cloning.is_some() {
            return;
        }
        let weak = cx.entity().downgrade();
        dialogs::prompt_text(
            window,
            cx,
            "Open from Overleaf",
            "Continue",
            "https://www.overleaf.com/project/…",
            "The project's address as shown in your browser, or its Git URL. TeXoria keeps a \
             copy you can edit offline, and syncs it through Overleaf's Git integration (a \
             premium feature).",
            move |text, window, cx| {
                let Ok(project) = weak.update(cx, |this, _| this.overleaf_project(&text)) else {
                    return true;
                };
                let Some(project) = project else {
                    window.push_notification(
                        Notification::error("That is not the address of an Overleaf project"),
                        cx,
                    );
                    return false;
                };
                weak.update(cx, |this, cx| this.open_overleaf(project, window, cx))
                    .ok();
                true
            },
        );
    }

    /// Get a token for the project's server, then a folder, then clone.
    pub(super) fn open_overleaf(
        &mut self,
        project: tex_git::overleaf::Project,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let token = overleaf::load_token(&project.server, cx);
        cx.spawn_in(window, async move |this, cx| {
            let token = token.await;
            this.update_in(cx, |this, window, cx| match token {
                Some(token) => this.choose_overleaf_dir(project, token, window, cx),
                None => {
                    let weak = cx.entity().downgrade();
                    let server = project.server.clone();
                    overleaf::prompt_token(
                        &server,
                        overleaf::TOKEN_NEEDED,
                        move |token, window, cx| {
                            let project = project.clone();
                            weak.update(cx, |this, cx| {
                                this.choose_overleaf_dir(project, token, window, cx)
                            })
                            .ok();
                        },
                        window,
                        cx,
                    );
                }
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn choose_overleaf_dir(
        &mut self,
        project: tex_git::overleaf::Project,
        token: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Git cannot tell the project's name: the user picks the folder's.
        let start = directories::UserDirs::new()
            .map(|d| d.document_dir().unwrap_or(d.home_dir()).to_path_buf())
            .unwrap_or_default();
        let path = cx.prompt_for_new_path(&start, Some("Overleaf Project"));
        cx.spawn_in(window, async move |this, cx| {
            let Some(dest) = path.await.ok().and_then(|r| r.ok()).flatten() else {
                return;
            };
            this.update_in(cx, |this, window, cx| {
                let credential = overleaf::credential(&project.server, token);
                this.clone_into(project.git_url, dest, Some(credential), window, cx)
            })
            .ok();
        })
        .detach();
    }
}

/// The `.tex` files of `dir` and its subfolders, those nearest the top first, then by path
/// (case-insensitive).
pub(super) fn tex_files(dir: &Path) -> Vec<PathBuf> {
    fn collect(nodes: Vec<files::FileNode>, out: &mut Vec<PathBuf>) {
        for node in nodes {
            match node.children {
                Some(children) => collect(children, out),
                None if tex_project::has_extension(&node.path, &["tex"]) => out.push(node.path),
                None => {}
            }
        }
    }
    let mut out = Vec::new();
    collect(files::scan(dir), &mut out);
    out.sort_by_cached_key(|p| (p.components().count(), p.to_string_lossy().to_lowercase()));
    out
}
