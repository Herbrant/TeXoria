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
    assert_eq!(find_main_file(dir.path()), None);
    std::fs::write(dir.path().join("notes.tex"), "no class here").unwrap();
    std::fs::write(dir.path().join("b.tex"), "\\documentclass{article}").unwrap();
    std::fs::write(dir.path().join("a.tex"), "\\documentclass{book}").unwrap();
    assert_eq!(find_main_file(dir.path()), Some(dir.path().join("a.tex")));
    std::fs::write(dir.path().join("main.tex"), "").unwrap();
    assert_eq!(
        find_main_file(dir.path()),
        Some(dir.path().join("main.tex"))
    );
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

/// The sample project opened on `main.tex`, in a temporary copy.
fn sample(cx: &mut TestAppContext) -> (tempfile::TempDir, WindowHandle<Root>, Entity<Workspace>) {
    let dir = testing::fixture_copy("sample");
    let (window, workspace) =
        testing::open(cx, Some(dir.path().join("main.tex")), testing::startup());
    (dir, window, workspace)
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
