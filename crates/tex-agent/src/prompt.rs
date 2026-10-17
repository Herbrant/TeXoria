//! What TeXoria tells the agent: the context of a session, and the requests
//! behind its buttons.

use std::path::Path;

use tex_log::{Diagnostic, Severity};

use crate::mcp::SERVER_NAME;

/// At most this many problems are listed in a request.
const MAX_PROBLEMS: usize = 40;

/// Ways to rewrite a passage, offered in the editor's context menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rewrite {
    Improve,
    Shorten,
    Formal,
    Grammar,
    /// The user's own instruction.
    Custom(String),
}

impl Rewrite {
    pub const PRESETS: [Rewrite; 4] = [
        Rewrite::Improve,
        Rewrite::Shorten,
        Rewrite::Formal,
        Rewrite::Grammar,
    ];

    pub fn label(&self) -> &str {
        match self {
            Rewrite::Improve => "Improve Writing",
            Rewrite::Shorten => "Make Shorter",
            Rewrite::Formal => "Make More Formal",
            Rewrite::Grammar => "Fix Grammar and Spelling",
            Rewrite::Custom(_) => "Edit with Instructions",
        }
    }

    fn instruction(&self) -> &str {
        match self {
            Rewrite::Improve => {
                "Rewrite it to read better: clearer, with a better flow, without changing \
                 what it says."
            }
            Rewrite::Shorten => "Make it more concise, keeping everything important it says.",
            Rewrite::Formal => {
                "Make its tone more formal, as in a scientific publication, without \
                 changing what it says."
            }
            Rewrite::Grammar => {
                "Fix its grammar, spelling and punctuation only; change nothing else."
            }
            Rewrite::Custom(text) => text,
        }
    }
}

/// The first message of a session: where the agent is and how to work here.
pub fn preamble(root: &Path, dir: &Path) -> String {
    let root = root.strip_prefix(dir).unwrap_or(root);
    format!(
        "You are working in TeXoria, a LaTeX editor, on the project in `{dir}`, whose root \
         file is `{root}`. The user writes the document; help them with it.\n\
         - Edit the source files in place; the user reviews your changes and can undo them.\n\
         - To build the document and get its errors and warnings, use the `build` tool \
         of the `{SERVER_NAME}` MCP server, not latexmk or another engine: TeXoria shows \
         the result in its preview. Its `pdf_page` tool shows you a page of the PDF, to \
         check the layout; `bib_search` finds the keys to cite.\n\
         - Build files are in `.texoria/`: never edit them.\n\
         - Keep the document's language, style and LaTeX conventions.",
        dir = dir.display(),
        root = root.display(),
    )
}

/// Ask to fix the errors of the last build.
pub fn fix_errors(dir: &Path, diagnostics: &[Diagnostic]) -> String {
    let errors: Vec<&Diagnostic> = diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    let listed = if errors.is_empty() {
        diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Warning)
            .collect()
    } else {
        errors
    };
    let what = if listed.iter().any(|d| d.severity == Severity::Error) {
        "fails to build"
    } else {
        "builds with warnings"
    };
    format!(
        "The document {what}. Fix the problems below with the smallest changes to the \
         sources, without changing the document's content or style. Then build it again \
         with the `build` tool to check that they are gone.\n\n{}",
        format_diagnostics(dir, &listed.into_iter().cloned().collect::<Vec<_>>())
    )
}

/// Ask to rewrite lines `first..=last` (1-based) of `file`.
pub fn rewrite(file: &Path, first: u32, last: u32, text: &str, how: &Rewrite) -> String {
    let lines = if first == last {
        format!("line {first}")
    } else {
        format!("lines {first}–{last}")
    };
    format!(
        "{intro} of `{file}`:\n\n{block}\n\n{instruction}\n\nEdit the file in place, \
         replacing only this passage. Keep its LaTeX commands, labels, citations, references \
         and math intact, and keep it in the language it is written in. Do not change anything \
         else.",
        intro = format_args!("This passage is at {lines}"),
        file = file.display(),
        block = fenced(text),
        instruction = how.instruction(),
    )
}

/// The selection attached to a chat message.
pub fn selection(file: &Path, first: u32, last: u32, text: &str) -> String {
    let lines = if first == last {
        format!("line {first}")
    } else {
        format!("lines {first}–{last}")
    };
    format!(
        "Selected in `{}`, {lines}:\n\n{}",
        file.display(),
        fenced(text)
    )
}

/// Problems as `file:line: error: message` lines, paths relative to `dir`.
pub fn format_diagnostics(dir: &Path, diagnostics: &[Diagnostic]) -> String {
    if diagnostics.is_empty() {
        return "No errors or warnings.".to_string();
    }
    let mut out: Vec<String> = diagnostics
        .iter()
        .take(MAX_PROBLEMS)
        .map(|d| {
            let severity = match d.severity {
                Severity::Error => "error",
                Severity::Warning => "warning",
                Severity::Info => "info",
            };
            let place = match (&d.file, d.line) {
                (Some(f), Some(l)) => {
                    format!("{}:{l}: ", f.strip_prefix(dir).unwrap_or(f).display())
                }
                (Some(f), None) => format!("{}: ", f.strip_prefix(dir).unwrap_or(f).display()),
                _ => String::new(),
            };
            format!("{place}{severity}: {}", d.message)
        })
        .collect();
    if diagnostics.len() > MAX_PROBLEMS {
        out.push(format!("… and {} more.", diagnostics.len() - MAX_PROBLEMS));
    }
    out.join("\n")
}

/// `text` in a fence longer than any backtick run inside it.
fn fenced(text: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for c in text.chars() {
        run = if c == '`' { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    let fence = "`".repeat(longest.max(2) + 1);
    format!("{fence}latex\n{}\n{fence}", text.trim_end_matches('\n'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn diag(severity: Severity, file: PathBuf, line: u32, message: &str) -> Diagnostic {
        Diagnostic {
            severity,
            file: Some(file),
            line: Some(line),
            message: message.into(),
        }
    }

    #[test]
    fn fix_errors_lists_errors_relative_to_the_project() {
        // Joined like the app's paths, so that the separators are the
        // platform's (`strip_prefix` keeps those of the original).
        let dir = Path::new("/p");
        let ds = [
            diag(
                Severity::Warning,
                dir.join("main.tex"),
                3,
                "Reference `x' undefined",
            ),
            diag(
                Severity::Error,
                dir.join("ch").join("a.tex"),
                12,
                "Undefined control sequence",
            ),
        ];
        let p = fix_errors(dir, &ds);
        assert!(p.starts_with("The document fails to build."));
        let a = Path::new("ch").join("a.tex");
        assert!(p.contains(&format!(
            "{}:12: error: Undefined control sequence",
            a.display()
        )));
        assert!(
            !p.contains("undefined\n"),
            "warnings are left out when there are errors"
        );

        let p = fix_errors(dir, &ds[..1]);
        assert!(p.starts_with("The document builds with warnings."));
        assert!(p.contains("main.tex:3: warning:"));
    }

    #[test]
    fn rewrite_quotes_the_passage() {
        let p = rewrite(
            Path::new("main.tex"),
            4,
            6,
            "Code: ```x```\n",
            &Rewrite::Custom("Translate it into Italian.".into()),
        );
        assert!(p.contains("lines 4–6 of `main.tex`"));
        assert!(p.contains("````latex\nCode: ```x```\n````"));
        assert!(p.contains("Translate it into Italian."));
        assert!(rewrite(Path::new("a.tex"), 2, 2, "x", &Rewrite::Shorten).contains("line 2 of"));
    }

    #[test]
    fn preamble_names_the_root_file() {
        let p = preamble(Path::new("/p/thesis.tex"), Path::new("/p"));
        assert!(p.contains("root file is `thesis.tex`"));
        assert!(p.contains("`build` tool"));
    }
}
