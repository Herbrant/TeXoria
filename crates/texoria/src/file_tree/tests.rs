use std::cell::RefCell;

use gpui_kit::base::actions::Confirm;
use gpui_kit::{ScrollStrategy, TestAppContext, WindowOptions};

use super::*;
use crate::testing;
// `super::*` brings gpui's `test` attribute, which would shadow the standard one, and
// gpui-kit's `gpui` alias, which the `#[gpui::test]` macro's paths must not find twice.
use core::prelude::v1::test;
use gpui_kit::gpui;

#[test]
fn ids_and_labels() {
    let path = Path::new("/project/ch/intro.tex");
    let spelled: PathBuf = path.components().collect();
    assert_eq!(item_id(path).as_ref(), spelled.to_string_lossy());
    // The same row whichever separators a path was built with.
    let folder = Path::new("/project");
    assert_eq!(
        item_id(&folder.join("ch/intro.tex")),
        item_id(&folder.join("ch").join("intro.tex"))
    );
    assert!(!reveal_label().is_empty());
    assert!(GitMark::Added < GitMark::Modified && GitMark::Modified < GitMark::Conflict);
}

struct Files {
    window: AnyWindowHandle,
    tree: Entity<FileTree>,
    requests: Rc<RefCell<Vec<FileOp>>>,
    opened: Rc<RefCell<Vec<PathBuf>>>,
    /// The project folder, canonical.
    dir: PathBuf,
    _tmp: tempfile::TempDir,
}

/// A Files view on a project of a main file, a chapter folder and a picture.
fn files(cx: &mut TestAppContext) -> Files {
    let tmp = tempfile::tempdir().unwrap();
    let dir = crate::paths::canonical(tmp.path());
    std::fs::write(dir.join("main.tex"), "\\documentclass{article}").unwrap();
    std::fs::create_dir_all(dir.join("ch/deep")).unwrap();
    std::fs::write(dir.join("ch/intro.tex"), "intro").unwrap();
    std::fs::write(dir.join("ch/deep/notes.tex"), "notes").unwrap();
    std::fs::write(dir.join("fig.png"), "png").unwrap();
    testing::init(cx);
    let (window, tree) = cx.update(|cx| {
        gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| cx.new(FileTree::new)).unwrap()
    });
    let requests: Rc<RefCell<Vec<FileOp>>> = Rc::default();
    let opened: Rc<RefCell<Vec<PathBuf>>> = Rc::default();
    let (r, o) = (requests.clone(), opened.clone());
    cx.update(|cx| {
        cx.subscribe(&tree, move |_, ev: &FileTreeEvent, _| match ev {
            FileTreeEvent::Request(op) => r.borrow_mut().push(op.clone()),
            FileTreeEvent::Open(p) => o.borrow_mut().push(p.clone()),
        })
        .detach()
    });
    let f = Files {
        window,
        tree,
        requests,
        opened,
        dir,
        _tmp: tmp,
    };
    f.draw(cx);
    let main = f.dir.join("main.tex");
    f.tree.update(cx, |t, cx| t.set_project(Some(main), cx));
    cx.run_until_parked();
    f.draw(cx);
    f
}

impl Files {
    fn draw(&self, cx: &mut TestAppContext) {
        cx.update_window(self.window, |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
    }

    fn update<R>(
        &self,
        cx: &mut TestAppContext,
        f: impl FnOnce(&mut FileTree, &mut Window, &mut Context<FileTree>) -> R,
    ) -> R {
        let tree = self.tree.clone();
        cx.update_window(self.window, |_, window, cx| {
            tree.update(cx, |t, cx| f(t, window, cx))
        })
        .unwrap()
    }

    /// The rows' paths relative to the project, in tree order.
    fn rows(&self, cx: &mut TestAppContext) -> Vec<String> {
        self.tree.read_with(cx, |t, cx| {
            let state = t.tree.read(cx);
            (0..)
                .map_while(|ix| state.entry(ix))
                .map(|e| {
                    let path = &t.rows[&e.item().id].path;
                    let rel = path.strip_prefix(&self.dir).unwrap();
                    rel.to_string_lossy().replace('\\', "/")
                })
                .collect()
        })
    }

    fn mark(&self, cx: &mut TestAppContext, rel: &str) -> Option<GitMark> {
        let path = self.dir.join(rel);
        self.tree
            .read_with(cx, |t, _| t.rows.get(&item_id(&path)).and_then(|r| r.mark))
    }

    fn type_and_confirm(&self, cx: &mut TestAppContext, text: &str) {
        self.draw(cx);
        cx.simulate_input(self.window, text);
        self.draw(cx);
        cx.dispatch_action(self.window, Confirm { secondary: false });
        cx.run_until_parked();
    }

    fn has_dialog(&self, cx: &mut TestAppContext) -> bool {
        cx.update_window(self.window, |_, window, cx| window.has_active_dialog(cx))
            .unwrap()
    }
}

#[gpui::test]
fn lists_the_project_folder(cx: &mut TestAppContext) {
    let f = files(cx);
    // Folders first; a folder's contents show once it is open.
    assert_eq!(f.rows(cx), ["ch", "fig.png", "main.tex"]);
    f.tree.read_with(cx, |t, _| {
        assert!(t.scanned);
        assert!(t.rows[&item_id(&f.dir.join("ch"))].is_dir);
        assert_eq!(t.relative(&f.dir.join("ch")), Some(Path::new("ch")));
        assert_eq!(t.relative(Path::new("/elsewhere")), None);
    });

    // Opening a folder is remembered across scans.
    let notes = item_id(&f.dir.join("ch/deep/notes.tex"));
    let state = f.tree.read_with(cx, |t, _| t.tree.clone());
    state.update(cx, |s, cx| s.reveal_item(&notes, ScrollStrategy::Top, cx));
    cx.run_until_parked();
    std::fs::write(f.dir.join("ch/outro.tex"), "").unwrap();
    f.tree.update(cx, |t, cx| t.refresh(cx));
    cx.run_until_parked();
    f.draw(cx);
    assert_eq!(
        f.rows(cx),
        [
            "ch",
            "ch/deep",
            "ch/deep/notes.tex",
            "ch/intro.tex",
            "ch/outro.tex",
            "fig.png",
            "main.tex"
        ]
    );

    // The active tab's file is selected.
    let intro = f.dir.join("ch/intro.tex");
    f.tree.update(cx, |t, cx| t.set_active(Some(intro), cx));
    let selected = f
        .tree
        .read_with(cx, |t, cx| t.tree.read(cx).selected_index());
    assert_eq!(selected, Some(3));
    f.tree.update(cx, |t, cx| t.set_active(None, cx));
    let selected = f
        .tree
        .read_with(cx, |t, cx| t.tree.read(cx).selected_index());
    assert_eq!(selected, None);

    // Closing the folder too.
    let ch = item_id(&f.dir.join("ch"));
    f.tree.update(cx, |t, cx| {
        t.tree
            .update(cx, |_, cx| cx.emit(TreeEvent::Collapsed(ch.clone())))
    });
    assert!(
        f.tree
            .read_with(cx, |t, _| !t.expanded.contains(&f.dir.join("ch")))
    );
}

#[gpui::test]
fn colors_files_and_their_folders(cx: &mut TestAppContext) {
    let f = files(cx);
    let marks = HashMap::from([
        (f.dir.join("ch/intro.tex"), GitMark::Added),
        (f.dir.join("ch/deep/notes.tex"), GitMark::Conflict),
        (f.dir.join("main.tex"), GitMark::Modified),
        // Outside the project: ignored.
        (PathBuf::from("/elsewhere/x.tex"), GitMark::Conflict),
    ]);
    f.tree.update(cx, |t, cx| t.set_marks(marks, cx));
    f.draw(cx);
    assert_eq!(f.mark(cx, "main.tex"), Some(GitMark::Modified));
    assert_eq!(f.mark(cx, "ch/intro.tex"), Some(GitMark::Added));
    // A folder shows the most important mark inside it.
    assert_eq!(f.mark(cx, "ch"), Some(GitMark::Conflict));
    assert_eq!(f.mark(cx, "ch/deep"), Some(GitMark::Conflict));
    assert_eq!(f.mark(cx, "fig.png"), None);
}

#[gpui::test]
fn empty_and_missing_projects(cx: &mut TestAppContext) {
    let f = files(cx);
    let empty = f.dir.join("ch/deep");
    std::fs::remove_file(empty.join("notes.tex")).unwrap();
    f.tree
        .update(cx, |t, cx| t.set_project(Some(empty.join("main.tex")), cx));
    cx.run_until_parked();
    f.draw(cx);
    f.tree
        .read_with(cx, |t, _| assert!(t.scanned && t.nodes.is_empty()));
    f.tree.update(cx, |t, cx| t.set_project(None, cx));
    cx.run_until_parked();
    f.draw(cx);
    f.tree.read_with(cx, |t, _| {
        assert_eq!(t.dir, None);
        assert!(t.rows.is_empty());
    });
    // Nothing to create a file in.
    f.update(cx, |t, window, cx| t.prompt_new(false, window, cx));
    assert!(!f.has_dialog(cx));
}

#[gpui::test]
fn creates_files_and_folders(cx: &mut TestAppContext) {
    let f = files(cx);
    f.update(cx, |t, window, cx| t.prompt_new(false, window, cx));
    f.type_and_confirm(cx, "chapter.tex");
    // In a folder, from its context menu.
    let ch = f.dir.join("ch");
    f.update(cx, |t, window, cx| {
        t.prompt_create(ch.clone(), true, window, cx)
    });
    f.type_and_confirm(cx, "figures");
    assert_eq!(
        f.requests.borrow().as_slice(),
        [
            FileOp::CreateFile(f.dir.join("chapter.tex")),
            FileOp::CreateDir(ch.join("figures")),
        ]
    );

    // A name that is taken, or no name: the dialog stays, with the reason.
    f.update(cx, |t, window, cx| t.prompt_new(false, window, cx));
    f.type_and_confirm(cx, "main.tex");
    assert!(f.has_dialog(cx));
    f.draw(cx);
    assert_eq!(f.requests.borrow().len(), 2);
}

#[gpui::test]
fn renames_and_deletes(cx: &mut TestAppContext) {
    let f = files(cx);
    let intro = f.dir.join("ch/intro.tex");
    // The name without its extension is selected: typing replaces it.
    f.update(cx, |t, window, cx| {
        t.prompt_rename(intro.clone(), window, cx)
    });
    f.type_and_confirm(cx, "opening");
    assert!(!f.has_dialog(cx), "{:?}", f.requests.borrow());
    // The same name: nothing to do.
    f.update(cx, |t, window, cx| {
        t.prompt_rename(intro.clone(), window, cx)
    });
    f.type_and_confirm(cx, "intro");
    assert!(!f.has_dialog(cx));
    // Taken by another file.
    f.update(cx, |t, window, cx| {
        t.prompt_rename(intro.clone(), window, cx)
    });
    f.type_and_confirm(cx, "../../main");
    assert!(f.has_dialog(cx));
    f.update(cx, |_, window, cx| window.close_dialog(cx));
    // A folder keeps its whole name selected.
    let ch = f.dir.join("ch");
    f.update(cx, |t, window, cx| t.prompt_rename(ch.clone(), window, cx));
    f.type_and_confirm(cx, "chapters");

    f.update(cx, |t, window, cx| {
        t.confirm_delete(intro.clone(), false, window, cx)
    });
    f.draw(cx);
    cx.dispatch_action(f.window, Confirm { secondary: false });
    cx.run_until_parked();
    f.update(cx, |t, window, cx| {
        t.confirm_delete(ch.clone(), true, window, cx)
    });
    f.draw(cx);
    cx.dispatch_action(f.window, Confirm { secondary: false });
    cx.run_until_parked();

    assert_eq!(
        f.requests.borrow().as_slice(),
        [
            FileOp::Rename {
                from: intro.clone(),
                to: f.dir.join("ch/opening.tex"),
            },
            FileOp::Rename {
                from: ch.clone(),
                to: f.dir.join("chapters"),
            },
            FileOp::Delete(intro),
            FileOp::Delete(ch),
        ]
    );
    assert!(f.opened.borrow().is_empty());
}
