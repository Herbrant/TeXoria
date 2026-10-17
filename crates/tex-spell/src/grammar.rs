//! Grammar and style checking with a LanguageTool server.
//!
//! The LaTeX source is sent as LanguageTool's "annotated text": the prose as
//! text, everything else as markup with what it stands for (`$x$` is a
//! noun, `\cite{…}` a number, a blank line a paragraph break), so that the
//! offsets of its findings are the source's. Long documents are split into
//! chunks at paragraph breaks, which are checked (and cached) one by one.

use std::ops::Range;
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow};
use serde::Deserialize;
use serde_json::json;
use tree_sitter::Tree;

use crate::extract;

/// Chunks are cut at the first paragraph break after this many bytes.
const CHUNK: usize = 12_000;
const TIMEOUT: Duration = Duration::from_secs(30);
/// Replacements offered for a finding, at most.
const MAX_REPLACEMENTS: usize = 6;

/// A stretch of the source: prose, or markup read as `interpret`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Piece {
    pub range: Range<usize>,
    /// `None` for prose.
    pub interpret: Option<String>,
}

/// The pieces of a document, covering it from start to end.
pub fn annotate(tree: &Tree, text: &str) -> Vec<Piece> {
    let mut pieces = Vec::new();
    let mut at = 0;
    for run in extract::prose_runs(tree, text) {
        if run.start < at {
            continue;
        }
        if run.start > at {
            gap(text, at..run.start, &mut pieces);
        }
        prose(text, run.clone(), &mut pieces);
        at = run.end;
    }
    if at < text.len() {
        gap(text, at..text.len(), &mut pieces);
    }
    pieces
}

/// Prose, with its ties (`~`) read as spaces and cut after each paragraph.
fn prose(text: &str, range: Range<usize>, out: &mut Vec<Piece>) {
    let mut start = range.start;
    let s = &text[range.clone()];
    let mut push = |r: Range<usize>, interpret: Option<&str>| {
        if !r.is_empty() {
            out.push(Piece {
                range: r,
                interpret: interpret.map(str::to_string),
            });
        }
    };
    let mut i = 0;
    while i < s.len() {
        let at = range.start + i;
        if s[i..].starts_with('~') {
            push(start..at, None);
            push(at..at + 1, Some(" "));
            start = at + 1;
            i += 1;
        } else if s[i..].starts_with(
            "

",
        ) {
            push(start..at + 2, None);
            start = at + 2;
            i += 2;
        } else {
            i += s[i..].chars().next().map_or(1, char::len_utf8);
        }
    }
    push(start..range.end, None);
}

/// Between two runs of prose: punctuation and spaces are prose too.
fn gap(text: &str, range: Range<usize>, out: &mut Vec<Piece>) {
    let s = &text[range.clone()];
    if s.contains(['\\', '{', '}', '$', '%', '&', '^', '_', '#']) {
        out.push(markup(text, range));
    } else {
        prose(text, range, out);
    }
}

/// What a stretch of markup stands for in a sentence.
fn markup(text: &str, range: Range<usize>) -> Piece {
    let s = &text[range.clone()];
    let space = |c: char| c.is_whitespace() || c == '~';
    let core = s.trim_matches(space);
    let paragraph =
        s.split('\n').skip(1).any(|l| l.trim().is_empty()) && s.matches('\n').count() > 1;
    let interpret = if paragraph {
        "\n\n".to_string()
    } else {
        let what = if core.starts_with('$') || core.starts_with("\\(") || core.starts_with("\\[") {
            "X"
        } else if [
            "\\ref",
            "\\eqref",
            "\\cref",
            "\\Cref",
            "\\autoref",
            "\\pageref",
            "\\cite",
            "\\parencite",
            "\\textcite",
            "\\citep",
            "\\citet",
        ]
        .iter()
        .any(|c| core.starts_with(c))
        {
            "1"
        } else {
            ""
        };
        let lead = s.starts_with(space);
        let trail = s.ends_with(space);
        match (what.is_empty(), lead || trail) {
            (true, true) => " ".to_string(),
            (true, false) => String::new(),
            (false, _) => format!(
                "{}{what}{}",
                if lead { " " } else { "" },
                if trail { " " } else { "" }
            ),
        }
    };
    Piece {
        range,
        interpret: Some(interpret),
    }
}

/// Split the pieces into chunks of about [`CHUNK`] bytes, at paragraph breaks.
pub fn chunks(text: &str, pieces: Vec<Piece>) -> Vec<Vec<Piece>> {
    let mut out = Vec::new();
    let mut current: Vec<Piece> = Vec::new();
    let mut size = 0;
    for p in pieces {
        size += p.range.len();
        let paragraph = match &p.interpret {
            Some(i) => i == "\n\n",
            None => text[p.range.clone()].ends_with("\n\n"),
        };
        current.push(p);
        if paragraph && size >= CHUNK {
            out.push(std::mem::take(&mut current));
            size = 0;
        }
    }
    if current.iter().any(|p| p.interpret.is_none()) {
        out.push(current);
    }
    // Chunks without prose need no check.
    out.retain(|c| c.iter().any(|p| p.interpret.is_none()));
    out
}

/// One finding of LanguageTool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// Bytes of the source.
    pub range: Range<usize>,
    pub message: String,
    pub replacements: Vec<String>,
    pub rule: String,
    /// `misspelling`, `grammar`, `style`, `typographical`…
    pub issue_type: String,
}

/// A LanguageTool server, through its HTTP API.
pub struct Server {
    url: String,
    agent: ureq::Agent,
}

#[derive(Deserialize)]
struct Language {
    #[serde(rename = "longCode")]
    long_code: String,
}

#[derive(Deserialize)]
struct Response {
    matches: Vec<Match>,
}

#[derive(Deserialize)]
struct Match {
    message: String,
    offset: usize,
    length: usize,
    #[serde(default)]
    replacements: Vec<Replacement>,
    rule: Rule,
}

#[derive(Deserialize)]
struct Replacement {
    value: String,
}

#[derive(Deserialize)]
struct Rule {
    id: String,
    #[serde(rename = "issueType", default)]
    issue_type: String,
}

impl Server {
    pub fn new(url: &str) -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(TIMEOUT))
            .user_agent(concat!("TeXoria/", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        Self {
            url: url.trim_end_matches('/').to_string(),
            agent,
        }
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    /// The language codes the server knows (`en-US`, `it`…): answers only
    /// when it is up.
    pub fn languages(&self) -> Result<Vec<String>> {
        let langs: Vec<Language> = self
            .agent
            .get(format!("{}/v2/languages", self.url))
            .config()
            .timeout_global(Some(Duration::from_secs(3)))
            .build()
            .call()
            .with_context(|| format!("LanguageTool at {} does not answer", self.url))?
            .body_mut()
            .read_json()?;
        Ok(langs.into_iter().map(|l| l.long_code).collect())
    }

    /// Check one chunk of `text` in `language`, without the rules in `disabled`.
    pub fn check(
        &self,
        text: &str,
        chunk: &[Piece],
        language: &str,
        disabled: &[String],
    ) -> Result<Vec<Problem>> {
        let Some(start) = chunk.first().map(|p| p.range.start) else {
            return Ok(Vec::new());
        };
        let annotation: Vec<_> = chunk
            .iter()
            .map(|p| match &p.interpret {
                None => json!({ "text": &text[p.range.clone()] }),
                Some(i) => json!({ "markup": &text[p.range.clone()], "interpretAs": i }),
            })
            .collect();
        let data = json!({ "annotation": annotation }).to_string();
        let disabled = disabled.join(",");
        let mut form = vec![("data", data.as_str()), ("language", language)];
        if !disabled.is_empty() {
            form.push(("disabledRules", disabled.as_str()));
        }
        let response: Response = self
            .agent
            .post(format!("{}/v2/check", self.url))
            .send_form(form)
            .with_context(|| format!("LanguageTool at {}", self.url))?
            .body_mut()
            .read_json()
            .context("reading LanguageTool's answer")?;
        let end = chunk.last().map_or(start, |p| p.range.end);
        let checked = &text[start..end];
        Ok(response
            .matches
            .into_iter()
            .filter_map(|m| {
                let from = utf16_to_byte(checked, m.offset)?;
                let to = utf16_to_byte(checked, m.offset + m.length)?;
                Some(Problem {
                    range: start + from..start + to,
                    message: m.message,
                    replacements: m
                        .replacements
                        .into_iter()
                        .take(MAX_REPLACEMENTS)
                        .map(|r| r.value)
                        .collect(),
                    rule: m.rule.id,
                    issue_type: m.rule.issue_type,
                })
            })
            .collect())
    }
}

/// The byte offset of a UTF-16 offset (LanguageTool counts Java chars).
pub fn utf16_to_byte(s: &str, utf16: usize) -> Option<usize> {
    let mut units = 0;
    for (i, c) in s.char_indices() {
        if units >= utf16 {
            return (units == utf16).then_some(i);
        }
        units += c.len_utf16();
    }
    (units == utf16).then_some(s.len())
}

/// The server's code for a spelling language: the same, its main variant
/// (`en` → `en-US`), or the language alone (`es-AR` → `es`).
pub fn pick_language(code: &str, supported: &[String]) -> Option<String> {
    let has = |c: &str| supported.iter().any(|s| s.eq_ignore_ascii_case(c));
    if has(code) {
        return Some(code.to_string());
    }
    let base = code.split('-').next().unwrap_or(code);
    let main = match base {
        "en" => "en-US",
        "de" => "de-DE",
        "pt" => "pt-PT",
        "nl" => "nl",
        "ca" => "ca-ES",
        other => other,
    };
    [main, base]
        .into_iter()
        .find(|c| has(c))
        .map(str::to_string)
        .or_else(|| {
            supported
                .iter()
                .find(|s| s.split('-').next() == Some(base))
                .cloned()
        })
}

/// A URL of the machine itself, where TeXoria may start a server.
pub fn local_port(url: &str) -> Option<u16> {
    let rest = url.strip_prefix("http://")?;
    let host_port = rest.split('/').next()?;
    let (host, port) = host_port.rsplit_once(':')?;
    matches!(host, "localhost" | "127.0.0.1" | "[::1]")
        .then(|| port.parse().ok())
        .flatten()
}

/// Whether `url` points to this machine (nothing leaves it).
pub fn is_local(url: &str) -> bool {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .unwrap_or(url);
    let host = rest.split(['/', ':']).next().unwrap_or("");
    matches!(host, "localhost" | "127.0.0.1" | "[")
}

/// Fail early on a URL that is not one.
pub fn validate_url(url: &str) -> Result<()> {
    if url.starts_with("http://") || url.starts_with("https://") {
        Ok(())
    } else {
        Err(anyhow!("{url} is not an http(s) URL"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pieces(doc: &str) -> Vec<(String, Option<String>)> {
        let tree = tex_syntax::parse(doc, None).unwrap();
        annotate(&tree, doc)
            .into_iter()
            .map(|p| (doc[p.range].to_string(), p.interpret))
            .collect()
    }

    #[test]
    fn annotates_latex() {
        let doc = "An \\emph{important} result, see~\\cite{knuth} and $x$.\n\nNext one.";
        let p = pieces(doc);
        let text: String = p
            .iter()
            .map(|(s, i)| i.clone().unwrap_or_else(|| s.clone()))
            .collect();
        assert_eq!(text, "An important result, see 1 and X.\n\nNext one.");
        // The pieces cover the source.
        assert_eq!(p.iter().map(|(s, _)| s.as_str()).collect::<String>(), doc);
    }

    #[test]
    fn splits_long_documents_at_paragraphs() {
        let para = "Some words here and there. ".repeat(100);
        let doc = vec![para; 10].join("\n\n");
        let tree = tex_syntax::parse(&doc, None).unwrap();
        let chunks = chunks(&doc, annotate(&tree, &doc));
        assert!(chunks.len() > 1);
        let covered: usize = chunks.iter().flatten().map(|p| p.range.len()).sum();
        assert_eq!(covered, doc.len());
    }

    #[test]
    fn converts_utf16_offsets() {
        let s = "aé𝕏b";
        assert_eq!(utf16_to_byte(s, 0), Some(0));
        assert_eq!(utf16_to_byte(s, 2), Some(3));
        assert_eq!(utf16_to_byte(s, 4), Some(7));
        assert_eq!(utf16_to_byte(s, 3), None);
        assert_eq!(utf16_to_byte(s, 5), Some(8));
    }

    #[test]
    fn picks_languages() {
        let supported: Vec<String> = ["en-US", "en-GB", "de-DE", "it", "es", "pt-BR", "pt-PT"]
            .map(String::from)
            .to_vec();
        assert_eq!(pick_language("en", &supported).as_deref(), Some("en-US"));
        assert_eq!(pick_language("en-GB", &supported).as_deref(), Some("en-GB"));
        assert_eq!(pick_language("de-CH", &supported).as_deref(), Some("de-DE"));
        assert_eq!(pick_language("es-AR", &supported).as_deref(), Some("es"));
        assert_eq!(pick_language("it", &supported).as_deref(), Some("it"));
        assert_eq!(pick_language("fi", &supported), None);
    }

    #[test]
    fn tells_local_servers() {
        assert_eq!(local_port("http://localhost:8081"), Some(8081));
        assert_eq!(local_port("http://127.0.0.1:8010/"), Some(8010));
        assert_eq!(local_port("https://api.languagetool.org"), None);
        assert!(is_local("http://localhost:8081"));
        assert!(!is_local("https://api.languagetool.org/v2"));
    }
}

#[cfg(test)]
mod live {
    use super::*;

    /// Against a real server: `TEXORIA_LANGUAGETOOL=https://api.languagetool.org`.
    #[test]
    fn checks_with_a_server() {
        let Ok(url) = std::env::var("TEXORIA_LANGUAGETOOL") else {
            return;
        };
        let server = Server::new(&url);
        let languages = server.languages().unwrap();
        assert!(languages.iter().any(|l| l == "en-US"));
        let doc = "This are a \\emph{test} with $x$, see~\\cite{knuth}.";
        let tree = tex_syntax::parse(doc, None).unwrap();
        let chunks = chunks(doc, annotate(&tree, doc));
        let found = server.check(doc, &chunks[0], "en-US", &[]).unwrap();
        eprintln!("{found:#?}");
        assert!(
            found
                .iter()
                .any(|p| &doc[p.range.clone()] == "This are" || &doc[p.range.clone()] == "are")
        );
    }
}
