//! Strategies shared by the property tests.

use proptest::prelude::*;

/// Pieces of LaTeX, prose of several scripts and stray markup: joined at
/// random, they make documents that are often unbalanced.
const TOKENS: &[&str] = &[
    "word",
    "Parola",
    "l'anno",
    "don't.",
    " ",
    " ",
    "\n",
    "\n\n",
    "~",
    ",",
    "--",
    "``",
    "''",
    "città",
    "e\u{301}",
    "שָׁלוֹם",
    "مرحبا",
    "नमस्ते",
    "𝕏",
    "Привет",
    "\\emph{",
    "\\textbf{",
    "}",
    "{",
    "[",
    "]",
    "$",
    "$x$",
    "\\(",
    "\\)",
    "\\[",
    "\\]",
    "\\cite{k}",
    "\\ref{a}",
    "\\verb|a b|",
    "\\verb",
    "|",
    "%",
    "% comment\n",
    "\\",
    "\\'e",
    "\\\"",
    "&",
    "^",
    "_",
    "#",
    "\\section{",
    "\\caption{",
    "\\title{",
    "\\begin{document}",
    "\\end{document}",
    "\\begin{figure}",
    "\\end{figure}",
    "\\begin{tikzpicture}",
    "\\end{tikzpicture}",
    "\\texttt{",
    "\\foo",
    "\\documentclass{article}",
];

pub fn latex() -> impl Strategy<Value = String> {
    prop::collection::vec(prop::sample::select(TOKENS), 0..48).prop_map(|t| t.concat())
}

/// Whether `ranges` are sorted slices of `text` on character boundaries.
pub fn valid_ranges(text: &str, ranges: &[std::ops::Range<usize>]) -> Result<(), String> {
    for r in ranges {
        if r.start > r.end || text.get(r.clone()).is_none() {
            return Err(format!("{r:?} is not a slice of {text:?}"));
        }
    }
    if !ranges.windows(2).all(|w| w[0].start <= w[1].start) {
        return Err(format!("{ranges:?} not sorted"));
    }
    Ok(())
}
