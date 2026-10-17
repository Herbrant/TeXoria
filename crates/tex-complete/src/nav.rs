//! What is under the cursor, where it is defined, and what to show about it
//! on hover.

use std::fmt::Write as _;
use std::ops::Range;
use std::path::{Path, PathBuf};

use tex_project::{Project, resolve_bib, resolve_graphics, resolve_tex};

use crate::{DB, PathKind, in_comment, is_cite_command, is_ref_command, path_kind, unclosed_brace};

/// A name in the source, with the bytes it spans.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Symbol {
    /// A label in `\ref{…}` and friends.
    Reference { name: String, range: Range<usize> },
    /// A key in `\cite{…}` and friends.
    Citation { key: String, range: Range<usize> },
    /// The name in `\label{…}`.
    Label { name: String, range: Range<usize> },
    /// A path in `\input{…}`, `\includegraphics{…}`, `\bibliography{…}`…
    Path {
        path: String,
        kind: PathKind,
        range: Range<usize>,
    },
    /// A package in `\usepackage{…}`.
    Package { name: String, range: Range<usize> },
    /// The name in `\begin{…}` or `\end{…}`.
    Environment { name: String, range: Range<usize> },
    /// A command name, backslash included.
    Command { name: String, range: Range<usize> },
}

impl Symbol {
    pub fn range(&self) -> Range<usize> {
        match self {
            Symbol::Reference { range, .. }
            | Symbol::Citation { range, .. }
            | Symbol::Label { range, .. }
            | Symbol::Path { range, .. }
            | Symbol::Package { range, .. }
            | Symbol::Environment { range, .. }
            | Symbol::Command { range, .. } => range.clone(),
        }
    }
}

/// The symbol at byte `offset` of `text`.
pub fn symbol_at(text: &str, offset: usize) -> Option<Symbol> {
    let offset = offset.min(text.len());
    if !text.is_char_boundary(offset) {
        return None;
    }
    let line_start = text[..offset].rfind('\n').map_or(0, |i| i + 1);
    let line_end = text[offset..].find('\n').map_or(text.len(), |i| offset + i);
    let before = &text[line_start..offset];
    if in_comment(before) {
        return None;
    }
    if let Some(sym) = command_at(text, offset, line_start, line_end) {
        return Some(sym);
    }
    let open = line_start + unclosed_brace(before)?;
    let close = closing_brace(&text[offset..line_end]).map(|i| offset + i)?;
    let head = text[line_start..open].trim_end();
    let head = if head.ends_with(']') {
        head.rfind('[').map_or(head, |i| &head[..i])
    } else {
        head
    };
    let cmd_start = head.rfind('\\')?;
    let cmd = head[cmd_start..].trim_end_matches('*');
    if !cmd[1..].chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    let path = path_kind(cmd);
    let list = !matches!(path, Some((_, false)));
    // The comma-separated item around the offset.
    let arg = open + 1..close;
    let (mut start, mut end) = (arg.start, arg.end);
    if list {
        if let Some(i) = text[arg.start..offset].rfind(',') {
            start = arg.start + i + 1;
        }
        if let Some(i) = text[offset..arg.end].find(',') {
            end = offset + i;
        }
    }
    let raw = &text[start..end];
    let name = raw.trim();
    if name.is_empty() {
        return None;
    }
    let start = start + (raw.len() - raw.trim_start().len());
    let range = start..start + name.len();
    let name = name.to_string();
    if let Some((kind, _)) = path {
        return Some(Symbol::Path {
            path: name,
            kind,
            range,
        });
    }
    Some(match cmd {
        "\\begin" | "\\end" => Symbol::Environment { name, range },
        "\\label" => Symbol::Label { name, range },
        "\\usepackage" | "\\RequirePackage" => Symbol::Package { name, range },
        c if is_ref_command(c) => Symbol::Reference { name, range },
        c if is_cite_command(c) => Symbol::Citation { key: name, range },
        _ => return None,
    })
}

/// A `\name` the offset is on (between its backslash and its end).
fn command_at(text: &str, offset: usize, line_start: usize, line_end: usize) -> Option<Symbol> {
    let b = text.as_bytes();
    let mut start = offset;
    while start > line_start && b[start - 1].is_ascii_alphabetic() {
        start -= 1;
    }
    let bs = if b.get(start) == Some(&b'\\') {
        start
    } else if start > line_start && b[start - 1] == b'\\' {
        start - 1
    } else {
        return None;
    };
    // `\\` is a line break, not a command.
    if bs > line_start && b[bs - 1] == b'\\' {
        return None;
    }
    let mut end = bs + 1;
    while end < line_end && b[end].is_ascii_alphabetic() {
        end += 1;
    }
    if end == bs + 1 || offset > end {
        return None;
    }
    Some(Symbol::Command {
        name: text[bs..end].to_string(),
        range: bs..end,
    })
}

/// Index of the `}` closing a group the text starts inside of.
fn closing_brace(s: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (i, c) in s.char_indices() {
        match c {
            '{' => depth += 1,
            '}' if depth == 0 => return Some(i),
            '}' => depth -= 1,
            _ => {}
        }
    }
    None
}

/// Where a definition is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// Bytes of a project file (a `.tex` or `.bib`).
    Source { file: PathBuf, range: Range<usize> },
    /// A file to open with another app (a picture).
    External(PathBuf),
}

/// Where `symbol` is defined in the project.
pub fn definition(project: &Project, symbol: &Symbol) -> Option<Target> {
    let dir = project.root_dir();
    match symbol {
        Symbol::Reference { name, .. } => project
            .labels()
            .into_iter()
            .find(|l| &l.label.name == name)
            .map(|l| Target::Source {
                file: l.file,
                range: l.label.range,
            }),
        Symbol::Citation { key, .. } => project
            .bib_files()
            .find_map(|(file, entries)| {
                let e = entries.iter().find(|e| &e.key == key)?;
                Some(Target::Source {
                    file: file.to_path_buf(),
                    range: e.key_range.clone(),
                })
            })
            .or_else(|| {
                project.indexes().find_map(|(file, index)| {
                    let item = index.bibitems.iter().find(|b| &b.name == key)?;
                    Some(Target::Source {
                        file: file.to_path_buf(),
                        range: item.range.clone(),
                    })
                })
            }),
        Symbol::Path { path, kind, .. } => {
            let file = match kind {
                PathKind::Tex => resolve_tex(dir, path),
                PathKind::Bib | PathKind::BibResource => resolve_bib(dir, path),
                PathKind::Graphics => {
                    return resolve_graphics(dir, &project.graphics_paths(), path)
                        .map(Target::External);
                }
            };
            file.is_file()
                .then_some(Target::Source { file, range: 0..0 })
        }
        Symbol::Command { name, .. } => project.indexes().find_map(|(file, index)| {
            let c = index.commands.iter().find(|c| &c.name == name)?;
            Some(Target::Source {
                file: file.to_path_buf(),
                range: c.range.clone(),
            })
        }),
        Symbol::Environment { name, .. } => project.indexes().find_map(|(file, index)| {
            let e = index.environments.iter().find(|e| &e.name == name)?;
            Some(Target::Source {
                file: file.to_path_buf(),
                range: e.range.clone(),
            })
        }),
        Symbol::Label { .. } | Symbol::Package { .. } => None,
    }
}

/// What to show about `symbol` on hover, as Markdown.
pub fn hover(project: &Project, symbol: &Symbol) -> Option<String> {
    let dir = project.root_dir();
    let place = |file: &Path, line: u32| format!("{}:{}", relative(dir, file), line + 1);
    let mut out = String::new();
    match symbol {
        Symbol::Reference { name, .. } => {
            let labels = project.labels();
            let Some(l) = labels.iter().find(|l| &l.label.name == name) else {
                return Some(format!("No `\\label{{{name}}}` in the project."));
            };
            let _ = write!(out, "**Label** `{name}`");
            if let Some(ctx) = &l.label.context {
                let _ = write!(out, " — {ctx}");
            }
            let _ = write!(out, "\n\n{}", place(&l.file, l.label.line));
        }
        Symbol::Label { name, .. } => {
            let uses: usize = project
                .indexes()
                .map(|(_, i)| i.references.iter().filter(|r| &r.name == name).count())
                .sum();
            let _ = write!(
                out,
                "**Label** `{name}`\n\nReferenced {}.",
                match uses {
                    0 => "nowhere".to_string(),
                    1 => "once".to_string(),
                    n => format!("{n} times"),
                }
            );
        }
        Symbol::Citation { key, .. } => {
            let found = project.bib_files().find_map(|(file, entries)| {
                entries.iter().find(|e| &e.key == key).map(|e| (file, e))
            });
            let Some((file, e)) = found else {
                let bibitem = project
                    .indexes()
                    .any(|(_, i)| i.bibitems.iter().any(|b| &b.name == key));
                return Some(if bibitem {
                    format!("**Citation** `{key}` (`\\bibitem`)")
                } else {
                    format!("No entry `{key}` in the project's bibliography.")
                });
            };
            if let Some(title) = &e.title {
                let _ = writeln!(out, "**{}**\n", escape(title));
            }
            if let Some(author) = &e.author {
                let _ = writeln!(out, "{}\n", escape(&author.replace(" and ", ", ")));
            }
            let venue = [
                "journal",
                "journaltitle",
                "booktitle",
                "publisher",
                "school",
            ]
            .iter()
            .find_map(|f| e.field(f));
            let mut tail: Vec<String> = venue
                .map(|v| format!("*{}*", escape(v)))
                .into_iter()
                .collect();
            tail.extend(e.year.clone());
            if !tail.is_empty() {
                let _ = writeln!(out, "{}\n", tail.join(", "));
            }
            let _ = write!(out, "`@{}{{{key}}}` — {}", e.kind, relative(dir, file));
        }
        Symbol::Path { path, kind, .. } => {
            let found = match definition(project, symbol) {
                Some(Target::Source { file, .. }) | Some(Target::External(file)) => Some(file),
                None => None,
            };
            match found {
                Some(file) => {
                    let size = std::fs::metadata(&file).map(|m| m.len()).unwrap_or(0);
                    let _ = write!(out, "`{}` — {}", relative(dir, &file), human_size(size));
                    if *kind == PathKind::Graphics {
                        out.push_str("\n\n⌘-click to open it.");
                    }
                }
                None => {
                    let _ = write!(out, "`{path}` was not found.");
                }
            }
        }
        Symbol::Package { name, .. } => {
            let _ = write!(
                out,
                "**Package** `{name}`\n\n[Documentation on texdoc.org](https://texdoc.org/pkg/{name})"
            );
        }
        Symbol::Environment { name, .. } => {
            let file = project.indexes().find_map(|(file, i)| {
                i.environments
                    .iter()
                    .any(|e| &e.name == name)
                    .then_some(file)
            });
            if let Some(file) = file {
                let _ = write!(out, "**Environment** `{name}` — {}", relative(dir, file));
            } else {
                let e = DB.environments.iter().find(|e| &e.name == name)?;
                let _ = write!(out, "**Environment** `{name}` — {}", e.detail);
            }
        }
        Symbol::Command { name, .. } => {
            if let Some((file, c)) = project.indexes().find_map(|(file, i)| {
                i.commands
                    .iter()
                    .find(|c| &c.name == name)
                    .map(|c| (file, c))
            }) {
                let args = match c.argc {
                    0 => String::new(),
                    1 => ", 1 argument".to_string(),
                    n => format!(", {n} arguments"),
                };
                let _ = write!(out, "`{name}`{args} — {}", place(file, c.line));
            } else {
                let c = DB.commands.iter().find(|c| &c.name == name)?;
                let _ = write!(out, "`{name}` — {}", c.detail);
            }
        }
    }
    Some(out)
}

fn relative(dir: &Path, file: &Path) -> String {
    file.strip_prefix(dir).unwrap_or(file).display().to_string()
}

fn human_size(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / (1 << 20) as f64),
        b if b >= 1 << 10 => format!("{} KB", b >> 10),
        b => format!("{b} bytes"),
    }
}

/// Markdown's special characters, so a title shows as written.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '*' | '_' | '`' | '[' | ']' | '<' | '>' | '#' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(src: &str) -> (String, usize) {
        let o = src.find('|').unwrap();
        (src.replacen('|', "", 1), o)
    }

    fn sym(src: &str) -> Option<Symbol> {
        let (t, o) = at(src);
        symbol_at(&t, o)
    }

    #[test]
    fn finds_symbols() {
        let (t, o) = at("See \\ref{sec:in|tro} now");
        assert_eq!(
            symbol_at(&t, o),
            Some(Symbol::Reference {
                name: "sec:intro".into(),
                range: 9..18
            })
        );
        assert!(matches!(
            sym("\\cite[p.~3]{knuth, lam|port}"),
            Some(Symbol::Citation { key, .. }) if key == "lamport"
        ));
        assert!(matches!(
            sym("\\includegraphics[width=3cm]{fig/a,|b}"),
            Some(Symbol::Path { path, kind: PathKind::Graphics, .. }) if path == "fig/a,b"
        ));
        assert!(matches!(
            sym("\\begin{theo|rem}"),
            Some(Symbol::Environment { name, .. }) if name == "theorem"
        ));
        assert!(matches!(
            sym("\\usepackage{amsmath, hyper|ref}"),
            Some(Symbol::Package { name, .. }) if name == "hyperref"
        ));
        assert!(matches!(
            sym("a \\no|rm{x}"),
            Some(Symbol::Command { name, .. }) if name == "\\norm"
        ));
        assert!(matches!(
            sym("a \\norm|{x}"),
            Some(Symbol::Command { name, .. }) if name == "\\norm"
        ));
        assert_eq!(sym("% \\ref{a|}"), None);
        assert_eq!(sym("\\textbf{bo|ld}"), None);
        assert_eq!(sym("plain te|xt"), None);
        assert_eq!(sym("line\\\\|next"), None);
    }

    fn sample() -> Project {
        let root =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/sample/main.tex");
        Project::open(std::fs::canonicalize(root).unwrap())
    }

    #[test]
    fn finds_definitions() {
        let p = sample();
        let label = definition(
            &p,
            &Symbol::Reference {
                name: "sec:methods".into(),
                range: 0..0,
            },
        );
        let Some(Target::Source { file, range }) = label else {
            panic!("{label:?}")
        };
        let text = std::fs::read_to_string(&file).unwrap();
        assert_eq!(&text[range], "sec:methods");

        let cite = definition(
            &p,
            &Symbol::Citation {
                key: "knuth1984".into(),
                range: 0..0,
            },
        );
        let Some(Target::Source { file, range }) = cite else {
            panic!("{cite:?}")
        };
        assert!(file.extension().is_some_and(|e| e == "bib"));
        assert_eq!(&std::fs::read_to_string(&file).unwrap()[range], "knuth1984");

        let input = definition(
            &p,
            &Symbol::Path {
                path: "chapters/methods".into(),
                kind: PathKind::Tex,
                range: 0..0,
            },
        );
        assert!(
            matches!(input, Some(Target::Source { file, .. }) if file.ends_with("chapters/methods.tex"))
        );

        let hover_text = hover(
            &p,
            &Symbol::Citation {
                key: "knuth1984".into(),
                range: 0..0,
            },
        )
        .unwrap();
        assert!(hover_text.contains("Knuth"), "{hover_text}");
    }
}
