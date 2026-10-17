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
    /// Bytes from the `@` to the closing delimiter (or the end of the file).
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
        let close = matching_close(b, j);
        let body_end = close.unwrap_or(b.len());
        i = body_end.min(b.len());
        if matches!(kind.as_str(), "comment" | "preamble" | "string") {
            continue;
        }
        let body = &src[body_start..body_end.min(src.len())];
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
            range: entry_start..close.map_or(src.len(), |c| c + 1),
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
        let (range, next) = match b[j] {
            b'{' => {
                let end = matching_close(b, j).unwrap_or(b.len());
                (j + 1..end, end + 1)
            }
            b'"' => {
                let end = s[j + 1..].find('"').map_or(b.len(), |e| j + 1 + e);
                (j + 1..end.min(s.len()), end + 1)
            }
            _ => {
                let end = s[j..].find(',').map_or(b.len(), |e| j + e);
                let raw = &s[j..end];
                (j..j + raw.trim_end().len(), end)
            }
        };
        out.push((name, clean(&s[range.clone()]), range));
        i = next;
        while i < b.len() && (b[i].is_ascii_whitespace() || b[i] == b',') {
            i += 1;
        }
    }
    out
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
}
