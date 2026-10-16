//! LaTeX syntax: the tree-sitter grammar, highlight query and a semantic
//! index (outline, labels, citations, includes, user macros) extracted from
//! a parse tree.

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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserCommand {
    /// Including the backslash, e.g. `\norm`.
    pub name: String,
    /// Number of arguments if declared with `[n]`.
    pub argc: u8,
    pub line: u32,
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
    pub citations: Vec<String>,
    /// Labels referenced with `\ref` and friends.
    pub references: Vec<String>,
    pub includes: Vec<Include>,
    pub commands: Vec<UserCommand>,
    /// User-defined environments and theorem-like environments.
    pub environments: Vec<String>,
    /// `\documentclass` — present only in root files.
    pub document_class: Option<String>,
    pub packages: Vec<String>,
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
        let mut out = Vec::new();
        let mut cursor = node.walk();
        for group in node.children_by_field_name(field, &mut cursor) {
            let mut c2 = group.walk();
            for item in group.children_by_field_name(inner, &mut c2) {
                let t = self.src(item).trim();
                if !t.is_empty() {
                    out.push(t.to_string());
                }
            }
        }
        out
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
                    let float = match name.trim_end_matches('*') {
                        "figure" | "wrapfigure" | "subfigure" => Some(OutlineKind::Figure),
                        "table" | "wraptable" | "subtable" => Some(OutlineKind::Table),
                        _ => None,
                    };
                    if let Some(k) = float {
                        let title = find_caption(node)
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
                if let Some(name) = node.child_by_field_name("name").map(|n| self.group_text(n)) {
                    let context = self
                        .env_stack
                        .last()
                        .cloned()
                        .or_else(|| self.section.clone());
                    index.labels.push(Label {
                        name,
                        line: Self::line(node),
                        context,
                    });
                }
            }
            "label_reference" => {
                index
                    .references
                    .extend(self.field_texts(node, "names", "label"));
            }
            "citation" => {
                index
                    .citations
                    .extend(self.field_texts(node, "keys", "text"));
            }
            "latex_include" => {
                for path in self.field_texts(node, "path", "path") {
                    index.includes.push(Include {
                        kind: IncludeKind::Tex,
                        path,
                        line: Self::line(node),
                    });
                }
            }
            "bibtex_include" => {
                for path in self.field_texts(node, "paths", "path") {
                    index.includes.push(Include {
                        kind: IncludeKind::Bib,
                        path,
                        line: Self::line(node),
                    });
                }
            }
            "biblatex_include" => {
                for path in self.field_texts(node, "glob", "pattern") {
                    index.includes.push(Include {
                        kind: IncludeKind::Bib,
                        path,
                        line: Self::line(node),
                    });
                }
            }
            "graphics_include" => {
                for path in self.field_texts(node, "path", "path") {
                    index.includes.push(Include {
                        kind: IncludeKind::Graphics,
                        path,
                        line: Self::line(node),
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
            }
            "generic_command" => {
                let name = node
                    .child_by_field_name("command")
                    .map(|c| self.src(c))
                    .unwrap_or_default();
                if matches!(name, "\\setmainlanguage" | "\\setdefaultlanguage")
                    && let Some(main) =
                        polyglossia_language(&self.text[node.start_byte() + name.len()..])
                {
                    index.main_language = Some(main);
                }
            }
            "new_command_definition" | "old_command_definition" | "let_command_definition" => {
                if let Some(decl) = node.child_by_field_name("declaration") {
                    let name = self.group_text(decl);
                    if name.starts_with('\\') {
                        let argc = node
                            .child_by_field_name("argc")
                            .and_then(|a| self.group_text(a).parse().ok())
                            .unwrap_or(0);
                        index.commands.push(UserCommand {
                            name,
                            argc,
                            line: Self::line(node),
                        });
                    }
                }
            }
            "environment_definition" => {
                if let Some(n) = node.child_by_field_name("name") {
                    index.environments.push(self.group_text(n));
                }
            }
            "theorem_definition" => {
                index
                    .environments
                    .extend(self.field_texts(node, "name", "text"));
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

/// First `\caption` directly inside an environment (not in nested floats).
fn find_caption(env: Node) -> Option<Node> {
    let mut stack = vec![env];
    while let Some(n) = stack.pop() {
        let mut cursor = n.walk();
        for child in n.children(&mut cursor) {
            if child.kind() == "caption" {
                return child.child_by_field_name("long");
            }
            if child.kind() != "generic_environment" {
                stack.push(child);
            }
        }
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
        assert_eq!(idx.references, vec!["eq:e"]);
        assert_eq!(idx.citations, vec!["knuth", "lamport"]);

        let cmds: Vec<_> = idx
            .commands
            .iter()
            .map(|c| (c.name.as_str(), c.argc))
            .collect();
        assert_eq!(cmds, vec![("\\R", 0), ("\\norm", 1)]);
        assert_eq!(idx.environments, vec!["theorem"]);

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
