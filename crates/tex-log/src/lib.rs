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
            // Continuation lines are indented or prefixed with `(pkgname)`.
            let mut message = c[3].trim().to_string();
            let mut j = i + 1;
            while j < lines.len() {
                let next = lines[j].as_str();
                let cont = match source {
                    Some(pkg) => next.strip_prefix(&format!("({pkg})")),
                    None => None,
                }
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
            i += 1;
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
fn unwrap_lines(log: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut joining = false;
    for raw in log.lines() {
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        if joining {
            out.last_mut().unwrap().push_str(raw);
        } else {
            out.push(raw.to_string());
        }
        // Count chars, not bytes: TeX wraps on the (UTF-8 aware) output width.
        joining = raw.chars().count() == WRAP_WIDTH;
    }
    out
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
}
