//! The prose of a LaTeX document: the words a spell checker should look at.
//!
//! Only `text` reachable through prose-carrying nodes is kept (the document
//! body, section titles, captions, footnotes and the arguments of ordinary
//! commands); math, comments, verbatim, labels, citations, paths, options,
//! environment arguments and the arguments of non-prose commands are skipped.

use std::ops::Range;

use tree_sitter::{Node, Tree};

/// Nodes whose `text` children are prose (besides `text` itself, ordinary
/// commands and environments).
const CONTAINERS: &[&str] = &[
    "source_file",
    "part",
    "chapter",
    "section",
    "subsection",
    "subsubsection",
    "paragraph",
    "subparagraph",
    "enum_item",
    "caption",
    "title_declaration",
    "hyperlink",
    "todo",
    "curly_group",
    "brack_group",
];

/// Commands whose arguments are not prose (code, units, lengths, keys, fonts,
/// languages...).
const SKIP_COMMANDS: &[&str] = &[
    "\\texttt",
    "\\verb",
    "\\verb*",
    "\\lstinline",
    "\\mintinline",
    "\\path",
    "\\email",
    "\\doi",
    "\\orcid",
    "\\bibitem",
    "\\nocite",
    "\\hyphenation",
    "\\foreignlanguage",
    "\\selectlanguage",
    "\\setmainlanguage",
    "\\setdefaultlanguage",
    "\\setotherlanguage",
    "\\setotherlanguages",
    "\\SI",
    "\\si",
    "\\num",
    "\\qty",
    "\\unit",
    "\\ang",
    "\\hspace",
    "\\vspace",
    "\\hspace*",
    "\\vspace*",
    "\\setlength",
    "\\addtolength",
    "\\setcounter",
    "\\addtocounter",
    "\\pagestyle",
    "\\thispagestyle",
    "\\pagenumbering",
    "\\setmainfont",
    "\\setsansfont",
    "\\setmonofont",
    "\\newfontfamily",
    "\\fontspec",
    "\\fontfamily",
    "\\usefont",
    "\\hypersetup",
    "\\geometry",
    "\\captionsetup",
    "\\lstset",
    "\\tikzset",
    "\\pgfplotsset",
    "\\sisetup",
    "\\usetikzlibrary",
    "\\usetheme",
    "\\usecolortheme",
    "\\usefonttheme",
    "\\useinnertheme",
    "\\useoutertheme",
    "\\setbeamertemplate",
    "\\setbeamercolor",
    "\\setbeamerfont",
    "\\rowcolor",
    "\\cellcolor",
    "\\columncolor",
    "\\theoremstyle",
    "\\graphicspath",
    "\\includepdf",
    "\\lstinputlisting",
    "\\inputminted",
    "\\printbibliography",
    "\\bibliographystyle",
    "\\ensuremath",
];

/// Environments whose body is not prose.
const SKIP_ENVIRONMENTS: &[&str] = &[
    "tikzpicture",
    "pgfpicture",
    "circuitikz",
    "axis",
    "forest",
    "filecontents",
    "filecontents*",
    "thebibliography",
    "otherlanguage",
    "otherlanguage*",
    "Verbatim",
    "BVerbatim",
    "LVerbatim",
    "alltt",
    "spverbatim",
];

/// Accent and special-letter commands: a word glued to one (`perch\'e`) is
/// only part of a word, so it is not checked.
const LETTER_COMMANDS: &[&str] = &[
    "\\'", "\\`", "\\^", "\\\"", "\\~", "\\=", "\\.", "\\u", "\\v", "\\H", "\\c", "\\d", "\\b",
    "\\r", "\\t", "\\k", "\\i", "\\j", "\\o", "\\O", "\\l", "\\L", "\\aa", "\\AA", "\\ae", "\\AE",
    "\\oe", "\\OE", "\\ss", "\\-",
];

/// Byte ranges of the words to check, in document order.
pub fn words(tree: &Tree, text: &str) -> Vec<Range<usize>> {
    collect(tree, text, worth_checking)
}

/// Words and formulas of a document, as texcount reports them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    /// Words of the text (not headings nor captions).
    pub text: usize,
    /// Words of part, chapter and section titles.
    pub headers: usize,
    /// Words of figure and table captions.
    pub captions: usize,
    pub inline_math: usize,
    pub display_math: usize,
}

impl Counts {
    /// Every word, headings and captions included.
    pub fn words(&self) -> usize {
        self.text + self.headers + self.captions
    }

    pub fn add(&mut self, other: &Counts) {
        self.text += other.text;
        self.headers += other.headers;
        self.captions += other.captions;
        self.inline_math += other.inline_math;
        self.display_math += other.display_math;
    }
}

/// Count the words and formulas of the document's prose (the preamble only
/// counts for its `\title`).
pub fn count(tree: &Tree, text: &str) -> Counts {
    let mut counts = Counts::default();
    let root = tree.root_node();
    for r in collect(tree, text, |_| true) {
        let mut node = root.descendant_for_byte_range(r.start, r.end);
        let mut place = None;
        while let Some(n) = node {
            match n.kind() {
                "part" | "chapter" | "section" | "subsection" | "subsubsection" | "paragraph"
                | "subparagraph" | "title_declaration" => {
                    // Words after a heading's title belong to its body.
                    let title = n.child_by_field_name("text");
                    if title.is_none_or(|t| t.byte_range().contains(&r.start)) {
                        place = Some(&mut counts.headers);
                        break;
                    }
                }
                "caption" => {
                    place = Some(&mut counts.captions);
                    break;
                }
                _ => {}
            }
            node = n.parent();
        }
        *place.unwrap_or(&mut counts.text) += 1;
    }
    let body = document_start(root, text);
    let mut stack = vec![root];
    while let Some(n) = stack.pop() {
        if n.end_byte() <= body {
            continue;
        }
        match n.kind() {
            "inline_formula" => counts.inline_math += 1,
            "displayed_equation" | "math_environment" => counts.display_math += 1,
            "comment" | "line_comment" | "block_comment" => {}
            _ => {
                let mut cursor = n.walk();
                stack.extend(n.children(&mut cursor));
            }
        }
    }
    counts
}

/// Where the body starts: `\begin{document}`, else the beginning.
fn document_start(root: Node, text: &str) -> usize {
    let mut cursor = root.walk();
    root.children(&mut cursor)
        .find(|c| {
            c.kind() == "generic_environment" && environment_name(*c, text) == Some("document")
        })
        .map_or(0, |d| d.start_byte())
}

/// Byte ranges of the words of the prose that `keep` accepts.
fn collect(tree: &Tree, text: &str, keep: fn(&str) -> bool) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    // Iterative: the tree is as deep as the document's nesting.
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        if node.kind() == "text" {
            text_words(node, text, keep, &mut out, &mut stack);
            continue;
        }
        let env = node.kind() == "generic_environment";
        // With a `document` environment, the preamble is markup but for the title.
        let top = node.kind() == "source_file" && has_document(node, text);
        // Groups right after `\begin{..}` are the environment's arguments.
        let mut args = env;
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            let kind = child.kind();
            if env && kind == "begin" {
                continue;
            }
            if args && matches!(kind, "curly_group" | "brack_group") {
                continue;
            }
            args = false;
            if top && kind == "text" {
                let mut c = child.walk();
                stack.extend(
                    child
                        .children(&mut c)
                        .filter(|n| n.kind() == "title_declaration"),
                );
                continue;
            }
            if top && kind != "title_declaration" && kind != "generic_environment" {
                continue;
            }
            if is_prose(child, text) {
                stack.push(child);
            }
        }
    }
    out.sort_by_key(|r| r.start);
    out
}

/// The words joined by spaces, for language detection.
pub fn prose(text: &str, words: &[Range<usize>]) -> String {
    let mut out = String::new();
    for w in words {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&text[w.clone()]);
    }
    out
}

fn is_prose(node: Node, text: &str) -> bool {
    match node.kind() {
        "text" => true,
        "generic_command" => !command_name(node, text).is_some_and(|n| SKIP_COMMANDS.contains(&n)),
        "generic_environment" => {
            !environment_name(node, text).is_some_and(|n| SKIP_ENVIRONMENTS.contains(&n))
        }
        kind => CONTAINERS.contains(&kind),
    }
}

fn has_document(root: Node, text: &str) -> bool {
    let mut cursor = root.walk();
    root.children(&mut cursor)
        .any(|c| c.kind() == "generic_environment" && environment_name(c, text) == Some("document"))
}

fn command_name<'a>(node: Node, text: &'a str) -> Option<&'a str> {
    let mut cursor = node.walk();
    let name = node
        .children(&mut cursor)
        .find(|c| c.kind() == "command_name")?;
    Some(&text[name.byte_range()])
}

fn environment_name<'a>(node: Node, text: &'a str) -> Option<&'a str> {
    let name = node
        .child_by_field_name("begin")?
        .child_by_field_name("name")?;
    let s = &text[name.byte_range()];
    Some(s.trim_start_matches('{').trim_end_matches('}').trim())
}

fn text_words<'t>(
    node: Node<'t>,
    text: &str,
    keep: fn(&str) -> bool,
    out: &mut Vec<Range<usize>>,
    stack: &mut Vec<Node<'t>>,
) {
    let mut cursor = node.walk();
    let children: Vec<Node> = node.children(&mut cursor).collect();
    for (i, &child) in children.iter().enumerate() {
        if child.kind() != "word" {
            if is_prose(child, text) {
                stack.push(child);
            }
            continue;
        }
        let prev = match i {
            0 => node.prev_sibling(),
            _ => Some(children[i - 1]),
        };
        let next = children.get(i + 1).copied();
        // `\verb|code|` is not parsed as verbatim: its delimited argument
        // shows up as the next word.
        if prev.is_some_and(|p| {
            p.kind() == "generic_command"
                && command_name(p, text).is_some_and(|n| n.starts_with("\\verb"))
        }) {
            continue;
        }
        let glued = |n: Option<Node>, at: usize| {
            n.is_some_and(|n| {
                (n.end_byte() == at || n.start_byte() == at)
                    && n.kind() == "generic_command"
                    && command_name(n, text).is_some_and(|c| LETTER_COMMANDS.contains(&c))
            })
        };
        let range = child.byte_range();
        let glued_start = glued(prev, range.start);
        let glued_end = glued(next, range.end);
        tokens(text, range, glued_start, glued_end, keep, out);
    }
}

/// Split a `word` node (which may carry punctuation, `don't.`) into words.
fn tokens(
    text: &str,
    range: Range<usize>,
    glued_start: bool,
    glued_end: bool,
    keep: fn(&str) -> bool,
    out: &mut Vec<Range<usize>>,
) {
    let s = &text[range.clone()];
    let mut found: Vec<Range<usize>> = Vec::new();
    let mut start: Option<usize> = None;
    let mut chars = s.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        let inner_apostrophe = matches!(c, '\'' | '\u{2019}')
            && start.is_some()
            && chars.peek().is_some_and(|&(_, n)| is_word_char(n));
        if is_word_char(c) || inner_apostrophe {
            start.get_or_insert(i);
        } else if let Some(st) = start.take() {
            found.push(st..i);
        }
    }
    if let Some(st) = start {
        found.push(st..s.len());
    }
    let n = found.len();
    for (k, r) in found.into_iter().enumerate() {
        if (k == 0 && glued_start && r.start == 0) || (k + 1 == n && glued_end && r.end == s.len())
        {
            continue;
        }
        if keep(&s[r.clone()]) {
            out.push(range.start + r.start..range.start + r.end);
        }
    }
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric()
        // Combining marks (Latin diacritics, Hebrew points, Arabic harakat,
        // Indic signs) and the joiners Persian writes inside words.
        || matches!(c,
            '\u{0300}'..='\u{036F}'
            | '\u{0483}'..='\u{0489}'
            | '\u{0591}'..='\u{05C7}'
            | '\u{064B}'..='\u{065F}'
            | '\u{0670}'
            | '\u{0900}'..='\u{0963}'
            | '\u{0966}'..='\u{0DFF}'
            | '\u{200C}'
            | '\u{200D}')
}

/// Skip what spell checkers usually skip: numbers, single letters, acronyms
/// and mixed-case identifiers.
fn worth_checking(word: &str) -> bool {
    let mut chars = word.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if chars.next().is_none() || word.chars().any(|c| c.is_numeric()) {
        return false;
    }
    let upper_after_first = word.chars().skip(1).filter(|c| c.is_uppercase()).count();
    !(upper_after_first > 0 && (first.is_uppercase() || first.is_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_words_and_formulas() {
        let doc = "\\documentclass{article}\n\\usepackage{amsmath}\n\\title{A Title}\n\
                   \\begin{document}\n\\section{Two words}\nOne 2 three, $x$ and\n\
                   \\begin{equation}y\\end{equation}\n\\[z\\]\n\
                   \\begin{figure}\\caption{A caption here}\\end{figure}\n\
                   % not counted\n\\end{document}\n";
        let tree = tex_syntax::parse(doc, None).unwrap();
        let c = count(&tree, doc);
        assert_eq!(
            c,
            Counts {
                text: 4,
                headers: 4,
                captions: 3,
                inline_math: 1,
                display_math: 2,
            }
        );
        assert_eq!(c.words(), 11);
    }

    fn check(doc: &str) -> Vec<&str> {
        let tree = tex_syntax::parse(doc, None).unwrap();
        words(&tree, doc).into_iter().map(|r| &doc[r]).collect()
    }

    #[test]
    fn keeps_prose_and_skips_markup() {
        let doc = r#"\documentclass[11pt]{article}
\usepackage[main=italian,english]{babel}
\begin{document}
\section[Breve]{Introduzione}
Questo è un testo, con \textbf{grassetto} e \emph{enfasi}~\cite{knuth} vedi \ref{sec:x}.
Formula $a+b$ e \[ x = y \] e \url{http://foo.bar}.
% un commento
\begin{figure}[htbp]
\caption{Una didascalia}\label{fig:a}
\end{figure}
\footnote{nota a piè} \href{http://x.y}{testo link}
\begin{verbatim}
codice
\end{verbatim}
\begin{tabular}{|c|l|} Cella & \multicolumn{2}{c}{Unita} \end{tabular}
\begin{align} x &= \text{dove} \end{align}
\texttt{mono} \hspace{2cm} \verb|code| \author{Mario Rossi} \title{Il titolo}
\end{document}
"#;
        assert_eq!(
            check(doc),
            vec![
                "Breve",
                "Introduzione",
                "Questo",
                "un",
                "testo",
                "con",
                "grassetto",
                "enfasi",
                "vedi",
                "Formula",
                "Una",
                "didascalia",
                "nota",
                "piè",
                "testo",
                "link",
                "Cella",
                "Unita",
                "Il",
                "titolo",
            ]
        );
    }

    #[test]
    fn skips_the_preamble_but_not_the_title() {
        let doc = "\\documentclass{article}\n\\foo{preambolo} libero\n\\title{Titolo}\n\
                   \\begin{document}\nCorpo\n\\end{document}\nDopo";
        assert_eq!(check(doc), vec!["Titolo", "Corpo"]);
        // A file without `document` (an \input chapter) is all body.
        assert_eq!(check("\\foo{capitolo} libero"), vec!["capitolo", "libero"]);
    }

    #[test]
    fn tokenizes_apostrophes_quotes_and_dashes() {
        assert_eq!(
            check("``Virgolette'' l'anno don't. l’anno Ciao--mondo e-mail"),
            vec![
                "Virgolette",
                "l'anno",
                "don't",
                "l’anno",
                "Ciao",
                "mondo",
                "mail"
            ]
        );
    }

    #[test]
    fn skips_numbers_acronyms_and_mixed_case() {
        assert_eq!(
            check("NASA usa 11pt e IPv6 con iPhone ma LaTeX Parola"),
            vec!["usa", "con", "ma", "Parola"]
        );
    }

    #[test]
    fn skips_words_glued_to_accents() {
        assert_eq!(check(r#"perch\'e citt\`a na\"ive bene"#), vec!["bene"]);
    }

    #[test]
    fn skips_non_prose_environments_and_commands() {
        let doc = r"\begin{tikzpicture}\node at (0,0) {Nodo};\end{tikzpicture}
\begin{otherlanguage}{english}Text\end{otherlanguage}
\foreignlanguage{english}{word} \setmainlanguage{italian} Fine";
        assert_eq!(check(doc), vec!["Fine"]);
    }

    #[test]
    fn handles_non_latin_scripts() {
        assert_eq!(check("Привет мир"), vec!["Привет", "мир"]);
        assert_eq!(check("नमस्ते दुनिया"), vec!["नमस्ते", "दुनिया"]);
    }

    #[test]
    fn deep_nesting_does_not_overflow() {
        let depth = 100_000;
        let doc = format!("{}parola{}", "{".repeat(depth), "}".repeat(depth));
        assert_eq!(check(&doc), vec!["parola"]);
    }

    #[test]
    fn joins_prose() {
        let doc = "Uno \\textbf{due} tre";
        let tree = tex_syntax::parse(doc, None).unwrap();
        assert_eq!(prose(doc, &words(&tree, doc)), "Uno due tre");
    }
}
