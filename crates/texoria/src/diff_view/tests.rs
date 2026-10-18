use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::component::diff::{DiffConflictResolution, DiffFile, DiffFileStatus};
use gpui_kit::{TestAppContext, WindowOptions};

use super::*;
use crate::testing::{self, GitSandbox};
// `super::*` brings gpui's `test` attribute, which would shadow the standard one, and
// gpui-kit's `gpui` alias, which the `#[gpui::test]` macro's paths must not find twice.
use core::prelude::v1::test;
use gpui_kit::gpui;

const PATCH: &str = "\
diff --git a/ch/intro.tex b/ch/intro.tex
--- a/ch/intro.tex
+++ b/ch/intro.tex
@@ -1,2 +1,2 @@
 \\section{Intro}
-Old text.
+New text.
diff --git a/notes.txt b/notes.txt
--- a/notes.txt
+++ b/notes.txt
@@ -1 +1 @@
-a
+b
";

const CONFLICT: &str = "\
\\section{Intro}
<<<<<<< HEAD
Ours.
=======
Theirs.
>>>>>>> branch
The end.
";

fn commit(sha: &str) -> Commit {
    Commit {
        sha: sha.into(),
        short: sha.chars().take(7).collect(),
        author: "Ada".into(),
        email: "ada@example.com".into(),
        time: 0,
        parents: Vec::new(),
        subject: "Rewrite the intro".into(),
    }
}

fn agent(patch: &str) -> DiffSource {
    DiffSource::Agent {
        dir: PathBuf::from("/project"),
        patch: patch.into(),
    }
}

fn working(path: &str, staged: bool, untracked: bool) -> DiffSource {
    DiffSource::Working {
        path: path.into(),
        staged,
        untracked,
    }
}

#[test]
fn untracked_patches_parse() {
    let patch = tex_git::untracked_patch("ch/intro.tex", b"\\section{Intro}\nText");
    let files = DiffFile::parse(&patch).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path().as_str(), "ch/intro.tex");
    assert_eq!(files[0].status(), DiffFileStatus::Added);
    assert_eq!(files[0].additions(), 2);

    let files = DiffFile::parse(&tex_git::untracked_patch("fig.pdf", b"%PDF\0")).unwrap();
    assert!(files[0].is_binary());
}

#[test]
fn sources_name_their_file() {
    assert_eq!(working("ch/a.tex", false, false).path(), Some("ch/a.tex"));
    let conflict = DiffSource::Conflict {
        path: "main.tex".into(),
    };
    assert_eq!(conflict.path(), Some("main.tex"));
    assert_eq!(DiffSource::Commit(commit("abc")).path(), None);
    assert_eq!(agent("").path(), None);
}

#[test]
fn modes() {
    assert_eq!(mode(true), DiffMode::Split);
    assert_eq!(mode(false), DiffMode::Unified);
}

#[test]
fn latex_files_are_highlighted_as_latex() {
    let files = files_for(&agent(PATCH), PATCH).unwrap();
    let langs: Vec<_> = files.iter().map(|f| f.language().to_string()).collect();
    assert_eq!(langs[0], latex::LANGUAGE);
    assert_ne!(langs[1], latex::LANGUAGE);

    let conflict = DiffSource::Conflict {
        path: "ch/intro.tex".into(),
    };
    let files = files_for(&conflict, CONFLICT).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].status(), DiffFileStatus::Conflicted);
    assert_eq!(files[0].conflicts().len(), 1);
    assert_eq!(files[0].language().as_str(), latex::LANGUAGE);
    // A conflict without its end.
    assert!(files_for(&conflict, "<<<<<<< HEAD\nours\n=======\n").is_err());
}

#[test]
fn texts_without_a_repository() {
    assert_eq!(load_text(None, &agent(PATCH)).unwrap(), PATCH);
    for source in [
        working("a.tex", false, false),
        working("a.tex", false, true),
        DiffSource::Conflict {
            path: "a.tex".into(),
        },
        DiffSource::Commit(commit("abc")),
    ] {
        let err = load_text(None, &source).unwrap_err();
        assert_eq!(err.to_string(), "No repository", "{source:?}");
    }
}

/// A diff view alone in a window, with the events it emitted.
fn view(
    cx: &mut TestAppContext,
    repo: Option<Repo>,
    source: DiffSource,
) -> (AnyWindowHandle, Entity<DiffView>, Rc<RefCell<Vec<String>>>) {
    testing::init(cx);
    let (window, view) = cx.update(|cx| {
        gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
            cx.new(|cx| DiffView::new(repo, source, false, cx))
        })
        .unwrap()
    });
    let log: Rc<RefCell<Vec<String>>> = Rc::default();
    let seen = log.clone();
    cx.update(|cx| {
        cx.subscribe(&view, move |_, ev: &DiffViewEvent, _| {
            seen.borrow_mut().push(match ev {
                DiffViewEvent::SplitChanged(split) => format!("split {split}"),
                DiffViewEvent::Resolved { path, text } => format!("resolved {path}: {text}"),
                _ => "other".into(),
            })
        })
        .detach()
    });
    cx.run_until_parked();
    draw(cx, window);
    (window, view, log)
}

fn draw(cx: &mut TestAppContext, window: AnyWindowHandle) {
    cx.update_window(window, |_, window, cx| window.draw(cx).clear(cx))
        .unwrap();
}

fn files(cx: &mut TestAppContext, view: &Entity<DiffView>) -> Vec<String> {
    view.read_with(cx, |v, cx| {
        (v.state.read(cx).files().iter())
            .map(|f| f.path().to_string())
            .collect()
    })
}

#[gpui::test]
fn shows_an_agents_changes(cx: &mut TestAppContext) {
    let (window, view, log) = view(cx, None, agent(PATCH));
    assert_eq!(files(cx, &view), ["ch/intro.tex", "notes.txt"]);
    view.read_with(cx, |v, _| {
        assert!(v.is_agent());
        assert_eq!(v.title(), "Agent changes");
        assert_eq!(v.error, None);
    });
    // An agent's patch is not read again.
    view.update(cx, |v, cx| v.reload(cx));
    assert_eq!(view.read_with(cx, |v, _| v.generation), 1);

    view.update(cx, |v, cx| v.set_split(true, cx));
    draw(cx, window);
    assert_eq!(log.borrow().as_slice(), ["split true"]);
    view.read_with(cx, |v, cx| {
        assert_eq!(v.state.read(cx).mode(), DiffMode::Split)
    });

    // A patch that does not parse is reported in place of the diff.
    view.update(cx, |v, cx| v.show(None, agent("@@ -1 +1 @@\n-a\n+b\n"), cx));
    cx.run_until_parked();
    draw(cx, window);
    view.read_with(cx, |v, _| assert!(v.error.is_some()));
    // Without a repository, a working-tree change cannot be read.
    view.update(cx, |v, cx| v.show(None, working("a.tex", false, false), cx));
    cx.run_until_parked();
    let error = view.read_with(cx, |v, _| v.error.clone());
    assert_eq!(error.as_deref(), Some("No repository"));
}

/// A repository with `main.tex` committed twice: its two commits, newest first.
fn history(s: &GitSandbox) -> (Repo, Vec<Commit>) {
    let dir = s.dir.join("repo");
    std::fs::create_dir_all(&dir).unwrap();
    s.run(&dir, &["init", "--quiet"]);
    std::fs::write(dir.join("main.tex"), "\\section{Intro}\nOld text.\n").unwrap();
    s.run(&dir, &["add", "-A"]);
    s.run(&dir, &["commit", "--quiet", "-m", "first"]);
    std::fs::write(dir.join("main.tex"), "\\section{Intro}\nNew text.\n").unwrap();
    s.run(&dir, &["commit", "--quiet", "-am", "second"]);
    let repo = s.git.discover_at(&dir).unwrap().unwrap();
    let log = repo.log(0, 10, None).unwrap();
    (repo, log)
}

#[gpui::test]
fn shows_working_changes_and_commits(cx: &mut TestAppContext) {
    let Some(s) = GitSandbox::new() else { return };
    let (repo, log) = history(&s);
    let root = repo.root().to_path_buf();
    std::fs::write(root.join("main.tex"), "\\section{Intro}\nEdited.\n").unwrap();
    let source = working("main.tex", false, false);
    let (window, view, _) = view(cx, Some(repo.clone()), source.clone());
    assert_eq!(files(cx, &view), ["main.tex"]);
    assert_eq!(view.read_with(cx, |v, _| v.title()), "main.tex (changes)");

    // The same diff again re-reads it, and an unchanged text is not parsed again.
    let loaded = view.read_with(cx, |v, _| v.loaded.clone());
    view.update(cx, |v, cx| v.show(Some(repo.clone()), source, cx));
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |v, _| v.loaded.clone()), loaded);
    std::fs::write(root.join("main.tex"), "\\section{Intro}\nEdited again.\n").unwrap();
    view.update(cx, |v, cx| v.reload(cx));
    cx.run_until_parked();
    assert_ne!(view.read_with(cx, |v, _| v.loaded.clone()), loaded);

    s.run(&root, &["add", "main.tex"]);
    view.update(cx, |v, cx| {
        v.show(Some(repo.clone()), working("main.tex", true, false), cx)
    });
    cx.run_until_parked();
    draw(cx, window);
    assert_eq!(files(cx, &view), ["main.tex"]);
    assert_eq!(view.read_with(cx, |v, _| v.title()), "main.tex (staged)");

    std::fs::create_dir_all(root.join("ch")).unwrap();
    std::fs::write(root.join("ch/new.tex"), "new\n").unwrap();
    view.update(cx, |v, cx| {
        v.show(Some(repo.clone()), working("ch/new.tex", false, true), cx)
    });
    cx.run_until_parked();
    draw(cx, window);
    assert_eq!(files(cx, &view), ["ch/new.tex"]);
    assert_eq!(view.read_with(cx, |v, _| v.title()), "new.tex (changes)");

    // A commit, against its parent; the first one adds the file.
    for (c, status) in [
        (&log[0], DiffFileStatus::Modified),
        (&log[1], DiffFileStatus::Added),
    ] {
        view.update(cx, |v, cx| {
            v.show(Some(repo.clone()), DiffSource::Commit(c.clone()), cx)
        });
        cx.run_until_parked();
        draw(cx, window);
        view.read_with(cx, |v, cx| {
            assert_eq!(v.title(), format!("Commit {}", c.short));
            assert_eq!(v.state.read(cx).files()[0].status(), status);
        });
    }
    // Commits do not change: no reload.
    let generation = view.read_with(cx, |v, _| v.generation);
    view.update(cx, |v, cx| v.reload(cx));
    assert_eq!(view.read_with(cx, |v, _| v.generation), generation);
}

#[gpui::test]
fn resolves_conflicts(cx: &mut TestAppContext) {
    let Some(s) = GitSandbox::new() else { return };
    let (repo, _) = history(&s);
    std::fs::write(repo.root().join("main.tex"), CONFLICT).unwrap();
    let source = DiffSource::Conflict {
        path: "main.tex".into(),
    };
    let (window, view, _) = view(cx, Some(repo.clone()), source);
    assert_eq!(view.read_with(cx, |v, _| v.title()), "main.tex (conflict)");
    view.read_with(cx, |v, cx| {
        assert_eq!(v.state.read(cx).resolved_text("main.tex"), None)
    });
    view.update(cx, |v, cx| {
        v.state.update(cx, |s, cx| {
            s.resolve_conflict("main.tex", 0, Some(DiffConflictResolution::Incoming), cx)
        })
    });
    draw(cx, window);
    view.read_with(cx, |v, cx| {
        assert_eq!(
            v.state.read(cx).resolved_text("main.tex").as_deref(),
            Some("\\section{Intro}\nTheirs.\nThe end.\n")
        )
    });

    // A file that is gone.
    view.update(cx, |v, cx| {
        v.show(
            Some(repo),
            DiffSource::Conflict {
                path: "gone.tex".into(),
            },
            cx,
        )
    });
    cx.run_until_parked();
    draw(cx, window);
    view.read_with(cx, |v, _| assert!(v.error.is_some()));
}

/// A diff that failed to load once shows again when it loads, even unchanged.
#[gpui::test]
fn recovers_from_a_failed_reload(cx: &mut TestAppContext) {
    let Some(s) = GitSandbox::new() else { return };
    let (repo, _) = history(&s);
    let new = repo.root().join("new.tex");
    std::fs::write(&new, "new\n").unwrap();
    let (window, view, _) = view(cx, Some(repo), working("new.tex", false, true));
    assert_eq!(files(cx, &view), ["new.tex"]);
    // Gone for a moment (a checkout, another editor saving).
    std::fs::remove_file(&new).unwrap();
    view.update(cx, |v, cx| v.reload(cx));
    cx.run_until_parked();
    view.read_with(cx, |v, _| assert!(v.error.is_some()));
    std::fs::write(&new, "new\n").unwrap();
    view.update(cx, |v, cx| v.reload(cx));
    cx.run_until_parked();
    draw(cx, window);
    view.read_with(cx, |v, _| assert_eq!(v.error, None));
    assert_eq!(files(cx, &view), ["new.tex"]);
}
