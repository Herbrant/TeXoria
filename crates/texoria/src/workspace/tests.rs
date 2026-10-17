use std::path::{Path, PathBuf};

use gpui_kit::TestAppContext;

use super::*;
use crate::testing;
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

#[gpui::test]
fn opens_a_project_folder(cx: &mut TestAppContext) {
    let dir = testing::fixture_copy("sample");
    let (_, workspace) = testing::open(cx, Some(dir.path().to_path_buf()), testing::startup());
    let main = std::fs::canonicalize(dir.path().join("main.tex")).unwrap();
    workspace.read_with(cx, |ws, _| {
        assert_eq!(ws.root.as_ref(), Some(&main));
        assert_eq!(ws.buffers.len(), 1);
        assert_eq!(ws.buffers[ws.active].path, main);
        let titles: Vec<_> = ws.outline.iter().map(|i| i.item.title.as_str()).collect();
        assert!(titles.contains(&"Introduction"), "{titles:?}");
    });
}
