//! Which file to open for a folder. Nothing is assumed about its name: the
//! documents are told apart from their parts by what they contain.

use std::cmp::Reverse;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use tex_syntax::{IncludeKind, Index};

use crate::{files, magic_root, normalize, read_regular, resolve_tex};

/// How deep a folder is searched: projects often keep their sources in a
/// subfolder (`src/`, `tex/`), rarely deeper.
const MAX_DEPTH: usize = 3;
/// Entries looked at, in case the folder is not a project (a home folder).
const MAX_ENTRIES: usize = 2_000;

struct Document {
    path: PathBuf,
    depth: usize,
    /// The `.tex` files it includes.
    parts: Vec<PathBuf>,
    /// Whether it has a `\begin{document}`: a class line alone may be a
    /// template or a shared preamble.
    body: bool,
}

/// The LaTeX documents in `dir` and its subfolders, most likely main file first.
///
/// A document is a `.tex` file with a `\documentclass` outside comments, that
/// no other document includes and whose `% !TeX root` comment, if any, does not
/// name another file. Those with a `\begin{document}` come first, then the
/// least deep, then those that include the most files, then by path.
pub fn main_files(dir: &Path) -> Vec<PathBuf> {
    let mut tex = Vec::new();
    let mut budget = MAX_ENTRIES;
    walk(dir, 0, &mut budget, &mut tex);

    let mut docs = Vec::new();
    for (path, depth) in tex {
        let path = normalize(&path);
        let Some(text) = read_regular(&path) else {
            continue;
        };
        if magic_root(&path, &text).is_some_and(|root| root != path)
            || !text.contains("\\documentclass")
        {
            continue;
        }
        let index = Index::from_text(&text);
        if index.document_class.is_none() {
            continue;
        }
        let base = path.parent().unwrap_or(dir);
        let parts = index
            .includes
            .iter()
            .filter(|i| i.kind == IncludeKind::Tex)
            .map(|i| resolve_tex(base, &i.path))
            .filter(|p| *p != path)
            .collect();
        docs.push(Document {
            path,
            depth,
            parts,
            body: text.contains("\\begin{document}"),
        });
    }

    // `subfiles` and `standalone` parts have a class of their own.
    let parts: HashSet<&PathBuf> = docs.iter().flat_map(|d| &d.parts).collect();
    let (mut main, included): (Vec<&Document>, Vec<&Document>) =
        docs.iter().partition(|d| !parts.contains(&d.path));
    if main.is_empty() {
        // Documents that include one another: none of them is a part.
        main = included;
    }
    main.sort_by_key(|d| (Reverse(d.body), d.depth, Reverse(d.parts.len()), &d.path));
    main.into_iter().map(|d| d.path.clone()).collect()
}

/// Collects the `.tex` files under `dir` with their depth, skipping hidden
/// folders (`.git`, `.texoria`), linked folders and huge files.
fn walk(dir: &Path, depth: usize, budget: &mut usize, out: &mut Vec<(PathBuf, usize)>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        if *budget == 0 {
            return;
        }
        *budget -= 1;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || files::is_hidden(&name) {
            continue;
        }
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if kind.is_dir() {
            if depth + 1 < MAX_DEPTH {
                walk(&path, depth + 1, budget, out);
            }
        } else if crate::has_extension(&path, &["tex"])
            && fs::metadata(&path).is_ok_and(|m| m.is_file() && m.len() <= files::MAX_TEXT_BYTES)
        {
            out.push((path, depth));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
        let path = dir.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
        path
    }

    const DOC: &str = "\\documentclass{article}\n\\begin{document}\nHi\n\\end{document}\n";

    #[test]
    fn no_document() {
        let dir = tempfile::tempdir().unwrap();
        assert!(main_files(dir.path()).is_empty());
        write(dir.path(), "main.tex", "\\section{Not a document}");
        write(dir.path(), "notes.tex", "% \\documentclass{article}");
        write(dir.path(), "class.txt", DOC);
        assert!(main_files(dir.path()).is_empty());
    }

    #[test]
    fn the_name_does_not_matter() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "main.tex", "\\section{Introduction}");
        let thesis = write(dir.path(), "zz-thesis.tex", DOC);
        assert_eq!(main_files(dir.path()), vec![thesis]);
    }

    #[test]
    fn looks_in_subfolders() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "README.md", "# Paper");
        let paper = write(dir.path(), "src/paper.tex", DOC);
        write(dir.path(), "a/b/c/deep.tex", DOC);
        write(dir.path(), ".texoria/build/old.tex", DOC);
        assert_eq!(main_files(dir.path()), vec![paper]);
    }

    #[test]
    fn leaves_out_included_documents() {
        let dir = tempfile::tempdir().unwrap();
        let book = write(
            dir.path(),
            "zbook.tex",
            "\\documentclass{book}\n\\begin{document}\n\\subfile{chapters/one}\n\\end{document}",
        );
        write(
            dir.path(),
            "chapters/one.tex",
            "\\documentclass[../zbook]{subfiles}\n\\begin{document}\nOne\n\\end{document}",
        );
        write(
            dir.path(),
            "a-figure.tex",
            "% !TeX root = zbook.tex\n\\documentclass{standalone}\n\\begin{document}x\\end{document}",
        );
        assert_eq!(main_files(dir.path()), vec![book]);
    }

    #[test]
    fn ranks_the_documents() {
        let dir = tempfile::tempdir().unwrap();
        let preamble = write(dir.path(), "a-preamble.tex", "\\documentclass{article}");
        let slides = write(dir.path(), "slides.tex", DOC);
        let thesis = write(
            dir.path(),
            "thesis.tex",
            "\\documentclass{book}\n\\begin{document}\n\\input{intro}\n\\end{document}",
        );
        write(dir.path(), "intro.tex", "\\chapter{Intro}");
        let nested = write(dir.path(), "talk/a.tex", DOC);
        assert_eq!(
            main_files(dir.path()),
            vec![thesis, slides, nested, preamble]
        );
    }

    #[test]
    fn documents_that_include_each_other() {
        let dir = tempfile::tempdir().unwrap();
        let a = write(
            dir.path(),
            "a.tex",
            "\\documentclass{article}\n\\begin{document}\\input{b}\\end{document}",
        );
        let b = write(
            dir.path(),
            "b.tex",
            "\\documentclass{article}\n\\begin{document}\\input{a}\\end{document}",
        );
        assert_eq!(main_files(dir.path()), vec![a, b]);
    }
}
