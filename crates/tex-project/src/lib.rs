//! A LaTeX project: the root file, every file reachable through `\input` /
//! `\include`, and the bibliographies they reference.

pub mod bib;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub use bib::BibEntry;
use tex_syntax::{Include, IncludeKind, Index, Label, OutlineItem, UserCommand};

/// Upper bound on files pulled in through includes, as a guard against
/// pathological projects.
const MAX_FILES: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectOutlineItem {
    pub file: PathBuf,
    pub item: OutlineItem,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectLabel {
    pub file: PathBuf,
    pub label: Label,
}

#[derive(Debug, Default)]
pub struct Project {
    root: PathBuf,
    files: HashMap<PathBuf, Index>,
    bibs: HashMap<PathBuf, Vec<BibEntry>>,
}

impl Project {
    /// Load `root` and everything reachable from it from disk.
    pub fn open(root: impl Into<PathBuf>) -> Self {
        let mut p = Project {
            root: root.into(),
            ..Default::default()
        };
        let root = p.root.clone();
        p.load_from_disk(&root);
        p
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn root_dir(&self) -> &Path {
        self.root.parent().unwrap_or(Path::new("."))
    }

    pub fn contains(&self, path: &Path) -> bool {
        self.files.contains_key(path)
    }

    pub fn index(&self, path: &Path) -> Option<&Index> {
        self.files.get(path)
    }

    /// Re-index `path` from an in-memory buffer (the editor's unsaved text),
    /// pulling in any newly referenced files from disk.
    pub fn update(&mut self, path: &Path, text: &str) {
        let index = Index::from_text(text);
        let includes = index.includes.clone();
        self.files.insert(path.to_path_buf(), index);
        self.load_includes(&includes);
    }

    /// Re-read every `.bib` file (call when one changes on disk).
    pub fn reload_bibs(&mut self) {
        let paths: Vec<_> = self.bibs.keys().cloned().collect();
        for p in paths {
            self.bibs.insert(p.clone(), read_bib(&p));
        }
    }

    pub fn is_bib(&self, path: &Path) -> bool {
        self.bibs.contains_key(path)
    }

    /// Every indexed `.tex` file, in document order starting from the root.
    pub fn files(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        self.walk(&self.root, &mut HashSet::new(), &mut |file, item| {
            if item.is_none() {
                out.push(file.to_path_buf());
            }
        });
        out
    }

    /// The document outline across all files, with included files spliced
    /// in where they are `\input`.
    pub fn outline(&self) -> Vec<ProjectOutlineItem> {
        let mut out = Vec::new();
        self.walk(&self.root, &mut HashSet::new(), &mut |file, item| {
            if let Some(item) = item {
                out.push(ProjectOutlineItem {
                    file: file.to_path_buf(),
                    item: item.clone(),
                });
            }
        });
        out
    }

    pub fn labels(&self) -> Vec<ProjectLabel> {
        self.files()
            .into_iter()
            .flat_map(|f| {
                let labels = self
                    .files
                    .get(&f)
                    .map(|i| i.labels.clone())
                    .unwrap_or_default();
                labels.into_iter().map(move |label| ProjectLabel {
                    file: f.clone(),
                    label,
                })
            })
            .collect()
    }

    pub fn bib_entries(&self) -> impl Iterator<Item = &BibEntry> {
        self.bibs.values().flatten()
    }

    pub fn commands(&self) -> impl Iterator<Item = &UserCommand> {
        self.files.values().flat_map(|i| i.commands.iter())
    }

    pub fn environments(&self) -> impl Iterator<Item = &str> {
        self.files
            .values()
            .flat_map(|i| i.environments.iter().map(String::as_str))
    }

    pub fn packages(&self) -> impl Iterator<Item = &str> {
        self.files
            .values()
            .flat_map(|i| i.packages.iter().map(String::as_str))
    }

    /// Depth-first traversal in document order. Calls `f(file, None)` when
    /// entering a file and `f(file, Some(item))` for each outline item.
    fn walk<'a>(
        &'a self,
        file: &Path,
        seen: &mut HashSet<PathBuf>,
        f: &mut dyn FnMut(&Path, Option<&'a OutlineItem>),
    ) {
        if !seen.insert(file.to_path_buf()) {
            return;
        }
        let Some(index) = self.files.get(file) else {
            return;
        };
        f(file, None);
        let mut includes: Vec<&Include> = index
            .includes
            .iter()
            .filter(|i| i.kind == IncludeKind::Tex)
            .collect();
        includes.sort_by_key(|i| i.line);
        let mut inc = includes.into_iter().peekable();
        for item in &index.outline {
            while let Some(i) = inc.next_if(|i| i.line < item.line) {
                self.walk(&resolve_tex(self.root_dir(), &i.path), seen, f);
            }
            f(file, Some(item));
        }
        for i in inc {
            self.walk(&resolve_tex(self.root_dir(), &i.path), seen, f);
        }
    }

    fn load_from_disk(&mut self, path: &Path) {
        if self.files.len() >= MAX_FILES || self.files.contains_key(path) {
            return;
        }
        let text = read_regular(path).unwrap_or_default();
        let index = Index::from_text(&text);
        let includes = index.includes.clone();
        self.files.insert(path.to_path_buf(), index);
        self.load_includes(&includes);
    }

    fn load_includes(&mut self, includes: &[Include]) {
        let dir = self.root_dir().to_path_buf();
        for inc in includes {
            match inc.kind {
                IncludeKind::Tex => {
                    let p = resolve_tex(&dir, &inc.path);
                    if p.is_file() {
                        self.load_from_disk(&p);
                    }
                }
                IncludeKind::Bib => {
                    // `\bibliography{a,b}` arrives as separate includes already.
                    let p = resolve_with_ext(&dir, &inc.path, "bib");
                    if let std::collections::hash_map::Entry::Vacant(slot) = self.bibs.entry(p) {
                        let entries = read_bib(slot.key());
                        slot.insert(entries);
                    }
                }
                IncludeKind::Graphics => {}
            }
        }
    }
}

fn read_bib(path: &Path) -> Vec<BibEntry> {
    read_regular(path)
        .map(|s| bib::parse(&s))
        .unwrap_or_default()
}

/// Read a text file, refusing anything but regular files: reading a named
/// pipe (or a device) would block the UI thread forever.
pub fn read_regular(path: &Path) -> Option<String> {
    if !path.is_file() {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

/// TeX resolves `\input` paths against the directory it runs in (the root
/// file's), appending `.tex` when there is no extension.
pub fn resolve_tex(root_dir: &Path, path: &str) -> PathBuf {
    resolve_with_ext(root_dir, path, "tex")
}

fn resolve_with_ext(root_dir: &Path, path: &str, ext: &str) -> PathBuf {
    let p = Path::new(path.trim());
    let p = if p.extension().is_some_and(|e| e == ext) {
        p.to_path_buf()
    } else {
        let mut s = p.as_os_str().to_owned();
        s.push(".");
        s.push(ext);
        PathBuf::from(s)
    };
    let joined = if p.is_absolute() { p } else { root_dir.join(p) };
    normalize(&joined)
}

fn normalize(p: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// Determine the root file for `file`:
/// 1. a `% !TEX root = path` magic comment in the first lines;
/// 2. the file itself if it contains `\documentclass`;
/// 3. a `.tex` file in the same or a parent directory (up to two levels)
///    that has `\documentclass` and `\input`s this file;
/// 4. otherwise the file itself.
pub fn find_root(file: &Path, text: &str) -> PathBuf {
    let dir = file.parent().unwrap_or(Path::new("."));
    for line in text.lines().take(20) {
        let l = line.trim_start_matches(['%', ' ', '\t']);
        let rest = l
            .strip_prefix("!TEX root")
            .or_else(|| l.strip_prefix("!TeX root"));
        if let Some(path) = rest.and_then(|r| r.trim().strip_prefix('=')) {
            return normalize(&dir.join(path.trim()));
        }
    }
    if text.contains("\\documentclass") {
        return file.to_path_buf();
    }
    let mut candidate_dir = Some(dir);
    for _ in 0..3 {
        let Some(d) = candidate_dir else { break };
        for entry in std::fs::read_dir(d).into_iter().flatten().flatten() {
            let p = entry.path();
            if p == file || p.extension().is_none_or(|e| e != "tex") {
                continue;
            }
            let Some(src) = read_regular(&p) else {
                continue;
            };
            if !src.contains("\\documentclass") {
                continue;
            }
            let idx = Index::from_text(&src);
            let includes_us = idx
                .includes
                .iter()
                .any(|i| i.kind == IncludeKind::Tex && resolve_tex(d, &i.path) == normalize(file));
            if includes_us {
                return p;
            }
        }
        candidate_dir = d.parent();
    }
    file.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/sample")
    }

    #[test]
    fn loads_sample_project() {
        let root = normalize(&sample().join("main.tex"));
        let p = Project::open(&root);
        let files: Vec<_> = p
            .files()
            .iter()
            .map(|f| f.file_name().unwrap().to_owned())
            .collect();
        assert_eq!(files, vec!["main.tex", "methods.tex"]);

        let outline: Vec<_> = p.outline().iter().map(|o| o.item.title.clone()).collect();
        assert_eq!(
            outline,
            vec![
                "Introduction",
                "Methods",
                "An example plot.",
                "Results",
                "Discussion"
            ]
        );

        let mut labels: Vec<_> = p.labels().iter().map(|l| l.label.name.clone()).collect();
        labels.sort();
        assert_eq!(
            labels,
            vec![
                "eq:euler",
                "fig:example",
                "sec:intro",
                "sec:methods",
                "sec:results"
            ]
        );

        let keys: HashSet<_> = p.bib_entries().map(|e| e.key.as_str()).collect();
        assert_eq!(keys, HashSet::from(["knuth1984", "lamport1986"]));
        let cmds: Vec<_> = p.commands().map(|c| c.name.as_str()).collect();
        assert!(cmds.contains(&"\\norm"));
    }

    #[test]
    fn update_from_buffer() {
        let root = normalize(&sample().join("main.tex"));
        let mut p = Project::open(&root);
        let methods = normalize(&sample().join("chapters/methods.tex"));
        p.update(&methods, "\\section{Approach}\\label{sec:approach}");
        let outline: Vec<_> = p.outline().iter().map(|o| o.item.title.clone()).collect();
        assert_eq!(
            outline,
            vec!["Introduction", "Approach", "Results", "Discussion"]
        );
    }

    #[cfg(unix)]
    #[test]
    fn skips_named_pipes() {
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("refs.bib");
        let ok = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            return;
        }
        let root = dir.path().join("main.tex");
        std::fs::write(&root, "\\documentclass{article}\\bibliography{refs}").unwrap();
        // Would block forever if the FIFO were read.
        let p = Project::open(&root);
        assert_eq!(p.bib_entries().count(), 0);
    }

    #[test]
    fn root_detection() {
        let methods = normalize(&sample().join("chapters/methods.tex"));
        let text = std::fs::read_to_string(&methods).unwrap();
        assert_eq!(
            find_root(&methods, &text),
            normalize(&sample().join("main.tex"))
        );

        let magic = "% !TEX root = ../thesis.tex\n\\section{x}";
        assert_eq!(
            find_root(Path::new("/p/ch/a.tex"), magic),
            PathBuf::from("/p/thesis.tex")
        );
    }
}
