//! Find (and replace) text in every text file of a project folder.

use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use regex::{Regex, RegexBuilder};

use crate::files::{self, FileNode};

/// Matches past this many are not reported (the search is too broad).
pub const MAX_MATCHES: usize = 5000;
/// Characters of context kept around a match in its preview.
const PREVIEW_CONTEXT: usize = 60;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Query {
    pub text: String,
    pub case_sensitive: bool,
    pub whole_word: bool,
    /// `text` is a regular expression (and the replacement may use `$1`).
    pub regex: bool,
}

impl Query {
    /// The query as a regular expression.
    pub fn compile(&self) -> Result<Regex, String> {
        let pattern = match (self.regex, self.whole_word) {
            (true, false) => self.text.clone(),
            // Not glued to a word character on either side (`\b` would require
            // one before a match starting with `\`, as in `\\section`).
            (true, true) => format!(r"\b{{start-half}}(?:{})\b{{end-half}}", self.text),
            (false, false) => regex::escape(&self.text),
            // A word boundary on each side that is a word character: `\section`
            // is a whole word in `x\section{}`, `section` is not.
            (false, true) => {
                let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
                format!(
                    "{}{}{}",
                    if word(self.text.chars().next()) {
                        r"\b"
                    } else {
                        ""
                    },
                    regex::escape(&self.text),
                    if word(self.text.chars().last()) {
                        r"\b"
                    } else {
                        ""
                    },
                )
            }
        };
        RegexBuilder::new(&pattern)
            .case_insensitive(!self.case_sensitive)
            .multi_line(true)
            .size_limit(1 << 20)
            .build()
            .map_err(|e| e.to_string())
    }

    /// `text` with every match replaced by `replacement`, and how many there were.
    /// Empty matches are skipped, as [`find`] does.
    pub fn replace_all(&self, regex: &Regex, text: &str, replacement: &str) -> (String, usize) {
        let mut out = String::with_capacity(text.len());
        let mut count = 0;
        let mut last = 0;
        for caps in regex.captures_iter(text) {
            let m = caps.get(0).expect("group 0 is the whole match");
            if m.is_empty() {
                continue;
            }
            out.push_str(&text[last..m.start()]);
            if self.regex {
                caps.expand(replacement, &mut out);
            } else {
                out.push_str(replacement);
            }
            last = m.end();
            count += 1;
        }
        out.push_str(&text[last..]);
        (out, count)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    /// Bytes of the file.
    pub range: Range<usize>,
    /// 0-based.
    pub line: u32,
    /// The text around the match, on one line.
    pub preview: String,
    /// The match's bytes in `preview`.
    pub highlight: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileMatches {
    pub path: PathBuf,
    pub matches: Vec<Match>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Results {
    pub files: Vec<FileMatches>,
    /// More matches were found than reported.
    pub truncated: bool,
}

impl Results {
    pub fn count(&self) -> usize {
        self.files.iter().map(|f| f.matches.len()).sum()
    }
}

/// The text files of `dir` (as the Files view lists them), in tree order.
pub fn text_files(dir: &Path) -> Vec<PathBuf> {
    fn walk(nodes: Vec<FileNode>, out: &mut Vec<PathBuf>) {
        for n in nodes {
            match n.children {
                Some(children) => walk(children, out),
                None => out.push(n.path),
            }
        }
    }
    let mut out = Vec::new();
    walk(files::scan(dir), &mut out);
    out.retain(|p| files::looks_like_text(p));
    out
}

/// Every match of `regex` in the text files of `dir`. `open` holds the text of
/// files open in the editor (unsaved edits included), read instead of the disk.
pub fn search(
    dir: &Path,
    regex: &Regex,
    open: &HashMap<PathBuf, Arc<str>>,
    cancel: &AtomicBool,
) -> Results {
    let mut results = Results::default();
    let mut total = 0;
    for path in text_files(dir) {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let text: Arc<str> = match open.get(&path) {
            Some(t) => t.clone(),
            None => match crate::read_regular(&path) {
                Some(t) => t.into(),
                None => continue,
            },
        };
        let matches = find(regex, &text, MAX_MATCHES - total);
        if matches.is_empty() {
            continue;
        }
        total += matches.len();
        results.files.push(FileMatches { path, matches });
        if total >= MAX_MATCHES {
            results.truncated = true;
            break;
        }
    }
    results
}

/// The matches of `regex` in `text`, at most `limit`.
pub fn find(regex: &Regex, text: &str, limit: usize) -> Vec<Match> {
    let mut out = Vec::new();
    let mut line = 0u32;
    let mut counted = 0;
    for m in regex.find_iter(text).filter(|m| !m.is_empty()).take(limit) {
        line += text[counted..m.start()].matches('\n').count() as u32;
        counted = m.start();
        let line_start = text[..m.start()].rfind('\n').map_or(0, |i| i + 1);
        let line_end = text[m.start()..]
            .find('\n')
            .map_or(text.len(), |i| m.start() + i);
        // The match's first line, with some context on each side.
        let end = m.end().min(line_end);
        let from = floor_char(
            text,
            m.start().saturating_sub(PREVIEW_CONTEXT).max(line_start),
        );
        let to = ceil_char(text, (end + PREVIEW_CONTEXT).min(line_end));
        let raw = &text[from..to];
        let lead = raw.len() - raw.trim_start().len();
        let preview = raw.trim().to_string();
        // Offsets in `preview`; a match starting in the leading whitespace is
        // highlighted from where the preview starts.
        let in_preview = |i: usize| (i.max(from + lead) - from - lead).min(preview.len());
        let start = in_preview(m.start());
        let highlight = start..in_preview(end).max(start);
        out.push(Match {
            range: m.range(),
            line,
            preview,
            highlight,
        });
    }
    out
}

fn floor_char(s: &str, mut i: usize) -> usize {
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn ceil_char(s: &str, mut i: usize) -> usize {
    while !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(text: &str) -> Query {
        Query {
            text: text.into(),
            ..Default::default()
        }
    }

    #[test]
    fn finds_with_options() {
        let text = "Euler's identity\n  holds: euler, Eulerian.\n";
        let all = find(&query("euler").compile().unwrap(), text, 10);
        assert_eq!(all.len(), 3);
        assert_eq!(all[1].line, 1);
        assert_eq!(all[1].preview, "holds: euler, Eulerian.");
        assert_eq!(&all[1].preview[all[1].highlight.clone()], "euler");

        let case = Query {
            case_sensitive: true,
            ..query("Euler")
        };
        assert_eq!(find(&case.compile().unwrap(), text, 10).len(), 2);
        let word = Query {
            whole_word: true,
            ..query("euler")
        };
        assert_eq!(find(&word.compile().unwrap(), text, 10).len(), 2);
        // Literal unless asked: the dot is no wildcard.
        assert!(find(&query("s.").compile().unwrap(), text, 10).is_empty());
        let re = Query {
            regex: true,
            ..query(r"\bE\w+")
        };
        assert_eq!(find(&re.compile().unwrap(), text, 10).len(), 3);
        assert!(
            Query {
                regex: true,
                ..query("(")
            }
            .compile()
            .is_err()
        );
    }

    #[test]
    fn whole_words_may_start_or_end_with_a_symbol() {
        let text = "\\section{A} \\sections \\subsection{B} x\\section";
        let word = |text: &str, regex| Query {
            whole_word: true,
            regex,
            ..query(text)
        };
        let found = |q: Query| {
            find(&q.compile().unwrap(), text, 10)
                .into_iter()
                .map(|m| m.range)
                .collect::<Vec<_>>()
        };
        assert_eq!(found(word("\\section", false)), vec![0..8, 38..46]);
        assert_eq!(found(word("{A}", false)), vec![8..11]);
        assert_eq!(found(word(r"\\sub\w+", true)), vec![22..33]);
        // Still a word boundary between word characters.
        assert_eq!(found(word("section", false)), vec![1..8, 39..46]);
        assert_eq!(found(word("x", false)), vec![37..38]);
    }

    #[test]
    fn replaces() {
        let literal = query("$x$");
        let re = literal.compile().unwrap();
        assert_eq!(
            literal.replace_all(&re, "a $x$ b $x$", "$1"),
            ("a $1 b $1".to_string(), 2)
        );
        let groups = Query {
            regex: true,
            ..query(r"\\emph\{(\w+)\}")
        };
        let re = groups.compile().unwrap();
        assert_eq!(
            groups.replace_all(&re, r"an \emph{idea}", r"\textit{$1}"),
            (r"an \textit{idea}".to_string(), 1)
        );
    }

    #[test]
    fn replaces_only_the_matches_it_counts() {
        // Empty matches are never listed, so they are not replaced either.
        let star = Query {
            regex: true,
            ..query("a*")
        };
        let re = star.compile().unwrap();
        assert_eq!(find(&re, "baab", 10).len(), 1);
        assert_eq!(star.replace_all(&re, "baab", "X"), ("bXb".to_string(), 1));
        let caret = Query {
            regex: true,
            ..query("^")
        };
        let re = caret.compile().unwrap();
        assert_eq!(
            caret.replace_all(&re, "a\nb", "> "),
            ("a\nb".to_string(), 0)
        );
        // `$0` and named groups expand in regex mode only.
        let named = Query {
            regex: true,
            ..query(r"(?<w>\w+)!")
        };
        let re = named.compile().unwrap();
        assert_eq!(
            named.replace_all(&re, "hi! yo!", "[$w/$0]"),
            ("[hi/hi!] [yo/yo!]".to_string(), 2)
        );
    }

    proptest::proptest! {
        #[test]
        fn replace_all_counts_what_it_replaces(
            text in "[ab\n ]{0,40}",
            pattern in "[ab*+?^$|.]{1,4}",
        ) {
            let q = Query { regex: true, ..query(&pattern) };
            let Ok(re) = q.compile() else { return Ok(()) };
            let (out, n) = q.replace_all(&re, &text, "\u{1}");
            proptest::prop_assert_eq!(n, find(&re, &text, usize::MAX).len());
            proptest::prop_assert_eq!(out.matches('\u{1}').count(), n);
        }
    }

    #[test]
    fn searches_a_folder_with_open_buffers() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("chapters")).unwrap();
        std::fs::create_dir_all(root.join(".texoria/build")).unwrap();
        std::fs::write(root.join("main.tex"), "needle\n").unwrap();
        std::fs::write(root.join("chapters/a.tex"), "no\n").unwrap();
        std::fs::write(root.join(".texoria/build/main.log"), "needle\n").unwrap();
        std::fs::write(root.join("pic.png"), [0u8, 1, 2]).unwrap();
        let open = HashMap::from([(root.join("chapters/a.tex"), Arc::from("a needle"))]);
        let re = query("needle").compile().unwrap();
        let results = search(root, &re, &open, &AtomicBool::new(false));
        let files: Vec<_> = results
            .files
            .iter()
            .map(|f| f.path.strip_prefix(root).unwrap().to_path_buf())
            .collect();
        assert_eq!(
            files,
            vec![PathBuf::from("chapters/a.tex"), PathBuf::from("main.tex")]
        );
        assert_eq!(results.count(), 2);
    }

    #[test]
    fn highlights_matches_starting_in_leading_whitespace() {
        let text = "  holds\n\t\\item x\n";
        let m = &find(&query("  holds").compile().unwrap(), text, 10)[0];
        assert_eq!(m.preview, "holds");
        assert_eq!(&m.preview[m.highlight.clone()], "holds");
        let re = Query {
            regex: true,
            ..query(r"[ \t]+\\item")
        };
        let m = &find(&re.compile().unwrap(), text, 10)[0];
        assert_eq!(m.line, 1);
        assert_eq!(&text[m.range.clone()], "\t\\item");
        assert_eq!(m.preview, "\\item x");
        assert_eq!(&m.preview[m.highlight.clone()], "\\item");
    }

    proptest::proptest! {
        #[test]
        fn highlights_stay_in_the_preview(
            text in "[a \t\né\\\\]{0,200}",
            needle in "[a \té\\\\]{1,4}",
            regex in proptest::bool::ANY,
        ) {
            let q = Query { regex, ..query(&needle) };
            let Ok(re) = q.compile() else { return Ok(()) };
            for m in find(&re, &text, MAX_MATCHES) {
                let h = m.highlight.clone();
                proptest::prop_assert!(h.start <= h.end && h.end <= m.preview.len());
                proptest::prop_assert!(m.preview.is_char_boundary(h.start));
                proptest::prop_assert!(m.preview.is_char_boundary(h.end));
                let found = &text[m.range.clone()];
                proptest::prop_assert_eq!(
                    m.line as usize,
                    text[..m.range.start].matches('\n').count()
                );
                // A one-line match without surrounding whitespace is shown whole.
                if !found.contains('\n') && found.trim() == found {
                    proptest::prop_assert_eq!(&m.preview[h], found);
                }
            }
        }
    }

    #[test]
    fn previews_cut_on_char_boundaries() {
        let text = format!("{}needle{}", "é".repeat(80), "à".repeat(80));
        let m = &find(&query("needle").compile().unwrap(), &text, 10)[0];
        assert_eq!(&m.preview[m.highlight.clone()], "needle");
    }

    #[test]
    fn stops_at_max_matches_and_when_cancelled() {
        let dir = tempfile::tempdir().unwrap();
        let per_file = MAX_MATCHES * 3 / 5;
        for name in ["a.tex", "b.tex", "c.tex"] {
            std::fs::write(dir.path().join(name), "x ".repeat(per_file)).unwrap();
        }
        let re = query("x").compile().unwrap();
        let none = HashMap::new();
        let results = search(dir.path(), &re, &none, &AtomicBool::new(false));
        assert!(results.truncated);
        assert_eq!(results.count(), MAX_MATCHES);
        let counts: Vec<_> = results.files.iter().map(|f| f.matches.len()).collect();
        assert_eq!(counts, [per_file, MAX_MATCHES - per_file]);

        let results = search(dir.path(), &re, &none, &AtomicBool::new(true));
        assert_eq!(results, Results::default());
    }

    #[test]
    fn previews_show_the_first_line_of_a_match() {
        let re = Query {
            text: r"b\s+c".into(),
            regex: true,
            ..Default::default()
        }
        .compile()
        .unwrap();
        let text = "a\r\n  b\r\n c d\r\nb c";
        let m = find(&re, text, MAX_MATCHES);
        assert_eq!(m.len(), 2);
        assert_eq!((m[0].line, m[0].preview.as_str()), (1, "b"));
        assert_eq!(&m[0].preview[m[0].highlight.clone()], "b");
        assert_eq!(&text[m[0].range.clone()], "b\r\n c");
        assert_eq!((m[1].line, m[1].preview.as_str()), (3, "b c"));
        assert_eq!(&m[1].preview[m[1].highlight.clone()], "b c");
        assert!(find(&re, text, 1).len() == 1 && find(&re, text, 0).is_empty());
    }

    #[test]
    fn invalid_or_huge_patterns_are_errors() {
        let regex = |text: &str| Query {
            text: text.into(),
            regex: true,
            ..Default::default()
        };
        assert!(regex("(").compile().is_err());
        assert!(regex(r"\w{1000}{1000}").compile().is_err());
        // Escaped when not a regex.
        assert!(query("(").compile().unwrap().is_match("f(x)"));
    }
}
