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

/// The name of the command `s` starts with (`\\cite` in `\\cite[p.~2]{k}`).
fn leading_command(s: &str) -> Option<&str> {
    let name = s.strip_prefix('\\')?;
    let len = name
        .find(|c: char| !c.is_ascii_alphabetic())
        .unwrap_or(name.len());
    (len > 0).then(|| &s[..len + 1])
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
        } else if leading_command(core)
            .is_some_and(|c| tex_syntax::is_ref_command(c) || tex_syntax::is_cite_command(c))
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
        let agent = crate::agent(TIMEOUT);
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
    let (host, port) = host_and_port(url.strip_prefix("http://")?)?;
    let local = host.eq_ignore_ascii_case("localhost") || matches!(host, "127.0.0.1" | "[::1]");
    local.then(|| port?.parse().ok()).flatten()
}

/// Whether `url` points to this machine (nothing leaves it).
pub fn is_local(url: &str) -> bool {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .unwrap_or(url);
    let Some((host, _)) = host_and_port(rest) else {
        return false;
    };
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    let ip = match host.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
        Some(v6) => v6
            .parse::<std::net::Ipv6Addr>()
            .ok()
            .map(std::net::IpAddr::V6),
        None => host
            .parse::<std::net::Ipv4Addr>()
            .ok()
            .map(std::net::IpAddr::V4),
    };
    ip.is_some_and(|ip| ip.to_canonical().is_loopback())
}

/// The host (IPv6 addresses in brackets) and the port of what follows a URL's
/// `scheme://`: its authority, without the user info.
fn host_and_port(rest: &str) -> Option<(&str, Option<&str>)> {
    // Browsers and URL parsers read a backslash as a slash.
    let authority = rest.split(['/', '\\', '?', '#']).next()?;
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let (host, port) = if host_port.starts_with('[') {
        let end = host_port.find(']')? + 1;
        let port = match &host_port[end..] {
            "" => None,
            p => Some(p.strip_prefix(':')?),
        };
        (&host_port[..end], port)
    } else {
        match host_port.split_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (host_port, None),
        }
    };
    (!host.is_empty()).then_some((host, port))
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
    use crate::testing::latex;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn utf16_offsets_round_trip(s in "\\PC{0,24}") {
            let mut units = 0;
            for (i, c) in s.char_indices() {
                prop_assert_eq!(utf16_to_byte(&s, units), Some(i));
                if c.len_utf16() == 2 {
                    // Between the two halves of a surrogate pair.
                    prop_assert_eq!(utf16_to_byte(&s, units + 1), None);
                }
                units += c.len_utf16();
            }
            prop_assert_eq!(utf16_to_byte(&s, units), Some(s.len()));
            prop_assert_eq!(utf16_to_byte(&s, units + 1), None);
        }

        #[test]
        fn pieces_cover_the_source(doc in latex()) {
            let tree = tex_syntax::parse(&doc, None).unwrap();
            let pieces = annotate(&tree, &doc);
            let mut at = 0;
            for p in &pieces {
                prop_assert_eq!(p.range.start, at, "{:?}", pieces);
                prop_assert!(p.range.end > p.range.start);
                prop_assert!(doc.get(p.range.clone()).is_some());
                at = p.range.end;
            }
            prop_assert_eq!(at, doc.len());

            // Chunks keep the pieces in order, and every piece of prose.
            let prose: Vec<_> = pieces.iter().filter(|p| p.interpret.is_none()).cloned().collect();
            let chunks = chunks(&doc, pieces.clone());
            let flat: Vec<Piece> = chunks.iter().flatten().cloned().collect();
            prop_assert!(flat.windows(2).all(|w| w[0].range.end == w[1].range.start)
                || chunks.len() > 1);
            for c in &chunks {
                prop_assert!(c.windows(2).all(|w| w[0].range.end == w[1].range.start));
                prop_assert!(c.iter().any(|p| p.interpret.is_none()));
            }
            let kept: Vec<_> = flat.into_iter().filter(|p| p.interpret.is_none()).collect();
            prop_assert_eq!(kept, prose);
        }
    }

    #[test]
    fn documents_without_prose_have_no_chunks() {
        for doc in [
            "",
            "$x$",
            "\\foo{} % only markup\n",
            "\\begin{tikzpicture}\\end{tikzpicture}",
        ] {
            let tree = tex_syntax::parse(doc, None).unwrap();
            assert!(chunks(doc, annotate(&tree, doc)).is_empty(), "{doc:?}");
        }
    }

    #[test]
    fn trailing_markup_stays_with_the_last_chunk() {
        let para = "Some words here and there. ".repeat(500);
        let doc = format!("{para}\n\n$y$ \\cite{{k}}");
        let tree = tex_syntax::parse(&doc, None).unwrap();
        let chunks = chunks(&doc, annotate(&tree, &doc));
        // The blank line is in the markup between the prose runs: no chunk of
        // markup alone is left over.
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].last().unwrap().range.end, doc.len());
    }

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
        // Any command referring to labels or citing stands for a number, and only those.
        for (doc, read) in [
            ("See \\autocite{k}.", "See 1."),
            ("See \\Cref{a,b}.", "See 1."),
            ("See \\footcite[p.~2]{k}.", "See 1."),
            ("See \\refstepcounter{x}.", "See ."),
        ] {
            let text: String = pieces(doc)
                .into_iter()
                .map(|(s, i)| i.unwrap_or(s))
                .collect();
            assert_eq!(text, read, "{doc}");
        }
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
        // Any variant of the language, as a last resort.
        assert_eq!(pick_language("pt", &supported).as_deref(), Some("pt-PT"));
        let dutch = vec!["nl-BE".to_string()];
        assert_eq!(pick_language("nl-NL", &dutch).as_deref(), Some("nl-BE"));
        assert_eq!(pick_language("EN-us", &supported).as_deref(), Some("EN-us"));
    }

    #[test]
    fn tells_local_servers() {
        assert_eq!(local_port("http://localhost:8081"), Some(8081));
        assert_eq!(local_port("http://127.0.0.1:8010/"), Some(8010));
        assert_eq!(local_port("https://api.languagetool.org"), None);
        assert!(is_local("http://localhost:8081"));
        assert!(!is_local("https://api.languagetool.org/v2"));
    }

    #[test]
    fn local_means_loopback_only() {
        for url in [
            "http://localhost:8081",
            "http://LOCALHOST:8081/v2",
            "http://localhost",
            "https://127.0.0.1/",
            "http://127.0.0.2:8010",
            "http://[::1]:8081/",
            "http://[::ffff:127.0.0.1]:8081",
            "http://user:pw@localhost:8081/",
            "localhost:8081",
        ] {
            assert!(is_local(url), "{url}");
        }
        for url in [
            // The host is what follows the user info.
            "http://localhost:pw@evil.com/",
            "http://localhost@evil.com:8081/",
            "http://127.0.0.1:8081@evil.com/",
            // Not loopback, though in brackets.
            "http://[::ffff:8.8.8.8]:8081/",
            "http://[2001:db8::1]/",
            "http://[/",
            "http://[::1/",
            "http://localhost.evil.com/",
            "http://127.0.0.1.evil.com/",
            "http://0.0.0.0:8081/",
            "http://evil.com\\@localhost/",
            "http://",
            "",
        ] {
            assert!(!is_local(url), "{url}");
        }
    }

    #[test]
    fn local_ports_need_a_local_host_and_a_port() {
        assert_eq!(local_port("http://[::1]:8081/v2"), Some(8081));
        assert_eq!(local_port("http://localhost:8081?x=1"), Some(8081));
        assert_eq!(local_port("http://localhost:8081#top"), Some(8081));
        assert_eq!(local_port("http://user@localhost:8081"), Some(8081));
        assert_eq!(local_port("http://localhost"), None);
        assert_eq!(local_port("http://localhost:"), None);
        assert_eq!(local_port("http://localhost:99999"), None);
        assert_eq!(local_port("http://[::1]"), None);
        // TeXoria would start a server for an address it cannot listen on.
        assert_eq!(local_port("https://localhost:8081"), None);
        assert_eq!(local_port("http://example.com:8081"), None);
        assert_eq!(local_port("http://localhost:8081@evil.com/"), None);
        assert_eq!(local_port("http://localhost:pw@evil.com:8081/"), None);
    }

    #[test]
    fn validates_urls() {
        assert!(validate_url("http://localhost:8081").is_ok());
        assert!(validate_url("https://api.languagetool.org").is_ok());
        for bad in ["localhost:8081", "ftp://x", "", "HTTP:/x"] {
            let e = validate_url(bad).unwrap_err();
            assert!(e.to_string().contains("is not an http(s) URL"), "{e}");
        }
    }
}

#[cfg(test)]
mod server_tests {
    use super::*;
    use test_support::http::{Response as Answer, serve, serve_paths};

    fn field(fields: &[(String, String)], name: &str) -> Option<String> {
        fields
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
    }

    #[test]
    fn lists_the_server_languages() {
        let server = serve_paths(vec![(
            "/v2/languages",
            Answer::ok(
                r#"[{"name":"English (US)","code":"en","longCode":"en-US"},
                    {"name":"Italian","code":"it","longCode":"it"}]"#,
            ),
        )]);
        let lt = Server::new(&format!("{}/", server.url()));
        assert_eq!(lt.url(), server.url());
        assert_eq!(lt.languages().unwrap(), ["en-US", "it"]);
        assert_eq!(server.requests()[0].method, "GET");
    }

    #[test]
    fn languages_fail_on_errors_and_garbage() {
        let down = serve(|_| Answer::status(503));
        let e = Server::new(down.url()).languages().unwrap_err();
        assert!(format!("{e:#}").contains("does not answer"), "{e:#}");
        let garbage = serve(|_| Answer::ok("<html>not json</html>"));
        assert!(Server::new(garbage.url()).languages().is_err());
        // Nothing listens there.
        assert!(
            Server::new(&test_support::http::closed_url())
                .languages()
                .is_err()
        );
    }

    /// A document of two paragraphs, and the pieces of the second one.
    fn second_paragraph(doc: &str) -> Vec<Piece> {
        let tree = tex_syntax::parse(doc, None).unwrap();
        let start = doc.find("\n\n").unwrap() + 2;
        annotate(&tree, doc)
            .into_iter()
            .filter(|p| p.range.start >= start)
            .collect()
    }

    #[test]
    fn maps_findings_back_to_the_source() {
        let doc = "Uno.\n\nDue 𝕏 erore $x$ fine.";
        let chunk = second_paragraph(doc);
        assert_eq!(chunk[0].range.start, 6);
        let replacements: Vec<_> = (0..10).map(|i| format!(r#"{{"value":"r{i}"}}"#)).collect();
        let body = format!(
            r#"{{"software":{{"name":"LanguageTool"}},"matches":[
                {{"message":"Typo","offset":7,"length":5,"replacements":[{}],
                  "rule":{{"id":"MORFOLOGIK","issueType":"misspelling"}}}},
                {{"message":"Inside a surrogate pair","offset":5,"length":1,
                  "rule":{{"id":"X"}}}},
                {{"message":"Past the end","offset":20,"length":9,"rule":{{"id":"Y"}}}},
                {{"message":"Whole","offset":0,"length":3,"replacements":[],
                  "rule":{{"id":"Z","issueType":"style"}}}}
            ]}}"#,
            replacements.join(",")
        );
        let server = serve_paths(vec![("/v2/check", Answer::ok(body))]);
        let lt = Server::new(server.url());
        let found = lt.check(doc, &chunk, "it", &[]).unwrap();
        assert_eq!(found.len(), 2, "{found:#?}");
        assert_eq!(&doc[found[0].range.clone()], "erore");
        assert_eq!(found[0].message, "Typo");
        assert_eq!(found[0].rule, "MORFOLOGIK");
        assert_eq!(found[0].issue_type, "misspelling");
        assert_eq!(found[0].replacements, ["r0", "r1", "r2", "r3", "r4", "r5"]);
        assert_eq!(&doc[found[1].range.clone()], "Due");
        assert_eq!(found[1].issue_type, "style");

        let req = &server.requests()[0];
        assert_eq!(req.method, "POST");
        let fields = req.form();
        assert_eq!(field(&fields, "language").as_deref(), Some("it"));
        assert_eq!(field(&fields, "disabledRules"), None);
        let data: serde_json::Value =
            serde_json::from_str(&field(&fields, "data").unwrap()).unwrap();
        let annotation = data["annotation"].as_array().unwrap();
        // Only the chunk, starting with its prose.
        assert_eq!(annotation[0]["text"], "Due 𝕏 erore");
        assert!(
            annotation
                .iter()
                .any(|a| a["markup"] == " $x$ " && a["interpretAs"] == " X ")
        );
    }

    #[test]
    fn sends_the_disabled_rules() {
        let server = serve(|_| Answer::ok(r#"{"matches":[]}"#));
        let lt = Server::new(server.url());
        let doc = "Uno.\n\nDue.";
        let disabled = [
            "WHITESPACE_RULE".to_string(),
            "COMMA_PARENTHESIS".to_string(),
        ];
        assert!(
            lt.check(doc, &second_paragraph(doc), "en-US", &disabled)
                .unwrap()
                .is_empty()
        );
        let fields = server.requests()[0].form();
        assert_eq!(
            field(&fields, "disabledRules").as_deref(),
            Some("WHITESPACE_RULE,COMMA_PARENTHESIS")
        );
        // An empty chunk asks nothing.
        assert!(lt.check(doc, &[], "en-US", &disabled).unwrap().is_empty());
        assert_eq!(server.requests().len(), 1);
    }

    #[test]
    fn check_fails_on_errors_and_garbage() {
        let doc = "Uno.\n\nDue.";
        let chunk = second_paragraph(doc);
        let busy = serve(|_| Answer::status(429).body("Too many requests"));
        let e = Server::new(busy.url())
            .check(doc, &chunk, "it", &[])
            .unwrap_err();
        assert!(format!("{e:#}").contains(busy.url()), "{e:#}");
        for body in [
            "not json",
            r#"{"no":"matches"}"#,
            r#"{"matches":[{"offset":1}]}"#,
        ] {
            let bad = serve(move |_| Answer::ok(body));
            let e = Server::new(bad.url())
                .check(doc, &chunk, "it", &[])
                .unwrap_err();
            assert!(
                format!("{e:#}").contains("reading LanguageTool's answer"),
                "{e:#}"
            );
        }
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
