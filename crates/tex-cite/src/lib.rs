//! Bibliography entries from the web: BibTeX for a DOI (doi.org content
//! negotiation), for an arXiv identifier (arXiv's API), or for the papers
//! whose title or authors match a search (Crossref).
//!
//! Nothing here runs on its own: every request answers an explicit action.

use std::collections::HashSet;
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow};
use serde::Deserialize;
use tex_project::bib;

const TIMEOUT: Duration = Duration::from_secs(20);
/// Crossref asks clients to say who they are.
const USER_AGENT: &str = concat!(
    "TeXoria/",
    env!("CARGO_PKG_VERSION"),
    " (https://github.com/Herbrant/TeXoria)"
);
/// Search results shown, at most.
const ROWS: usize = 10;
/// Larger answers are not an entry.
const MAX_BODY: u64 = 2 << 20;

/// What the user typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lookup {
    Doi(String),
    Arxiv(String),
    Search(String),
}

/// Tell a DOI (or its URL), an arXiv identifier (or its URL) and anything else.
pub fn classify(input: &str) -> Lookup {
    let s = input.trim();
    let lower = s.to_ascii_lowercase();
    for prefix in [
        "https://doi.org/",
        "http://doi.org/",
        "https://dx.doi.org/",
        "doi.org/",
        "doi:",
    ] {
        if lower.starts_with(prefix) {
            let doi = s[prefix.len()..].trim();
            // Only the prefix so far: nothing to look up yet.
            if doi.is_empty() {
                return Lookup::Search(s.to_string());
            }
            return Lookup::Doi(doi.to_string());
        }
    }
    if tex_project::check::valid_doi(s) {
        return Lookup::Doi(s.to_string());
    }
    let arxiv = [
        "https://arxiv.org/abs/",
        "http://arxiv.org/abs/",
        "https://arxiv.org/pdf/",
        "arxiv.org/abs/",
        "arxiv:",
    ]
    .iter()
    .find_map(|p| lower.starts_with(p).then(|| &s[p.len()..]))
    .unwrap_or(s)
    .trim_end_matches(".pdf");
    if is_arxiv_id(arxiv) {
        return Lookup::Arxiv(arxiv.to_string());
    }
    Lookup::Search(s.to_string())
}

/// `2301.01234`, `2301.01234v2`, or the old style `hep-th/9901001` and `math.GT/0309136v1`.
fn is_arxiv_id(s: &str) -> bool {
    let id = s.rsplit_once('v').map_or(s, |(id, version)| {
        if version.chars().all(|c| c.is_ascii_digit()) && !version.is_empty() {
            id
        } else {
            s
        }
    });
    let digits = |s: &str| s.chars().all(|c| c.is_ascii_digit());
    if let Some((archive, num)) = id.split_once('/') {
        // An archive, and for some of them a subject class: `math.GT`, `cs.AI`.
        let (archive, class) = archive.split_once('.').unwrap_or((archive, "A"));
        return !archive.is_empty()
            && archive.chars().all(|c| c.is_ascii_lowercase() || c == '-')
            && !class.is_empty()
            && class.chars().all(|c| c.is_ascii_uppercase())
            && num.len() == 7
            && digits(num);
    }
    if let Some((yymm, num)) = id.split_once('.') {
        return yymm.len() == 4 && digits(yymm) && (4..=5).contains(&num.len()) && digits(num);
    }
    false
}

/// A paper found by a search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub title: String,
    pub authors: Vec<String>,
    pub year: Option<String>,
    pub venue: Option<String>,
    pub doi: String,
}

pub struct Client {
    agent: ureq::Agent,
    doi_base: String,
    arxiv_base: String,
    crossref_base: String,
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

impl Client {
    pub fn new() -> Self {
        Self::with_bases(
            "https://doi.org",
            "https://export.arxiv.org",
            "https://api.crossref.org",
        )
    }

    /// A client that asks other servers than doi.org, arXiv's API and Crossref (each a scheme
    /// and host, without a trailing slash): a mirror, or a local server in tests.
    pub fn with_bases(doi: &str, arxiv: &str, crossref: &str) -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(TIMEOUT))
            .user_agent(USER_AGENT)
            .build()
            .into();
        let base = |s: &str| s.trim_end_matches('/').to_string();
        Self {
            agent,
            doi_base: base(doi),
            arxiv_base: base(arxiv),
            crossref_base: base(crossref),
        }
    }

    /// The BibTeX of a DOI, from its registration agency.
    pub fn doi(&self, doi: &str) -> Result<String> {
        let url = format!("{}/{}", self.doi_base, encode_path(doi.trim()));
        let text = self
            .agent
            .get(&url)
            .header("Accept", "application/x-bibtex; charset=utf-8")
            .call()
            .map_err(|e| match e {
                ureq::Error::StatusCode(404) => anyhow!("No DOI {doi}"),
                e => anyhow!(e).context(format!("looking up {doi}")),
            })?
            .body_mut()
            .with_config()
            .limit(MAX_BODY)
            .read_to_string()?;
        if !text.trim_start().starts_with('@') {
            return Err(anyhow!("doi.org has no BibTeX for {doi}"));
        }
        Ok(text)
    }

    /// The BibTeX of an arXiv preprint (its published DOI's if it has one).
    pub fn arxiv(&self, id: &str) -> Result<String> {
        let url = format!("{}/api/query?id_list={}", self.arxiv_base, encode_query(id));
        let atom = self
            .agent
            .get(&url)
            .call()
            .with_context(|| format!("looking up arXiv:{id}"))?
            .body_mut()
            .with_config()
            .limit(MAX_BODY)
            .read_to_string()?;
        let paper = parse_arxiv(&atom).ok_or_else(|| anyhow!("No arXiv preprint {id}"))?;
        if let Some(doi) = &paper.doi
            && let Ok(bibtex) = self.doi(doi)
        {
            return Ok(bibtex);
        }
        Ok(paper.bibtex())
    }

    /// Papers whose metadata match `query` (title, authors, year…).
    pub fn search(&self, query: &str) -> Result<Vec<Candidate>> {
        let url = format!(
            "{}/works?query.bibliographic={}&rows={ROWS}&select=DOI,title,author,issued,container-title",
            self.crossref_base,
            encode_query(query)
        );
        let response: CrossrefResponse = self
            .agent
            .get(&url)
            .call()
            .context("searching Crossref")?
            .body_mut()
            .with_config()
            .limit(MAX_BODY)
            .read_json()
            .context("reading Crossref's answer")?;
        Ok(response
            .message
            .items
            .into_iter()
            .filter_map(Work::candidate)
            .collect())
    }
}

#[derive(Deserialize)]
struct CrossrefResponse {
    message: CrossrefMessage,
}

#[derive(Deserialize)]
struct CrossrefMessage {
    #[serde(default)]
    items: Vec<Work>,
}

#[derive(Deserialize)]
struct Work {
    #[serde(rename = "DOI")]
    doi: String,
    #[serde(default)]
    title: Vec<String>,
    #[serde(default)]
    author: Vec<Author>,
    issued: Option<Issued>,
    #[serde(rename = "container-title", default)]
    container: Vec<String>,
}

#[derive(Deserialize)]
struct Author {
    given: Option<String>,
    family: Option<String>,
    name: Option<String>,
}

#[derive(Deserialize)]
struct Issued {
    #[serde(rename = "date-parts", default)]
    parts: Vec<Vec<Option<i64>>>,
}

impl Work {
    fn candidate(self) -> Option<Candidate> {
        let title = self.title.into_iter().next()?;
        Some(Candidate {
            title: collapse(&title),
            authors: self
                .author
                .into_iter()
                .filter_map(|a| match (a.given, a.family, a.name) {
                    (Some(g), Some(f), _) => Some(format!("{g} {f}")),
                    (None, Some(f), _) => Some(f),
                    (_, _, Some(n)) => Some(n),
                    _ => None,
                })
                .collect(),
            year: self
                .issued
                .and_then(|i| i.parts.first()?.first().copied().flatten())
                .map(|y| y.to_string()),
            venue: self.container.into_iter().next().map(|v| collapse(&v)),
            doi: self.doi,
        })
    }
}

/// What arXiv's Atom feed says about a preprint.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Preprint {
    id: String,
    title: String,
    authors: Vec<String>,
    year: String,
    category: Option<String>,
    doi: Option<String>,
}

impl Preprint {
    fn bibtex(&self) -> String {
        let mut out = format!("@misc{{arxiv{},\n", self.id.replace(['/', '.'], ""));
        let authors: Vec<String> = self.authors.iter().map(|a| tex_value(a)).collect();
        out.push_str(&format!("  title = {{{}}},\n", tex_value(&self.title)));
        out.push_str(&format!("  author = {{{}}},\n", authors.join(" and ")));
        out.push_str(&format!("  year = {{{}}},\n", self.year));
        out.push_str(&format!("  eprint = {{{}}},\n", self.id));
        out.push_str("  archivePrefix = {arXiv},\n");
        if let Some(c) = &self.category {
            out.push_str(&format!("  primaryClass = {{{c}}},\n"));
        }
        out.push_str(&format!("  url = {{https://arxiv.org/abs/{}}},\n", self.id));
        out.push('}');
        out
    }
}

/// arXiv's text as a BibTeX value: `%`, `&` and `#` escaped (math and other
/// commands stay as they are), and braces that do not pair up dropped, since
/// they would end the entry early or never.
fn tex_value(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    // Where each `{` not closed yet is in `out`, and how long it is (`\{` too).
    let mut open: Vec<(usize, usize)> = Vec::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some(b @ ('{' | '}')) => brace(&mut out, &mut open, b, "\\"),
                Some(next) => {
                    out.push('\\');
                    out.push(next);
                }
                // It would escape the value's closing brace.
                None => {}
            },
            '{' | '}' => brace(&mut out, &mut open, c, ""),
            '%' | '&' | '#' => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    // Unclosed ones, from the last.
    for (at, len) in open.into_iter().rev() {
        out.replace_range(at..at + len, "");
    }
    out
}

/// Push a brace (`prefix` is `\` for an escaped one), unless it closes nothing.
fn brace(out: &mut String, open: &mut Vec<(usize, usize)>, b: char, prefix: &str) {
    if b == '{' {
        open.push((out.len(), prefix.len() + 1));
    } else if open.pop().is_none() {
        return;
    }
    out.push_str(prefix);
    out.push(b);
}

/// The first entry of an arXiv Atom feed (there is one per requested id).
fn parse_arxiv(atom: &str) -> Option<Preprint> {
    let entry = between(atom, "<entry>", "</entry>")?;
    let id_url = between(entry, "<id>", "</id>")?.trim();
    let id = id_url.rsplit_once("/abs/")?.1.to_string();
    let title = collapse(&unescape(between(entry, "<title>", "</title>")?));
    let authors = entry
        .split("<author>")
        .skip(1)
        .filter_map(|a| between(a, "<name>", "</name>"))
        .map(|n| collapse(&unescape(n)))
        .collect::<Vec<_>>();
    let year = between(entry, "<published>", "</published>")?
        .trim()
        .chars()
        .take(4)
        .collect();
    let category = entry
        .split_once("<arxiv:primary_category")
        .and_then(|(_, rest)| between(rest, "term=\"", "\""))
        .map(str::to_string);
    let doi = between(entry, "<arxiv:doi>", "</arxiv:doi>").map(|d| d.trim().to_string());
    (!title.is_empty()).then_some(Preprint {
        id,
        title,
        authors,
        year,
        category,
        doi,
    })
}

fn between<'a>(s: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let from = s.find(start)? + start.len();
    let to = s[from..].find(end)? + from;
    Some(&s[from..to])
}

/// XML's predefined entities and character references, decoded in one pass (`&amp;lt;` is
/// `&lt;`). Anything else, or a reference that is not a character, stays as it is.
fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        // Entities are short: never look far for their `;`.
        let decoded = rest
            .bytes()
            .take(12)
            .position(|b| b == b';')
            .and_then(|end| Some((entity(&rest[1..end])?, end)));
        match decoded {
            Some((c, end)) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// The character of an entity's name (between `&` and `;`).
fn entity(name: &str) -> Option<char> {
    let c = match name {
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "amp" => '&',
        _ => {
            let num = name.strip_prefix('#')?;
            let (digits, radix) = match num.strip_prefix(['x', 'X']) {
                Some(hex) => (hex, 16),
                None => (num, 10),
            };
            if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
                return None;
            }
            return char::from_u32(u32::from_str_radix(digits, radix).ok()?);
        }
    };
    Some(c)
}

fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn encode_query(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// A DOI in a URL path: its `/` stay, everything unusual is escaped.
fn encode_path(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'.'
            | b'_'
            | b'~'
            | b'/'
            | b'('
            | b')'
            | b';'
            | b':' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// The entry of `bibtex` written one field per line, under `key`. `None` if
/// it would corrupt the file it goes into: not closed, without a type, with a
/// field name BibTeX cannot read or a value whose braces do not pair up.
pub fn normalize(bibtex: &str, key: &str) -> Option<String> {
    let entry = bib::parse(bibtex).into_iter().next()?;
    if entry.truncated || entry.kind.is_empty() {
        return None;
    }
    // The entry's closing delimiter.
    let body_end = entry.range.end - 1;
    let mut out = format!("@{}{{{key},\n", entry.kind);
    for f in &entry.fields {
        let value = value_text(bibtex, f.range.clone(), body_end)?;
        if !identifier(&f.name) || !balanced(&value) {
            return None;
        }
        out.push_str(&format!("  {} = {value},\n", f.name));
    }
    out.push('}');
    Some(out)
}

/// A field's value to write: a number or a macro name (`jan`) bare, a
/// concatenation (`"a" # b`) as it is if well formed, anything else in braces.
fn value_text(bibtex: &str, range: std::ops::Range<usize>, body_end: usize) -> Option<String> {
    let b = bibtex.as_bytes();
    let raw = collapse(&bibtex[range.clone()]);
    let opened = range.start > 0 && matches!(b[range.start - 1], b'{' | b'"');
    // A `}` that ends the entry does not end a bare value.
    let closed = range.end < body_end && matches!(b[range.end], b'}' | b'"');
    let expr = &bibtex[range.start - opened as usize..range.end + closed as usize];
    if let Some(pieces) = concatenation(expr) {
        return pieces.iter().all(|p| piece(p)).then(|| collapse(expr));
    }
    if number(&raw) || (!opened && identifier(&raw)) {
        Some(raw)
    } else {
        // Its braces must pair up inside the value's own.
        balanced(&raw).then(|| format!("{{{raw}}}"))
    }
}

fn number(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_digit())
}

/// One piece of a concatenation: `{...}`, `"..."`, a number or a macro name.
fn piece(p: &str) -> bool {
    let p = p.trim();
    let delimited = |open: char, close: char| {
        p.len() >= 2
            && p.starts_with(open)
            && p.ends_with(close)
            && balanced(&p[1..p.len() - 1])
            && concatenation(p).is_none()
    };
    delimited('{', '}') || delimited('"', '"') || number(p) || identifier(p)
}

/// A field or macro name BibTeX reads: a letter, then letters, digits and `_-:.+/`.
fn identifier(s: &str) -> bool {
    s.starts_with(|c: char| c.is_ascii_alphabetic())
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-:.+/".contains(c))
}

/// The pieces of `expr` if it joins several with `#` (outside braces and quotes).
fn concatenation(expr: &str) -> Option<Vec<&str>> {
    let (mut depth, mut quoted) = (0usize, false);
    let mut pieces = Vec::new();
    let mut start = 0;
    for (i, c) in expr.bytes().enumerate() {
        match c {
            b'{' => depth += 1,
            b'}' => depth = depth.saturating_sub(1),
            b'"' if depth == 0 => quoted = !quoted,
            b'#' if depth == 0 && !quoted => {
                pieces.push(&expr[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    (!pieces.is_empty()).then(|| {
        pieces.push(&expr[start..]);
        pieces
    })
}

/// Whether every `}` closes an earlier `{` and every `{` is closed.
fn balanced(s: &str) -> bool {
    let mut depth = 0usize;
    for c in s.bytes() {
        match c {
            b'{' => depth += 1,
            b'}' if depth == 0 => return false,
            b'}' => depth -= 1,
            _ => {}
        }
    }
    depth == 0
}

/// A key like `knuth1984texbook` for the entry of `bibtex`, not in `taken`.
pub fn make_key(bibtex: &str, taken: &HashSet<String>) -> String {
    let entry = bib::parse(bibtex).into_iter().next();
    let field = |name: &str| entry.as_ref().and_then(|e| e.field(name)).unwrap_or("");
    let author = field("author");
    let first = author.split(" and ").next().unwrap_or("").trim();
    let last = match first.split_once(',') {
        Some((last, _)) => last,
        None => first.rsplit(' ').next().unwrap_or(first),
    };
    let year: String = field("year")
        .chars()
        .chain(field("date").chars())
        .filter(char::is_ascii_digit)
        .take(4)
        .collect();
    const SMALL: &[&str] = &[
        "a", "an", "the", "on", "of", "in", "for", "and", "to", "with",
    ];
    let word = field("title")
        .split(|c: char| !c.is_alphanumeric())
        .find(|w| w.len() > 2 && !SMALL.contains(&w.to_lowercase().as_str()))
        .unwrap_or("");
    let base: String = format!("{}{year}{}", ascii(last), ascii(word));
    let base = if base.is_empty() {
        "ref".to_string()
    } else {
        base
    };
    if !taken.contains(&base) {
        return base;
    }
    // Then numbers: `taken` is finite, so one of them is free.
    ('b'..='z')
        .map(|c| format!("{base}{c}"))
        .chain((2..).map(|n| format!("{base}{n}")))
        .find(|k| !taken.contains(k))
        .expect("a free key")
}

/// Lower-case ASCII letters and digits, accents dropped (`Gödel` → `godel`).
fn ascii(s: &str) -> String {
    s.chars()
        .filter_map(|c| {
            let c = match c {
                'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'À' | 'Á' | 'Â' | 'Ã' | 'Ä' | 'Å' => {
                    'a'
                }
                'è' | 'é' | 'ê' | 'ë' | 'È' | 'É' | 'Ê' | 'Ë' => 'e',
                'ì' | 'í' | 'î' | 'ï' | 'Ì' | 'Í' | 'Î' | 'Ï' => 'i',
                'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'Ò' | 'Ó' | 'Ô' | 'Õ' | 'Ö' | 'Ø' => {
                    'o'
                }
                'ù' | 'ú' | 'û' | 'ü' | 'Ù' | 'Ú' | 'Û' | 'Ü' => 'u',
                'ç' | 'Ç' | 'ć' | 'č' => 'c',
                'ñ' | 'Ñ' | 'ń' => 'n',
                'ß' => 's',
                'š' | 'Š' | 'ś' => 's',
                'ž' | 'Ž' | 'ź' | 'ż' => 'z',
                'ł' | 'Ł' => 'l',
                c => c,
            };
            c.is_ascii_alphanumeric().then(|| c.to_ascii_lowercase())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_input() {
        assert_eq!(
            classify(" 10.1145/3368089 "),
            Lookup::Doi("10.1145/3368089".into())
        );
        assert_eq!(
            classify("https://doi.org/10.1000/xyz"),
            Lookup::Doi("10.1000/xyz".into())
        );
        assert_eq!(
            classify("arXiv:2301.01234v2"),
            Lookup::Arxiv("2301.01234v2".into())
        );
        assert_eq!(
            classify("https://arxiv.org/abs/hep-th/9901001"),
            Lookup::Arxiv("hep-th/9901001".into())
        );
        assert_eq!(classify("2301.01234"), Lookup::Arxiv("2301.01234".into()));
        assert_eq!(
            classify("attention is all you need"),
            Lookup::Search("attention is all you need".into())
        );
    }

    #[test]
    fn classifies_old_arxiv_identifiers() {
        for id in [
            "math.GT/0309136",
            "math.GT/0309136v1",
            "solv-int/9901001v2",
            "cs.AI/0101001",
            "q-bio/0401001v12",
        ] {
            assert_eq!(classify(id), Lookup::Arxiv(id.into()), "{id}");
            let url = format!("https://arxiv.org/abs/{id}");
            assert_eq!(classify(&url), Lookup::Arxiv(id.into()), "{url}");
        }
        for not in [
            "math./0309136",
            "Math.GT/0309136",
            "math.G1/0309136",
            "math.GT/0309136v",
        ] {
            assert_eq!(classify(not), Lookup::Search(not.into()), "{not}");
        }
    }

    const ATOM: &str = r#"<?xml version="1.0"?><feed><title>query</title>
<entry>
  <id>http://arxiv.org/abs/1706.03762v7</id>
  <published>2017-06-12T17:57:34Z</published>
  <title>Attention Is All
    You Need</title>
  <author><name>Ashish Vaswani</name></author>
  <author><name>Noam Shazeer</name></author>
  <arxiv:primary_category xmlns:arxiv="http://arxiv.org/schemas/atom" term="cs.CL"/>
</entry></feed>"#;

    #[test]
    fn reads_arxiv() {
        let p = parse_arxiv(ATOM).unwrap();
        assert_eq!(p.id, "1706.03762v7");
        assert_eq!(p.title, "Attention Is All You Need");
        assert_eq!(p.authors, vec!["Ashish Vaswani", "Noam Shazeer"]);
        assert_eq!(p.year, "2017");
        assert_eq!(p.category.as_deref(), Some("cs.CL"));
        let bib = p.bibtex();
        let e = &bib::parse(&bib)[0];
        assert_eq!(e.field("eprint"), Some("1706.03762v7"));
        assert_eq!(e.author.as_deref(), Some("Ashish Vaswani and Noam Shazeer"));
        assert!(parse_arxiv("<feed></feed>").is_none());
    }

    #[test]
    fn reads_crossref() {
        let json = r#"{"message":{"items":[{"DOI":"10.1/x","title":["A  Study"],
            "author":[{"given":"Ada","family":"Lovelace"},{"name":"Team"}],
            "issued":{"date-parts":[[1843,1]]},"container-title":["Notes"]},
            {"DOI":"10.1/untitled"}]}}"#;
        let r: CrossrefResponse = serde_json::from_str(json).unwrap();
        let c: Vec<_> = r
            .message
            .items
            .into_iter()
            .filter_map(Work::candidate)
            .collect();
        assert_eq!(
            c,
            vec![Candidate {
                title: "A Study".into(),
                authors: vec!["Ada Lovelace".into(), "Team".into()],
                year: Some("1843".into()),
                venue: Some("Notes".into()),
                doi: "10.1/x".into(),
            }]
        );
    }

    #[test]
    fn normalizes_and_names_entries() {
        let doi = "@article{Knuth_1984, title={The {\\TeX}book}, volume={1}, \
                   author={Knuth, Donald E.}, year={1984}, journal={Computers}}";
        let taken = HashSet::from(["knuth1984texbook".to_string()]);
        let key = make_key(doi, &taken);
        assert_eq!(key, "knuth1984texbookb");
        let entry = normalize(doi, &key).unwrap();
        assert_eq!(
            entry,
            "@article{knuth1984texbookb,\n  title = {The {\\TeX}book},\n  volume = 1,\n  \
             author = {Knuth, Donald E.},\n  year = 1984,\n  journal = {Computers},\n}"
        );
        let e = &bib::parse(&entry)[0];
        assert_eq!(e.title.as_deref(), Some("The \\TeXbook"));
        assert_eq!(
            make_key("@misc{x, author={Kurt Gödel}}", &HashSet::new()),
            "godel"
        );
        assert_eq!(make_key("@misc{x}", &HashSet::new()), "ref");
    }

    #[test]
    fn refuses_entries_that_would_corrupt_a_bib() {
        for bad in [
            // Found by fuzzing: no type, a junk field name, unbalanced braces.
            ",\0@{  \0=@\0\0,\0@{  \0=@{!{!\0",
            "@{k, title={T}}",
            "@misc{k, title={T}",
            "@misc{k, ti tle={T}}",
            "@misc{k, {title}={T}}",
            "@misc{k, title = \"a{b\"}",
            // A value whose first brace closes early, a concatenation of nothing.
            "@misc{{, t = @a(}\\{,}}",
            "@misc{k, t = #\"}",
        ] {
            assert_eq!(normalize(bad, "key"), None, "{bad:?}");
        }
    }

    #[test]
    fn keeps_macros_and_concatenations() {
        let bib = "@misc{k, month = jan, note = \"a\" #  {b} # mar, year = {2000},\n\
                   title=\"T\", howpublished = 12a, empty = {}}";
        let once = normalize(bib, "key").unwrap();
        assert_eq!(
            once,
            "@misc{key,\n  month = jan,\n  note = \"a\" # {b} # mar,\n  year = 2000,\n  \
             title = {T},\n  howpublished = {12a},\n  empty = {},\n}"
        );
        assert_eq!(normalize(&once, "key").as_deref(), Some(once.as_str()));
    }

    #[test]
    fn classifies_prefixed_and_odd_input() {
        let doi = |s: &str| Lookup::Doi(s.into());
        assert_eq!(classify("doi.org/10.1/x"), doi("10.1/x"));
        assert_eq!(classify("DOI: 10.1/X "), doi("10.1/X"));
        assert_eq!(classify("http://doi.org/10.1/x"), doi("10.1/x"));
        assert_eq!(classify("https://dx.doi.org/10.1/x"), doi("10.1/x"));
        assert_eq!(
            classify("https://arxiv.org/pdf/2301.01234v2.pdf"),
            Lookup::Arxiv("2301.01234v2".into())
        );
        assert_eq!(classify("0704.0001"), Lookup::Arxiv("0704.0001".into()));
        assert_eq!(
            classify("hep-th/9901001"),
            Lookup::Arxiv("hep-th/9901001".into())
        );
        for search in [
            "",
            "2301.012",
            "23011.01234",
            "2301.01234v",
            "hep-th/990100",
            "/9901001",
        ] {
            assert_eq!(classify(search), Lookup::Search(search.into()), "{search}");
        }
    }

    #[test]
    fn encodes_urls() {
        assert_eq!(encode_query("a b&c=d/é~"), "a+b%26c%3Dd%2F%C3%A9~");
        assert_eq!(
            encode_path("10.1/a b(c);d:e?f#g"),
            "10.1/a%20b(c);d:e%3Ff%23g"
        );
    }

    #[test]
    fn unescapes_entities_once() {
        assert_eq!(
            unescape("&lt;a&gt; &quot;b&quot; &apos;c&apos;"),
            "<a> \"b\" 'c'"
        );
        assert_eq!(unescape("&amp;lt;"), "&lt;");
        assert_eq!(unescape("AT&amp;T &unknown;"), "AT&T &unknown;");
    }

    #[test]
    fn unescapes_numeric_entities() {
        assert_eq!(unescape("G&#246;del &#x2013; &#X3b1;"), "Gödel – α");
        assert_eq!(unescape("&amp;#38; &#38;lt;"), "&#38; &lt;");
        // Not characters: left as they are.
        assert_eq!(
            unescape("&#; &#x; &#xD800; &#99999999999; &#12"),
            "&#; &#x; &#xD800; &#99999999999; &#12"
        );
    }

    #[test]
    fn reads_odd_arxiv_feeds() {
        let doi = ATOM.replace("</entry>", "<arxiv:doi> 10.1/pub </arxiv:doi></entry>");
        assert_eq!(parse_arxiv(&doi).unwrap().doi.as_deref(), Some("10.1/pub"));
        // An identifier that does not exist: an entry without a title.
        let empty = ATOM.replace("Attention Is All\n    You Need", " ");
        assert!(parse_arxiv(&empty).is_none());
        // arXiv's error feed is not a preprint.
        let error = ATOM.replace("abs/1706.03762v7", "api/errors#incorrect_id_format_for_x");
        assert!(parse_arxiv(&error).is_none());
        let undated = ATOM.replace("<published>2017-06-12T17:57:34Z</published>", "");
        assert!(parse_arxiv(&undated).is_none());
        let p = parse_arxiv(&ATOM.replace("<arxiv:primary_category", "<other")).unwrap();
        assert_eq!(p.category, None);
        assert!(!p.bibtex().contains("primaryClass"));
    }

    #[test]
    fn reads_crossref_authors_and_dates() {
        let work = |json: &str| {
            serde_json::from_str::<Work>(json)
                .unwrap()
                .candidate()
                .unwrap()
        };
        let c = work(
            r#"{"DOI":"10.1/x","title":["T"],"author":[{"family":"Euclid"},{"given":"Only"}],
                "issued":{"date-parts":[]}}"#,
        );
        assert_eq!(c.authors, vec!["Euclid"]);
        assert_eq!(c.year, None);
        assert!(serde_json::from_str::<Work>(r#"{"title":["T"]}"#).is_err());
    }

    #[test]
    fn names_entries_from_their_fields() {
        let key = |bib: &str| make_key(bib, &HashSet::new());
        assert_eq!(
            key(
                "@book{x, author={Lamport, Leslie and Knuth, D.}, title={On the Design}, date={1986-01}}"
            ),
            "lamport1986design"
        );
        assert_eq!(
            key("@book{x, title={Über die Hypothesen}, year={1854}}"),
            "1854uber"
        );
        assert_eq!(key("@book{x, author={Ada}, year={n.d.}}"), "ada");
        let taken: HashSet<_> = std::iter::once("ref".to_string())
            .chain(('b'..='y').map(|c| format!("ref{c}")))
            .collect();
        assert_eq!(make_key("@misc{x}", &taken), "refz");
    }

    #[test]
    fn numbered_keys_are_free_too() {
        let mut taken: HashSet<_> = std::iter::once("ref".to_string())
            .chain(('b'..='z').map(|c| format!("ref{c}")))
            .collect();
        // The count of taken keys, as a suffix, is taken as well.
        taken.insert(format!("ref{}", taken.len() + 1));
        let key = make_key("@misc{x}", &taken);
        assert!(!taken.contains(&key), "{key}");
        assert!(key.starts_with("ref") && key[3..].chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn a_doi_prefix_alone_is_no_doi() {
        for s in ["https://doi.org/", "doi:", " DOI: ", "doi.org/"] {
            assert_eq!(classify(s), Lookup::Search(s.trim().into()), "{s:?}");
        }
    }

    #[test]
    fn preprints_escape_what_tex_would_misread() {
        let p = Preprint {
            id: "2301.01234".into(),
            title: "50% of {x & y} in $O(n_1)$, #1 \\& 2\\% }odd{ \\".into(),
            authors: vec!["A. {B".into(), "C} D".into()],
            year: "2023".into(),
            category: Some("cs.LG".into()),
            doi: None,
        };
        let bib = p.bibtex();
        let e = &bib::parse(&bib)[0];
        assert!(!e.truncated, "{bib}");
        assert_eq!(e.key, "arxiv230101234");
        let names: Vec<_> = e.fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "title",
                "author",
                "year",
                "eprint",
                "archiveprefix",
                "primaryclass",
                "url"
            ]
        );
        assert_eq!(
            e.title.as_deref(),
            Some("50\\% of x \\& y in $O(n_1)$, \\#1 \\& 2\\% odd")
        );
        assert_eq!(e.author.as_deref(), Some("A. B and C D"));
    }

    proptest::proptest! {
        #[test]
        fn classifies_every_doi(d in "10\\.[0-9]{4,5}(\\.[0-9]{1,3})?/[A-Za-z0-9._;()/:<>-]{1,20}") {
            proptest::prop_assert_eq!(classify(&d), Lookup::Doi(d.clone()));
            proptest::prop_assert_eq!(classify(&format!("https://doi.org/{d}")), Lookup::Doi(d.clone()));
            proptest::prop_assert_eq!(classify(&format!(" doi:{d}\t")), Lookup::Doi(d.clone()));
        }

        #[test]
        fn encodes_to_safe_ascii(s in "\\PC*") {
            for (encoded, extra) in [(encode_query(&s), "+"), (encode_path(&s), "/();:")] {
                proptest::prop_assert!(encoded.chars().all(|c| c.is_ascii_alphanumeric()
                    || "-._~%".contains(c) || extra.contains(c)), "{}", encoded);
                proptest::prop_assert_eq!(percent_decode(&encoded.replace('+', "%20")), s.clone());
            }
        }

        #[test]
        fn keys_are_ascii_and_free(
            author in "\\PC{0,20}",
            title in "\\PC{0,30}",
            year in "[0-9]{0,4}",
            taken in proptest::collection::hash_set("[a-z]{0,6}[0-9]{0,4}[a-z]?", 0..40),
        ) {
            let bib = format!("@misc{{x, author={{{}}}, title={{{}}}, year={{{year}}}}}",
                author.replace(['{', '}'], ""), title.replace(['{', '}'], ""));
            let key = make_key(&bib, &taken);
            proptest::prop_assert!(!key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric()));
            proptest::prop_assert!(!taken.contains(&key), "{} is taken", key);
        }

        #[test]
        fn preprints_round_trip(
            title in "[a-z {}%&#\\\\$_]{0,30}",
            authors in proptest::collection::vec("[A-Za-z {}.\\\\]{1,12}", 1..4),
        ) {
            let p = Preprint {
                id: "2301.01234".into(),
                title,
                authors,
                year: "2023".into(),
                category: None,
                doi: None,
            };
            let bib = p.bibtex();
            let entries = bib::parse(&bib);
            proptest::prop_assert_eq!(entries.len(), 1, "{}", bib);
            let e = &entries[0];
            proptest::prop_assert!(!e.truncated, "{}", bib);
            proptest::prop_assert_eq!(e.fields.len(), 6, "{}", bib);
            proptest::prop_assert_eq!(e.field("eprint"), Some("2301.01234"));
        }

        #[test]
        fn normalized_entries_are_balanced_fixed_points(
            pieces in proptest::collection::vec(proptest::sample::select(&[
                "@misc{", "@{", "@a(", "k,", " t = ", "x=", "{", "}", "(", ")", "\"", "#",
                ",", "=", " ", "\n", "\0", "@", "jan", "12", "é", "\\{", "%",
            ][..]), 0..30),
            any_text in "\\PC{0,40}",
        ) {
            for bib in [pieces.concat(), any_text] {
                if let Some(once) = normalize(&bib, "key") {
                    proptest::prop_assert!(balanced(&once), "{:?} -> {:?}", bib, once);
                    let twice = normalize(&once, "key");
                    proptest::prop_assert_eq!(twice.as_ref(), Some(&once), "{:?}", bib);
                }
            }
        }

        #[test]
        fn normalizing_is_idempotent(
            kind in "[a-z]{1,10}",
            fields in proptest::collection::vec(("[a-z]{1,8}", "[A-Za-z0-9 .,:;-]{0,20}", 0..3u8), 0..6),
        ) {
            let mut bib = format!("@{kind}{{orig,\n");
            for (name, value, delim) in &fields {
                let value = match delim {
                    0 => format!("{{{value}}}"),
                    1 => format!("\"{value}\""),
                    _ => value.trim().replace([',', ' '], "").chars().take(1).collect::<String>() + "1",
                };
                bib.push_str(&format!("  {name} = {value},\n"));
            }
            bib.push('}');
            let once = normalize(&bib, "key").unwrap();
            proptest::prop_assert_eq!(normalize(&once, "key").unwrap(), once.clone());
            let entry = &bib::parse(&once)[0];
            proptest::prop_assert_eq!(&entry.key, "key");
            proptest::prop_assert_eq!(entry.fields.len(), fields.len());
        }
    }

    fn percent_decode(s: &str) -> String {
        let mut out = Vec::new();
        let b = s.as_bytes();
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'%' {
                out.push(u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap());
                i += 3;
            } else {
                out.push(b[i]);
                i += 1;
            }
        }
        String::from_utf8(out).unwrap()
    }

    /// Against the real services: `TEXORIA_CITE_ONLINE=1`.
    #[test]
    fn looks_up_online() {
        if std::env::var("TEXORIA_CITE_ONLINE").is_err() {
            return;
        }
        let c = Client::new();
        let bib = c.doi("10.1145/359576.359579").unwrap();
        assert!(bib.contains("@article"), "{bib}");
        let bib = c.arxiv("1706.03762").unwrap();
        assert!(bib.to_lowercase().contains("attention"), "{bib}");
        let found = c.search("Attention is all you need Vaswani").unwrap();
        assert!(!found.is_empty());
    }
}
