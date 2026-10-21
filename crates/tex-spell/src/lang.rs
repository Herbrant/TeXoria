//! Which language a document is written in.

use tex_syntax::{Index, MainLanguage};
use whatlang::Detector;

use crate::catalog;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Chosen by the user.
    User,
    /// A `% !TeX spellcheck = ..` magic comment.
    Magic,
    /// babel/polyglossia.
    Package,
    /// Recognized from the text itself.
    Text,
    /// Nothing to go by: the system language.
    Fallback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Detection {
    pub code: &'static str,
    pub source: Source,
}

/// Prose shorter than this does not say much about its language.
const MIN_PROSE: usize = 80;
/// Enough text to recognize a language; the detector is linear in it.
const MAX_PROSE: usize = 20_000;

/// The language of a `% !TeX spellcheck = de_DE` magic comment (TeXShop,
/// TeXstudio, TeXworks) in the first lines.
pub fn magic_comment(text: &str) -> Option<&'static str> {
    tex_syntax::magic_comments(text, "spellcheck").find_map(catalog::resolve)
}

/// The main language declared with babel or polyglossia (babel's is its
/// `main=` option or else the last language among the class and package
/// options).
pub fn declared(index: &Index) -> Option<&'static str> {
    if let Some(MainLanguage { name, variant }) = &index.main_language {
        let by_variant = variant
            .as_deref()
            .and_then(|v| catalog::alias(&format!("{v}{name}")).or_else(|| catalog::alias(v)));
        if let Some(code) = by_variant.or_else(|| catalog::alias(name)) {
            return Some(code);
        }
    }
    index
        .language_options
        .iter()
        .rev()
        .find_map(|o| catalog::alias(o))
}

/// Recognize the language of `prose` (the words of a document), choosing the
/// regional variant for `region` when there is one.
pub fn recognize(prose: &str, region: Option<&str>) -> Option<&'static str> {
    if prose.chars().count() < MIN_PROSE {
        return None;
    }
    let mut end = prose.len().min(MAX_PROSE);
    while !prose.is_char_boundary(end) {
        end -= 1;
    }
    let info = Detector::with_allowlist(catalog::detectable()).detect(&prose[..end])?;
    if !info.is_reliable() {
        return None;
    }
    catalog::for_detected(info.lang(), region)
}

/// What a file says about its language and what the project's root file says
/// (`root` is `None` for a root file).
#[derive(Clone, Copy)]
pub struct Clues<'a> {
    pub text: &'a str,
    pub index: Option<&'a Index>,
    pub root_text: Option<&'a str>,
    pub root_index: Option<&'a Index>,
    /// The file's prose (see [`crate::extract::prose`]).
    pub prose: &'a str,
    /// The root's prose, for a file with too little of its own (a short chapter).
    pub root_prose: &'a str,
    /// The system locale's region, for regional variants.
    pub region: Option<&'a str>,
    pub fallback: &'static str,
}

/// Magic comment (the file's, then the root's), then babel/polyglossia (the
/// root's, then the file's), then the text (the file's, then the root's), then the
/// fallback.
pub fn detect(c: &Clues) -> Detection {
    let found = |code: Option<&'static str>, source| code.map(|code| Detection { code, source });
    found(magic_comment(c.text), Source::Magic)
        .or_else(|| found(c.root_text.and_then(magic_comment), Source::Magic))
        .or_else(|| found(c.root_index.and_then(declared), Source::Package))
        .or_else(|| found(c.index.and_then(declared), Source::Package))
        .or_else(|| found(recognize(c.prose, c.region), Source::Text))
        .or_else(|| found(recognize(c.root_prose, c.region), Source::Text))
        .unwrap_or(Detection {
            code: c.fallback,
            source: Source::Fallback,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn magic_comments_never_panic(text in "(%[ %\t]*(!TeX spellcheck ?= ?)?\\PC{0,16}\n){0,4}") {
            if let Some(code) = magic_comment(&text) {
                prop_assert!(catalog::get(code).is_some());
            }
        }
    }

    #[test]
    fn magic_comments_are_in_the_first_lines_only() {
        assert_eq!(magic_comment("% !TeX spellcheck de_DE"), None);
        assert_eq!(magic_comment("% é!TeX spellcheck = de_DE"), None);
        assert_eq!(magic_comment("% !TeX spellcheck="), None);
        let late = format!("{}% !TeX spellcheck = de_DE\n", "\n".repeat(20));
        assert_eq!(magic_comment(&late), None);
        assert_eq!(magic_comment(&late[1..]), Some("de"));
    }

    #[test]
    fn recognizes_long_text_cut_inside_a_character() {
        // The cut at MAX_PROSE falls inside a two-byte `è`.
        let prose = (0..200)
            .map(|pad| {
                format!(
                    "{}{}",
                    " ".repeat(pad),
                    format!("{ITALIAN} perché è così ").repeat(MAX_PROSE / 100)
                )
            })
            .find(|p| !p.is_char_boundary(MAX_PROSE))
            .unwrap();
        assert_eq!(recognize(&prose, None), Some("it"));
    }

    #[test]
    fn declared_variants_fall_back_to_the_language() {
        // An unknown variant: the language itself.
        let idx = Index::from_text("\\setmainlanguage[variant=martian]{german}");
        assert_eq!(declared(&idx), Some("de"));
        // A variant that is a language of its own.
        let idx = Index::from_text("\\setmainlanguage[variant=british]{klingonese}");
        assert_eq!(declared(&idx), Some("en-GB"));
        let idx = Index::from_text("\\setmainlanguage{klingonese}\n\\usepackage[italian]{babel}");
        assert_eq!(declared(&idx), Some("it"));
    }

    const ITALIAN: &str = "Questo documento descrive il funzionamento del sistema e le scelte \
        progettuali che hanno guidato lo sviluppo del programma negli ultimi anni.";
    const ENGLISH: &str = "This document describes how the system works and the design \
        choices that guided the development of the program over the last few years.";
    const GERMAN: &str = "Dieses Dokument beschreibt die Funktionsweise des Systems und die \
        Entscheidungen, die die Entwicklung des Programms in den letzten Jahren geprägt haben.";

    fn clues<'a>(text: &'a str, prose: &'a str) -> Clues<'a> {
        Clues {
            text,
            index: None,
            root_text: None,
            root_index: None,
            prose,
            root_prose: "",
            region: None,
            fallback: "en",
        }
    }

    #[test]
    fn reads_magic_comments() {
        assert_eq!(magic_comment("% !TeX spellcheck = de_DE\n"), Some("de"));
        assert_eq!(magic_comment("%!TEX spellcheck=en_GB\n"), Some("en-GB"));
        assert_eq!(
            magic_comment("% !TeX root = main.tex\n%  !tex SpellCheck = it-IT"),
            Some("it")
        );
        assert_eq!(magic_comment("text % !TeX spellcheck = de_DE"), None);
        assert_eq!(magic_comment("% !TeX spellcheck = klingonese"), None);
    }

    #[test]
    fn reads_babel_and_polyglossia() {
        let idx =
            Index::from_text("\\documentclass[a4paper,english]{article}\n\\usepackage{babel}");
        assert_eq!(declared(&idx), Some("en"));
        let idx = Index::from_text(
            "\\documentclass[english]{article}\n\\usepackage[ngerman,italian]{babel}",
        );
        assert_eq!(declared(&idx), Some("it"));
        let idx = Index::from_text("\\usepackage[main=british,italian]{babel}");
        assert_eq!(declared(&idx), Some("en-GB"));
        let idx =
            Index::from_text("\\usepackage{polyglossia}\n\\setmainlanguage[variant=swiss]{german}");
        assert_eq!(declared(&idx), Some("de-CH"));
        let idx = Index::from_text("\\setdefaultlanguage[variant=uk]{english}");
        assert_eq!(declared(&idx), Some("en-GB"));
        let idx = Index::from_text("\\setmainlanguage{french}");
        assert_eq!(declared(&idx), Some("fr"));
        let idx = Index::from_text("\\documentclass[11pt]{article}\\usepackage[utf8]{inputenc}");
        assert_eq!(declared(&idx), None);
    }

    #[test]
    fn recognizes_text() {
        assert_eq!(recognize(ITALIAN, None), Some("it"));
        assert_eq!(recognize(ENGLISH, None), Some("en"));
        assert_eq!(recognize(ENGLISH, Some("GB")), Some("en-GB"));
        assert_eq!(recognize(GERMAN, None), Some("de"));
        assert_eq!(recognize("Ciao a tutti", None), None);
    }

    #[test]
    fn follows_priorities() {
        let d = detect(&clues("", ITALIAN));
        assert_eq!((d.code, d.source), ("it", Source::Text));

        let d = detect(&clues("", "troppo corto"));
        assert_eq!((d.code, d.source), ("en", Source::Fallback));

        // A short chapter: the language of the document it is part of.
        let d = detect(&Clues {
            root_prose: ITALIAN,
            ..clues("", "Methods")
        });
        assert_eq!((d.code, d.source), ("it", Source::Text));
        let d = detect(&Clues {
            root_prose: ITALIAN,
            ..clues("", GERMAN)
        });
        assert_eq!((d.code, d.source), ("de", Source::Text));

        let root = Index::from_text("\\usepackage[french]{babel}");
        let d = detect(&Clues {
            root_index: Some(&root),
            ..clues("", ITALIAN)
        });
        assert_eq!((d.code, d.source), ("fr", Source::Package));

        let d = detect(&Clues {
            root_index: Some(&root),
            root_text: Some("% !TeX spellcheck = de_DE"),
            ..clues("", ITALIAN)
        });
        assert_eq!((d.code, d.source), ("de", Source::Magic));

        let d = detect(&Clues {
            root_text: Some("% !TeX spellcheck = de_DE"),
            ..clues("% !TeX spellcheck = it_IT", ENGLISH)
        });
        assert_eq!((d.code, d.source), ("it", Source::Magic));
    }
}
