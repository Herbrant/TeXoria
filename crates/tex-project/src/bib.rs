//! Tolerant BibTeX/BibLaTeX reader: just enough for citation completion.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BibEntry {
    pub key: String,
    /// Lower-cased entry type (`article`, `book`...).
    pub kind: String,
    pub title: Option<String>,
    pub author: Option<String>,
    pub year: Option<String>,
}

pub fn parse(src: &str) -> Vec<BibEntry> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(at) = src[i..].find('@') {
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
        let body_end = matching_close(b, j).unwrap_or(b.len());
        i = body_end.min(b.len());
        if matches!(kind.as_str(), "comment" | "preamble" | "string") {
            continue;
        }
        let body = &src[body_start..body_end.min(src.len())];
        let Some((key, fields)) = body.split_once(',') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        let mut entry = BibEntry {
            key: key.to_string(),
            kind,
            title: None,
            author: None,
            year: None,
        };
        for (name, value) in fields_of(fields) {
            match name.as_str() {
                "title" => entry.title = Some(value),
                "author" => entry.author = Some(value),
                "year" => entry.year = Some(value),
                "date" if entry.year.is_none() => {
                    entry.year = Some(value.chars().take(4).collect())
                }
                _ => {}
            }
        }
        out.push(entry);
    }
    out
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

fn fields_of(s: &str) -> Vec<(String, String)> {
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
        let (value, next) = match b[j] {
            b'{' => {
                let end = matching_close(b, j).unwrap_or(b.len());
                (&s[j + 1..end], end + 1)
            }
            b'"' => {
                let end = s[j + 1..].find('"').map_or(b.len(), |e| j + 1 + e);
                (&s[j + 1..end.min(s.len())], end + 1)
            }
            _ => {
                let end = s[j..].find(',').map_or(b.len(), |e| j + e);
                (&s[j..end], end)
            }
        };
        out.push((name, clean(value)));
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
        assert_eq!(e.len(), 2, "{e:#?}");
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
        }
        assert_eq!(
            parse("@misc{k, title = {café").remove(0).title.as_deref(),
            Some("café")
        );
    }
}
