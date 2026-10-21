//! What is under the cursor, where it is defined, and what to show about it
//! on hover.

use std::fmt::Write as _;
use std::ops::Range;
use std::path::{Path, PathBuf};

use tex_project::{Project, resolve_bib, resolve_graphics, resolve_tex, source_path};

use crate::{DB, PathKind, in_comment, path_kind, unclosed_brace};
use tex_syntax::{is_cite_command, is_ref_command};

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
    let place =
        |file: &Path, line: u32| format!("{}:{}", escape(&source_path(dir, file)), line + 1);
    let mut out = String::new();
    match symbol {
        Symbol::Reference { name, .. } => {
            let labels = project.labels();
            let Some(l) = labels.iter().find(|l| &l.label.name == name) else {
                let label = code(&format!("\\label{{{name}}}"));
                return Some(format!("No {label} in the project."));
            };
            let _ = write!(out, "**Label** {}", code(name));
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
                "**Label** {}\n\nReferenced {}.",
                code(name),
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
                    format!("**Citation** {} (`\\bibitem`)", code(key))
                } else {
                    format!("No entry {} in the project's bibliography.", code(key))
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
            let entry = code(&format!("@{}{{{key}}}", e.kind));
            let _ = write!(out, "{entry} — {}", escape(&source_path(dir, file)));
        }
        Symbol::Path { path, kind, .. } => {
            let found = match definition(project, symbol) {
                Some(Target::Source { file, .. }) | Some(Target::External(file)) => Some(file),
                None => None,
            };
            match found {
                Some(file) => {
                    let size = std::fs::metadata(&file).map(|m| m.len()).unwrap_or(0);
                    let _ = write!(
                        out,
                        "{} — {}",
                        code(&source_path(dir, &file)),
                        human_size(size)
                    );
                    if *kind == PathKind::Graphics {
                        out.push_str("\n\n⌘-click to open it.");
                    }
                }
                None => {
                    let _ = write!(out, "{} was not found.", code(path));
                }
            }
        }
        Symbol::Package { name, .. } => {
            let _ = write!(out, "**Package** {}", code(name));
            // Only a real package name goes into a link (a project's text could
            // otherwise point it anywhere).
            let plain = name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
            if plain && !name.is_empty() {
                let _ = write!(
                    out,
                    "\n\n[Documentation on texdoc.org](https://texdoc.org/pkg/{name})"
                );
            }
        }
        Symbol::Environment { name, .. } => {
            let file = project.indexes().find_map(|(file, i)| {
                i.environments
                    .iter()
                    .any(|e| &e.name == name)
                    .then_some(file)
            });
            if let Some(file) = file {
                let _ = write!(
                    out,
                    "**Environment** {} — {}",
                    code(name),
                    escape(&source_path(dir, file))
                );
            } else {
                let e = DB.environments.iter().find(|e| &e.name == name)?;
                let _ = write!(out, "**Environment** {} — {}", code(name), e.detail);
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
                let _ = write!(out, "{}{args} — {}", code(name), place(file, c.line));
            } else {
                let c = DB.commands.iter().find(|c| &c.name == name)?;
                let _ = write!(out, "{} — {}", code(name), c.detail);
            }
        }
    }
    Some(out)
}

fn human_size(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / (1 << 20) as f64),
        b if b >= 1 << 10 => format!("{} KB", b >> 10),
        b => format!("{b} bytes"),
    }
}

/// `s` as a Markdown code span, whatever backticks it holds.
fn code(s: &str) -> String {
    let longest = s.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(longest + 1);
    // A space keeps a backtick at either end apart from the fence.
    let pad = if s.starts_with('`') || s.ends_with('`') {
        " "
    } else {
        ""
    };
    format!("{fence}{pad}{s}{pad}{fence}")
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
    use crate::temp_project;
    use proptest::prelude::*;
    use test_support::at;

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

    #[test]
    fn hovers_escape_names_and_paths() {
        let (_dir, p) = temp_project(&[("main.tex", "\\input{x_y_}"), ("x_y_.tex", "\\label{a}")]);
        let hover = |s: Symbol| hover(&p, &s).unwrap();
        let range = 0..0;
        // A code span with a backtick inside.
        let label = hover(Symbol::Label {
            name: "a`b".into(),
            range: range.clone(),
        });
        assert!(label.starts_with("**Label** ``a`b``"), "{label}");
        // A file name in plain text, not emphasis.
        let reference = hover(Symbol::Reference {
            name: "a".into(),
            range: range.clone(),
        });
        assert!(reference.ends_with("x\\_y\\_.tex:1"), "{reference}");
        // A name that is no package gets no link.
        let package = hover(Symbol::Package {
            name: "x](https://evil.example)[y".into(),
            range: range.clone(),
        });
        assert_eq!(package, "**Package** `x](https://evil.example)[y`");
        let package = hover(Symbol::Package {
            name: "tikz-cd".into(),
            range,
        });
        assert!(
            package.ends_with("(https://texdoc.org/pkg/tikz-cd)"),
            "{package}"
        );
    }

    fn sample() -> Project {
        Project::open(test_support::fixtures().join("sample/main.tex"))
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

    #[test]
    fn symbol_edge_cases() {
        assert_eq!(
            sym("\\label{ sec:a|b }"),
            Some(Symbol::Label {
                name: "sec:ab".into(),
                range: 8..14
            })
        );
        assert!(matches!(
            sym("\\end{ite|mize}"),
            Some(Symbol::Environment { name, .. }) if name == "itemize"
        ));
        assert!(matches!(
            sym("\\RequirePackage[x]{tik|z}"),
            Some(Symbol::Package { name, .. }) if name == "tikz"
        ));
        assert!(matches!(
            sym("\\ref*{a|}"),
            Some(Symbol::Reference { name, .. }) if name == "a"
        ));
        assert!(matches!(
            sym("\\bibliography{a,b|c}"),
            Some(Symbol::Path { path, kind: PathKind::Bib, range }) if path == "bc" && range == (16..18)
        ));
        // At either end of a command name.
        assert!(matches!(sym("|\\alpha"), Some(Symbol::Command { name, .. }) if name == "\\alpha"));
        assert!(matches!(sym("\\alpha|"), Some(Symbol::Command { name, .. }) if name == "\\alpha"));
        assert_eq!(sym("\\alpha |x"), None);
        assert_eq!(sym("\\|"), None);
        // Empty items, unknown commands, groups without a command.
        assert_eq!(sym("\\cite{a,|,b}"), None);
        assert_eq!(sym("\\cite{|}"), None);
        assert_eq!(sym("\\textit{x}{a|}"), None);
        assert_eq!(sym("{a|}"), None);
        assert_eq!(sym("\\x1{a|}"), None);
        // The argument must close on the same line, whatever its ending.
        assert_eq!(sym("\\ref{a|\nb}"), None);
        assert!(matches!(
            sym("x\r\n\\ref{a|}\r\n"),
            Some(Symbol::Reference { range, .. }) if range == (8..9)
        ));
        // An offset past the end is the end; one inside a character is nothing.
        assert!(matches!(
            symbol_at("\\alpha", 100),
            Some(Symbol::Command { range, .. }) if range == (0..6)
        ));
        assert_eq!(symbol_at("\\ref{é}", 6), None);
        assert_eq!(symbol_at("", 0), None);
    }

    #[test]
    fn symbol_ranges() {
        let all = [
            Symbol::Reference {
                name: "a".into(),
                range: 1..2,
            },
            Symbol::Citation {
                key: "a".into(),
                range: 1..2,
            },
            Symbol::Label {
                name: "a".into(),
                range: 1..2,
            },
            Symbol::Path {
                path: "a".into(),
                kind: PathKind::Tex,
                range: 1..2,
            },
            Symbol::Package {
                name: "a".into(),
                range: 1..2,
            },
            Symbol::Environment {
                name: "a".into(),
                range: 1..2,
            },
            Symbol::Command {
                name: "a".into(),
                range: 1..2,
            },
        ];
        assert!(all.iter().all(|s| s.range() == (1..2)));
    }

    const MAIN: &str = "\\documentclass{article}\n\\usepackage{graphicx}\n\
        \\graphicspath{{img/}}\n\\newcommand{\\one}[1]{#1}\n\\newcommand{\\two}[2]{#1#2}\n\
        \\newcommand{\\zero}{z}\n\\newenvironment{box}{}{}\n\\begin{document}\n\
        \\section{Intro}\\label{sec:a}\n\\ref{sec:a} \\ref{sec:a} \\ref{sec:b}\n\
        \\input{part}\n\\includegraphics{plot}\n\\bibliography{refs,missing}\n\
        \\begin{thebibliography}{9}\n\\bibitem{old} Old.\n\\end{thebibliography}\n\
        \\end{document}\n";

    fn project() -> (tempfile::TempDir, Project) {
        temp_project(&[
            ("main.tex", MAIN),
            (
                "part.tex",
                "\\label{sec:b}\n\\label{sec:unused}\n\\cref{sec:b}",
            ),
            ("img/plot.png", &"x".repeat(2048)),
            (
                "refs.bib",
                "@article{full, title={On *Stars*}, author={A and B}, journal={J_1}, year={2001}}\n\
                 @misc{bare, note={x}}\n",
            ),
        ])
    }

    fn reference(name: &str) -> Symbol {
        Symbol::Reference {
            name: name.into(),
            range: 0..0,
        }
    }

    fn label(name: &str) -> Symbol {
        Symbol::Label {
            name: name.into(),
            range: 0..0,
        }
    }

    fn citation(key: &str) -> Symbol {
        Symbol::Citation {
            key: key.into(),
            range: 0..0,
        }
    }

    fn path(path: &str, kind: PathKind) -> Symbol {
        Symbol::Path {
            path: path.into(),
            kind,
            range: 0..0,
        }
    }

    fn command(name: &str) -> Symbol {
        Symbol::Command {
            name: name.into(),
            range: 0..0,
        }
    }

    fn environment(name: &str) -> Symbol {
        Symbol::Environment {
            name: name.into(),
            range: 0..0,
        }
    }

    /// The text a definition points to.
    fn target_text(target: Option<Target>) -> String {
        match target {
            Some(Target::Source { file, range }) => {
                let text = std::fs::read_to_string(file).unwrap();
                text[range].to_string()
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn definitions_of_every_kind() {
        let (dir, p) = project();
        assert_eq!(target_text(definition(&p, &reference("sec:b"))), "sec:b");
        assert_eq!(definition(&p, &reference("nowhere")), None);
        assert_eq!(target_text(definition(&p, &citation("full"))), "full");
        // A `\bibitem` when no bibliography has the key.
        assert_eq!(target_text(definition(&p, &citation("old"))), "old");
        assert_eq!(definition(&p, &citation("nokey")), None);
        assert_eq!(target_text(definition(&p, &command("\\two"))), "\\two");
        assert_eq!(definition(&p, &command("\\alpha")), None);
        assert_eq!(target_text(definition(&p, &environment("box"))), "box");
        assert_eq!(definition(&p, &environment("itemize")), None);

        assert_eq!(
            definition(&p, &path("refs", PathKind::Bib)),
            Some(Target::Source {
                file: dir.path().join("refs.bib"),
                range: 0..0
            })
        );
        assert!(matches!(
            definition(&p, &path("refs.bib", PathKind::BibResource)),
            Some(Target::Source { file, .. }) if file == dir.path().join("refs.bib")
        ));
        assert_eq!(definition(&p, &path("missing", PathKind::Bib)), None);
        assert_eq!(
            target_text(definition(&p, &path("part", PathKind::Tex))),
            ""
        );
        // Pictures through `\graphicspath`, opened with another app.
        assert_eq!(
            definition(&p, &path("plot", PathKind::Graphics)),
            Some(Target::External(dir.path().join("img/plot.png")))
        );
        assert_eq!(definition(&p, &path("nope", PathKind::Graphics)), None);
        assert_eq!(definition(&p, &label("sec:a")), None);
        let package = Symbol::Package {
            name: "graphicx".into(),
            range: 0..0,
        };
        assert_eq!(definition(&p, &package), None);
    }

    #[test]
    fn hovers_of_every_kind() {
        let (_dir, p) = project();
        let hover = |s: Symbol| hover(&p, &s);
        assert_eq!(
            hover(reference("sec:a")).as_deref(),
            Some("**Label** `sec:a` — Intro\n\nmain.tex:9")
        );
        assert_eq!(
            hover(reference("nowhere")).as_deref(),
            Some("No `\\label{nowhere}` in the project.")
        );
        // Labels count their references across files.
        let uses = |name: &str| hover(label(name)).unwrap();
        assert_eq!(uses("sec:a"), "**Label** `sec:a`\n\nReferenced 2 times.");
        assert_eq!(uses("sec:b"), "**Label** `sec:b`\n\nReferenced 2 times.");
        assert_eq!(
            uses("sec:unused"),
            "**Label** `sec:unused`\n\nReferenced nowhere."
        );
        let (_d, single) = temp_project(&[("main.tex", "\\label{x}\\ref{x}")]);
        assert_eq!(
            super::hover(&single, &label("x")).unwrap(),
            "**Label** `x`\n\nReferenced once."
        );

        // Citations: Markdown is escaped, the venue is in italics.
        assert_eq!(
            hover(citation("full")).unwrap(),
            "**On \\*Stars\\***\n\nA, B\n\n*J\\_1*, 2001\n\n`@article{full}` — refs.bib"
        );
        assert_eq!(hover(citation("bare")).unwrap(), "`@misc{bare}` — refs.bib");
        assert_eq!(
            hover(citation("old")).as_deref(),
            Some("**Citation** `old` (`\\bibitem`)")
        );
        assert_eq!(
            hover(citation("nokey")).as_deref(),
            Some("No entry `nokey` in the project's bibliography.")
        );

        assert_eq!(
            hover(path("plot", PathKind::Graphics)).as_deref(),
            Some("`img/plot.png` — 2 KB\n\n⌘-click to open it.")
        );
        assert!(
            hover(path("part", PathKind::Tex))
                .unwrap()
                .starts_with("`part.tex` — ")
        );
        assert_eq!(
            hover(path("gone", PathKind::Tex)).as_deref(),
            Some("`gone` was not found.")
        );
        let package = Symbol::Package {
            name: "graphicx".into(),
            range: 0..0,
        };
        assert_eq!(
            hover(package).as_deref(),
            Some(
                "**Package** `graphicx`\n\n[Documentation on texdoc.org](https://texdoc.org/pkg/graphicx)"
            )
        );

        // User definitions first, then the database, else nothing.
        assert_eq!(
            hover(environment("box")).as_deref(),
            Some("**Environment** `box` — main.tex")
        );
        assert!(
            hover(environment("itemize"))
                .unwrap()
                .starts_with("**Environment** `itemize` — ")
        );
        assert_eq!(hover(environment("nosuchenv")), None);
        assert_eq!(
            hover(command("\\zero")).as_deref(),
            Some("`\\zero` — main.tex:6")
        );
        assert_eq!(
            hover(command("\\one")).as_deref(),
            Some("`\\one`, 1 argument — main.tex:4")
        );
        assert_eq!(
            hover(command("\\two")).as_deref(),
            Some("`\\two`, 2 arguments — main.tex:5")
        );
        assert!(
            hover(command("\\section"))
                .unwrap()
                .starts_with("`\\section` — ")
        );
        assert_eq!(hover(command("\\nosuchcommand")), None);
    }

    #[test]
    fn sizes_and_escapes() {
        assert_eq!(human_size(0), "0 bytes");
        assert_eq!(human_size(1023), "1023 bytes");
        assert_eq!(human_size(1024), "1 KB");
        assert_eq!(human_size((1 << 20) - 1), "1023 KB");
        assert_eq!(human_size(1 << 20), "1.0 MB");
        assert_eq!(human_size(3 << 19), "1.5 MB");
        assert_eq!(
            escape("a*b_c`d[e]f<g>h#i\\j"),
            "a\\*b\\_c\\`d\\[e\\]f\\<g\\>h\\#i\\\\j"
        );
        assert_eq!(escape("plain é"), "plain é");
        let dir = Path::new("/p");
        assert_eq!(
            source_path(dir, Path::new("/elsewhere/x.tex")),
            Path::new("/elsewhere/x.tex").display().to_string()
        );
    }

    fn naive_closing_brace(s: &str) -> Option<usize> {
        s.match_indices('}').map(|(i, _)| i).find(|&i| {
            let before = &s[..i];
            before.matches('{').count() == before.matches('}').count()
                && (0..i)
                    .filter(|&k| s.is_char_boundary(k))
                    .all(|k| s[..k].matches('{').count() >= s[..k].matches('}').count())
        })
    }

    proptest! {
        #[test]
        fn symbols_name_their_range(text in "[a-z\\\\{}\\[\\],%* \n\r:é]{0,40}", at in 0usize..48) {
            let offset = at.min(text.len());
            prop_assume!(text.is_char_boundary(offset));
            if let Some(symbol) = symbol_at(&text, offset) {
                let range = symbol.range();
                let name = match &symbol {
                    Symbol::Reference { name, .. }
                    | Symbol::Label { name, .. }
                    | Symbol::Package { name, .. }
                    | Symbol::Environment { name, .. }
                    | Symbol::Command { name, .. } => name,
                    Symbol::Citation { key, .. } => key,
                    Symbol::Path { path, .. } => path,
                };
                prop_assert_eq!(&text[range.clone()], name.as_str());
                // On the offset's line.
                prop_assert!(!text[range.start.min(offset)..range.end.max(offset)].contains('\n'));
            }
        }

        #[test]
        fn closing_brace_matches_a_naive_search(s in "[a{}é]{0,30}") {
            prop_assert_eq!(closing_brace(&s), naive_closing_brace(&s));
        }
    }
}
