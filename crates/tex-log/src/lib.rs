//! Parser for TeX engine log files (pdfTeX, XeTeX, LuaTeX, Tectonic).
//!
//! The log format is notoriously loose: lines are hard-wrapped at
//! `max_print_line` (79 by default), the current input file is only
//! recoverable by tracking `(file ... )` parentheses, and messages come in a
//! dozen shapes. This parser is heuristic but robust in practice.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

/// Default TeX Live / MiKTeX `max_print_line`.
const WRAP_WIDTH: usize = 79;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    Error,
    Warning,
    /// Low-importance messages such as underfull boxes.
    Info,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Diagnostic {
    pub severity: Severity,
    /// Source file the message refers to, resolved against the build root.
    pub file: Option<PathBuf>,
    /// 1-based line number.
    pub line: Option<u32>,
    pub message: String,
}

static FILE_LINE_ERROR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(.+\.[A-Za-z0-9]+):(\d+): (.*)$").unwrap());
static LINE_CONTEXT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^l\.(\d+) ?(.*)$").unwrap());
static WARNING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:(LaTeX|LaTeX Font)|(?:Package|Class|Module) (\S+)) Warning: (.*)$").unwrap()
});
static ON_INPUT_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"on input line (\d+)\.?").unwrap());
static BAD_BOX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(Over|Under)full \\([hv])box (.*?)(?: in paragraph| in alignment| detected)? at lines? (\d+)(?:--\d+)?")
        .unwrap()
});
static MISSING_FILE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^No file (.+)\.$").unwrap());

/// Parse a complete log. `root` is the directory the engine was run from;
/// relative file names are resolved against it.
/// Whether the log says the document needs to run programs (minted, svg,
/// pythontex, TikZ externalization…) and could not.
pub fn needs_shell_escape(log: &str) -> bool {
    let log = log.to_ascii_lowercase();
    log.contains("-shell-escape")
        || log.contains("shell escape")
        || log.contains("shell-escape")
        || (log.contains("runsystem(") && log.contains("disabled"))
}

pub fn parse(log: &str, root: &Path) -> Vec<Diagnostic> {
    parse_bytes(log.as_bytes(), root)
}

/// [`parse`] for the log file's raw bytes. Prefer it to decoding the file
/// first: pdfTeX and LuaTeX may wrap a line in the middle of a UTF-8
/// character, which only decodes once the line is joined again.
pub fn parse_bytes(log: &[u8], root: &Path) -> Vec<Diagnostic> {
    let lines = unwrap_lines(log);
    let mut out: Vec<Diagnostic> = Vec::new();
    let mut files = FileStack::default();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i].as_str();

        // `-file-line-error` style: `./main.tex:12: Undefined control sequence.`
        if let Some(c) = FILE_LINE_ERROR.captures(line) {
            let (detail, next) = error_detail(&lines, i + 1);
            out.push(Diagnostic {
                severity: Severity::Error,
                file: Some(resolve(root, &c[1])),
                line: c[2].parse().ok(),
                message: join_message(&c[3], detail.as_deref()),
            });
            i = next;
            continue;
        }

        // Classic style: `! Undefined control sequence.` followed by `l.12 ...`.
        if let Some(msg) = line.strip_prefix("! ") {
            let (detail, next) = error_detail(&lines, i + 1);
            let line_no = lines[i + 1..next.min(lines.len())]
                .iter()
                .find_map(|l| LINE_CONTEXT.captures(l).and_then(|c| c[1].parse().ok()));
            out.push(Diagnostic {
                severity: Severity::Error,
                file: files.current().map(|f| resolve(root, f)),
                line: line_no,
                message: join_message(msg, detail.as_deref()),
            });
            i = next;
            continue;
        }

        if let Some(c) = WARNING.captures(line) {
            let source = c.get(2).map(|m| m.as_str());
            let is_info = c.get(1).is_some_and(|m| m.as_str() == "LaTeX Font");
            // Continuation lines are indented or prefixed with `(pkgname)`
            // (`(Font)` for font warnings).
            let prefix = match source {
                Some(pkg) => Some(format!("({pkg})")),
                None if is_info => Some("(Font)".to_string()),
                None => None,
            };
            let mut message = c[3].trim().to_string();
            let mut j = i + 1;
            while j < lines.len() {
                let next = lines[j].as_str();
                let cont = prefix
                    .as_deref()
                    .and_then(|p| next.strip_prefix(p))
                    .or_else(|| next.strip_prefix("               "));
                match cont {
                    Some(rest) if !next.trim().is_empty() => {
                        message.push(' ');
                        message.push_str(rest.trim());
                        j += 1;
                    }
                    _ => break,
                }
            }
            let line_no = ON_INPUT_LINE
                .captures(&message)
                .and_then(|c| c[1].parse().ok());
            let message = ON_INPUT_LINE
                .replace(&message, "")
                .trim()
                .trim_end_matches('.')
                .to_string();
            let message = match source {
                Some(pkg) => format!("{pkg}: {message}"),
                None => message,
            };
            out.push(Diagnostic {
                severity: if is_info {
                    Severity::Info
                } else {
                    Severity::Warning
                },
                file: files.current().map(|f| resolve(root, f)),
                line: line_no,
                message,
            });
            i = j;
            continue;
        }

        if let Some(c) = BAD_BOX.captures(line) {
            let over = &c[1] == "Over";
            out.push(Diagnostic {
                severity: if over {
                    Severity::Warning
                } else {
                    Severity::Info
                },
                file: files.current().map(|f| resolve(root, f)),
                line: c[4].parse().ok(),
                message: format!("{}full \\{}box {}", &c[1], &c[2], c[3].trim()),
            });
            // The box's contents follow, up to a blank line: document text,
            // whose parentheses do not open or close files.
            i += 1;
            while i < lines.len() && !lines[i].trim().is_empty() {
                i += 1;
            }
            continue;
        }

        if let Some(c) = MISSING_FILE.captures(line) {
            out.push(Diagnostic {
                severity: Severity::Info,
                file: None,
                line: None,
                message: format!("No file {}", &c[1]),
            });
        }

        files.scan(line);
        i += 1;
    }

    dedup(out)
}

/// Undo the engine's hard wrapping at [`WRAP_WIDTH`] columns.
fn unwrap_lines(log: &[u8]) -> Vec<String> {
    if log.is_empty() {
        return Vec::new();
    }
    let by_bytes = counts_bytes(log);
    let log = log.strip_suffix(b"\n").unwrap_or(log);
    let mut out: Vec<Vec<u8>> = Vec::new();
    let mut joining = false;
    for raw in log.split(|&b| b == b'\n') {
        let raw = raw.strip_suffix(b"\r").unwrap_or(raw);
        match out.last_mut() {
            Some(last) if joining => last.extend_from_slice(raw),
            _ => out.push(raw.to_vec()),
        }
        let width = if by_bytes {
            raw.len()
        } else {
            String::from_utf8_lossy(raw).chars().count()
        };
        joining = width == WRAP_WIDTH;
    }
    out.into_iter()
        .map(|l| {
            String::from_utf8(l).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into())
        })
        .collect()
}

/// Whether the engine wraps at [`WRAP_WIDTH`] bytes rather than characters,
/// from the banner on the log's first line. pdfTeX and LuaTeX count bytes;
/// XeTeX counts characters, and so does Tectonic (XeTeX), which writes no
/// banner.
fn counts_bytes(log: &[u8]) -> bool {
    let banner = log.split(|&b| b == b'\n').next().unwrap_or_default();
    banner.starts_with(b"This is ") && !banner.windows(5).any(|w| w == b"XeTeX")
}

/// Collect the lines following an error up to and including the `l.N`
/// context, returning a short detail string and the index after the block.
fn error_detail(lines: &[String], start: usize) -> (Option<String>, usize) {
    // For undefined control sequences TeX puts the offending command at the
    // end of the *top* context line: either `l.N ... \foo` or, when the
    // error happened inside a macro expansion, `\R ->\mathbb `.
    let token = lines
        .get(start)
        .map(|l| {
            LINE_CONTEXT
                .captures(l)
                .map_or(l.as_str(), |c| c.get(2).unwrap().as_str())
        })
        .and_then(|l| l.trim_end().rsplit(char::is_whitespace).next())
        .map(|t| t.rsplit_once("->").map_or(t, |(_, r)| r))
        .filter(|t| t.starts_with('\\'))
        .map(str::to_string);

    // Errors end at the `l.N` line plus the continuation line that holds
    // the rest of the source; cap the search to avoid eating the log.
    let end = (start + 12).min(lines.len());
    for j in start..end {
        if LINE_CONTEXT.is_match(&lines[j]) {
            let next = if j + 1 < lines.len() && !lines[j + 1].trim().is_empty() {
                j + 2
            } else {
                j + 1
            };
            return (token, next);
        }
        if lines[j].starts_with("! ") || FILE_LINE_ERROR.is_match(&lines[j]) {
            return (token, j);
        }
    }
    (token, start)
}

fn join_message(msg: &str, detail: Option<&str>) -> String {
    let msg = msg.trim();
    match detail {
        Some(d) if msg.starts_with("Undefined control sequence") => {
            format!("Undefined control sequence {d}")
        }
        _ => msg.trim_end_matches('.').to_string(),
    }
}

fn resolve(root: &Path, file: &str) -> PathBuf {
    let file = file.strip_prefix("./").unwrap_or(file);
    let path = Path::new(file);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}

fn dedup(diags: Vec<Diagnostic>) -> Vec<Diagnostic> {
    let mut seen = std::collections::HashSet::new();
    diags
        .into_iter()
        .filter(|d| seen.insert(d.clone()))
        .collect()
}

/// Tracks which input file TeX is currently reading via `(file` / `)`.
#[derive(Default)]
struct FileStack {
    /// `None` entries are parentheses that did not open a file.
    stack: Vec<Option<String>>,
}

impl FileStack {
    fn current(&self) -> Option<&str> {
        self.stack.iter().rev().find_map(|f| f.as_deref())
    }

    fn scan(&mut self, line: &str) {
        let bytes = line.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'(' => {
                    let rest = &line[i + 1..];
                    let end = rest
                        .find(|c: char| {
                            c.is_whitespace() || matches!(c, '(' | ')' | '[' | '{' | '<')
                        })
                        .unwrap_or(rest.len());
                    let token = &rest[..end];
                    if looks_like_path(token) {
                        self.stack.push(Some(token.to_string()));
                        i += 1 + end;
                        continue;
                    }
                    self.stack.push(None);
                }
                b')' => {
                    self.stack.pop();
                }
                _ => {}
            }
            i += 1;
        }
    }
}

fn looks_like_path(token: &str) -> bool {
    if token.is_empty() {
        return false;
    }
    let has_sep = token.contains('/') || token.contains('\\');
    let ext = Path::new(token).extension().and_then(|e| e.to_str());
    let has_ext = ext.is_some_and(|e| {
        !e.is_empty() && e.len() <= 8 && e.chars().all(|c| c.is_ascii_alphanumeric())
    });
    has_ext && (has_sep || token.starts_with('.') || !token.contains(':'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tells_when_shell_escape_is_needed() {
        assert!(needs_shell_escape(
            "! Package minted Error: You must invoke LaTeX with the -shell-escape flag."
        ));
        assert!(needs_shell_escape(
            "Package svg Error: You didn't enable `shell escape' (or `write18')"
        ));
        assert!(needs_shell_escape(
            "runsystem(inkscape a.svg)...disabled (restricted)."
        ));
        assert!(!needs_shell_escape(
            "runsystem(repstopdf a.eps)...executed safely (allowed)."
        ));
        assert!(!needs_shell_escape("Output written on main.pdf (1 page)."));
    }

    #[test]
    fn real_pdflatex_log() {
        let log = include_str!("../../../tests/fixtures/logs/pdflatex-undefined-cs.log");
        let d = parse(log, &root());
        let errors: Vec<_> = d.iter().filter(|d| d.severity == Severity::Error).collect();
        assert_eq!(errors.len(), 1, "{d:#?}");
        assert_eq!(
            errors[0].file.as_deref(),
            Some(Path::new("/proj/chapters/methods.tex"))
        );
        assert_eq!(errors[0].line, Some(3));
        assert_eq!(errors[0].message, "Undefined control sequence \\mathbb");

        let refs: Vec<_> = d
            .iter()
            .filter(|d| d.message.starts_with("Reference"))
            .collect();
        assert_eq!(refs.len(), 2);
        // Back in main.tex once methods.tex is closed.
        assert_eq!(refs[0].file.as_deref(), Some(Path::new("/proj/main.tex")));
        assert_eq!(refs[0].line, Some(24));
        // Package `Info:` lines are noise and must not surface.
        assert!(d.iter().all(|d| !d.message.contains("epstopdf")), "{d:#?}");
    }

    fn root() -> PathBuf {
        PathBuf::from("/proj")
    }

    #[test]
    fn classic_undefined_control_sequence() {
        let log = "\
This is pdfTeX, Version 3.141592653
(./main.tex
LaTeX2e <2024-06-01>
(/usr/local/texlive/2024/texmf-dist/tex/latex/base/article.cls
Document Class: article 2024/02/08 v1.4n Standard LaTeX document class
)
! Undefined control sequence.
l.7 Hello \\foo
                world
)
";
        let d = parse(log, &root());
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].severity, Severity::Error);
        assert_eq!(d[0].file.as_deref(), Some(Path::new("/proj/main.tex")));
        assert_eq!(d[0].line, Some(7));
        assert_eq!(d[0].message, "Undefined control sequence \\foo");
    }

    #[test]
    fn file_line_error_style() {
        let log = "\
(./main.tex (./chapters/intro.tex
./chapters/intro.tex:3: LaTeX Error: Environment foo undefined.

See the LaTeX manual or LaTeX Companion for explanation.
Type  H <return>  for immediate help.
 ...

l.3 \\begin{foo}

))
";
        let d = parse(log, &root());
        assert_eq!(d.len(), 1);
        assert_eq!(
            d[0].file.as_deref(),
            Some(Path::new("/proj/chapters/intro.tex"))
        );
        assert_eq!(d[0].line, Some(3));
        assert_eq!(d[0].message, "LaTeX Error: Environment foo undefined");
    }

    #[test]
    fn warnings_and_boxes() {
        let log = "\
(./main.tex
LaTeX Warning: Reference `fig:x' on page 1 undefined on input line 12.

Package hyperref Warning: Token not allowed in a PDF string (Unicode):
(hyperref)                removing `math shift' on input line 20.

Overfull \\hbox (12.3pt too wide) in paragraph at lines 30--32
[]\\OT1/cmr/m/n/10 text

Underfull \\vbox (badness 10000) detected at line 40
)
";
        let d = parse(log, &root());
        assert_eq!(d.len(), 4, "{d:#?}");
        assert_eq!(d[0].severity, Severity::Warning);
        assert_eq!(d[0].line, Some(12));
        assert_eq!(d[0].message, "Reference `fig:x' on page 1 undefined");
        assert_eq!(d[1].line, Some(20));
        assert!(d[1].message.starts_with("hyperref: Token not allowed"));
        assert!(d[1].message.contains("removing `math shift'"));
        assert_eq!(d[2].severity, Severity::Warning);
        assert_eq!(d[2].line, Some(30));
        assert_eq!(d[3].severity, Severity::Info);
        assert_eq!(d[3].line, Some(40));
        assert!(
            d.iter()
                .all(|x| x.file.as_deref() == Some(Path::new("/proj/main.tex")))
        );
    }

    #[test]
    fn wrapped_paths_are_joined() {
        // A package path long enough to be hard-wrapped at 79 columns; if the
        // wrap were not undone, `.sty)` would be misread and unbalance the stack.
        let full =
            "(/usr/local/texlive/2024/texmf-dist/tex/latex/somepackage/a-rather-long-name/xy.sty)";
        let log = format!(
            "{}\n{}\n(./main.tex\n! Missing $ inserted.\nl.4 a_\n     b\n)\n",
            &full[..WRAP_WIDTH],
            &full[WRAP_WIDTH..]
        );
        let d = parse(&log, &root());
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].file.as_deref(), Some(Path::new("/proj/main.tex")));
        assert_eq!(d[0].line, Some(4));
        assert_eq!(d[0].message, "Missing $ inserted");
    }

    fn fixture(name: &str) -> String {
        let path = test_support::fixtures().join("logs").join(name);
        String::from_utf8_lossy(&std::fs::read(path).unwrap()).into_owned()
    }

    #[test]
    fn real_clean_log_has_no_diagnostics() {
        // The sample project's own build: it must stay clean (see AGENTS.md).
        let log = fixture("pdflatex-sample-clean.log");
        assert!(log.starts_with("This is pdfTeX"));
        assert_eq!(parse(&log, &root()), vec![]);
        assert!(!needs_shell_escape(&log));
    }

    #[test]
    fn wrapped_accented_lines_are_joined() {
        // The same document through each engine: pdfTeX and LuaTeX wrap at 79
        // bytes, XeTeX (and so Tectonic) at 79 characters.
        let label = "sec:perché-qualità-già-così-città-però-più-università-novità-verità";
        for name in [
            "pdflatex-mixed.log",
            "lualatex-mixed.log",
            "xelatex-mixed.log",
            "tectonic-mixed.log",
        ] {
            let d = parse(&fixture(name), &root());
            let reference = d
                .iter()
                .find(|d| d.message.contains("sec:"))
                .unwrap_or_else(|| panic!("{name}: {d:#?}"));
            assert_eq!(
                reference.message,
                format!("Reference `{label}' on page 1 undefined"),
                "{name}"
            );
            assert_eq!(reference.line, Some(3), "{name}");
            assert_eq!(
                reference.file.as_deref(),
                Some(Path::new("/proj/main.tex")),
                "{name}"
            );
        }
    }

    #[test]
    fn parentheses_in_box_contents_are_not_files() {
        // chap.tex typesets an unbalanced `(` in an overfull box, which the
        // log shows: main.tex is still the file once chap.tex is closed.
        for name in [
            "pdflatex-mixed.log",
            "lualatex-mixed.log",
            "xelatex-mixed.log",
        ] {
            let d = parse(&fixture(name), &root());
            let find = |text: &str| {
                d.iter()
                    .find(|d| d.message.contains(text))
                    .unwrap_or_else(|| panic!("{name}: {d:#?}"))
            };
            let overfull = find("Overfull");
            assert_eq!(overfull.file.as_deref(), Some(Path::new("/proj/chap.tex")));
            assert_eq!(overfull.line, Some(1));
            let missing = find("`missing'");
            assert_eq!(
                missing.file.as_deref(),
                Some(Path::new("/proj/main.tex")),
                "{name}"
            );
            assert_eq!(missing.line, Some(5));
        }

        // And a `)` in them does not close the file.
        let log = "\
(./main.tex (./chap.tex
Underfull \\hbox (badness 10000) in paragraph at lines 1--2
[]\\OT1/cmr/m/n/10 (see) and) more
 []

LaTeX Warning: Something on input line 3.

)
LaTeX Warning: Other on input line 9.

)
";
        let d = parse(log, &root());
        assert_eq!(d.len(), 3, "{d:#?}");
        assert_eq!(d[1].file.as_deref(), Some(Path::new("/proj/chap.tex")));
        assert_eq!(d[2].file.as_deref(), Some(Path::new("/proj/main.tex")));
    }

    #[test]
    fn font_warnings_are_info_with_their_continuation() {
        for (name, shape) in [
            ("pdflatex-mixed.log", "OT1/cmr"),
            ("lualatex-mixed.log", "TU/lmr"),
            ("xelatex-mixed.log", "TU/lmr"),
            ("tectonic-mixed.log", "TU/lmr"),
        ] {
            let d = parse(&fixture(name), &root());
            let font = d
                .iter()
                .find(|d| d.message.starts_with("Font shape"))
                .unwrap_or_else(|| panic!("{name}: {d:#?}"));
            assert_eq!(font.severity, Severity::Info);
            assert_eq!(
                font.message,
                format!("Font shape `{shape}/bx/sc' undefined using `{shape}/bx/n' instead"),
                "{name}"
            );
            assert_eq!(font.line, Some(6), "{name}");
            assert_eq!(font.file.as_deref(), Some(Path::new("/proj/main.tex")));
        }
    }

    #[test]
    fn wrap_inside_a_utf8_character() {
        // pdfTeX breaks after the 79th byte, here the first byte of an `é`.
        let label = "é".repeat(30);
        let line =
            format!("LaTeX Warning: Reference `{label}' on page 1 undefined on input line 3.");
        let (head, tail) = line.as_bytes().split_at(WRAP_WIDTH);
        assert!(std::str::from_utf8(head).is_err());
        let mut log = b"This is pdfTeX, Version 3.141592653-2.6-1.40.29\n(./main.tex\n".to_vec();
        log.extend_from_slice(head);
        log.push(b'\n');
        log.extend_from_slice(tail);
        log.extend_from_slice(b"\n\n)\n");

        let d = parse_bytes(&log, &root());
        assert_eq!(d.len(), 1, "{d:#?}");
        assert_eq!(
            d[0].message,
            format!("Reference `{label}' on page 1 undefined")
        );
        assert_eq!(d[0].line, Some(3));
        // Decoding the file first cannot give the character back.
        let lossy = parse(&String::from_utf8_lossy(&log), &root());
        assert_ne!(lossy, d);
    }

    #[test]
    fn invalid_utf8_is_replaced() {
        let d = parse_bytes(b"(./main.tex\n! Bad \xff byte.\nl.2 x\n)\n", &root());
        assert_eq!(d[0].message, "Bad \u{fffd} byte");
    }

    #[test]
    fn empty_and_blank_logs() {
        assert_eq!(parse("", &root()), vec![]);
        assert_eq!(parse("\n\n\r\n", &root()), vec![]);
        assert_eq!(unwrap_lines(b""), Vec::<String>::new());
    }

    #[test]
    fn crlf_logs_parse_like_lf_ones() {
        for name in ["pdflatex-undefined-cs.log", "pdflatex-sample-clean.log"] {
            let lf = fixture(name);
            let crlf = lf.replace('\n', "\r\n");
            assert_eq!(parse(&crlf, &root()), parse(&lf, &root()), "{name}");
        }
    }

    #[test]
    fn class_and_module_warnings() {
        let log = "\
(./main.tex
Class beamer Warning: Frame too tall on input line 9.

Module foo Warning: Something odd
(foo)                spanning lines on input line 11.
)
";
        let d = parse(log, &root());
        assert_eq!(d.len(), 2, "{d:#?}");
        assert_eq!(d[0].message, "beamer: Frame too tall");
        assert_eq!(d[0].line, Some(9));
        assert_eq!(d[1].message, "foo: Something odd spanning lines");
        assert_eq!(d[1].line, Some(11));
        assert!(d.iter().all(|d| d.severity == Severity::Warning));
    }

    #[test]
    fn missing_files_are_info() {
        let d = parse("(./main.tex\nNo file main.toc.\n)\n", &root());
        assert_eq!(
            d,
            vec![Diagnostic {
                severity: Severity::Info,
                file: None,
                line: None,
                message: "No file main.toc".into(),
            }]
        );
    }

    #[test]
    fn repeated_messages_are_reported_once() {
        let warning = "LaTeX Warning: Reference `x' on page 1 undefined on input line 3.\n\n";
        let log = format!("(./main.tex\n{warning}{warning}(./a.tex\n{warning})\n)\n");
        let d = parse(&log, &root());
        // Same message, line and file: once; another file: kept.
        assert_eq!(d.len(), 2, "{d:#?}");
        assert_eq!(d[1].file.as_deref(), Some(Path::new("/proj/a.tex")));
    }

    #[test]
    fn error_without_line_context() {
        let log = "\
(./main.tex
! Emergency stop.
<*> main.tex

*** (job aborted, no legal \\end found)

)
";
        let d = parse(log, &root());
        assert_eq!(d.len(), 1, "{d:#?}");
        assert_eq!(d[0].message, "Emergency stop");
        assert_eq!(d[0].line, None);
        assert_eq!(d[0].file.as_deref(), Some(Path::new("/proj/main.tex")));
    }

    #[test]
    fn back_to_back_errors() {
        let log = "\
(./main.tex
! Missing number, treated as zero.
! Illegal unit of measure (pt inserted).
l.5 \\hskip x

)
";
        let d = parse(log, &root());
        assert_eq!(d.len(), 2, "{d:#?}");
        // The first error stops at the second one: no `l.N` of its own.
        assert_eq!(d[0].message, "Missing number, treated as zero");
        assert_eq!(d[0].line, None);
        assert_eq!(d[1].message, "Illegal unit of measure (pt inserted)");
        assert_eq!(d[1].line, Some(5));

        let log = "./a.tex:1: First.\n./a.tex:2: Second.\nl.2 x\n";
        let d = parse(log, &root());
        assert_eq!(d.len(), 2, "{d:#?}");
        assert_eq!((d[0].line, d[1].line), (Some(1), Some(2)));
    }

    #[test]
    fn line_context_on_the_last_line() {
        let d = parse(
            "(./main.tex\n! Undefined control sequence.\nl.9 \\oops",
            &root(),
        );
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].line, Some(9));
        assert_eq!(d[0].message, "Undefined control sequence \\oops");
    }

    #[test]
    fn line_context_is_looked_for_only_nearby() {
        let filler = "help text\n".repeat(15);
        let log = format!("(./main.tex\n! Something wrong.\n{filler}l.40 x\n)\n");
        let d = parse(&log, &root());
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].line, None, "{d:#?}");
    }

    #[test]
    fn unbalanced_closing_parens_are_harmless() {
        let d = parse(")))\n! Oops.\nl.1 x\n", &root());
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].file, None);
        assert_eq!(d[0].line, Some(1));
    }

    #[test]
    fn natural_line_of_wrap_width() {
        // TeX breaks a line as soon as it is 79 columns wide, so a natural line
        // of exactly that width is followed by an empty one: joining it with
        // that (and only that) gives the line back.
        let natural = "B".repeat(WRAP_WIDTH);
        let log = format!("{natural}\n\n! Oops.\nl.2 x\n");
        assert_eq!(
            unwrap_lines(log.as_bytes()),
            vec![natural.as_str(), "! Oops.", "l.2 x"]
        );
        assert_eq!(parse(&log, &root())[0].line, Some(2));
    }

    #[test]
    fn file_line_error_with_an_absolute_path() {
        let log = "(/tmp/p/main.tex\n/tmp/p/main.tex:3: Undefined control sequence.\nl.3 \\x\n";
        let d = parse(log, &root());
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].line, Some(3));
        #[cfg(unix)]
        assert_eq!(d[0].file.as_deref(), Some(Path::new("/tmp/p/main.tex")));
    }

    #[cfg(windows)]
    #[test]
    fn windows_paths() {
        let log = "\
(C:\\p\\main.tex
C:\\p\\main.tex:3: Undefined control sequence.
l.3 \\x

(C:/p/sub/intro.tex
! Oops.
l.4 y
))
";
        let d = parse(log, Path::new("D:\\elsewhere"));
        assert_eq!(d.len(), 2, "{d:#?}");
        assert_eq!(d[0].file.as_deref(), Some(Path::new("C:\\p\\main.tex")));
        assert_eq!(d[0].line, Some(3));
        assert_eq!(d[1].file.as_deref(), Some(Path::new("C:/p/sub/intro.tex")));
    }

    #[test]
    fn what_looks_like_a_path() {
        for yes in [
            "./main.tex",
            "/usr/share/a.sty",
            "chapters\\x.tex",
            "main.tex",
        ] {
            assert!(looks_like_path(yes), "{yes}");
        }
        for no in ["", "see", "e.g.,", "x.toolongextension", "a.b-c", "C:x.tex"] {
            assert!(!looks_like_path(no), "{no}");
        }
    }

    mod props {
        use super::*;
        use proptest::prelude::*;

        /// Lines in the shapes the parser looks for, plus noise.
        fn log_line() -> impl Strategy<Value = String> {
            prop_oneof![
                Just("(./main.tex".to_string()),
                Just("(./chapters/intro.tex".to_string()),
                Just(")".to_string()),
                Just(String::new()),
                Just("! Undefined control sequence.".to_string()),
                Just("l.3 \\foo".to_string()),
                Just("./main.tex:7: LaTeX Error: Oops.".to_string()),
                Just("LaTeX Warning: Reference `a' undefined on input line 4.".to_string()),
                Just("LaTeX Font Warning: Font shape `x' undefined".to_string()),
                Just("Package pkg Warning: Hmm".to_string()),
                Just("(pkg)                more on input line 9.".to_string()),
                Just("               continued".to_string()),
                Just("Overfull \\hbox (1.0pt too wide) in paragraph at lines 1--2".to_string()),
                Just("[]\\OT1/cmr/m/n/10 (text".to_string()),
                Just("No file main.aux.".to_string()),
                "[a-zé()\\\\./ ]{79}",
                "[^\r\n]{0,100}",
            ]
        }

        fn log() -> impl Strategy<Value = String> {
            prop::collection::vec(log_line(), 0..40).prop_map(|l| l.join("\n"))
        }

        proptest! {
            #[test]
            fn never_panics(text in "\\PC*") {
                let _ = parse(&text, &root());
                let _ = needs_shell_escape(&text);
            }

            #[test]
            fn never_panics_on_log_shapes(text in log()) {
                for d in parse(&text, &root()) {
                    prop_assert!(!d.message.contains('\n'));
                    prop_assert!(d.line.is_none_or(|l| l < 1000));
                }
            }

            #[test]
            fn crlf_is_lf(text in log()) {
                prop_assert_eq!(parse(&text.replace('\n', "\r\n"), &root()), parse(&text, &root()));
            }

            #[test]
            fn dedup_is_idempotent(text in log()) {
                let once = parse(&text, &root());
                let set: std::collections::HashSet<_> = once.iter().collect();
                prop_assert_eq!(set.len(), once.len());
                prop_assert_eq!(dedup(once.clone()), once);
            }

            #[test]
            fn bytes_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..400)) {
                let _ = parse_bytes(&bytes, &root());
            }

            #[test]
            fn looks_like_path_never_panics(token in "\\PC*") {
                let _ = looks_like_path(&token);
            }
        }
    }
}
