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
    const GRAPHICS: &[&str] = &["pdf", "png", "jpg", "jpeg", "eps", "svg"];

    fn accepts(self, ext: &str) -> bool {
        let ext = ext.to_ascii_lowercase();
        match self {
            PathKind::Tex => ext == "tex",
            PathKind::Graphics => Self::GRAPHICS.contains(&ext.as_str()),
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
    let b = before.as_bytes();
    (0..b.len()).any(|i| b[i] == b'%' && (i == 0 || b[i - 1] != b'\\'))
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

pub(crate) fn is_ref_command(c: &str) -> bool {
    let c = c.trim_start_matches('\\').to_ascii_lowercase();
    c.ends_with("ref") || c.ends_with("refs") || c == "labelcref"
}

pub(crate) fn is_cite_command(c: &str) -> bool {
    let c = c.trim_start_matches('\\').to_ascii_lowercase();
    c.contains("cite") || c == "citetext"
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
    let mut rest = text;
    let mut offset = 0;
    while let Some(i) = rest.find('\\') {
        let after = &rest[i + 1..];
        let (is_begin, tail) = if let Some(t) = after.strip_prefix("begin{") {
            (true, t)
        } else if let Some(t) = after.strip_prefix("end{") {
            (false, t)
        } else {
            offset += i + 1;
            rest = &text[offset..];
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
        offset += i + 1;
        rest = &text[offset..];
    }
    stack.pop().map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn at(text: &str) -> (String, usize) {
        let i = text.find('|').unwrap();
        (text.replacen('|', "", 1), i)
    }

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
        let root =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/sample/main.tex");
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
}
