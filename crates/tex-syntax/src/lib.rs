//! LaTeX syntax: the tree-sitter grammar, highlight query and a semantic
//! index (outline, labels, citations, includes, user macros) extracted from
//! a parse tree.

use std::ops::Range;

use tree_sitter::{Language, Node, Parser, Tree};

/// Highlight query using the capture names understood by gpui-component.
pub const HIGHLIGHTS: &str = include_str!("../queries/highlights.scm");

pub fn language() -> Language {
    codebook_tree_sitter_latex::LANGUAGE.into()
}

pub fn parse(text: &str, old: Option<&Tree>) -> Option<Tree> {
    let mut parser = Parser::new();
    parser.set_language(&language()).ok()?;
    parser.parse(text, old)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OutlineKind {
    Part,
    Chapter,
    Section,
    Subsection,
    Subsubsection,
    Paragraph,
    Subparagraph,
    Figure,
    Table,
}

impl OutlineKind {
    /// Nesting depth used for indentation in the outline panel.
    pub fn depth(self) -> u8 {
        match self {
            OutlineKind::Part => 0,
            OutlineKind::Chapter => 1,
            OutlineKind::Section => 2,
            OutlineKind::Subsection => 3,
            OutlineKind::Subsubsection => 4,
            OutlineKind::Paragraph => 5,
            OutlineKind::Subparagraph => 6,
            // Floats sit one level below whatever section contains them.
            OutlineKind::Figure | OutlineKind::Table => 7,
        }
    }

    pub fn is_float(self) -> bool {
        matches!(self, OutlineKind::Figure | OutlineKind::Table)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutlineItem {
    pub kind: OutlineKind,
    pub title: String,
    /// Starred (`\section*`) — not numbered.
    pub starred: bool,
    /// 0-based row of the command.
    pub line: u32,
    pub byte: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Label {
    pub name: String,
    pub line: u32,
    /// Bytes of the name inside the braces.
    pub range: Range<usize>,
    /// What the label is attached to: the enclosing environment
    /// (`equation`, `figure`...) or section title, for completion details.
    pub context: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IncludeKind {
    /// `\input` / `\include` / `\subfile`
    Tex,
    /// `\bibliography` / `\addbibresource`
    Bib,
    Graphics,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Include {
    pub kind: IncludeKind,
    pub path: String,
    pub line: u32,
    /// Bytes of the path inside the braces.
    pub range: Range<usize>,
}

/// A key used by a command: a label in `\ref`, a citation key in `\cite`, a
/// `\bibitem` key, a user environment's name in `\newenvironment`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyRef {
    pub name: String,
    /// Bytes of the key in the source.
    pub range: Range<usize>,
    /// 0-based row of the key.
    pub line: u32,
    /// The command it appears in, with its backslash (`\cref`, `\citep`).
    pub command: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserCommand {
    /// Including the backslash, e.g. `\norm`.
    pub name: String,
    /// Number of arguments if declared with `[n]`.
    pub argc: u8,
    pub line: u32,
    /// Bytes of the name in the declaration.
    pub range: Range<usize>,
}

/// A main language set with polyglossia (`\setmainlanguage[variant=uk]{english}`)
/// or babel's `main=` option.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MainLanguage {
    pub name: String,
    pub variant: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Index {
    pub outline: Vec<OutlineItem>,
    pub labels: Vec<Label>,
    /// Keys referenced with `\cite` and friends.
    pub citations: Vec<KeyRef>,
    /// Labels referenced with `\ref` and friends.
    pub references: Vec<KeyRef>,
    /// Keys defined with `\bibitem` (a `thebibliography` environment).
    pub bibitems: Vec<KeyRef>,
    pub includes: Vec<Include>,
    /// Folders of `\graphicspath{{a/}{b/}}`, as written.
    pub graphics_paths: Vec<String>,
    pub commands: Vec<UserCommand>,
    /// User-defined environments and theorem-like environments.
    pub environments: Vec<KeyRef>,
    /// `\documentclass` — present only in root files.
    pub document_class: Option<String>,
    pub packages: Vec<String>,
    /// Each package name of `\usepackage`, with where it is.
    pub package_uses: Vec<KeyRef>,
    /// Options of `\documentclass` and `babel` without a value, in order: babel
    /// takes the last one that names a language as the main language.
    pub language_options: Vec<String>,
    pub main_language: Option<MainLanguage>,
}

impl Index {
    pub fn build(tree: &Tree, text: &str) -> Self {
        let mut index = Index::default();
        let mut ctx = Walk {
            text,
            env_stack: Vec::new(),
            section: None,
        };
        ctx.walk(tree.root_node(), &mut index);
        index
    }

    pub fn from_text(text: &str) -> Self {
        parse(text, None)
            .map(|t| Self::build(&t, text))
            .unwrap_or_default()
    }
}

struct Walk<'a> {
    text: &'a str,
    env_stack: Vec<String>,
    section: Option<String>,
}

impl<'a> Walk<'a> {
    fn src(&self, node: Node) -> &'a str {
        &self.text[node.byte_range()]
    }

    /// Text inside a `{...}` group, whitespace-collapsed.
    fn group_text(&self, node: Node) -> String {
        let s = self.src(node);
        let s = s.strip_prefix(['{', '[']).unwrap_or(s);
        let s = s.strip_suffix(['}', ']']).unwrap_or(s);
        s.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    fn line(node: Node) -> u32 {
        node.start_position().row as u32
    }

    /// Collect the text of every child named `field` of `node` (handles
    /// lists like `{a,b,c}`).
    fn field_texts(&self, node: Node, field: &str, inner: &str) -> Vec<String> {
        self.field_items(node, field, inner)
            .into_iter()
            .map(|(t, _, _)| t)
            .collect()
    }

    /// Like `field_texts`, with the byte range and row of each trimmed item.
    fn field_items(
        &self,
        node: Node,
        field: &str,
        inner: &str,
    ) -> Vec<(String, Range<usize>, u32)> {
        let mut out = Vec::new();
        let mut cursor = node.walk();
        for group in node.children_by_field_name(field, &mut cursor) {
            let mut c2 = group.walk();
            for item in group.children_by_field_name(inner, &mut c2) {
                let raw = self.src(item);
                // Paths keep the comments of a list split over lines: leave them out.
                let blank = blank_comments(raw);
                let t = blank.trim();
                if !t.is_empty() {
                    let lead = blank.len() - blank.trim_start().len();
                    let name = &raw[lead..lead + t.len()];
                    let start = item.start_byte() + lead;
                    let line = Self::line(item) + raw[..lead].matches('\n').count() as u32;
                    out.push((name.to_string(), start..start + t.len(), line));
                }
            }
        }
        out
    }

    /// The trimmed contents of a `{...}` group and their byte range.
    fn group_item(&self, node: Node) -> (String, Range<usize>) {
        let raw = self.src(node);
        let inner = raw.strip_prefix(['{', '[']).unwrap_or(raw);
        let inner = inner.strip_suffix(['}', ']']).unwrap_or(inner);
        let offset = node.start_byte() + (raw.len() - raw.trim_start_matches(['{', '[']).len());
        let t = inner.trim();
        let start = offset + (inner.len() - inner.trim_start().len());
        (t.to_string(), start..start + t.len())
    }

    fn key_refs(&self, node: Node, field: &str, inner: &str) -> Vec<KeyRef> {
        let command = self.command_name(node);
        self.field_items(node, field, inner)
            .into_iter()
            .map(|(name, range, line)| KeyRef {
                line,
                name,
                range,
                command: command.clone(),
            })
            .collect()
    }

    fn command_name(&self, node: Node) -> String {
        node.child_by_field_name("command")
            .map(|c| self.src(c).to_string())
            .unwrap_or_default()
    }

    /// `key[=value]` pairs of a `[...]` options group.
    fn options(&self, node: Node) -> Vec<(String, Option<String>)> {
        let Some(group) = node.child_by_field_name("options") else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let mut cursor = group.walk();
        for pair in group.children_by_field_name("pair", &mut cursor) {
            let Some(key) = pair.child_by_field_name("key") else {
                continue;
            };
            let value = pair
                .child_by_field_name("value")
                .map(|v| self.src(v).trim().to_string());
            out.push((self.src(key).trim().to_string(), value));
        }
        out
    }

    /// Pre-order traversal. Iterative: the tree is as deep as the document's
    /// nesting, and recursing could overflow the stack on pathological input.
    fn walk(&mut self, root: Node, index: &mut Index) {
        let mut cursor = root.walk();
        // For each node on the current path: did entering it push an env?
        let mut pushed: Vec<bool> = Vec::new();
        loop {
            pushed.push(self.enter(cursor.node(), index));
            if cursor.goto_first_child() {
                continue;
            }
            loop {
                if pushed.pop() == Some(true) {
                    self.env_stack.pop();
                }
                if cursor.goto_next_sibling() {
                    break;
                }
                if !cursor.goto_parent() {
                    return;
                }
            }
        }
    }

    /// Record what `node` contributes; returns whether it pushed onto
    /// `env_stack` (popped once its subtree is done).
    fn enter(&mut self, node: Node, index: &mut Index) -> bool {
        let kind = node.kind();
        let mut pushed_env = false;

        match kind {
            "part" | "chapter" | "section" | "subsection" | "subsubsection" | "paragraph"
            | "subparagraph" => {
                let ok = match kind {
                    "part" => OutlineKind::Part,
                    "chapter" => OutlineKind::Chapter,
                    "section" => OutlineKind::Section,
                    "subsection" => OutlineKind::Subsection,
                    "subsubsection" => OutlineKind::Subsubsection,
                    "paragraph" => OutlineKind::Paragraph,
                    _ => OutlineKind::Subparagraph,
                };
                let starred = node
                    .child_by_field_name("command")
                    .is_some_and(|c| self.src(c).ends_with('*'));
                let title = node
                    .child_by_field_name("text")
                    .map(|t| self.group_text(t))
                    .unwrap_or_default();
                self.section = Some(title.clone());
                index.outline.push(OutlineItem {
                    kind: ok,
                    title,
                    starred,
                    line: Self::line(node),
                    byte: node.start_byte(),
                });
            }
            "generic_environment" | "math_environment" => {
                if let Some(name) = node
                    .child_by_field_name("begin")
                    .and_then(|b| b.child_by_field_name("name"))
                    .map(|n| self.group_text(n))
                {
                    if let Some(k) = float_kind(&name) {
                        let title = find_caption(node, self.text)
                            .map(|c| self.group_text(c))
                            .unwrap_or_default();
                        index.outline.push(OutlineItem {
                            kind: k,
                            title,
                            starred: false,
                            line: Self::line(node),
                            byte: node.start_byte(),
                        });
                    }
                    // `document` wraps everything and says nothing about a label.
                    if name != "document" {
                        self.env_stack.push(name);
                        pushed_env = true;
                    }
                }
            }
            "label_definition" => {
                if let Some((name, range)) =
                    node.child_by_field_name("name").map(|n| self.group_item(n))
                {
                    let context = self
                        .env_stack
                        .last()
                        .cloned()
                        .or_else(|| self.section.clone());
                    index.labels.push(Label {
                        name,
                        line: Self::line(node),
                        range,
                        context,
                    });
                }
            }
            "label_reference" => {
                index
                    .references
                    .extend(self.key_refs(node, "names", "label"));
            }
            "citation" => {
                index.citations.extend(self.key_refs(node, "keys", "text"));
            }
            "latex_include" => {
                for (path, range, _) in self.field_items(node, "path", "path") {
                    index.includes.push(Include {
                        kind: IncludeKind::Tex,
                        path,
                        line: Self::line(node),
                        range,
                    });
                }
            }
            "bibtex_include" => {
                for (path, range, _) in self.field_items(node, "paths", "path") {
                    index.includes.push(Include {
                        kind: IncludeKind::Bib,
                        path,
                        line: Self::line(node),
                        range,
                    });
                }
            }
            "biblatex_include" => {
                for (path, range, _) in self.field_items(node, "glob", "pattern") {
                    index.includes.push(Include {
                        kind: IncludeKind::Bib,
                        path,
                        line: Self::line(node),
                        range,
                    });
                }
            }
            "graphics_include" => {
                for (path, range, _) in self.field_items(node, "path", "path") {
                    index.includes.push(Include {
                        kind: IncludeKind::Graphics,
                        path,
                        line: Self::line(node),
                        range,
                    });
                }
            }
            "class_include" => {
                index.document_class = self.field_texts(node, "path", "path").into_iter().next();
                let options = self.options(node);
                index.language_options.extend(
                    options
                        .into_iter()
                        .filter(|(_, v)| v.is_none())
                        .map(|(k, _)| k),
                );
            }
            "package_include" => {
                let paths = self.field_texts(node, "paths", "path");
                if paths.iter().any(|p| p == "babel") {
                    for (key, value) in self.options(node) {
                        match value {
                            None => index.language_options.push(key),
                            Some(name) if key == "main" => {
                                index.main_language = Some(MainLanguage {
                                    name,
                                    variant: None,
                                });
                            }
                            Some(_) => {}
                        }
                    }
                }
                index.packages.extend(paths);
                index
                    .package_uses
                    .extend(self.key_refs(node, "paths", "path"));
            }
            "generic_command" => {
                let name = node
                    .child_by_field_name("command")
                    .map(|c| self.src(c))
                    .unwrap_or_default();
                let after = node.start_byte() + name.len();
                match name {
                    "\\setmainlanguage" | "\\setdefaultlanguage" => {
                        if let Some(main) = polyglossia_language(&self.text[after..]) {
                            index.main_language = Some(main);
                        }
                    }
                    "\\bibitem" => {
                        if let Some((key, range)) = braced_after_options(self.text, after) {
                            let below = self.text[after..range.start].matches('\n').count();
                            index.bibitems.push(KeyRef {
                                name: key.to_string(),
                                line: Self::line(node) + below as u32,
                                range,
                                command: name.to_string(),
                            });
                        }
                    }
                    "\\graphicspath" => {
                        if let Some((paths, _)) = braced_after_options(self.text, after) {
                            index.graphics_paths.extend(
                                paths
                                    .split(['{', '}'])
                                    .map(str::trim)
                                    .filter(|p| !p.is_empty())
                                    .map(str::to_string),
                            );
                        }
                    }
                    _ => {}
                }
            }
            "new_command_definition" | "old_command_definition" | "let_command_definition" => {
                if let Some(decl) = node.child_by_field_name("declaration") {
                    let (name, range) = self.group_item(decl);
                    if name.starts_with('\\') {
                        let argc = node
                            .child_by_field_name("argc")
                            .and_then(|a| self.group_text(a).parse().ok())
                            .unwrap_or(0);
                        index.commands.push(UserCommand {
                            name,
                            argc,
                            line: Self::line(node),
                            range,
                        });
                    }
                }
            }
            "environment_definition" => {
                if let Some(n) = node.child_by_field_name("name") {
                    let (name, range) = self.group_item(n);
                    index.environments.push(KeyRef {
                        name,
                        line: Self::line(n),
                        range,
                        command: self.command_name(node),
                    });
                }
            }
            "theorem_definition" => {
                index
                    .environments
                    .extend(self.key_refs(node, "name", "text"));
            }
            _ => {}
        }
        pushed_env
    }
}

/// The language of polyglossia's `[variant=..]{name}` arguments, which the
/// grammar leaves outside the command when there is an optional argument.
fn polyglossia_language(args: &str) -> Option<MainLanguage> {
    let mut rest = args.trim_start();
    let mut variant = None;
    if let Some(r) = rest.strip_prefix('[') {
        let (opts, r) = r.split_once(']')?;
        variant = opts.split(',').find_map(|o| {
            let (k, v) = o.split_once('=')?;
            (k.trim() == "variant").then(|| v.trim().to_string())
        });
        rest = r.trim_start();
    }
    let (name, _) = rest.strip_prefix('{')?.split_once('}')?;
    let name = name.trim();
    (!name.is_empty()).then(|| MainLanguage {
        name: name.to_string(),
        variant,
    })
}

/// The `{...}` argument starting at `from` (after optional `[...]` groups),
/// trimmed, with its byte range: for commands whose arguments the grammar
/// does not attach to them.
fn braced_after_options(text: &str, from: usize) -> Option<(&str, Range<usize>)> {
    let b = text.as_bytes();
    let mut i = from;
    loop {
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if b.get(i) == Some(&b'[') {
            i = group_end(b, i)? + 1;
        } else {
            break;
        }
    }
    if b.get(i) != Some(&b'{') {
        return None;
    }
    let k = group_end(b, i)?;
    let inner = &text[i + 1..k];
    let t = inner.trim();
    let start = i + 1 + (inner.len() - inner.trim_start().len());
    (!t.is_empty()).then_some((t, start..start + t.len()))
}

/// Where the `[...]` or `{...}` group opened at `open` ends: escaped characters (`\}`) do not
/// count, and an option's `]` inside braces (`[{a]b}]`) does not end it.
fn group_end(b: &[u8], open: usize) -> Option<usize> {
    let close = if b[open] == b'[' { b']' } else { b'}' };
    let mut depth = 0usize;
    let mut k = open + 1;
    while k < b.len() {
        match b[k] {
            b'\\' => k += 1,
            b'{' => depth += 1,
            b'}' if depth > 0 => depth -= 1,
            c if c == close && depth == 0 => return Some(k),
            _ => {}
        }
        k += 1;
    }
    None
}

/// `s` with its `%` comments replaced by spaces, byte for byte.
fn blank_comments(s: &str) -> std::borrow::Cow<'_, str> {
    if !s.contains('%') {
        return s.into();
    }
    let mut out = s.as_bytes().to_vec();
    let mut i = 0;
    while i < out.len() {
        match out[i] {
            b'\\' => i += 1,
            b'%' => {
                while i < out.len() && out[i] != b'\n' {
                    out[i] = b' ';
                    i += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    // Whole characters were replaced, up to a newline or the end.
    String::from_utf8(out).expect("UTF-8").into()
}

/// The outline kind of a float environment's name.
fn float_kind(name: &str) -> Option<OutlineKind> {
    match name.trim_end_matches('*') {
        "figure" | "wrapfigure" | "subfigure" => Some(OutlineKind::Figure),
        "table" | "wraptable" | "subtable" => Some(OutlineKind::Table),
        _ => None,
    }
}

/// First `\caption` of a float environment, also inside other environments (`center`,
/// `minipage`) but not in nested floats, which have their own.
fn find_caption<'t>(env: Node<'t>, text: &str) -> Option<Node<'t>> {
    let is_float = |n: Node| {
        n.kind() == "generic_environment"
            && n.child_by_field_name("begin")
                .and_then(|b| b.child_by_field_name("name"))
                .and_then(|name| {
                    float_kind(text[name.byte_range()].trim_matches(['{', '}']).trim())
                })
                .is_some()
    };
    // Depth first, in source order: the first caption wins.
    let mut stack = vec![env];
    while let Some(n) = stack.pop() {
        if n.kind() == "caption" {
            return n.child_by_field_name("long");
        }
        let mut cursor = n.walk();
        let children: Vec<_> = n.children(&mut cursor).filter(|c| !is_float(*c)).collect();
        stack.extend(children.into_iter().rev());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = r#"\documentclass{article}
\usepackage{amsmath,hyperref}
\newcommand{\R}{\mathbb{R}}
\newcommand{\norm}[1]{\lVert #1 \rVert}
\newtheorem{theorem}{Theorem}
\begin{document}
\section{Intro duction}
\label{sec:intro}
See \ref{eq:e} and \cite{knuth, lamport}.
\subsection*{Details}
\begin{equation}
  e = mc^2 \label{eq:e}
\end{equation}
\begin{figure}
  \caption{A nice  plot}
  \label{fig:plot}
\end{figure}
\input{chapters/a}
\bibliography{refs}
\end{document}
"#;

    #[test]
    fn highlight_query_compiles() {
        tree_sitter::Query::new(&language(), HIGHLIGHTS).expect("valid highlights.scm");
    }

    #[test]
    fn builds_index() {
        let idx = Index::from_text(DOC);
        assert_eq!(idx.document_class.as_deref(), Some("article"));
        assert_eq!(idx.packages, vec!["amsmath", "hyperref"]);

        let outline: Vec<_> = idx
            .outline
            .iter()
            .map(|o| (o.kind, o.title.as_str(), o.line))
            .collect();
        assert_eq!(
            outline,
            vec![
                (OutlineKind::Section, "Intro duction", 6),
                (OutlineKind::Subsection, "Details", 9),
                (OutlineKind::Figure, "A nice plot", 13),
            ]
        );
        assert!(idx.outline[1].starred);

        let labels: Vec<_> = idx
            .labels
            .iter()
            .map(|l| (l.name.as_str(), l.context.as_deref()))
            .collect();
        assert_eq!(
            labels,
            vec![
                ("sec:intro", Some("Intro duction")),
                ("eq:e", Some("equation")),
                ("fig:plot", Some("figure")),
            ]
        );
        let names = |v: &[KeyRef]| v.iter().map(|k| k.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&idx.references), vec!["eq:e"]);
        assert_eq!(names(&idx.citations), vec!["knuth", "lamport"]);

        let cmds: Vec<_> = idx
            .commands
            .iter()
            .map(|c| (c.name.as_str(), c.argc))
            .collect();
        assert_eq!(cmds, vec![("\\R", 0), ("\\norm", 1)]);
        assert_eq!(names(&idx.environments), vec!["theorem"]);

        let inc: Vec<_> = idx
            .includes
            .iter()
            .map(|i| (&i.kind, i.path.as_str()))
            .collect();
        assert_eq!(
            inc,
            vec![
                (&IncludeKind::Tex, "chapters/a"),
                (&IncludeKind::Bib, "refs")
            ]
        );
    }

    #[test]
    fn records_positions() {
        let idx = Index::from_text(DOC);
        let at = |r: &Range<usize>| &DOC[r.clone()];
        assert_eq!(at(&idx.labels[0].range), "sec:intro");
        assert_eq!(at(&idx.references[0].range), "eq:e");
        assert_eq!(idx.references[0].command, "\\ref");
        assert_eq!(at(&idx.citations[1].range), "lamport");
        assert_eq!(idx.citations[1].command, "\\cite");
        assert_eq!(at(&idx.includes[0].range), "chapters/a");
        assert_eq!(at(&idx.commands[1].range), "\\norm");
        assert_eq!(at(&idx.environments[0].range), "theorem");
    }

    #[test]
    fn finds_bibitems_and_graphics_paths() {
        let src = "\\graphicspath{{img/}{ fig/ }}\n\\begin{thebibliography}{9}\n\\bibitem{knuth} K.\n\\bibitem[L]{ lamport } L.\n\\end{thebibliography}";
        let idx = Index::from_text(src);
        assert_eq!(idx.graphics_paths, vec!["img/", "fig/"]);
        let keys: Vec<_> = idx.bibitems.iter().map(|k| &src[k.range.clone()]).collect();
        assert_eq!(keys, vec!["knuth", "lamport"]);
    }

    #[test]
    fn finds_declared_languages() {
        let idx = Index::from_text(
            "\\documentclass[a4paper,english,fontsize=11pt]{article}\n\
             \\usepackage[utf8]{inputenc}\n\\usepackage[ngerman, main=italian]{babel}",
        );
        assert_eq!(idx.language_options, vec!["a4paper", "english", "ngerman"]);
        assert_eq!(
            idx.main_language,
            Some(MainLanguage {
                name: "italian".into(),
                variant: None
            })
        );
        assert_eq!(idx.packages, vec!["inputenc", "babel"]);

        let idx = Index::from_text("\\setmainlanguage[variant=british]{english}");
        assert_eq!(
            idx.main_language,
            Some(MainLanguage {
                name: "english".into(),
                variant: Some("british".into())
            })
        );
        let idx = Index::from_text("\\setdefaultlanguage{french}");
        assert_eq!(idx.main_language.map(|m| m.name).as_deref(), Some("french"));
    }

    #[test]
    fn deep_nesting_does_not_overflow() {
        let depth = 200_000;
        let doc = format!(
            "\\section{{A}}{}{}\\label{{x}}",
            "{".repeat(depth),
            "}".repeat(depth)
        );
        let idx = Index::from_text(&doc);
        assert_eq!(idx.outline.len(), 1);
        assert_eq!(idx.labels[0].name, "x");
    }

    fn names(v: &[KeyRef]) -> Vec<&str> {
        v.iter().map(|k| k.name.as_str()).collect()
    }

    #[test]
    fn outline_kinds_nest_and_floats_sit_below_sections() {
        use OutlineKind::*;
        let kinds = [
            Part,
            Chapter,
            Section,
            Subsection,
            Subsubsection,
            Paragraph,
            Subparagraph,
            Figure,
        ];
        for pair in kinds.windows(2) {
            assert!(pair[0].depth() < pair[1].depth(), "{pair:?}");
        }
        assert_eq!(Figure.depth(), Table.depth());
        let floats: Vec<_> = [kinds.as_slice(), &[Table]]
            .concat()
            .into_iter()
            .filter(|k| k.is_float())
            .collect();
        assert_eq!(floats, [Figure, Table]);
    }

    #[test]
    fn indexes_every_outline_kind() {
        let src = "\\part{P}\\chapter{C}\\paragraph{G}\\subparagraph*{S}\n\
                   \\subsubsection{  Many \n words }\n\
                   \\begin{table}\\caption{T}\\end{table}\n\
                   \\begin{figure*}\\begin{subfigure}{x}\\caption{in}\\end{subfigure}\
                   \\caption{out}\\end{figure*}\n\
                   \\begin{wrapfigure}{r}{3cm}x\\end{wrapfigure}\\begin{wraptable}{l}{1cm}\\caption{t}\\end{wraptable}";
        let idx = Index::from_text(src);
        let outline: Vec<_> = idx
            .outline
            .iter()
            .map(|o| (o.kind, o.title.as_str(), o.starred, o.line))
            .collect();
        use OutlineKind::*;
        assert_eq!(
            outline,
            [
                (Part, "P", false, 0),
                (Chapter, "C", false, 0),
                (Paragraph, "G", false, 0),
                (Subparagraph, "S", true, 0),
                (Subsubsection, "Many words", false, 1),
                (Table, "T", false, 3),
                // The caption of a nested float is its own.
                (Figure, "out", false, 4),
                (Figure, "in", false, 4),
                // No caption.
                (Figure, "", false, 5),
                (Table, "t", false, 5),
            ]
        );
        for item in &idx.outline {
            assert!(src[item.byte..].starts_with('\\'), "{item:?}");
        }
    }

    #[test]
    fn finds_captions_inside_other_environments_of_a_float() {
        let src = "\\begin{figure}\\begin{center}\\includegraphics{a}\\caption{Centered}\\end{center}\\end{figure}\n\
                   \\begin{table}\\begin{minipage}{.5\\linewidth}\\begin{subtable}{x}\\caption{inner}\\end{subtable}\
                   \\caption{Side}\\end{minipage}\\end{table}";
        let titles: Vec<_> = Index::from_text(src)
            .outline
            .into_iter()
            .map(|o| (o.kind, o.title))
            .collect();
        use OutlineKind::*;
        assert_eq!(
            titles,
            [
                (Figure, "Centered".to_string()),
                (Table, "Side".to_string()),
                (Table, "inner".to_string()),
            ]
        );
    }

    #[test]
    fn label_context_is_the_innermost_environment_or_the_section() {
        let src = "\\label{top}\\begin{document}\\label{doc}\n\
                   \\section{S}\\label{s}\\begin{equation}\\label{eq}\\end{equation}\\label{after}\n\
                   \\begin{align*}\\label{al}\\begin{itemize}\\item\\label{it}\\end{itemize}\\end{align*}\n\
                   \\end{document}";
        let idx = Index::from_text(src);
        let labels: Vec<_> = idx
            .labels
            .iter()
            .map(|l| (l.name.as_str(), l.context.as_deref(), l.line))
            .collect();
        assert_eq!(
            labels,
            [
                ("top", None, 0),
                // `document` is not a context.
                ("doc", None, 0),
                ("s", Some("S"), 1),
                ("eq", Some("equation"), 1),
                ("after", Some("S"), 1),
                ("al", Some("align*"), 2),
                ("it", Some("itemize"), 2),
            ]
        );
    }

    #[test]
    fn indexes_every_include_kind() {
        let src = "\\include{a}\\subfile{ b }\n\\addbibresource{refs.bib}\\bibliography{x,y}\n\
                   \\includegraphics[width=1cm]{img/p}";
        let idx = Index::from_text(src);
        let inc: Vec<_> = idx
            .includes
            .iter()
            .map(|i| {
                (
                    i.kind.clone(),
                    i.path.as_str(),
                    i.line,
                    &src[i.range.clone()],
                )
            })
            .collect();
        use IncludeKind::*;
        assert_eq!(
            inc,
            [
                (Tex, "a", 0, "a"),
                (Tex, "b", 0, "b"),
                (Bib, "refs.bib", 1, "refs.bib"),
                (Bib, "x", 1, "x"),
                (Bib, "y", 1, "y"),
                (Graphics, "img/p", 2, "img/p"),
            ]
        );
    }

    #[test]
    fn indexes_command_and_environment_definitions() {
        let src = "\\renewcommand\\bar[2]{x}\n\\def\\baz#1{x}\\let\\qux\\relax\n\
                   \\newcommand{\\big}[300]{x}\\newcommand{\\nn}[x]{x}\\newcommand{{\\w}}{x}\n\
                   \\newenvironment{ proof* }[1]{a}{b}\\newtheorem{thm}[section]{Theorem}";
        let idx = Index::from_text(src);
        let cmds: Vec<_> = idx
            .commands
            .iter()
            .map(|c| (c.name.as_str(), c.argc, c.line, &src[c.range.clone()]))
            .collect();
        assert_eq!(
            cmds,
            [
                ("\\bar", 2, 0, "\\bar"),
                ("\\baz", 0, 1, "\\baz"),
                ("\\qux", 0, 1, "\\qux"),
                // An argument count that is not a number, or too large, counts as none.
                ("\\big", 0, 2, "\\big"),
                ("\\nn", 0, 2, "\\nn"),
                ("\\w", 0, 2, "\\w"),
            ]
        );
        assert_eq!(names(&idx.environments), ["proof*", "thm"]);
        let env = &idx.environments[0];
        assert_eq!(
            (&src[env.range.clone()], env.line, env.command.as_str()),
            ("proof*", 3, "\\newenvironment")
        );
        assert_eq!(idx.environments[1].command, "\\newtheorem");
    }

    #[test]
    fn records_package_uses_and_citation_keys() {
        let src =
            "\\usepackage{ a , b }\n\\cite[p.~3]{k}\\cite{}\\citep{a,,b}\n\\eqref{eq}\\cref{x, y}";
        let idx = Index::from_text(src);
        assert_eq!(idx.packages, ["a", "b"]);
        let uses: Vec<_> = idx
            .package_uses
            .iter()
            .map(|k| (&src[k.range.clone()], k.line, k.command.as_str()))
            .collect();
        assert_eq!(uses, [("a", 0, "\\usepackage"), ("b", 0, "\\usepackage")]);
        // Empty keys are skipped.
        assert_eq!(names(&idx.citations), ["k", "a", "b"]);
        assert_eq!(idx.citations[1].command, "\\citep");
        assert_eq!(idx.citations[1].line, 1);
        assert_eq!(names(&idx.references), ["eq", "x", "y"]);
        assert_eq!(idx.references[2].command, "\\cref");
    }

    #[test]
    fn empty_text_has_an_empty_index() {
        assert_eq!(Index::from_text(""), Index::default());
        assert_eq!(Index::from_text("just words"), Index::default());
    }

    #[test]
    fn positions_count_bytes_and_crlf_lines() {
        let src = "é ü\r\n\\label{ñ}\r\n% \\label{commented}\r\n\\ref{ñ}\\section{日本}";
        let idx = Index::from_text(src);
        assert_eq!(idx.labels.len(), 1);
        let l = &idx.labels[0];
        assert_eq!((&src[l.range.clone()], l.line), ("ñ", 1));
        let r = &idx.references[0];
        assert_eq!((&src[r.range.clone()], r.line), ("ñ", 3));
        assert_eq!(idx.outline[0].title, "日本");
        assert_eq!(idx.outline[0].line, 3);
    }

    #[test]
    fn reads_polyglossia_arguments() {
        let lang = |s: &str| polyglossia_language(s);
        assert_eq!(
            lang(" [ variant = uk , x=y ] { english } rest"),
            Some(MainLanguage {
                name: "english".into(),
                variant: Some("uk".into())
            })
        );
        assert_eq!(
            lang("[babelshorthands]{german}"),
            Some(MainLanguage {
                name: "german".into(),
                variant: None
            })
        );
        assert_eq!(lang("{}"), None);
        assert_eq!(lang("{  }"), None);
        assert_eq!(lang("[variant=uk{english}"), None);
        assert_eq!(lang("{english"), None);
        assert_eq!(lang(""), None);
        let idx = Index::from_text("\\setmainlanguage{}\\setmainlanguage[variant=uk");
        assert_eq!(idx.main_language, None);
    }

    #[test]
    fn reads_braced_arguments_after_options() {
        fn arg(s: &str) -> Option<(&str, &str)> {
            braced_after_options(s, 0).map(|(t, r)| (t, &s[r]))
        }
        assert_eq!(arg(" [a] [b]\n{ key }"), Some(("key", "key")));
        assert_eq!(arg("{a{b}c}"), Some(("a{b}c", "a{b}c")));
        assert_eq!(arg("{}"), None);
        assert_eq!(arg("{ }"), None);
        assert_eq!(arg("{unclosed"), None);
        assert_eq!(arg("[unclosed{key}"), None);
        assert_eq!(arg("no group"), None);
        assert_eq!(arg(""), None);
        // From the middle of a text.
        let s = "ab{cd}";
        assert_eq!(braced_after_options(s, 2), Some(("cd", 3..5)));
        assert_eq!(braced_after_options(s, 6), None);
    }

    #[test]
    fn braced_arguments_skip_escaped_braces_and_braces_in_options() {
        fn arg(s: &str) -> Option<(&str, &str)> {
            braced_after_options(s, 0).map(|(t, r)| (t, &s[r]))
        }
        assert_eq!(arg("{a\\}b}"), Some(("a\\}b", "a\\}b")));
        assert_eq!(arg("{a\\{b}"), Some(("a\\{b", "a\\{b")));
        assert_eq!(arg("{a\\\\}b}"), Some(("a\\\\", "a\\\\")));
        assert_eq!(arg("[{a]b}]{key}"), Some(("key", "key")));
        assert_eq!(arg("[\\]]{key}"), Some(("key", "key")));
        let src = "\\bibitem{a\\}b} A.\n\\bibitem[{[1]}]{c} C.";
        assert_eq!(names(&Index::from_text(src).bibitems), ["a\\}b", "c"]);
    }

    #[test]
    fn list_items_leave_out_comments() {
        let src = "\\usepackage{amsmath, % math\n  amssymb,\n%  graphicx,\n  50\\%pkg}\n\
                   \\input{ch % chapter\n}\\bibliography{\n  refs}";
        let idx = Index::from_text(src);
        assert_eq!(idx.packages, ["amsmath", "amssymb", "50\\%pkg"]);
        let uses: Vec<_> = idx
            .package_uses
            .iter()
            .map(|k| (&src[k.range.clone()], k.line))
            .collect();
        assert_eq!(uses, [("amsmath", 0), ("amssymb", 1), ("50\\%pkg", 3)]);
        let inc: Vec<_> = idx
            .includes
            .iter()
            .map(|i| (i.path.as_str(), &src[i.range.clone()]))
            .collect();
        assert_eq!(inc, [("ch", "ch"), ("refs", "refs")]);
        let idx = Index::from_text("\\bibitem{\n  k}\\cite{\n%c\n a}");
        assert_eq!((idx.bibitems[0].line, idx.citations[0].line), (1, 3));
    }

    #[test]
    fn incremental_reparse_is_consistent() {
        let tree = parse(DOC, None).unwrap();
        let edited = DOC.replace("Intro duction", "Introduction");
        let mut old = tree.clone();
        let start = DOC.find("Intro duction").unwrap();
        old.edit(&tree_sitter::InputEdit {
            start_byte: start,
            old_end_byte: start + "Intro duction".len(),
            new_end_byte: start + "Introduction".len(),
            start_position: tree_sitter::Point::new(6, 9),
            old_end_position: tree_sitter::Point::new(6, 22),
            new_end_position: tree_sitter::Point::new(6, 21),
        });
        let new_tree = parse(&edited, Some(&old)).unwrap();
        assert_eq!(
            Index::build(&new_tree, &edited).outline[0].title,
            "Introduction"
        );
    }
}
