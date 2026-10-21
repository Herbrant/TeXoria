//! Paths in the form TeXoria shows, stores and compares them.

use std::path::Path;

pub use tex_sys::paths::{canonical, plain};

/// `path` relative to `dir` (the project's folder), as the system writes it; `path` itself
/// outside of it.
pub fn relative<'a>(path: &'a Path, dir: Option<&Path>) -> &'a Path {
    dir.and_then(|d| path.strip_prefix(d).ok()).unwrap_or(path)
}

/// The last part of `path`'s name (`main.tex`), empty if it has none.
pub fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Whether `path` is LaTeX source, by its extension in any case (`.TEX` from old systems).
pub fn is_latex(path: &Path) -> bool {
    tex_project::has_extension(path, &["tex", "ltx", "sty", "cls", "dtx"])
}

/// Whether `path` is a BibTeX database (`.bib`, in any case).
pub fn is_bibtex(path: &Path) -> bool {
    tex_project::has_extension(path, &["bib"])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_relative_to_the_project() {
        let dir = Path::new("/p");
        assert_eq!(
            relative(&dir.join("ch/a.tex"), Some(dir)),
            Path::new("ch/a.tex")
        );
        assert_eq!(
            relative(Path::new("/q/a.tex"), Some(dir)),
            Path::new("/q/a.tex")
        );
        assert_eq!(relative(Path::new("/p/a.tex"), None), Path::new("/p/a.tex"));
    }

    #[test]
    fn file_names() {
        assert_eq!(file_name(Path::new("/p/ch/a.tex")), "a.tex");
        assert_eq!(file_name(Path::new("/")), "");
    }

    #[test]
    fn latex_sources() {
        for p in [
            "main.tex",
            "a/b/MAIN.TEX",
            "x.sty",
            "x.cls",
            "x.ltx",
            "pkg.dtx",
            "a.b.Tex",
        ] {
            assert!(is_latex(Path::new(p)), "{p}");
        }
        for p in ["refs.bib", "Makefile", "tex", ".tex.bak", "notes.txt"] {
            assert!(!is_latex(Path::new(p)), "{p}");
        }
    }

    #[test]
    fn bibtex_databases() {
        for p in ["refs.bib", "a/REFS.BIB"] {
            assert!(is_bibtex(Path::new(p)), "{p}");
        }
        for p in ["main.tex", "bib", ".bib.bak", "refs.bbl"] {
            assert!(!is_bibtex(Path::new(p)), "{p}");
        }
    }
}
