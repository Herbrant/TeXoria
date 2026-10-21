//! Context-aware LaTeX completion: commands, environments, labels,
//! citations, packages and file paths, ranked with fuzzy matching.
//!
//! The output is editor-agnostic: each [`Item`] says which byte range of the
//! document to replace, the text to insert and where the cursor should end.

use std::ops::Range;
use std::path::Path;
use std::sync::LazyLock;

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use serde::Deserialize;
use tex_project::Project;
use tex_syntax::{is_cite_command, is_ref_command};

pub mod nav;

const MAX_ITEMS: usize = 100;
/// Entries read from a folder for path completion, as a guard against huge ones.
const MAX_DIR_ENTRIES: usize = 2000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ItemKind {
    Command,
    UserCommand,
    Environment,
    Label,
    Citation,
    Package,
    File,
    Folder,
}

/// The files a path argument expects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathKind {
    /// `\input`, `\include`: `.tex` files, written without the extension.
    Tex,
    /// `\includegraphics`: pictures and PDFs, with their extension.
    Graphics,
    /// `\bibliography`: `.bib` files, without the extension.
    Bib,
    /// `\addbibresource`: `.bib` files, with the extension.
    BibResource,
}

impl PathKind {
    fn accepts(self, ext: &str) -> bool {
        let ext = ext.to_ascii_lowercase();
        match self {
            PathKind::Tex => ext == "tex",
            PathKind::Graphics => tex_project::GRAPHICS_EXTENSIONS.contains(&ext.as_str()),
            PathKind::Bib | PathKind::BibResource => ext == "bib",
        }
    }

    fn keeps_extension(self) -> bool {
        matches!(self, PathKind::Graphics | PathKind::BibResource)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub kind: ItemKind,
    /// What the menu shows.
    pub label: String,
    pub detail: Option<String>,
    /// Document byte range to replace.
    pub replace: Range<usize>,
    pub insert: String,
    /// Byte offset of the cursor inside `insert` after insertion.
    pub cursor: usize,
}

/// What the user is completing, with the byte range of the typed prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Context {
    /// `\fo|`
    Command { prefix: Range<usize> },
    /// `\begin{ite|`
    Begin { prefix: Range<usize> },
    /// `\end{|`
    End { prefix: Range<usize> },
    /// `\ref{sec:|`, `\cref{a,b|`
    Label { prefix: Range<usize> },
    /// `\cite{knu|`
    Citation { prefix: Range<usize> },
    /// `\usepackage{ams|`
    Package { prefix: Range<usize> },
    /// `\input{chapters/me|`, relative to the root file's folder.
    Path {
        prefix: Range<usize>,
        kind: PathKind,
    },
}

#[derive(Deserialize)]
pub(crate) struct Database {
    pub(crate) commands: Vec<CommandSpec>,
    pub(crate) environments: Vec<EnvSpec>,
    packages: Vec<String>,
}

#[derive(Deserialize)]
pub(crate) struct CommandSpec {
    pub(crate) name: String,
    snippet: String,
    pub(crate) detail: String,
}

#[derive(Deserialize)]
pub(crate) struct EnvSpec {
    pub(crate) name: String,
    body: String,
    pub(crate) detail: String,
}

pub(crate) static DB: LazyLock<Database> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../data/latex.json")).expect("valid latex.json")
});

/// Detect the completion context at byte `offset` of `text`.
pub fn context(text: &str, offset: usize) -> Option<Context> {
    if !text.is_char_boundary(offset) {
        return None;
    }
    let line_start = text[..offset].rfind('\n').map_or(0, |i| i + 1);
    let before = &text[line_start..offset];
    if in_comment(before) {
        return None;
    }

    // Inside a `{...}` argument? Find the innermost unclosed brace.
    if let Some(open) = unclosed_brace(before) {
        let cmd_end = open;
        // Skip an optional `[...]` between command and brace (`\cite[p.~3]{`).
        let head = before[..cmd_end].trim_end();
        let head = if head.ends_with(']') {
            head.rfind('[').map_or(head, |i| &head[..i])
        } else {
            head
        };
        let cmd_start = head.rfind('\\')?;
        let cmd = &head[cmd_start..];
        if !cmd[1..]
            .chars()
            .all(|c| c.is_ascii_alphabetic() || c == '*')
        {
            return None;
        }
        let base = cmd.trim_end_matches('*');
        let path = path_kind(base);
        // For lists (`\cite{a,b|`) only the last element is the prefix; a
        // file name may contain commas, unless the command takes a list.
        let arg = &before[open + 1..];
        let list = !matches!(path, Some((_, false)));
        let item_start = if list {
            arg.rfind(',').map_or(0, |i| i + 1)
        } else {
            0
        };
        let item_start =
            item_start + (arg[item_start..].len() - arg[item_start..].trim_start().len());
        let prefix = line_start + open + 1 + item_start..offset;
        if let Some((kind, _)) = path {
            return Some(Context::Path { prefix, kind });
        }
        return match base {
            "\\begin" => Some(Context::Begin { prefix }),
            "\\end" => Some(Context::End { prefix }),
            "\\usepackage" | "\\RequirePackage" => Some(Context::Package { prefix }),
            c if is_ref_command(c) => Some(Context::Label { prefix }),
            c if is_cite_command(c) => Some(Context::Citation { prefix }),
            _ => None,
        };
    }

    // `\name` being typed.
    let bs = before.rfind('\\')?;
    let word = &before[bs + 1..];
    if word.chars().all(|c| c.is_ascii_alphabetic() || c == '*') {
        return Some(Context::Command {
            prefix: line_start + bs..offset,
        });
    }
    None
}

pub(crate) fn in_comment(before: &str) -> bool {
    comment_start(before).is_some()
}

/// Where the `%` comment of `line` starts, if it has one: `\%` is a percent
/// sign, but `\\%` a line break and a comment.
pub(crate) fn comment_start(line: &str) -> Option<usize> {
    let b = line.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 2,
            b'%' => return Some(i),
            _ => i += 1,
        }
    }
    None
}

pub(crate) fn unclosed_brace(s: &str) -> Option<usize> {
    let mut depth = 0i32;
    for (i, ch) in s.char_indices().rev() {
        match ch {
            '}' => depth += 1,
            '{' if depth == 0 => return Some(i),
            '{' => depth -= 1,
            _ => {}
        }
    }
    None
}

/// The files a command's argument names, and whether it is a list.
pub(crate) fn path_kind(cmd: &str) -> Option<(PathKind, bool)> {
    Some(match cmd {
        "\\input" | "\\include" | "\\subfile" | "\\subfileinclude" => (PathKind::Tex, false),
        "\\includeonly" => (PathKind::Tex, true),
        "\\includegraphics" | "\\includepdf" | "\\includesvg" => (PathKind::Graphics, false),
        "\\bibliography" => (PathKind::Bib, true),
        "\\addbibresource" | "\\addglobalbib" => (PathKind::BibResource, false),
        _ => return None,
    })
}

/// Compute completions at `offset`. `indent` is one indentation unit.
pub fn complete(text: &str, offset: usize, project: Option<&Project>, indent: &str) -> Vec<Item> {
    let Some(ctx) = context(text, offset) else {
        return Vec::new();
    };
    if let Context::Path { prefix, kind } = &ctx {
        return match project {
            Some(p) => complete_path(p.root_dir(), &text[prefix.clone()], prefix.clone(), *kind),
            None => Vec::new(),
        };
    }
    // If an auto-inserted `}` follows the cursor, swallow it.
    let closing = text[offset..].starts_with('}');

    let mut candidates: Vec<(String, Item)> = Vec::new();
    match &ctx {
        Context::Command { prefix } => {
            for c in &DB.commands {
                let (insert, cursor) = expand_snippet(&c.snippet);
                candidates.push((
                    c.name.clone(),
                    Item {
                        kind: ItemKind::Command,
                        label: c.name.clone(),
                        detail: Some(c.detail.clone()),
                        replace: prefix.clone(),
                        insert,
                        cursor,
                    },
                ));
            }
            if let Some(p) = project {
                let mut seen = std::collections::HashSet::new();
                for uc in p.commands() {
                    if !seen.insert(uc.name.clone()) {
                        continue;
                    }
                    let snippet = format!("{}{}", uc.name, "{$1}".repeat(uc.argc as usize));
                    let (insert, cursor) = expand_snippet(&snippet);
                    let detail = if uc.argc > 0 {
                        format!("User command [{}]", uc.argc)
                    } else {
                        "User command".into()
                    };
                    candidates.push((
                        uc.name.clone(),
                        Item {
                            kind: ItemKind::UserCommand,
                            label: uc.name.clone(),
                            detail: Some(detail),
                            replace: prefix.clone(),
                            insert,
                            cursor,
                        },
                    ));
                }
            }
        }
        Context::Begin { prefix } => {
            let line_start = text[..offset].rfind('\n').map_or(0, |i| i + 1);
            let base_indent: String = text[line_start..]
                .chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .collect();
            let replace = prefix.start..offset + closing as usize;
            let mut envs: Vec<(&str, &str, &str)> = DB
                .environments
                .iter()
                .map(|e| (e.name.as_str(), e.body.as_str(), e.detail.as_str()))
                .collect();
            if let Some(p) = project {
                for name in p.environments() {
                    if !envs.iter().any(|(n, _, _)| *n == name) {
                        envs.push((name, "$0", "User environment"));
                    }
                }
            }
            for (name, body, detail) in envs {
                let inner = format!("{base_indent}{indent}");
                let body = body.replace('\n', &format!("\n{inner}"));
                let snippet = format!("{name}}}\n{inner}{body}\n{base_indent}\\end{{{name}}}");
                let (insert, cursor) = expand_snippet(&snippet);
                candidates.push((
                    name.to_string(),
                    Item {
                        kind: ItemKind::Environment,
                        label: name.to_string(),
                        detail: Some(detail.to_string()),
                        replace: replace.clone(),
                        insert,
                        cursor,
                    },
                ));
            }
        }
        Context::End { prefix } => {
            let replace = prefix.start..offset + closing as usize;
            // Everything before this `\end` (an offset found by search, so
            // always on a char boundary).
            let before = text[..prefix.start].rfind("\\end").unwrap_or(0);
            if let Some(env) = innermost_open_env(&text[..before]) {
                let insert = format!("{env}}}");
                let cursor = insert.len();
                candidates.push((
                    env.clone(),
                    Item {
                        kind: ItemKind::Environment,
                        label: env,
                        detail: Some("Close environment".into()),
                        replace,
                        insert,
                        cursor,
                    },
                ));
            }
        }
        Context::Label { prefix } => {
            if let Some(p) = project {
                for l in p.labels() {
                    let name = l.label.name;
                    candidates.push((
                        name.clone(),
                        Item {
                            kind: ItemKind::Label,
                            cursor: name.len(),
                            insert: name.clone(),
                            label: name,
                            detail: l.label.context,
                            replace: prefix.clone(),
                        },
                    ));
                }
            }
        }
        Context::Citation { prefix } => {
            if let Some(p) = project {
                for e in p.bib_entries() {
                    let detail = match (&e.author, &e.year, &e.title) {
                        (a, y, Some(t)) => Some(format!(
                            "{}{}{t}",
                            a.as_deref().map(|a| format!("{a} · ")).unwrap_or_default(),
                            y.as_deref().map(|y| format!("{y} · ")).unwrap_or_default()
                        )),
                        _ => None,
                    };
                    // Match on title/author too: people remember those, not keys.
                    let filter = format!(
                        "{} {} {}",
                        e.key,
                        e.title.as_deref().unwrap_or(""),
                        e.author.as_deref().unwrap_or("")
                    );
                    candidates.push((
                        filter,
                        Item {
                            kind: ItemKind::Citation,
                            label: e.key.clone(),
                            detail,
                            replace: prefix.clone(),
                            insert: e.key.clone(),
                            cursor: e.key.len(),
                        },
                    ));
                }
            }
        }
        Context::Package { prefix } => {
            for name in &DB.packages {
                candidates.push((
                    name.clone(),
                    Item {
                        kind: ItemKind::Package,
                        label: name.clone(),
                        detail: None,
                        replace: prefix.clone(),
                        insert: name.clone(),
                        cursor: name.len(),
                    },
                ));
            }
        }
        // Handled above.
        Context::Path { .. } => {}
    }

    let typed = match &ctx {
        Context::Command { prefix } => &text[prefix.clone()],
        Context::Begin { prefix }
        | Context::End { prefix }
        | Context::Label { prefix }
        | Context::Citation { prefix }
        | Context::Package { prefix }
        | Context::Path { prefix, .. } => &text[prefix.clone()],
    };
    rank(typed, candidates)
}

/// The folders and files of `kind` in the folder that `typed` points into,
/// relative to `root_dir` (TeX's working directory).
fn complete_path(root_dir: &Path, typed: &str, replace: Range<usize>, kind: PathKind) -> Vec<Item> {
    let (folder, name) = match typed.rfind('/') {
        Some(i) => (&typed[..=i], &typed[i + 1..]),
        None => ("", typed),
    };
    let dir = if Path::new(folder).is_absolute() {
        Path::new(folder).to_path_buf()
    } else {
        root_dir.join(folder)
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut candidates = Vec::new();
    for entry in entries.flatten().take(MAX_DIR_ENTRIES) {
        let file_name = entry.file_name().to_string_lossy().into_owned();
        // Hidden files, and TeXoria's build folder.
        if file_name.starts_with('.') {
            continue;
        }
        let is_dir = entry.file_type().is_ok_and(|t| t.is_dir());
        let (label, insert, item_kind) = if is_dir {
            let label = format!("{file_name}/");
            (label.clone(), format!("{folder}{label}"), ItemKind::Folder)
        } else {
            let path = Path::new(&file_name);
            let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
                continue;
            };
            if !kind.accepts(ext) {
                continue;
            }
            let written = if kind.keeps_extension() {
                file_name.clone()
            } else {
                file_name[..file_name.len() - ext.len() - 1].to_string()
            };
            (
                file_name.clone(),
                format!("{folder}{written}"),
                ItemKind::File,
            )
        };
        let cursor = insert.len();
        candidates.push((
            label.clone(),
            Item {
                kind: item_kind,
                label,
                detail: None,
                replace: replace.clone(),
                insert,
                cursor,
            },
        ));
    }
    // Folders first, then by name, before fuzzy ranking keeps that order on ties.
    candidates.sort_by(|a, b| {
        (a.1.kind != ItemKind::Folder, &a.0).cmp(&(b.1.kind != ItemKind::Folder, &b.0))
    });
    rank(name, candidates)
}

fn rank(typed: &str, candidates: Vec<(String, Item)>) -> Vec<Item> {
    if typed.is_empty() || typed == "\\" {
        let mut items: Vec<Item> = candidates.into_iter().map(|(_, i)| i).collect();
        items.truncate(MAX_ITEMS);
        return items;
    }
    let needle = typed.trim_start_matches('\\');
    let pattern = Pattern::parse(needle, CaseMatching::Smart, Normalization::Smart);
    let mut matcher = Matcher::new(Config::DEFAULT);
    let mut buf = Vec::new();
    let mut scored: Vec<(u32, usize, Item)> = candidates
        .into_iter()
        .enumerate()
        .filter_map(|(i, (filter, item))| {
            let hay = filter.trim_start_matches('\\');
            let mut score = pattern.score(Utf32Str::new(hay, &mut buf), &mut matcher)?;
            // Strongly prefer prefix matches: typing `\sec` should put
            // `\section` before `\subsection`.
            if hay.starts_with(needle) {
                score += 1000;
            }
            if hay == needle {
                score += 1000;
            }
            Some((score, i, item))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored
        .into_iter()
        .take(MAX_ITEMS)
        .map(|(_, _, i)| i)
        .collect()
}

/// Expand an LSP-style snippet (`$1`, `${1:default}`, `$0`) into plain text
/// and the cursor position (first tab stop, else `$0`, else the end).
pub fn expand_snippet(snippet: &str) -> (String, usize) {
    let mut out = String::with_capacity(snippet.len());
    let mut first: Option<(u32, usize)> = None;
    let mut zero: Option<usize> = None;
    let b = snippet.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'$' && i + 1 < b.len() {
            let (n, default, next) = if b[i + 1] == b'{' {
                let end = snippet[i..].find('}').map_or(b.len(), |e| i + e);
                let inner = &snippet[i + 2..end];
                let (n, d) = inner.split_once(':').unwrap_or((inner, ""));
                (n.parse::<u32>().ok(), d, end + 1)
            } else {
                let end = snippet[i + 1..]
                    .find(|c: char| !c.is_ascii_digit())
                    .map_or(b.len(), |e| i + 1 + e);
                (snippet[i + 1..end].parse::<u32>().ok(), "", end)
            };
            if let Some(n) = n {
                if n == 0 {
                    zero.get_or_insert(out.len());
                } else if first.is_none_or(|(m, _)| n < m) {
                    first = Some((n, out.len()));
                }
                out.push_str(default);
                i = next;
                continue;
            }
        }
        let ch = snippet[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    let cursor = first.map(|(_, p)| p).or(zero).unwrap_or(out.len());
    (out, cursor)
}

/// The innermost `\begin{x}` without a matching `\end{x}` in `text`.
fn innermost_open_env(text: &str) -> Option<String> {
    let mut stack: Vec<&str> = Vec::new();
    for line in text.split('\n') {
        // Environments in comments neither open nor close.
        let mut rest = &line[..comment_start(line).unwrap_or(line.len())];
        while let Some(i) = rest.find('\\') {
            rest = &rest[i + 1..];
            let (is_begin, tail) = if let Some(t) = rest.strip_prefix("begin{") {
                (true, t)
            } else if let Some(t) = rest.strip_prefix("end{") {
                (false, t)
            } else {
                continue;
            };
            if let Some(close) = tail.find('}') {
                let name = &tail[..close];
                if is_begin {
                    stack.push(name);
                } else if let Some(pos) = stack.iter().rposition(|n| *n == name) {
                    stack.truncate(pos);
                }
            }
        }
    }
    stack.pop().map(str::to_string)
}

/// A project written to a temporary folder, for the tests: the first file is the root.
#[cfg(test)]
pub(crate) fn temp_project(files: &[(&str, &str)]) -> (tempfile::TempDir, Project) {
    let dir = tempfile::tempdir().unwrap();
    for (name, text) in files {
        let path = dir.path().join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let project = Project::open(dir.path().join(files[0].0));
    (dir, project)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::path::Path;
    use test_support::at;

    #[test]
    fn detects_contexts() {
        let cases = [
            ("Some \\sec|", Some("command")),
            ("\\begin{ite|", Some("begin")),
            ("\\end{|", Some("end")),
            ("see \\ref{sec:|", Some("label")),
            ("\\cref{a,b|", Some("label")),
            ("\\cite[p.~3]{knu|", Some("citation")),
            ("\\usepackage[utf8]{inp|", Some("package")),
            ("% \\sec|", None),
            ("plain text|", None),
            ("\\textbf{bold|", None),
            ("\\input{chapters/me|", Some("path")),
            ("\\includegraphics[width=3cm]{fig|", Some("path")),
            ("\\bibliography{a,b|", Some("path")),
        ];
        for (src, want) in cases {
            let (t, o) = at(src);
            let got = context(&t, o).map(|c| match c {
                Context::Command { .. } => "command",
                Context::Begin { .. } => "begin",
                Context::End { .. } => "end",
                Context::Label { .. } => "label",
                Context::Citation { .. } => "citation",
                Context::Package { .. } => "package",
                Context::Path { .. } => "path",
            });
            assert_eq!(got, want, "{src}");
        }
        let (t, o) = at("\\cref{a, b|");
        assert_eq!(context(&t, o), Some(Context::Label { prefix: 9..10 }));
    }

    #[test]
    fn path_prefix() {
        let (t, o) = at("\\input{a, b|");
        assert_eq!(
            context(&t, o),
            Some(Context::Path {
                prefix: 7..11,
                kind: PathKind::Tex
            })
        );
        let (t, o) = at("\\bibliography{refs, mo|");
        assert_eq!(
            context(&t, o),
            Some(Context::Path {
                prefix: 20..22,
                kind: PathKind::Bib
            })
        );
    }

    #[test]
    fn completes_paths() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("chapters")).unwrap();
        std::fs::create_dir_all(root.join(".texoria")).unwrap();
        for f in [
            "main.tex",
            "chapters/intro.tex",
            "chapters/plot.png",
            "refs.bib",
        ] {
            std::fs::write(root.join(f), "").unwrap();
        }
        let labels = |items: Vec<Item>| {
            items
                .into_iter()
                .map(|i| (i.label, i.insert))
                .collect::<Vec<_>>()
        };
        let all = complete_path(root, "", 0..0, PathKind::Tex);
        assert_eq!(
            labels(all),
            vec![
                ("chapters/".to_string(), "chapters/".to_string()),
                ("main.tex".to_string(), "main".to_string()),
            ]
        );
        let inside = complete_path(root, "chapters/in", 0..11, PathKind::Tex);
        assert_eq!(
            labels(inside),
            vec![("intro.tex".to_string(), "chapters/intro".to_string())]
        );
        let pictures = complete_path(root, "chapters/", 0..9, PathKind::Graphics);
        assert_eq!(
            labels(pictures),
            vec![("plot.png".to_string(), "chapters/plot.png".to_string())]
        );
        let bib = complete_path(root, "", 0..0, PathKind::BibResource);
        assert!(labels(bib).contains(&("refs.bib".to_string(), "refs.bib".to_string())));
    }

    #[test]
    fn command_ranking_prefers_prefix() {
        let (t, o) = at("\\sec|");
        let items = complete(&t, o, None, "  ");
        assert_eq!(items[0].label, "\\section");
        assert_eq!(items[0].insert, "\\section{}");
        assert_eq!(items[0].cursor, "\\section{".len());
        assert_eq!(items[0].replace, 0..4);
    }

    #[test]
    fn begin_inserts_matching_end() {
        let (t, o) = at("  \\begin{item|}");
        let items = complete(&t, o, None, "  ");
        let it = &items[0];
        assert_eq!(it.label, "itemize");
        assert_eq!(it.insert, "itemize}\n    \\item \n  \\end{itemize}");
        assert_eq!(&it.insert[..it.cursor], "itemize}\n    \\item ");
        // The auto-closed brace after the cursor is replaced too.
        assert_eq!(it.replace.end, t.len());
    }

    #[test]
    fn end_closes_innermost() {
        let (t, o) = at("\\begin{a}\\begin{b}\\begin{c}\\end{c}\n\\end{|");
        let items = complete(&t, o, None, "  ");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].insert, "b}");

        // Multi-byte text between `\end` and `{` must not split a char.
        let (t, o) = at("\\begin{a}\\end\u{a0}\u{3000}{|");
        assert_eq!(complete(&t, o, None, "  ")[0].insert, "a}");
    }

    #[test]
    fn labels_and_citations_from_project() {
        let root = test_support::fixtures().join("sample/main.tex");
        let p = Project::open(std::fs::canonicalize(root).unwrap());

        let (t, o) = at("\\ref{sec:me|");
        let items = complete(&t, o, Some(&p), "  ");
        assert_eq!(items[0].label, "sec:methods");

        // Citation search matches on title words.
        let (t, o) = at("\\cite{texbook|");
        let items = complete(&t, o, Some(&p), "  ");
        assert_eq!(items[0].label, "knuth1984");
        assert!(items[0].detail.as_deref().unwrap().contains("Knuth"));

        let (t, o) = at("\\no|");
        let items = complete(&t, o, Some(&p), "  ");
        let norm = items.iter().find(|i| i.label == "\\norm").unwrap();
        assert_eq!(norm.kind, ItemKind::UserCommand);
        assert_eq!(norm.insert, "\\norm{}");
    }

    #[test]
    fn snippet_expansion() {
        assert_eq!(expand_snippet("\\frac{$1}{$2}"), ("\\frac{}{}".into(), 6));
        assert_eq!(expand_snippet("a${1:x}b"), ("axb".into(), 1));
        assert_eq!(expand_snippet("x$0y"), ("xy".into(), 1));
        assert_eq!(expand_snippet("\\alpha"), ("\\alpha".into(), 6));
    }

    fn labels(items: &[Item]) -> Vec<&str> {
        items.iter().map(|i| i.label.as_str()).collect()
    }

    #[test]
    fn context_edge_cases() {
        let ctx = |src: &str| {
            let (t, o) = at(src);
            context(&t, o)
        };
        // Only the cursor's line counts, whatever its line ending.
        assert_eq!(
            ctx("\\cite{a}\r\n\\ref{x|"),
            Some(Context::Label { prefix: 15..16 })
        );
        assert_eq!(
            ctx("\\section{A}\r\n\\sec|"),
            Some(Context::Command { prefix: 13..17 })
        );
        // An argument opened on an earlier line is not seen.
        assert_eq!(ctx("\\cite{a,\n  b|"), None);
        // Starred commands and the list commands.
        assert_eq!(ctx("\\ref*{x|"), Some(Context::Label { prefix: 6..7 }));
        assert_eq!(
            ctx("\\includeonly{a,b|"),
            Some(Context::Path {
                prefix: 15..16,
                kind: PathKind::Tex
            })
        );
        assert_eq!(
            ctx("\\addbibresource{my refs,1.b|"),
            Some(Context::Path {
                prefix: 16..27,
                kind: PathKind::BibResource
            })
        );
        assert_eq!(
            ctx("\\RequirePackage{x|"),
            Some(Context::Package { prefix: 16..17 })
        );
        // Only the first argument of a command is known.
        assert_eq!(ctx("\\citep{a}{b|"), None);
        // A brace without a command, or after something that is not a name.
        assert_eq!(ctx("{a|"), None);
        assert_eq!(ctx("\\1{a|"), None);
        assert_eq!(ctx("\\@x{a|"), None);
        assert_eq!(ctx("\\foo1|"), None);
        // An escaped percent sign is text.
        assert_eq!(ctx("50\\% \\sec|"), Some(Context::Command { prefix: 5..9 }));
        assert_eq!(ctx("|"), None);
    }

    #[test]
    fn offsets_outside_the_text_or_inside_a_character() {
        assert_eq!(context("\\sec", 10), None);
        assert_eq!(context("\\ref{é", 6), None);
        assert!(complete("\\sec", 10, None, "  ").is_empty());
        assert!(complete("\\ref{é", 6, None, "  ").is_empty());
    }

    #[test]
    fn paths_need_a_project() {
        let (t, o) = at("\\input{ch|");
        assert!(complete(&t, o, None, "  ").is_empty());
        let (dir, p) = temp_project(&[("main.tex", ""), ("chapter.tex", "")]);
        let items = complete(&t, o, Some(&p), "  ");
        assert_eq!(labels(&items), vec!["chapter.tex"]);
        assert_eq!(items[0].replace, 7..9);
        drop(dir);
    }

    #[test]
    fn completes_packages_and_nothing_without_a_project() {
        let (t, o) = at("\\usepackage{amsm|}");
        let items = complete(&t, o, None, "  ");
        assert_eq!(items[0].label, "amsmath");
        assert_eq!(items[0].kind, ItemKind::Package);
        // The closing brace stays: only environments swallow it.
        assert_eq!(items[0].replace, 12..16);
        for src in ["\\ref{a|", "\\cite{a|"] {
            let (t, o) = at(src);
            assert!(complete(&t, o, None, "  ").is_empty(), "{src}");
        }
    }

    #[test]
    fn completes_from_a_project() {
        let (_dir, p) = temp_project(&[
            (
                "main.tex",
                "\\newcommand{\\pair}[2]{(#1,#2)}\n\\newcommand{\\dup}{a}\n\
                 \\newtheorem{conj}{Conjecture}\n\\newenvironment{itemize}{}{}\n\
                 \\input{other}\n\\bibliography{refs}",
            ),
            ("other.tex", "\\newcommand{\\dup}{b}\n\\label{sec:other}"),
            (
                "refs.bib",
                "@book{full, title={Full}, author={A. Author}, year=2000}\n\
                 @book{titled, title={Only a title}}\n@misc{bare, note={x}}",
            ),
        ]);
        let (t, o) = at("\\pai|");
        let items = complete(&t, o, Some(&p), "  ");
        let pair = items.iter().find(|i| i.label == "\\pair").unwrap();
        assert_eq!(pair.detail.as_deref(), Some("User command [2]"));
        assert_eq!(pair.insert, "\\pair{}{}");
        assert_eq!(pair.cursor, "\\pair{".len());

        // A command defined twice is offered once.
        let (t, o) = at("\\dup|");
        let items = complete(&t, o, Some(&p), "  ");
        assert_eq!(items.iter().filter(|i| i.label == "\\dup").count(), 1);
        assert_eq!(items[0].detail.as_deref(), Some("User command"));

        // User environments, but not twice when the database has them.
        let (t, o) = at("\\begin{conj|");
        let items = complete(&t, o, Some(&p), "  ");
        assert_eq!(items[0].label, "conj");
        assert_eq!(items[0].detail.as_deref(), Some("User environment"));
        assert_eq!(items[0].insert, "conj}\n  \n\\end{conj}");
        assert_eq!(items[0].cursor, "conj}\n  ".len());
        let (t, o) = at("\\begin{itemize|");
        let items = complete(&t, o, Some(&p), "  ");
        assert_eq!(items.iter().filter(|i| i.label == "itemize").count(), 1);

        let (t, o) = at("\\cref{sec:oth|");
        let items = complete(&t, o, Some(&p), "  ");
        assert_eq!(labels(&items), vec!["sec:other"]);
        assert_eq!(items[0].kind, ItemKind::Label);

        // Citation details from whatever the entry has.
        let (t, o) = at("\\cite{|");
        let items = complete(&t, o, Some(&p), "  ");
        let detail = |key: &str| {
            items
                .iter()
                .find(|i| i.label == key)
                .unwrap()
                .detail
                .clone()
        };
        assert_eq!(detail("full").as_deref(), Some("A. Author · 2000 · Full"));
        assert_eq!(detail("titled").as_deref(), Some("Only a title"));
        assert_eq!(detail("bare"), None);
    }

    #[test]
    fn links_are_not_references() {
        for src in ["\\href{https://x|", "\\hyperref{x|", "\\Href{x|"] {
            let (t, o) = at(src);
            assert_eq!(context(&t, o), None, "{src}");
        }
        for src in [
            "\\ref{|",
            "\\eqref{|",
            "\\Cref{|",
            "\\autoref{|",
            "\\pageref{|",
        ] {
            let (t, o) = at(src);
            assert!(
                matches!(context(&t, o), Some(Context::Label { .. })),
                "{src}"
            );
        }
        let (t, o) = at("\\href{https://ex|ample.org}{site}");
        assert_eq!(nav::symbol_at(&t, o), None);
    }

    #[test]
    fn a_percent_after_a_line_break_starts_a_comment() {
        let (t, o) = at("a \\\\% \\sec|");
        assert_eq!(context(&t, o), None);
        let (t, o) = at("a \\\\\\% \\sec|");
        assert!(matches!(context(&t, o), Some(Context::Command { .. })));
        let (t, o) = at("x\\\\%\\ref{a|}");
        assert_eq!(nav::symbol_at(&t, o), None);
    }

    #[test]
    fn end_ignores_environments_in_comments() {
        let (t, o) = at("\\begin{a}\n% \\begin{b}\nx % \\end{a}\n\\end{|");
        let items = complete(&t, o, None, "  ");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].insert, "a}");
        // An escaped percent sign is text.
        assert_eq!(
            innermost_open_env("\\begin{a}50\\% \\begin{b}"),
            Some("b".into())
        );
    }

    #[test]
    fn end_without_an_open_environment() {
        for src in ["\\end{|", "\\begin{a}\\end{a}\\end{|", "\\begin{a\\end{|"] {
            let (t, o) = at(src);
            assert!(complete(&t, o, None, "  ").is_empty(), "{src}");
        }
        // An `\end` without its name closes nothing.
        assert_eq!(innermost_open_env("\\begin{a}\\end{a"), Some("a".into()));
        assert_eq!(innermost_open_env("\\begin{a}\\begin{b}\\end{a}"), None);
        assert_eq!(innermost_open_env("\\begin{a}\\end{b}"), Some("a".into()));
    }

    #[test]
    fn begin_keeps_the_line_indentation() {
        let (t, o) = at("\t\\begin{equation*|");
        let items = complete(&t, o, None, "\t");
        let it = items.iter().find(|i| i.label == "equation*").unwrap();
        assert!(
            it.insert.ends_with("\n\t\\end{equation*}"),
            "{:?}",
            it.insert
        );
        assert_eq!(it.replace, 8..t.len());
    }

    #[test]
    fn ranking() {
        // Nothing typed: candidates in order, capped.
        let (t, o) = at("\\|");
        let items = complete(&t, o, None, "  ");
        assert_eq!(items.len(), MAX_ITEMS);
        assert_eq!(items[0].label, DB.commands[0].name);

        let item = |label: &str| Item {
            kind: ItemKind::Label,
            label: label.into(),
            detail: None,
            replace: 0..0,
            insert: label.into(),
            cursor: 0,
        };
        let many: Vec<_> = (0..MAX_ITEMS + 50)
            .map(|i| (format!("x{i}"), item(&format!("x{i}"))))
            .collect();
        assert_eq!(rank("x", many.clone()).len(), MAX_ITEMS);
        assert_eq!(rank("", many).len(), MAX_ITEMS);
        // Equal scores keep the candidates' order.
        let ties = vec![("ab".into(), item("first")), ("ab".into(), item("second"))];
        assert_eq!(labels(&rank("ab", ties)), vec!["first", "second"]);
        // An exact match beats a longer prefix match, which beats a fuzzy one.
        let mixed = vec![
            ("a_x_b".into(), item("fuzzy")),
            ("abc".into(), item("prefix")),
            ("ab".into(), item("exact")),
        ];
        assert_eq!(labels(&rank("ab", mixed)), vec!["exact", "prefix", "fuzzy"]);
        assert!(rank("zzz", vec![("abc".into(), item("abc"))]).is_empty());
    }

    #[test]
    fn snippet_edge_cases() {
        // Lowest tab stop first, whatever its place.
        assert_eq!(expand_snippet("a$2b$1c"), ("abc".into(), 2));
        assert_eq!(expand_snippet("$0a$0b"), ("ab".into(), 0));
        // Dollars that are not tab stops stay.
        assert_eq!(expand_snippet("$"), ("$".into(), 1));
        assert_eq!(expand_snippet("a$x$"), ("a$x$".into(), 4));
        assert_eq!(expand_snippet("${x:y}"), ("${x:y}".into(), 6));
        // An unclosed placeholder runs to the end.
        assert_eq!(expand_snippet("a${1:bc"), ("abc".into(), 1));
        assert_eq!(expand_snippet("a${1"), ("a".into(), 1));
        assert_eq!(expand_snippet("${"), ("${".into(), 2));
        // Defaults are not nested: the first `}` ends one.
        assert_eq!(expand_snippet("${1:a${2:b}}"), ("a${2:b}".into(), 0));
        assert_eq!(expand_snippet("é${1:ü}ß"), ("éüß".into(), 2));
        assert_eq!(expand_snippet(""), (String::new(), 0));
    }

    #[test]
    fn path_completion_edge_cases() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("sub/deep")).unwrap();
        for f in [
            "Main.TEX",
            "README",
            ".hidden.tex",
            "sub/a.tex",
            "sub/b.bib",
        ] {
            std::fs::write(root.join(f), "").unwrap();
        }
        let names = |items: Vec<Item>| {
            items
                .into_iter()
                .map(|i| (i.label, i.insert, i.kind))
                .collect::<Vec<_>>()
        };
        // Extensions in any case; no hidden files, nor files without extension.
        assert_eq!(
            names(complete_path(root, "", 0..0, PathKind::Tex)),
            vec![
                ("sub/".into(), "sub/".into(), ItemKind::Folder),
                ("Main.TEX".into(), "Main".into(), ItemKind::File),
            ]
        );
        assert_eq!(
            names(complete_path(root, "sub/", 0..4, PathKind::Bib)),
            vec![
                ("deep/".into(), "sub/deep/".into(), ItemKind::Folder),
                ("b.bib".into(), "sub/b".into(), ItemKind::File),
            ]
        );
        // Parent folders and absolute folders.
        let items = complete_path(&root.join("sub/deep"), "../", 0..0, PathKind::Tex);
        assert!(names(items).iter().any(|(_, i, _)| i == "../a"));
        let abs = format!("{}/", root.join("sub").to_str().unwrap().replace('\\', "/"));
        let items = complete_path(Path::new("/nowhere"), &abs, 0..0, PathKind::Tex);
        assert_eq!(items.len(), 2, "{abs}");
        assert!(items.iter().any(|i| i.insert == format!("{abs}a")));
        // A folder that does not exist.
        assert!(complete_path(root, "missing/", 0..0, PathKind::Tex).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn path_completion_follows_symlinked_folders() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("real")).unwrap();
        std::fs::write(root.join("real/x.tex"), "").unwrap();
        std::os::unix::fs::symlink(root.join("real"), root.join("link")).unwrap();
        let items = complete_path(root, "link/", 0..0, PathKind::Tex);
        assert_eq!(labels(&items), vec!["x.tex"]);
        assert_eq!(items[0].insert, "link/x");
    }

    #[test]
    fn many_entries_are_capped() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..MAX_DIR_ENTRIES + 10 {
            std::fs::write(dir.path().join(format!("f{i}.tex")), "").unwrap();
        }
        let items = complete_path(dir.path(), "", 0..0, PathKind::Tex);
        assert_eq!(items.len(), MAX_ITEMS);
    }

    /// Text made of what matters to the context: commands, braces, brackets,
    /// commas, comments, lines and multi-byte characters.
    const LATEXISH: &str = "[a-z\\\\{}\\[\\],%* \n\r$0-9é€]{0,40}";

    fn naive_unclosed_brace(s: &str) -> Option<usize> {
        let opens: Vec<usize> = s.match_indices('{').map(|(i, _)| i).collect();
        opens.into_iter().rev().find(|&i| {
            let after = &s[i + 1..];
            let mut depth = 0i32;
            for c in after.chars() {
                match c {
                    '{' => depth += 1,
                    '}' => depth -= 1,
                    _ => {}
                }
                if depth < 0 {
                    return false;
                }
            }
            true
        })
    }

    proptest! {
        #[test]
        fn context_and_complete_never_panic(text in LATEXISH, at in 0usize..64) {
            let offset = at.min(text.len());
            prop_assume!(text.is_char_boundary(offset));
            if let Some(ctx) = context(&text, offset) {
                let prefix = match ctx {
                    Context::Command { prefix }
                    | Context::Begin { prefix }
                    | Context::End { prefix }
                    | Context::Label { prefix }
                    | Context::Citation { prefix }
                    | Context::Package { prefix }
                    | Context::Path { prefix, .. } => prefix,
                };
                prop_assert!(prefix.start <= prefix.end && prefix.end == offset);
                prop_assert!(text.is_char_boundary(prefix.start));
            }
            for item in complete(&text, offset, None, "  ") {
                prop_assert!(item.cursor <= item.insert.len());
                prop_assert!(item.insert.is_char_boundary(item.cursor));
                prop_assert!(item.replace.end <= text.len());
                prop_assert!(text.is_char_boundary(item.replace.start));
                prop_assert!(text.is_char_boundary(item.replace.end));
            }
        }

        #[test]
        fn snippets_put_the_cursor_inside(snippet in "[a-z${}:0-9é]{0,30}") {
            let (out, cursor) = expand_snippet(&snippet);
            prop_assert!(cursor <= out.len());
            prop_assert!(out.is_char_boundary(cursor));
            if !snippet.contains('$') {
                prop_assert_eq!(&out, &snippet);
                prop_assert_eq!(cursor, out.len());
            }
        }

        #[test]
        fn comments_start_after_an_even_number_of_backslashes(s in "[a\\\\%é]{0,20}") {
            // A `%` preceded by an even run of backslashes.
            let naive = s.char_indices().find(|&(i, c)| {
                c == '%' && s[..i].chars().rev().take_while(|&c| c == '\\').count() % 2 == 0
            });
            prop_assert_eq!(comment_start(&s), naive.map(|(i, _)| i));
        }

        #[test]
        fn unclosed_brace_matches_a_naive_search(s in "[a{}é]{0,30}") {
            prop_assert_eq!(unclosed_brace(&s), naive_unclosed_brace(&s));
        }

        #[test]
        fn ranking_is_capped(n in 0usize..300, typed in "[a-c]{0,2}") {
            let candidates = (0..n)
                .map(|i| {
                    let name = format!("{}{i}", ["a", "b", "c"][i % 3]);
                    let item = Item {
                        kind: ItemKind::Label,
                        label: name.clone(),
                        detail: None,
                        replace: 0..0,
                        insert: name.clone(),
                        cursor: 0,
                    };
                    (name, item)
                })
                .collect();
            prop_assert!(rank(&typed, candidates).len() <= MAX_ITEMS.min(n));
        }
    }
}
