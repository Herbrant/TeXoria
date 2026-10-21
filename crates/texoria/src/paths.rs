//! Paths in the form TeXoria shows, stores and compares them.

use std::path::Path;

pub use tex_sys::paths::{canonical, plain};

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
