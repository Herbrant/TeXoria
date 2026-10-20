use std::path::{Path, PathBuf};

use gpui_kit::{Entity, TestAppContext, WindowHandle, base::Root};

use super::*;
use crate::{paths, testing};
// `super::*` brings gpui's `test` attribute, which would shadow the standard one, and
// gpui-kit's `gpui` alias, which the `#[gpui::test]` macro's paths must not find twice.
use core::prelude::v1::test;
use gpui_kit::gpui;

#[test]
fn vendor_dirs_only_for_binaries_built_in_the_checkout() {
    let repo = Path::new("/src/texoria");
    assert_eq!(
        vendor_dir_of(
            repo,
            Path::new("/src/texoria/target/debug/texoria"),
            "pdfium"
        ),
        Some(PathBuf::from("/src/texoria/vendor/pdfium"))
    );
    assert_eq!(
        vendor_dir_of(
            repo,
            Path::new("/Applications/TeXoria.app/Contents/MacOS/texoria"),
            "pdfium"
        ),
        None
    );
    // A sibling folder whose name only starts like `target`.
    assert_eq!(
        vendor_dir_of(repo, Path::new("/src/texoria/target-old/texoria"), "pdfium"),
        None
    );
}

#[test]
fn pdfium_is_looked_for_next_to_the_executable_first() {
    let dirs = pdfium_dirs_of(
        Some(Path::new("/opt/app/bin/texoria")),
        Some(PathBuf::from("/src/vendor/pdfium")),
    );
    assert_eq!(dirs.first(), Some(&PathBuf::from("/opt/app/bin")));
    assert!(dirs.contains(&PathBuf::from("/opt/app/bin/../lib/texoria")));
    assert!(dirs.contains(&PathBuf::from("/opt/app/bin/../Frameworks")));
    assert_eq!(dirs.last(), Some(&PathBuf::from("/src/vendor/pdfium")));
    assert!(pdfium_dirs_of(None, None).is_empty());
}

#[test]
fn finds_the_main_file_of_a_folder() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(find_main_file(dir.path(), &[]), None);
    std::fs::write(dir.path().join("main.tex"), "\\section{Not the root}").unwrap();
    let doc = "\\documentclass{article}\n\\begin{document}\n\\end{document}";
    std::fs::write(dir.path().join("b.tex"), doc).unwrap();
    std::fs::write(dir.path().join("a.tex"), doc).unwrap();
    let (a, b) = (dir.path().join("a.tex"), dir.path().join("b.tex"));
    assert_eq!(find_main_file(dir.path(), &[]), Some(a.clone()));
    // The document opened last wins.
    let other = PathBuf::from("/elsewhere/main.tex");
    assert_eq!(find_main_file(dir.path(), &[other, b.clone(), a]), Some(b));
}

#[test]
fn package_names() {
    for ok in [
        "amsmath",
        "biblatex-chicago",
        "l3kernel",
        "pgf_v3",
        "tikz.code",
    ] {
        assert!(is_package_name(ok), "{ok}");
    }
    for bad in [
        "", "-h", "..", "../x", "a/b", "a b", "x?y=1", "é", "a#b", "%2e%2e",
    ] {
        assert!(!is_package_name(bad), "{bad}");
    }
}

#[test]
fn run_summaries() {
    let done = |status| CompileState::Done {
        status,
        elapsed: Duration::from_secs(1),
        errors: 0,
        warnings: 0,
    };
    assert_eq!(
        run_summary(&done(Status::Success)),
        "The last run succeeded."
    );
    assert_eq!(run_summary(&done(Status::Failed)), "The last run failed.");
    assert!(run_summary(&done(Status::Cancelled)).contains("stopped"));
    assert!(run_summary(&CompileState::Idle).contains("not been built"));
    assert_eq!(
        run_summary(&CompileState::Failed("No TeX engine".into())),
        "No TeX engine"
    );
}

#[test]
fn file_uris() {
    assert_eq!(
        file_uri(Path::new("/home/me/a b.tex")),
        "file:///home/me/a%20b.tex"
    );
    assert_eq!(
        file_uri(Path::new(r"C:\Users\me\é.tex")),
        "file:///C:/Users/me/%C3%A9.tex"
    );
    assert_eq!(
        file_uri(Path::new(r"\\server\share\x.tex")),
        "file://server/share/x.tex"
    );
}

#[gpui::test]
fn opens_a_project_folder(cx: &mut TestAppContext) {
    let dir = testing::fixture_copy("sample");
    let (_, workspace) = testing::open(cx, Some(dir.path().to_path_buf()), testing::startup());
    let main = paths::canonical(&dir.path().join("main.tex"));
    workspace.read_with(cx, |ws, _| {
        assert_eq!(ws.root.as_ref(), Some(&main));
        assert_eq!(ws.buffers.len(), 1);
        assert_eq!(ws.buffers[ws.active].path, main);
        let titles: Vec<_> = ws.outline.iter().map(|i| i.item.title.as_str()).collect();
        assert!(titles.contains(&"Introduction"), "{titles:?}");
    });
}

/// Answers the next open dialog with `path`, after checking what it was asked to pick.
fn pick(
    cx: &mut TestAppContext,
    window: WindowHandle<Root>,
    workspace: &Entity<Workspace>,
    folder: bool,
    path: PathBuf,
) {
    cx.update_window(window.into(), |_, window, cx| {
        workspace.update(cx, |ws, cx| ws.prompt_open(folder, window, cx))
    })
    .unwrap();
    cx.simulate_path_prompt_response(|options| {
        // Outside macOS a dialog that may pick folders shows only folders.
        assert_eq!(options.files, !folder);
        assert_eq!(options.directories, folder || cfg!(target_os = "macos"));
        Some(vec![path])
    });
    cx.run_until_parked();
}

#[gpui::test]
fn opens_a_file_or_a_folder_from_the_dialog(cx: &mut TestAppContext) {
    let dir = testing::fixture_copy("sample");
    let (window, workspace) = testing::open(cx, None, testing::startup());
    let main = paths::canonical(&dir.path().join("main.tex"));
    let methods = paths::canonical(&dir.path().join("chapters/methods.tex"));

    pick(cx, window, &workspace, false, methods.clone());
    workspace.read_with(cx, |ws, _| {
        assert_eq!(ws.root.as_ref(), Some(&main));
        assert_eq!(ws.buffers[ws.active].path, methods);
    });

    let (window, workspace) = testing::open(cx, None, testing::startup());
    pick(cx, window, &workspace, true, dir.path().to_path_buf());
    workspace.read_with(cx, |ws, _| {
        assert_eq!(ws.root.as_ref(), Some(&main));
        assert_eq!(ws.buffers[ws.active].path, main);
    });
}

/// The sample project opened on `main.tex`, in a temporary copy.
fn sample(cx: &mut TestAppContext) -> (tempfile::TempDir, WindowHandle<Root>, Entity<Workspace>) {
    let dir = testing::fixture_copy("sample");
    let (window, workspace) =
        testing::open(cx, Some(dir.path().join("main.tex")), testing::startup());
    (dir, window, workspace)
}

#[gpui::test]
fn bib_files_are_highlighted_as_bibtex(cx: &mut TestAppContext) {
    let (dir, window, workspace) = sample(cx);
    let bib = paths::canonical(&dir.path().join("refs.bib"));
    with(cx, window, &workspace, |ws, window, cx| {
        ws.activate_file(bib.clone(), window, cx);
    });
    workspace.read_with(cx, |ws, cx| {
        let language = |ix: usize| ws.buffers[ix].editor.read(cx).language_name();
        assert_eq!(ws.buffers[ws.active].path, bib);
        assert_eq!(language(ws.active), crate::latex::BIBTEX);
        assert_eq!(language(0), crate::latex::LANGUAGE);
    });
}

#[gpui::test]
fn reindexing_parses_only_the_buffers_that_changed(cx: &mut TestAppContext) {
    let (dir, window, workspace) = sample(cx);
    let main = paths::canonical(&dir.path().join("main.tex"));
    let methods = paths::canonical(&dir.path().join("chapters/methods.tex"));
    let titles = |cx: &mut TestAppContext| {
        workspace.read_with(cx, |ws, _| {
            (ws.outline.iter())
                .map(|i| i.item.title.clone())
                .collect::<Vec<_>>()
        })
    };
    with(cx, window, &workspace, |ws, window, cx| {
        ws.activate_file(methods.clone(), window, cx);
        ws.reindex(cx);
        // An index that main.tex's text would not give: kept while main.tex is unchanged.
        let tampered = "\\section{Tampered}\n\\input{chapters/methods}\n";
        ws.project.write().unwrap().update(&main, tampered);
        let b = ws.buffers.iter().find(|b| b.path == methods).unwrap();
        b.editor.update(cx, |state, cx| {
            state.set_selected_range(0..0, cx);
            state.insert("\\section{Added}\n".to_string(), window, cx);
        });
        ws.reindex(cx);
    });
    let after_edit = titles(cx);
    assert!(
        after_edit.contains(&"Tampered".to_string()),
        "{after_edit:?}"
    );
    assert!(after_edit.contains(&"Added".to_string()), "{after_edit:?}");

    with(cx, window, &workspace, |ws, _, cx| ws.reindex_all(cx));
    let after_all = titles(cx);
    assert!(
        !after_all.contains(&"Tampered".to_string()),
        "{after_all:?}"
    );
    assert!(
        after_all.contains(&"Introduction".to_string()),
        "{after_all:?}"
    );
}

fn query(text: &str) -> tex_project::search::Query {
    tex_project::search::Query {
        text: text.into(),
        case_sensitive: true,
        whole_word: false,
        regex: false,
    }
}

#[gpui::test]
fn replace_all_edits_open_buffers_and_writes_the_others(cx: &mut TestAppContext) {
    let (dir, window, workspace) = sample(cx);
    let main = paths::canonical(&dir.path().join("main.tex"));
    let methods = paths::canonical(&dir.path().join("chapters/methods.tex"));
    let before = std::fs::read_to_string(&methods).unwrap();
    assert!(before.contains("sec:"), "the fixture changed");
    cx.update_window(window.into(), |_, window, cx| {
        workspace.update(cx, |ws, cx| {
            ws.replace_all(
                &query("sec:"),
                "s:",
                &[main.clone(), methods.clone()],
                window,
                cx,
            )
        })
    })
    .unwrap();
    cx.run_until_parked();
    workspace.read_with(cx, |ws, cx| {
        let b = ws.buffers.iter().find(|b| b.path == main).unwrap();
        let text = b.editor.read(cx).text().to_string();
        assert!(
            !text.contains("sec:") && text.contains("\\label{s:intro}"),
            "{text}"
        );
        // In the editor, not saved: it can be undone.
        assert_ne!(std::fs::read_to_string(&main).unwrap(), text);
    });
    assert_eq!(
        std::fs::read_to_string(&methods).unwrap(),
        before.replace("sec:", "s:")
    );
}

#[gpui::test]
fn replace_all_goes_through_every_file_when_the_search_stopped(cx: &mut TestAppContext) {
    let (dir, _, workspace) = sample(cx);
    let main = paths::canonical(&dir.path().join("main.tex"));
    workspace.read_with(cx, |ws, _| {
        assert_eq!(
            ws.replace_targets(std::slice::from_ref(&main), false),
            std::slice::from_ref(&main)
        );
        let all = ws.replace_targets(std::slice::from_ref(&main), true);
        for f in ["main.tex", "refs.bib", "chapters/methods.tex"] {
            assert!(all.iter().any(|p| p.ends_with(f)), "{f} not in {all:?}");
        }
    });
}

#[test]
fn file_operations() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    create_file(&d.join("ch/new.tex")).unwrap();
    assert!(d.join("ch/new.tex").is_file());
    assert_eq!(
        create_file(&d.join("ch/new.tex")).unwrap_err().kind(),
        std::io::ErrorKind::AlreadyExists
    );
    std::fs::write(d.join("other.tex"), "keep").unwrap();
    assert_eq!(
        rename(&d.join("ch/new.tex"), &d.join("other.tex"))
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::AlreadyExists
    );
    assert_eq!(
        std::fs::read_to_string(d.join("other.tex")).unwrap(),
        "keep"
    );
    rename(&d.join("ch/new.tex"), &d.join("ch/renamed.tex")).unwrap();
    // Only the case changes: the same file on case-insensitive disks.
    rename(&d.join("ch/renamed.tex"), &d.join("ch/Renamed.tex")).unwrap();
    assert!(d.join("ch/Renamed.tex").exists());
    remove(&d.join("ch")).unwrap();
    assert!(!d.join("ch").exists());
    remove(&d.join("other.tex")).unwrap();
    assert!(remove(&d.join("missing")).is_err());
}

/// Deleting a link to a folder deletes the link, never the folder's files.
#[test]
fn removing_a_folder_link_keeps_its_target() {
    let dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    std::fs::write(target.path().join("theirs.tex"), "").unwrap();
    let link = dir.path().join("link");
    #[cfg(unix)]
    std::os::unix::fs::symlink(target.path(), &link).unwrap();
    // Creating links needs the developer mode or administrator rights on Windows.
    #[cfg(windows)]
    if let Err(e) = std::os::windows::fs::symlink_dir(target.path(), &link) {
        eprintln!("cannot create a folder link ({e}); skipping");
        return;
    }
    remove(&link).unwrap();
    assert!(link.symlink_metadata().is_err());
    assert!(target.path().join("theirs.tex").exists());
}

// ── Files and Git, as the sidebars ask for them ─────────────────────────

/// `f` on the workspace in its window.
fn with<R>(
    cx: &mut TestAppContext,
    window: WindowHandle<Root>,
    workspace: &Entity<Workspace>,
    f: impl FnOnce(&mut Workspace, &mut Window, &mut Context<Workspace>) -> R,
) -> R {
    cx.update_window(window.into(), |_, window, cx| {
        workspace.update(cx, |ws, cx| f(ws, window, cx))
    })
    .unwrap()
}

fn tabs(cx: &mut TestAppContext, workspace: &Entity<Workspace>, dir: &Path) -> Vec<String> {
    let dir = paths::canonical(dir);
    workspace.read_with(cx, |ws, _| {
        (ws.buffers.iter())
            .map(|b| {
                b.path
                    .strip_prefix(&dir)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect()
    })
}

fn text_of(cx: &mut TestAppContext, workspace: &Entity<Workspace>, path: &Path) -> String {
    workspace.read_with(cx, |ws, cx| {
        let b = ws.buffers.iter().find(|b| b.path == path).unwrap();
        b.editor.read(cx).text().to_string()
    })
}

#[gpui::test]
fn file_operations_keep_tabs_in_step(cx: &mut TestAppContext) {
    let (dir, window, workspace) = sample(cx);
    let d = paths::canonical(dir.path());
    let run = |cx: &mut TestAppContext, op: FileOp| {
        with(cx, window, &workspace, |ws, window, cx| {
            ws.run_file_op(op, window, cx)
        });
        cx.run_until_parked();
    };
    // A text file opens from the Files view.
    let methods = d.join("chapters/methods.tex");
    with(cx, window, &workspace, |ws, window, cx| {
        let files = ws.files.clone();
        ws.on_file_tree_event(&files, &FileTreeEvent::Open(methods.clone()), window, cx)
    });
    assert_eq!(
        tabs(cx, &workspace, &d),
        ["main.tex", "chapters/methods.tex"]
    );

    // A new file opens in a tab.
    run(cx, FileOp::CreateFile(d.join("notes.tex")));
    assert!(d.join("notes.tex").is_file());
    assert_eq!(
        tabs(cx, &workspace, &d),
        ["main.tex", "chapters/methods.tex", "notes.tex"]
    );
    run(cx, FileOp::CreateDir(d.join("figures/plots")));
    assert!(d.join("figures/plots").is_dir());

    // Tabs follow a file and a folder.
    run(
        cx,
        FileOp::Rename {
            from: methods.clone(),
            to: d.join("chapters/method.tex"),
        },
    );
    run(
        cx,
        FileOp::Rename {
            from: d.join("chapters"),
            to: d.join("parts"),
        },
    );
    assert_eq!(
        tabs(cx, &workspace, &d),
        ["main.tex", "parts/method.tex", "notes.tex"]
    );
    let text = text_of(cx, &workspace, &d.join("parts/method.tex"));
    assert_eq!(
        text,
        std::fs::read_to_string(d.join("parts/method.tex")).unwrap()
    );

    // The root file renamed: the project follows it.
    run(
        cx,
        FileOp::Rename {
            from: d.join("main.tex"),
            to: d.join("paper.tex"),
        },
    );
    workspace.read_with(cx, |ws, _| assert_eq!(ws.root, Some(d.join("paper.tex"))));
    assert_eq!(
        tabs(cx, &workspace, &d),
        ["paper.tex", "parts/method.tex", "notes.tex"]
    );

    // Deleting a folder closes the tabs of its files; the active one stays active.
    with(cx, window, &workspace, |ws, _, _| ws.active = 2);
    run(cx, FileOp::Delete(d.join("parts")));
    assert!(!d.join("parts").exists());
    assert_eq!(tabs(cx, &workspace, &d), ["paper.tex", "notes.tex"]);
    workspace.read_with(cx, |ws, _| assert_eq!(ws.active, 1));

    // A failure leaves everything as it was, with a word why.
    run(cx, FileOp::CreateFile(d.join("notes.tex")));
    run(cx, FileOp::Delete(d.join("missing.tex")));
    assert_eq!(tabs(cx, &workspace, &d), ["paper.tex", "notes.tex"]);
    let notes = cx
        .update_window(window.into(), |_, window, cx| {
            window.notifications(cx).len()
        })
        .unwrap();
    assert_eq!(notes, 2);
}

/// The sample project in a repository the workspace trusts, driven by the sandbox's git,
/// with its `main` pushed to a bare `origin`.
fn sample_repo(
    cx: &mut TestAppContext,
    s: &testing::GitSandbox,
) -> (
    tempfile::TempDir,
    PathBuf,
    PathBuf,
    WindowHandle<Root>,
    Entity<Workspace>,
) {
    let tmp = testing::fixture_copy("sample");
    let d = paths::canonical(tmp.path());
    s.run(&d, &["init", "--quiet"]);
    s.run(&d, &["add", "-A"]);
    s.run(&d, &["commit", "--quiet", "-m", "sample"]);
    let bare = s.dir.join("origin.git");
    s.run(&s.dir, &["init", "--bare", "--quiet", "origin.git"]);
    s.run(&bare, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    s.run(&d, &["remote", "add", "origin", &bare.to_string_lossy()]);
    s.run(&d, &["push", "--quiet", "-u", "origin", "main"]);
    let (window, workspace) = testing::open(cx, Some(d.join("main.tex")), testing::startup());
    // Untrusted until now: git has not run. Trusted, it runs with the sandbox's git.
    workspace.update(cx, |ws, cx| {
        assert!(ws.git.read(cx).repo().is_none());
        ws.git.update(cx, |g, _| g.use_git(s.git.clone()));
        ws.trust_git(cx);
    });
    cx.run_until_parked();
    workspace.read_with(cx, |ws, cx| {
        assert!(ws.settings.is_trusted(&d));
        assert!(ws.git.read(cx).repo().is_some());
    });
    (tmp, d, bare, window, workspace)
}

fn git_request(
    cx: &mut TestAppContext,
    window: WindowHandle<Root>,
    workspace: &Entity<Workspace>,
    op: GitOp,
) {
    with(cx, window, workspace, |ws, window, cx| {
        let git = ws.git.clone();
        ws.on_git_event(&git, &GitPanelEvent::Request(op), window, cx)
    });
    cx.run_until_parked();
}

#[gpui::test]
fn git_operations_reload_open_tabs(cx: &mut TestAppContext) {
    let Some(s) = testing::GitSandbox::new() else {
        return;
    };
    let (_tmp, d, bare, window, workspace) = sample_repo(cx, &s);
    let (main, methods, extra) = (
        d.join("main.tex"),
        d.join("chapters/methods.tex"),
        d.join("extra.tex"),
    );
    let original = std::fs::read_to_string(&main).unwrap();
    std::fs::write(&extra, "extra\n").unwrap();
    with(cx, window, &workspace, |ws, window, cx| {
        ws.activate_file(methods.clone(), window, cx);
        ws.activate_file(extra.clone(), window, cx);
    });
    // Changed by another editor: picked up when the window comes back.
    std::fs::write(&main, "changed elsewhere\n").unwrap();
    with(cx, window, &workspace, |ws, window, cx| {
        ws.reload_from_disk(window, cx);
        ws.git.update(cx, |g, cx| g.refresh(cx));
    });
    cx.run_until_parked();
    assert_eq!(text_of(cx, &workspace, &main), "changed elsewhere\n");
    let marks = workspace.read_with(cx, |ws, cx| ws.git.read(cx).file_marks());
    assert_eq!(marks.get(&main), Some(&crate::file_tree::GitMark::Modified));
    assert_eq!(marks.get(&extra), Some(&crate::file_tree::GitMark::Added));

    // Discarding puts the file back in its tab, and closes the deleted file's.
    git_request(
        cx,
        window,
        &workspace,
        GitOp::Discard {
            tracked: vec!["main.tex".into()],
            untracked: vec!["extra.tex".into()],
        },
    );
    assert_eq!(text_of(cx, &workspace, &main), original);
    assert_eq!(
        tabs(cx, &workspace, &d),
        ["main.tex", "chapters/methods.tex"]
    );

    // So does a pull.
    let other = s.dir.join("other");
    s.run(
        &s.dir,
        &["clone", "--quiet", &bare.to_string_lossy(), "other"],
    );
    std::fs::write(other.join("chapters/methods.tex"), "pulled\n").unwrap();
    s.run(&other, &["commit", "--quiet", "-am", "elsewhere"]);
    s.run(&other, &["push", "--quiet"]);
    git_request(cx, window, &workspace, GitOp::Pull);
    assert_eq!(text_of(cx, &workspace, &methods), "pulled\n");
}

#[gpui::test]
fn diff_tabs(cx: &mut TestAppContext) {
    let Some(s) = testing::GitSandbox::new() else {
        return;
    };
    let (_tmp, d, _, window, workspace) = sample_repo(cx, &s);
    let main = d.join("main.tex");
    let original = std::fs::read_to_string(&main).unwrap();
    std::fs::write(&main, "changed\n").unwrap();
    let git_event = |cx: &mut TestAppContext, ev: GitPanelEvent| {
        with(cx, window, &workspace, |ws, window, cx| {
            let git = ws.git.clone();
            ws.on_git_event(&git, &ev, window, cx)
        });
        cx.run_until_parked();
    };
    let diff_event = |cx: &mut TestAppContext, ev: DiffViewEvent| {
        with(cx, window, &workspace, |ws, window, cx| {
            let diff = ws.diff.clone().unwrap();
            ws.on_diff_event(&diff, &ev, window, cx)
        });
        cx.run_until_parked();
    };
    let source = DiffSource::Working {
        path: "main.tex".into(),
        staged: false,
        untracked: false,
    };
    git_event(cx, GitPanelEvent::OpenDiff(source.clone()));
    workspace.read_with(cx, |ws, cx| {
        assert!(ws.diff_active);
        let diff = ws.diff.as_ref().unwrap().read(cx);
        assert_eq!(diff.title(), "main.tex (changes)");
    });
    // A new status re-reads it; the side-by-side choice is remembered.
    git_event(cx, GitPanelEvent::StatusChanged);
    git_event(cx, GitPanelEvent::WorktreeChanged);
    diff_event(cx, DiffViewEvent::SplitChanged(true));
    workspace.read_with(cx, |ws, _| assert!(ws.settings.diff_split));
    // Opening the file leaves the diff for the editor.
    diff_event(cx, DiffViewEvent::Open(main.clone()));
    workspace.read_with(cx, |ws, _| assert!(!ws.diff_active && ws.diff.is_some()));

    // Restoring the last commit's version.
    let sha = s.run(&d, &["rev-parse", "HEAD"]);
    diff_event(
        cx,
        DiffViewEvent::Restore {
            sha,
            path: "main.tex".into(),
        },
    );
    assert_eq!(std::fs::read_to_string(&main).unwrap(), original);
    assert_eq!(text_of(cx, &workspace, &main), original);

    // A resolved conflict is written and staged, and its tab closes.
    std::fs::write(d.join("refs.bib"), "% resolved?\n").unwrap();
    diff_event(
        cx,
        DiffViewEvent::Resolved {
            path: "refs.bib".into(),
            text: "% resolved\n".into(),
        },
    );
    assert_eq!(
        std::fs::read_to_string(d.join("refs.bib")).unwrap(),
        "% resolved\n"
    );
    assert_eq!(s.run(&d, &["diff", "--cached", "--name-only"]), "refs.bib");
    workspace.read_with(cx, |ws, _| assert!(ws.diff.is_none() && !ws.diff_active));

    git_event(cx, GitPanelEvent::OpenDiff(source));
    diff_event(cx, DiffViewEvent::Close);
    workspace.read_with(cx, |ws, _| assert!(ws.diff.is_none()));
}

#[gpui::test]
fn no_diff_without_a_trusted_repository(cx: &mut TestAppContext) {
    let (_dir, window, workspace) = sample(cx);
    with(cx, window, &workspace, |ws, window, cx| {
        let source = DiffSource::Conflict {
            path: "main.tex".into(),
        };
        ws.open_diff(source, window, cx)
    });
    workspace.read_with(cx, |ws, _| assert!(ws.diff.is_none() && !ws.diff_active));
}
