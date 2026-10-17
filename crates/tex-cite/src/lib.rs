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
            return Lookup::Doi(s[prefix.len()..].trim().to_string());
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

/// `2301.01234`, `2301.01234v2`, or the old style `hep-th/9901001`.
fn is_arxiv_id(s: &str) -> bool {
    let id = s.split_once('v').map_or(s, |(id, version)| {
        if version.chars().all(|c| c.is_ascii_digit()) && !version.is_empty() {
            id
        } else {
            s
        }
    });
    if let Some((yymm, num)) = id.split_once('.') {
        return yymm.len() == 4
            && yymm.chars().all(|c| c.is_ascii_digit())
            && (4..=5).contains(&num.len())
            && num.chars().all(|c| c.is_ascii_digit());
    }
    if let Some((archive, num)) = id.split_once('/') {
        return !archive.is_empty()
            && archive
                .chars()
                .all(|c| c.is_ascii_lowercase() || c == '-' || c == '.')
            && num.len() == 7
            && num.chars().all(|c| c.is_ascii_digit());
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
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

impl Client {
    pub fn new() -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(TIMEOUT))
            .user_agent(USER_AGENT)
            .build()
            .into();
        Self { agent }
    }

    /// The BibTeX of a DOI, from its registration agency.
    pub fn doi(&self, doi: &str) -> Result<String> {
        let url = format!("https://doi.org/{}", encode_path(doi.trim()));
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
        let url = format!(
            "https://export.arxiv.org/api/query?id_list={}",
            encode_query(id)
        );
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
            "https://api.crossref.org/works?query.bibliographic={}&rows={ROWS}&select=DOI,title,author,issued,container-title",
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
        out.push_str(&format!("  title = {{{}}},\n", self.title));
        out.push_str(&format!("  author = {{{}}},\n", self.authors.join(" and ")));
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

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
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

/// The entry of `bibtex` written one field per line, under `key`.
pub fn normalize(bibtex: &str, key: &str) -> Option<String> {
    let entry = bib::parse(bibtex).into_iter().next()?;
    let mut out = format!("@{}{{{key},\n", entry.kind);
    for f in &entry.fields {
        let raw = collapse(&bibtex[f.range.clone()]);
        let value = if raw.chars().all(|c| c.is_ascii_digit()) {
            raw
        } else {
            format!("{{{raw}}}")
        };
        out.push_str(&format!("  {} = {value},\n", f.name));
    }
    out.push('}');
    Some(out)
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
    ('b'..='z')
        .map(|c| format!("{base}{c}"))
        .find(|k| !taken.contains(k))
        .unwrap_or_else(|| format!("{base}{}", taken.len()))
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
