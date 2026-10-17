//! The Files view of the sidebar: the project directory as a tree.
//!
//! Directories are scanned on the background executor. Changes to the files
//! go through the workspace ([`FileTreeEvent::Request`]), which saves open
//! buffers first and keeps their tabs in step.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::list::ListItem;
use gpui_kit::component::menu::PopupMenuItem;
use gpui_kit::component::tree::{TreeEvent, TreeItem, TreeState, tree};
use gpui_kit::component::{
    ActiveTheme, Icon, Sizable, WindowExt as _, button::ButtonVariant, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use tex_project::files::{self, FileNode};

use crate::assets::Glyph;
use crate::git_panel::open_form;

/// A file's state in Git, in increasing order of importance (a directory
/// shows its most important one).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum GitMark {
    Added,
    Modified,
    Conflict,
}

/// A change to the project's files, carried out by the workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileOp {
    CreateFile(PathBuf),
    CreateDir(PathBuf),
    Rename { from: PathBuf, to: PathBuf },
    Delete(PathBuf),
}

pub enum FileTreeEvent {
    Open(PathBuf),
    Request(FileOp),
}

impl EventEmitter<FileTreeEvent> for FileTree {}

/// What a row shows besides its label (the tree only knows ids and labels).
struct Row {
    path: PathBuf,
    is_dir: bool,
    mark: Option<GitMark>,
}

pub struct FileTree {
    dir: Option<PathBuf>,
    /// The project's root file: it cannot be deleted from here.
    root_file: Option<PathBuf>,
    tree: Entity<TreeState>,
    nodes: Vec<FileNode>,
    /// By tree item id.
    rows: Rc<HashMap<SharedString, Row>>,
    expanded: HashSet<PathBuf>,
    marks: HashMap<PathBuf, GitMark>,
    /// The file of the active tab, selected in the tree.
    active: Option<PathBuf>,
    scanned: bool,
    /// Bumped on every scan: older results are dropped.
    generation: u64,
    _scan: Task<()>,
    _subscription: Subscription,
}

impl FileTree {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let tree = cx.new(|cx| TreeState::new(cx));
        let subscription = cx.subscribe(&tree, |this, _, ev: &TreeEvent, _| {
            let (id, expanded) = match ev {
                TreeEvent::Expanded(id) => (id, true),
                TreeEvent::Collapsed(id) => (id, false),
            };
            if let Some(row) = this.rows.get(id) {
                if expanded {
                    this.expanded.insert(row.path.clone());
                } else {
                    this.expanded.remove(&row.path);
                }
            }
        });
        FileTree {
            dir: None,
            root_file: None,
            tree,
            nodes: Vec::new(),
            rows: Rc::default(),
            expanded: HashSet::new(),
            marks: HashMap::new(),
            active: None,
            scanned: false,
            generation: 0,
            _scan: Task::ready(()),
            _subscription: subscription,
        }
    }

    /// Show the project whose root file is `root_file`.
    pub fn set_project(&mut self, root_file: Option<PathBuf>, cx: &mut Context<Self>) {
        let dir = root_file
            .as_deref()
            .and_then(Path::parent)
            .map(Path::to_path_buf);
        if dir != self.dir {
            self.dir = dir;
            self.nodes.clear();
            self.expanded.clear();
            self.marks.clear();
            self.active = None;
            self.scanned = false;
            self.rebuild(cx);
        }
        self.root_file = root_file;
        self.refresh(cx);
    }

    /// Scan the project directory again.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        let Some(dir) = self.dir.clone() else {
            return;
        };
        let scan = cx.background_spawn(async move { files::scan(&dir) });
        self._scan = cx.spawn(async move |this, cx| {
            let nodes = scan.await;
            this.update(cx, |this, cx| {
                if this.generation == generation {
                    this.nodes = nodes;
                    this.scanned = true;
                    this.rebuild(cx);
                }
            })
            .ok();
        });
    }

    /// Color files by their state in Git (absolute paths).
    pub fn set_marks(&mut self, marks: HashMap<PathBuf, GitMark>, cx: &mut Context<Self>) {
        if marks != self.marks {
            self.marks = marks;
            self.rebuild(cx);
        }
    }

    pub fn set_active(&mut self, path: Option<PathBuf>, cx: &mut Context<Self>) {
        if path != self.active {
            self.active = path;
            self.select_active(cx);
        }
    }

    fn select_active(&mut self, cx: &mut Context<Self>) {
        let ix = self
            .active
            .as_deref()
            .and_then(|p| self.tree.read(cx).index_of(&item_id(p)));
        self.tree
            .update(cx, |tree, cx| tree.set_selected_index(ix, cx));
    }

    /// Hand the scanned nodes to the tree, keeping folders open.
    fn rebuild(&mut self, cx: &mut Context<Self>) {
        // A directory shows the most important mark of its contents.
        let mut dir_marks: HashMap<&Path, GitMark> = HashMap::new();
        if let Some(dir) = &self.dir {
            for (path, &mark) in &self.marks {
                for parent in path.ancestors().skip(1) {
                    if parent == dir || !parent.starts_with(dir) {
                        break;
                    }
                    let slot = dir_marks.entry(parent).or_insert(mark);
                    *slot = (*slot).max(mark);
                }
            }
        }
        let mut rows = HashMap::new();
        let items: Vec<TreeItem> = self
            .nodes
            .iter()
            .map(|n| self.item(n, &dir_marks, &mut rows))
            .collect();
        self.rows = Rc::new(rows);
        self.tree.update(cx, |tree, cx| tree.set_items(items, cx));
        self.select_active(cx);
        cx.notify();
    }

    fn item(
        &self,
        node: &FileNode,
        dir_marks: &HashMap<&Path, GitMark>,
        rows: &mut HashMap<SharedString, Row>,
    ) -> TreeItem {
        let id = item_id(&node.path);
        let mark = match node.is_dir() {
            true => dir_marks.get(node.path.as_path()),
            false => self.marks.get(&node.path),
        };
        rows.insert(
            id.clone(),
            Row {
                path: node.path.clone(),
                is_dir: node.is_dir(),
                mark: mark.copied(),
            },
        );
        let item = TreeItem::new(id, node.name.clone());
        match &node.children {
            Some(children) => item
                .children(children.iter().map(|c| self.item(c, dir_marks, rows)))
                .expanded(self.expanded.contains(&node.path)),
            None => item,
        }
    }

    // ── Changes ──────────────────────────────────────────────────────────

    /// Ask for the name of a new file (or folder) in the project directory.
    pub fn prompt_new(&mut self, folder: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(dir) = self.dir.clone() {
            self.prompt_create(dir, folder, window, cx);
        }
    }

    fn prompt_create(
        &mut self,
        dir: PathBuf,
        folder: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let placeholder = if folder { "figures" } else { "chapter.tex" };
        let name = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        let location: SharedString = match self.relative(&dir) {
            Some(rel) if !rel.as_os_str().is_empty() => format!("In {}", rel.display()).into(),
            _ => "In the project folder".into(),
        };
        let error = Rc::new(Cell::new(None::<&'static str>));
        let weak = cx.entity().downgrade();
        let focus = name.focus_handle(cx);
        let (body_name, body_error) = (name.clone(), error.clone());
        open_form(
            window,
            cx,
            if folder { "New Folder" } else { "New File" },
            "Create",
            Some(focus),
            move |_, cx| name_form(location.clone(), &body_name, body_error.get(), cx),
            move |window, cx| {
                let result =
                    files::new_entry_path(&dir, &name.read(cx).value()).and_then(|path| match path
                        .symlink_metadata()
                    {
                        Ok(_) => Err("A file or folder with that name already exists."),
                        Err(_) => Ok(path),
                    });
                let path = match result {
                    Ok(path) => path,
                    Err(e) => {
                        error.set(Some(e));
                        window.refresh();
                        return false;
                    }
                };
                let op = if folder {
                    FileOp::CreateDir(path)
                } else {
                    FileOp::CreateFile(path)
                };
                weak.update(cx, |_, cx| cx.emit(FileTreeEvent::Request(op)))
                    .ok();
                true
            },
        );
    }

    fn prompt_rename(&mut self, from: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let current = from
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        // Preselect the name without its extension, as file managers do.
        let stem = match current.rfind('.') {
            Some(dot) if dot > 0 && !from.is_dir() => dot,
            _ => current.len(),
        };
        let name = cx.new(|cx| {
            let mut s = InputState::new(window, cx);
            s.set_value(current.clone(), window, cx);
            s.set_selected_range(0..stem, cx);
            s
        });
        let location: SharedString = format!("Rename {current} to:").into();
        let error = Rc::new(Cell::new(None::<&'static str>));
        let weak = cx.entity().downgrade();
        let focus = name.focus_handle(cx);
        let (body_name, body_error) = (name.clone(), error.clone());
        open_form(
            window,
            cx,
            "Rename",
            "Rename",
            Some(focus),
            move |_, cx| name_form(location.clone(), &body_name, body_error.get(), cx),
            move |window, cx| {
                let result = files::rename_target(&from, &name.read(cx).value()).and_then(|to| {
                    match to.symlink_metadata() {
                        Ok(_) if !files::is_same_file(&from, &to) => {
                            Err("A file or folder with that name already exists.")
                        }
                        _ => Ok(to),
                    }
                });
                let to = match result {
                    Ok(to) => to,
                    Err(e) => {
                        error.set(Some(e));
                        window.refresh();
                        return false;
                    }
                };
                if to != from {
                    let op = FileOp::Rename {
                        from: from.clone(),
                        to,
                    };
                    weak.update(cx, |_, cx| cx.emit(FileTreeEvent::Request(op)))
                        .ok();
                }
                true
            },
        );
    }

    fn confirm_delete(
        &mut self,
        path: PathBuf,
        is_dir: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let title: SharedString = format!("Delete {name}?").into();
        let description: SharedString = if is_dir {
            "The folder and everything in it are deleted. This cannot be undone.".into()
        } else {
            "This cannot be undone.".into()
        };
        let weak = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let (weak, path) = (weak.clone(), path.clone());
            alert
                .title(title.clone())
                .description(description.clone())
                .confirm()
                .ok_text("Delete")
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, _, cx| {
                    let op = FileOp::Delete(path.clone());
                    weak.update(cx, |_, cx| cx.emit(FileTreeEvent::Request(op)))
                        .ok();
                    true
                })
        });
    }

    fn relative<'a>(&self, path: &'a Path) -> Option<&'a Path> {
        path.strip_prefix(self.dir.as_ref()?).ok()
    }
}

/// Tree items are identified by their path.
fn item_id(path: &Path) -> SharedString {
    path.to_string_lossy().into_owned().into()
}

fn name_form(
    caption: SharedString,
    input: &Entity<InputState>,
    error: Option<&'static str>,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    v_flex()
        .gap_2()
        .child(
            div()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(caption),
        )
        .child(Input::new(input))
        .when_some(error, |d, e| {
            d.child(div().text_sm().text_color(theme.danger).child(e))
        })
        .into_any_element()
}

fn menu_item(
    weak: &WeakEntity<FileTree>,
    label: &'static str,
    f: impl Fn(&mut FileTree, &mut Window, &mut Context<FileTree>) + 'static,
) -> PopupMenuItem {
    let weak = weak.clone();
    PopupMenuItem::new(label).on_click(move |_, window, cx| {
        weak.update(cx, |this, cx| f(this, window, cx)).ok();
    })
}

fn reveal_label() -> &'static str {
    if cfg!(target_os = "macos") {
        "Reveal in Finder"
    } else if cfg!(target_os = "windows") {
        "Reveal in File Explorer"
    } else {
        "Show in File Manager"
    }
}

impl Render for FileTree {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let empty = match &self.dir {
            None => Some("No project open"),
            Some(_) if self.scanned && self.nodes.is_empty() => Some("Empty folder"),
            Some(_) => None,
        };
        if let Some(message) = empty {
            return div()
                .px_4()
                .text_base()
                .text_color(theme.muted_foreground)
                .child(message)
                .into_any_element();
        }

        let weak = cx.entity().downgrade();
        let rows = self.rows.clone();
        let menu_weak = weak.clone();
        let menu_rows = self.rows.clone();
        let root_file = self.root_file.clone();
        tree(&self.tree, move |ix, entry, _, _, cx| {
            let theme = cx.theme();
            let row = rows.get(&entry.item().id);
            let is_dir = row.is_some_and(|r| r.is_dir);
            let color = match row.and_then(|r| r.mark) {
                Some(GitMark::Added) => Some(theme.success),
                Some(GitMark::Modified) => Some(theme.warning),
                Some(GitMark::Conflict) => Some(theme.danger),
                None => None,
            }
            // Directories only summarize their contents.
            .map(|c| if is_dir { c.opacity(0.8) } else { c });
            let chevron = Icon::new(Glyph::Expand)
                .small()
                .text_color(theme.muted_foreground)
                .when(entry.is_expanded(), |i| {
                    i.transform(Transformation::rotate(percentage(0.25)))
                });
            let icon = Icon::new(if is_dir {
                Glyph::Folder
            } else {
                Glyph::PlainFile
            })
            .small()
            .text_color(theme.muted_foreground.opacity(0.8));
            let open = row.filter(|r| !r.is_dir).map(|r| r.path.clone());
            let weak = weak.clone();
            ListItem::new(ix)
                .pl(px(8. + entry.depth() as f32 * 12.))
                .pr_2()
                .py_0p5()
                .mx_1()
                .rounded(theme.radius)
                .text_base()
                .text_color(theme.sidebar_foreground)
                .child(
                    h_flex()
                        .gap_1p5()
                        .child(if is_dir {
                            chevron.into_any_element()
                        } else {
                            div().w(px(12.)).flex_none().into_any_element()
                        })
                        .child(icon)
                        .child(
                            div()
                                .truncate()
                                .when_some(color, |d, c| d.text_color(c))
                                .child(entry.item().label.clone()),
                        ),
                )
                .when_some(open, |item, path| {
                    item.on_click(move |_, _, cx| {
                        weak.update(cx, |_, cx| cx.emit(FileTreeEvent::Open(path.clone())))
                            .ok();
                    })
                })
        })
        .context_menu(move |_, entry, menu, _, _| {
            let Some(row) = menu_rows.get(&entry.item().id) else {
                return menu;
            };
            let path = row.path.clone();
            let is_dir = row.is_dir;
            let dir = if is_dir {
                path.clone()
            } else {
                path.parent().map(Path::to_path_buf).unwrap_or_default()
            };
            let is_root = root_file.as_ref() == Some(&path);
            let weak = &menu_weak;
            let (new_dir, folder_dir) = (dir.clone(), dir);
            let (reveal, rename, delete) = (path.clone(), path.clone(), path);
            menu.item(menu_item(weak, "New File…", move |this, window, cx| {
                this.prompt_create(new_dir.clone(), false, window, cx)
            }))
            .item(menu_item(weak, "New Folder…", move |this, window, cx| {
                this.prompt_create(folder_dir.clone(), true, window, cx)
            }))
            .separator()
            .item(menu_item(weak, reveal_label(), move |_, _, cx| {
                cx.reveal_path(&reveal)
            }))
            .separator()
            .item(menu_item(weak, "Rename…", move |this, window, cx| {
                this.prompt_rename(rename.clone(), window, cx)
            }))
            .item(
                menu_item(weak, "Delete…", move |this, window, cx| {
                    this.confirm_delete(delete.clone(), is_dir, window, cx)
                })
                .disabled(is_root),
            )
        })
        .into_any_element()
    }
}
