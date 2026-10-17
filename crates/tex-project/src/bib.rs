//! Tolerant BibTeX/BibLaTeX reader, keeping where each entry and field is.

use std::ops::Range;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BibEntry {
    pub key: String,
    /// Lower-cased entry type (`article`, `book`...).
    pub kind: String,
    pub title: Option<String>,
    pub author: Option<String>,
    pub year: Option<String>,
    /// Every field in order, `name` lower-cased.
    pub fields: Vec<BibField>,
    /// Bytes from the `@` to the closing delimiter (or, if it is missing, to
    /// the end of the line before the next entry or to the end of the file).
    pub range: Range<usize>,
    pub key_range: Range<usize>,
    /// 0-based row of the key.
    pub line: u32,
    /// The closing delimiter is missing.
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BibField {
    pub name: String,
    /// Without delimiters, case-protection braces nor repeated whitespace.
    pub value: String,
    /// Bytes of the raw value, delimiters excluded.
    pub range: Range<usize>,
    /// 0-based row of the value's start.
    pub line: u32,
}

impl BibEntry {
    pub fn field(&self, name: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|f| f.name == name)
            .map(|f| f.value.as_str())
    }
}

pub fn parse(src: &str) -> Vec<BibEntry> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    // Rows are counted as the parse moves forward.
    let mut rows = Rows { src, at: 0, row: 0 };
    while let Some(at) = src[i..].find('@') {
        let entry_start = i + at;
        i += at + 1;
        let kind_end = i + src[i..]
            .find(|c: char| !c.is_ascii_alphanumeric())
            .unwrap_or(src.len() - i);
        let kind = src[i..kind_end].to_ascii_lowercase();
        let mut j = kind_end;
        while j < b.len() && b[j].is_ascii_whitespace() {
            j += 1;
        }
        if j >= b.len() || (b[j] != b'{' && b[j] != b'(') {
            continue;
        }
        let body_start = j + 1;
        let (body_end, close) = match entry_end(b, j) {
            Ok(close) => (close, Some(close)),
            Err(end) => (end, None),
        };
        i = body_end;
        if matches!(kind.as_str(), "comment" | "preamble" | "string") {
            continue;
        }
        let body = &src[body_start..body_end];
        let (raw_key, fields) = body.split_once(',').unwrap_or((body, ""));
        let key = raw_key.trim();
        if key.is_empty() || key.contains(char::is_whitespace) {
            continue;
        }
        let key_start = body_start + (raw_key.len() - raw_key.trim_start().len());
        let fields_start = body_start + raw_key.len() + 1;
        let mut entry = BibEntry {
            key: key.to_string(),
            kind,
            title: None,
            author: None,
            year: None,
            fields: Vec::new(),
            range: entry_start..close.map_or(body_end, |c| c + 1),
            key_range: key_start..key_start + key.len(),
            line: rows.row_of(key_start),
            truncated: close.is_none(),
        };
        for (name, value, range) in fields_of(fields) {
            let range = range.start + fields_start..range.end + fields_start;
            match name.as_str() {
                "title" => entry.title = Some(value.clone()),
                "author" => entry.author = Some(value.clone()),
                "year" => entry.year = Some(value.clone()),
                "date" if entry.year.is_none() => {
                    entry.year = Some(value.chars().take(4).collect())
                }
                _ => {}
            }
            let line = rows.row_of(range.start);
            entry.fields.push(BibField {
                name,
                value,
                range,
                line,
            });
        }
        out.push(entry);
    }
    out
}

/// 0-based rows of increasing offsets, without counting from the start each time.
struct Rows<'a> {
    src: &'a str,
    at: usize,
    row: u32,
}

impl Rows<'_> {
    fn row_of(&mut self, offset: usize) -> u32 {
        if offset >= self.at {
            self.row += self.src.as_bytes()[self.at..offset]
                .iter()
                .filter(|&&b| b == b'\n')
                .count() as u32;
        } else {
            self.row = self.src.as_bytes()[..offset]
                .iter()
                .filter(|&&b| b == b'\n')
                .count() as u32;
        }
        self.at = offset;
        self.row
    }
}

/// Where the entry whose body opens at `open` ends: the index of its closing
/// delimiter, or if there is none, `Err` with the index of the newline before
/// the next entry (an `@type{` at the start of a line) or the text's length.
/// In an `@type(...)` entry, `)` inside braces or quotes does not close it.
fn entry_end(b: &[u8], open: usize) -> Result<usize, usize> {
    let parens = b[open] == b'(';
    let mut depth = 0usize;
    let mut quoted = false;
    for k in open + 1..b.len() {
        match b[k] {
            b'{' => depth += 1,
            b'}' if depth > 0 => depth -= 1,
            b'}' if !parens => return Ok(k),
            b'"' if parens && depth == 0 => quoted = !quoted,
            b')' if parens && depth == 0 && !quoted => return Ok(k),
            b'\n' if starts_entry(&b[k + 1..]) => return Err(k),
            _ => {}
        }
    }
    Err(b.len())
}

/// Whether `line` starts with `@type{` or `@type(`, after blanks.
fn starts_entry(line: &[u8]) -> bool {
    let start = line
        .iter()
        .position(|&c| c != b' ' && c != b'\t')
        .unwrap_or(line.len());
    let Some(rest) = line[start..].strip_prefix(b"@") else {
        return false;
    };
    let kind = rest
        .iter()
        .take_while(|c| c.is_ascii_alphanumeric())
        .count();
    let after = rest[kind..].trim_ascii_start();
    kind > 0 && matches!(after.first(), Some(b'{' | b'('))
}

/// Index of the delimiter closing the one at `open`, honoring nesting.
fn matching_close(b: &[u8], open: usize) -> Option<usize> {
    let (o, c) = if b[open] == b'{' {
        (b'{', b'}')
    } else {
        (b'(', b')')
    };
    let mut depth = 0usize;
    for (k, &ch) in b.iter().enumerate().skip(open) {
        if ch == o {
            depth += 1;
        } else if ch == c {
            depth -= 1;
            if depth == 0 {
                return Some(k);
            }
        }
    }
    None
}

fn fields_of(s: &str) -> Vec<(String, String, Range<usize>)> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let Some(eq) = s[i..].find('=') else { break };
        let name = s[i..i + eq]
            .trim()
            .trim_start_matches(',')
            .trim()
            .to_ascii_lowercase();
        let mut j = i + eq + 1;
        while j < b.len() && b[j].is_ascii_whitespace() {
            j += 1;
        }
        if j >= b.len() {
            break;
        }
        // A value is one or more pieces joined with `#`.
        let mut raw = String::new();
        let mut range: Option<Range<usize>> = None;
        let next = loop {
            let (piece, next) = value_piece(s, j);
            let start = range.map_or(piece.start, |r| r.start);
            range = Some(start..piece.end);
            raw.push_str(&s[piece]);
            let mut k = next;
            while k < b.len() && b[k].is_ascii_whitespace() {
                k += 1;
            }
            if b.get(k) != Some(&b'#') {
                break next;
            }
            j = k + 1;
            while j < b.len() && b[j].is_ascii_whitespace() {
                j += 1;
            }
            if j >= b.len() {
                break b.len();
            }
        };
        out.push((name, clean(&raw), range.unwrap_or(j..j)));
        i = next;
        while i < b.len() && (b[i].is_ascii_whitespace() || b[i] == b',') {
            i += 1;
        }
    }
    out
}

/// The piece of a value starting at `j` (`{...}`, `"..."` or a bare number or
/// macro name): its bytes without delimiters, and where the text after it starts.
fn value_piece(s: &str, j: usize) -> (Range<usize>, usize) {
    let b = s.as_bytes();
    match b[j] {
        b'{' => {
            let end = matching_close(b, j).unwrap_or(b.len());
            (j + 1..end, end + 1)
        }
        b'"' => {
            // A quote inside braces (`{"}`) does not end the value.
            let mut depth = 0usize;
            let end = (j + 1..b.len())
                .find(|&k| match b[k] {
                    b'{' => {
                        depth += 1;
                        false
                    }
                    b'}' => {
                        depth = depth.saturating_sub(1);
                        false
                    }
                    c => c == b'"' && depth == 0,
                })
                .unwrap_or(b.len());
            (j + 1..end, end + 1)
        }
        _ => {
            let end = s[j..].find([',', '#']).map_or(b.len(), |e| j + e);
            (j..j + s[j..end].trim_end().len(), end)
        }
    }
}

/// Drop braces used for case protection and collapse whitespace.
fn clean(v: &str) -> String {
    v.replace(['{', '}'], "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_entries() {
        let src = r#"
% comment
@string{aw = "Addison-Wesley"}
@Book{knuth1984,
  author    = {Donald E. Knuth},
  title     = {The {\TeX}book},
  year      = 1984,
}
@article(lamport,
  title = "{LaTeX}: a document preparation system",
  date = {1986-01-01}
)
@misc{broken
"#;
        let e = parse(src);
        // The unterminated entry is kept so that it can be reported.
        assert_eq!(e.len(), 3, "{e:#?}");
        assert_eq!(e[2].key, "broken");
        assert!(e[2].truncated);
        assert_eq!(e[0].key, "knuth1984");
        assert_eq!(e[0].kind, "book");
        assert_eq!(e[0].title.as_deref(), Some("The \\TeXbook"));
        assert_eq!(e[0].year.as_deref(), Some("1984"));
        assert_eq!(e[1].key, "lamport");
        assert_eq!(
            e[1].title.as_deref(),
            Some("LaTeX: a document preparation system")
        );
        assert_eq!(e[1].year.as_deref(), Some("1986"));
        assert_eq!(&src[e[0].key_range.clone()], "knuth1984");
        assert!(
            src[e[0].range.clone()].starts_with("@Book{") && src[e[0].range.clone()].ends_with('}')
        );
        let title = &e[0].fields[1];
        assert_eq!(title.name, "title");
        assert_eq!(&src[title.range.clone()], "The {\\TeX}book");
        let year = &e[0].fields[2];
        assert_eq!(&src[year.range.clone()], "1984");
        assert_eq!(e[1].field("date"), Some("1986-01-01"));
        assert_eq!(e[0].line, 3);
        assert_eq!(title.line, 5);
        assert_eq!(e[1].line, 8);
        assert!(!e[0].truncated);
    }

    #[test]
    fn truncated_field_does_not_panic() {
        // A .bib saved mid-edit: the last field's brace is never closed.
        for src in [
            "@misc{k, title = {",
            "@misc{k, title = {café",
            "@misc{k, t = \"é",
        ] {
            let e = parse(src);
            assert_eq!(e.len(), 1, "{src}");
            assert_eq!(e[0].key, "k");
            assert!(e[0].truncated);
        }
        assert_eq!(
            parse("@misc{k, title = {café").remove(0).title.as_deref(),
            Some("café")
        );
    }

    fn keys(entries: &[BibEntry]) -> Vec<&str> {
        entries.iter().map(|e| e.key.as_str()).collect()
    }

    #[test]
    fn paren_entries_may_hold_parens_in_braces_and_quotes() {
        let src = "@article(k, title={a)b}, note=\"(c))\", year=2000)\n@misc{m, title={M}}";
        let e = parse(src);
        assert_eq!(keys(&e), ["k", "m"]);
        assert!(!e[0].truncated);
        assert_eq!(e[0].title.as_deref(), Some("a)b"));
        assert_eq!(e[0].field("note"), Some("(c))"));
        assert_eq!(e[0].year.as_deref(), Some("2000"));
        assert!(src[e[0].range.clone()].ends_with("2000)"));
    }

    #[test]
    fn quoted_values_may_hold_quotes_in_braces() {
        let e = parse("@misc{k, title = \"a {\"} b\", year = 2000}");
        assert_eq!(e[0].title.as_deref(), Some("a \" b"));
        assert_eq!(e[0].year.as_deref(), Some("2000"));
    }

    #[test]
    fn reads_concatenated_values() {
        let src = "@misc{k, title = \"a\" # { b} # mac, month = jan # \"~1\", year = 2000}";
        let e = parse(src);
        let names: Vec<_> = e[0].fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["title", "month", "year"]);
        assert_eq!(e[0].title.as_deref(), Some("a bmac"));
        assert_eq!(e[0].field("month"), Some("jan~1"));
        assert_eq!(&src[e[0].fields[1].range.clone()], "jan # \"~1");
        assert_eq!(e[0].year.as_deref(), Some("2000"));
    }

    #[test]
    fn an_unterminated_entry_ends_where_the_next_one_starts() {
        let src = "@misc{a, title={x}\n@book{b, title={y}}\n  @misc{c, title={unclosed\n\t@misc{d}";
        let e = parse(src);
        assert_eq!(keys(&e), ["a", "b", "c", "d"]);
        let truncated: Vec<_> = e.iter().map(|e| e.truncated).collect();
        assert_eq!(truncated, [true, false, true, false]);
        assert_eq!(&src[e[0].range.clone()], "@misc{a, title={x}");
        assert_eq!(e[2].title.as_deref(), Some("unclosed"));
        assert_eq!((e[1].line, e[3].line), (1, 3));
        // An `@` inside a value is no new entry.
        let e = parse("@misc{a, note={write to me@example.org\n @ home}}");
        assert_eq!(keys(&e), ["a"]);
        assert!(!e[0].truncated);
    }

    #[test]
    fn odd_keys_and_stray_at_signs() {
        let src = "contact: me@example.org\n@ misc{x}\n@misc{café_2020, title={é}}\n\
                   @misc{has space, title={x}}\n@misc{, title={x}}\n@misc\n@misc{only}";
        let e = parse(src);
        assert_eq!(keys(&e), ["café_2020", "only"]);
        assert_eq!(&src[e[0].key_range.clone()], "café_2020");
        assert_eq!(e[1].fields, []);
        // Not entries, though BibTeX reads them.
        let e = parse("@comment{a, title={x}}@preamble{\"x\"}@string(s = {x})@misc{k}");
        assert_eq!(keys(&e), ["k"]);
    }

    #[test]
    fn rows_can_go_back() {
        let src = "a\nb\nc\nd";
        let mut rows = Rows { src, at: 0, row: 0 };
        assert_eq!(rows.row_of(6), 3);
        assert_eq!(rows.row_of(2), 1);
        assert_eq!(rows.row_of(4), 2);
        assert_eq!(rows.row_of(0), 0);
    }

    mod props {
        use super::*;
        use proptest::prelude::*;
        use proptest::sample::select;

        const PIECES: &[&str] = &[
            "@misc{",
            "@article(",
            "@string{",
            "@comment{",
            "k",
            "key,",
            " title = ",
            "{",
            "}",
            "(",
            ")",
            "\"",
            "#",
            ",",
            "=",
            " ",
            "\n",
            "\r\n",
            "@",
            "é",
            "year=2000",
            "x",
            "date={2020-01}",
            "\\{",
        ];

        fn bib() -> impl Strategy<Value = String> {
            proptest::collection::vec(select(PIECES), 0..40).prop_map(|v| v.concat())
        }

        fn check(src: &str) -> Result<(), TestCaseError> {
            for e in parse(src) {
                prop_assert_eq!(&src[e.key_range.clone()], e.key.as_str());
                prop_assert!(e.range.start <= e.key_range.start && e.key_range.end <= e.range.end);
                prop_assert!(src.is_char_boundary(e.range.end));
                prop_assert_eq!(
                    e.line as usize,
                    src[..e.key_range.start].matches('\n').count()
                );
                for f in &e.fields {
                    prop_assert!(f.range.start <= f.range.end && f.range.end <= src.len());
                    prop_assert!(src.is_char_boundary(f.range.start));
                    prop_assert!(src.is_char_boundary(f.range.end));
                    prop_assert_eq!(f.line as usize, src[..f.range.start].matches('\n').count());
                }
            }
            Ok(())
        }

        proptest! {
            #[test]
            fn ranges_and_rows_hold_on_bib_like_text(src in bib()) {
                check(&src)?;
            }

            #[test]
            fn never_panics_on_any_text(src in any::<String>()) {
                check(&src)?;
            }

            #[test]
            fn rows_match_a_naive_count(
                src in "[a\n]{0,40}",
                offsets in proptest::collection::vec(0usize..=40, 1..8),
            ) {
                let mut rows = Rows { src: &src, at: 0, row: 0 };
                for o in offsets {
                    let o = o.min(src.len());
                    prop_assert_eq!(rows.row_of(o) as usize, src[..o].matches('\n').count());
                }
            }
        }
    }
}
