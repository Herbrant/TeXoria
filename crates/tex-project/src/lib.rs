//! A LaTeX project: the root file, every file reachable through `\input` /
//! `\include`, and the bibliographies they reference.

pub mod bib;
pub mod check;
pub mod files;
pub mod main_file;
pub mod search;

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

/// A file's text parsed for `Project::apply`, away from the project (on another
/// thread, without holding its lock): `Project::is_bib` tells which kind.
#[derive(Debug)]
pub enum Parsed {
    Tex(Box<Index>),
    Bib(Vec<BibEntry>),
}

#[derive(Debug, Default)]
pub struct Project {
    root: PathBuf,
    files: HashMap<PathBuf, Index>,
    bibs: HashMap<PathBuf, Vec<BibEntry>>,
    /// Files indexed from a buffer with `update`: kept even when no file of the project
    /// includes them, for `index`.
    buffers: HashSet<PathBuf>,
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
    /// pulling in any newly referenced files from disk and forgetting the files
    /// and bibliographies no longer referenced. A bibliography of the project is
    /// re-parsed instead.
    pub fn update(&mut self, path: &Path, text: &str) {
        let parsed = match self.is_bib(path) {
            true => Parsed::Bib(bib::parse(text)),
            false => Parsed::Tex(Box::new(Index::from_text(text))),
        };
        self.apply(path, parsed);
    }

    /// What `update` does once the text is parsed (the slow part, which needs no
    /// project: see `Parsed`). A text parsed as the wrong kind, because the file became
    /// or stopped being one of the project's bibliographies since, is ignored.
    pub fn apply(&mut self, path: &Path, parsed: Parsed) {
        let index = match parsed {
            Parsed::Bib(new) => {
                if let Some(entries) = self.bibs.get_mut(path) {
                    *entries = new;
                }
                return;
            }
            Parsed::Tex(_) if self.is_bib(path) => return,
            Parsed::Tex(index) => *index,
        };
        let includes = index.includes.clone();
        self.files.insert(path.to_path_buf(), index);
        self.buffers.insert(path.to_path_buf());
        self.load_includes(&includes);
        self.prune();
    }

    /// Drop the files read from disk that the root no longer reaches, and the
    /// bibliographies its files no longer name.
    fn prune(&mut self) {
        let reachable: HashSet<PathBuf> = self.files().into_iter().collect();
        let buffers = &self.buffers;
        self.files
            .retain(|p, _| reachable.contains(p) || buffers.contains(p));
        let dir = self.root_dir();
        let named: HashSet<PathBuf> = reachable
            .iter()
            .filter_map(|f| self.files.get(f))
            .flat_map(|i| &i.includes)
            .filter(|i| i.kind == IncludeKind::Bib)
            .map(|i| resolve_bib(dir, &i.path))
            .collect();
        self.bibs.retain(|p, _| named.contains(p));
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

    /// Each bibliography of the project (existing or not) with its entries.
    pub fn bib_files(&self) -> impl Iterator<Item = (&Path, &[BibEntry])> {
        self.bibs.iter().map(|(p, e)| (p.as_path(), e.as_slice()))
    }

    /// The folders of every `\graphicspath` in the project.
    pub fn graphics_paths(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for (_, i) in self.indexes() {
            for g in &i.graphics_paths {
                if !out.contains(g) {
                    out.push(g.clone());
                }
            }
        }
        out
    }

    /// The `.tex` files of the project with their index, in document order
    /// (like [`Project::files`]).
    pub fn indexes(&self) -> impl Iterator<Item = (&Path, &Index)> {
        self.files()
            .into_iter()
            .filter_map(|f| self.files.get_key_value(&f))
            .map(|(p, i)| (p.as_path(), i))
    }

    pub fn commands(&self) -> impl Iterator<Item = &UserCommand> {
        self.indexes().flat_map(|(_, i)| i.commands.iter())
    }

    pub fn environments(&self) -> impl Iterator<Item = &str> {
        self.indexes()
            .flat_map(|(_, i)| i.environments.iter().map(|e| e.name.as_str()))
    }

    pub fn packages(&self) -> impl Iterator<Item = &str> {
        self.indexes()
            .flat_map(|(_, i)| i.packages.iter().map(String::as_str))
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
        includes.sort_by_key(|i| i.range.start);
        let mut inc = includes.into_iter().peekable();
        for item in &index.outline {
            while let Some(i) = inc.next_if(|i| i.range.start < item.byte) {
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
                    let p = resolve_bib(&dir, &inc.path);
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

/// A `\bibliography` / `\addbibresource` path, with `.bib` added if missing.
pub fn resolve_bib(root_dir: &Path, path: &str) -> PathBuf {
    resolve_with_ext(root_dir, path, "bib")
}

/// Whether the extension of `path` is one of `extensions`, in any case (`.TEX` from old
/// systems).
pub fn has_extension(path: &Path, extensions: &[&str]) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| extensions.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// Extensions tried, in order, for an `\includegraphics` without one.
pub const GRAPHICS_EXTENSIONS: &[&str] = &["pdf", "png", "jpg", "jpeg", "eps", "svg"];

/// The file of an `\includegraphics{name}`: `name` as is or with one of
/// [`GRAPHICS_EXTENSIONS`], in the root file's folder, then in each folder of
/// `\graphicspath`.
pub fn resolve_graphics(root_dir: &Path, graphics_paths: &[String], name: &str) -> Option<PathBuf> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    let dirs = std::iter::once(root_dir.to_path_buf())
        .chain(graphics_paths.iter().map(|g| root_dir.join(g)));
    for dir in dirs {
        let base = normalize(&dir.join(name));
        if base.is_file() {
            return Some(base);
        }
        for ext in GRAPHICS_EXTENSIONS {
            let mut s = base.as_os_str().to_owned();
            s.push(".");
            s.push(ext);
            let p = PathBuf::from(s);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
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
///    that has `\documentclass` and `\input`s this file (huge files are not
///    read);
/// 4. otherwise the file itself.
pub fn find_root(file: &Path, text: &str) -> PathBuf {
    let dir = file.parent().unwrap_or(Path::new("."));
    if let Some(root) = magic_root(file, text) {
        return root;
    }
    if text.contains("\\documentclass") {
        return file.to_path_buf();
    }
    let mut candidate_dir = Some(dir);
    for _ in 0..3 {
        let Some(d) = candidate_dir else { break };
        for entry in std::fs::read_dir(d).into_iter().flatten().flatten() {
            let p = entry.path();
            if p == file || !has_extension(&p, &["tex"]) {
                continue;
            }
            // Runs on the UI thread, maybe in a folder full of unrelated files.
            if entry
                .metadata()
                .is_ok_and(|m| m.len() > files::MAX_TEXT_BYTES)
            {
                continue;
            }
            let Some(src) = read_regular(&p) else {
                continue;
            };
            if !src.contains("\\documentclass") {
                continue;
            }
            let idx = Index::from_text(&src);
            let includes_us = idx.includes.iter().any(|i| match i.kind {
                IncludeKind::Tex => resolve_tex(d, &i.path) == normalize(file),
                IncludeKind::Bib => resolve_bib(d, &i.path) == normalize(file),
                IncludeKind::Graphics => false,
            });
            if includes_us {
                return p;
            }
        }
        candidate_dir = d.parent();
    }
    // Not a LaTeX source (a `.bib` no document names, a package, notes): the folder's
    // document, which a build of the file itself would not be.
    if !has_extension(file, &["tex"])
        && let Some(main) = main_file::main_files(dir).into_iter().next()
    {
        return main;
    }
    file.to_path_buf()
}

/// The file named by a `% !TEX root = path` magic comment in the first lines of `file`.
fn magic_root(file: &Path, text: &str) -> Option<PathBuf> {
    let dir = file.parent().unwrap_or(Path::new("."));
    tex_syntax::magic_comment(text, "root").map(|path| normalize(&dir.join(path)))
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

    /// Write `files` (path, text) into a new temporary folder.
    fn project(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (path, text) in files {
            let path = dir.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        dir
    }

    fn titles(p: &Project) -> Vec<String> {
        p.outline().into_iter().map(|o| o.item.title).collect()
    }

    #[test]
    fn splices_includes_on_the_line_of_a_section() {
        let dir = project(&[
            (
                "main.tex",
                "\\section{A}\\input{b}\\section{C}\n\\input{d}\\section{E}\\input{f}",
            ),
            ("b.tex", "\\section{B}"),
            ("d.tex", "\\section{D}"),
            ("f.tex", "\\section{F}"),
        ]);
        let p = Project::open(dir.path().join("main.tex"));
        assert_eq!(titles(&p), ["A", "B", "C", "D", "E", "F"]);
    }

    #[test]
    fn forgets_files_and_bibliographies_no_longer_included() {
        let dir = project(&[
            ("main.tex", "\\input{a}\\bibliography{refs}\\cite{k}"),
            (
                "a.tex",
                "\\label{x}\\newcommand\\fromA{}\\graphicspath{{a/}}",
            ),
            ("b.tex", "\\label{x}"),
            ("refs.bib", "@misc{k, title={T}}"),
        ]);
        let main = dir.path().join("main.tex");
        let mut p = Project::open(&main);
        // Typed one letter at a time: neither `b`'s predecessor nor `r.bib` stay.
        for text in ["\\input{a}\\input{b}", "\\input{b}\\bibliography{r}"] {
            p.update(&main, text);
        }
        p.update(&main, "\\input{b}\\bibliography{refs}\\cite{k}\\cite{nope}");
        let names = |p: &Project| -> Vec<String> {
            p.indexes()
                .map(|(f, _)| f.file_name().unwrap().to_string_lossy().into_owned())
                .collect()
        };
        assert_eq!(names(&p), ["main.tex", "b.tex"]);
        assert!(!p.contains(&dir.path().join("a.tex")));
        assert_eq!(p.commands().count(), 0);
        assert!(p.graphics_paths().is_empty());
        let bibs: Vec<_> = p.bib_files().map(|(f, _)| f.to_path_buf()).collect();
        assert_eq!(bibs, [dir.path().join("refs.bib")]);
        let codes: Vec<_> = check::run(&p).iter().map(|i| i.code).collect();
        assert_eq!(codes, ["undefined-citation"]);

        // A buffer outside the project keeps its index, but does not count.
        let other = dir.path().join("other.tex");
        p.update(&other, "\\label{x}\\newcommand\\other{}");
        assert!(p.index(&other).is_some());
        assert_eq!(names(&p), ["main.tex", "b.tex"]);
        assert_eq!(p.commands().count(), 0);
        assert!(check::run(&p).iter().all(|i| i.code != "duplicate-label"));
    }

    #[test]
    fn include_cycles_are_walked_once() {
        let dir = project(&[
            ("main.tex", "\\section{M}\\input{a}"),
            ("a.tex", "\\section{A}\\input{b}\\input{main}"),
            ("b.tex", "\\section{B}\\input{a}\\input{b}"),
        ]);
        let p = Project::open(dir.path().join("main.tex"));
        assert_eq!(titles(&p), ["M", "A", "B"]);
        assert_eq!(p.files().len(), 3);
    }

    #[test]
    fn loads_at_most_max_files() {
        let mut files: Vec<(String, String)> = vec![("main.tex".into(), "\\input{f0}".into())];
        for i in 0..MAX_FILES + 5 {
            files.push((format!("f{i}.tex"), format!("\\input{{f{}}}", i + 1)));
        }
        let files: Vec<(&str, &str)> = files
            .iter()
            .map(|(a, b)| (a.as_str(), b.as_str()))
            .collect();
        let dir = project(&files);
        let p = Project::open(dir.path().join("main.tex"));
        assert_eq!(p.files().len(), MAX_FILES);
    }

    #[test]
    fn updates_and_reloads_bibliographies() {
        let dir = project(&[
            (
                "main.tex",
                "\\documentclass{article}\\addbibresource{refs.bib}\\input{ch}",
            ),
            ("refs.bib", "@misc{a, title={A}}"),
            ("ch.tex", "\\label{c}"),
        ]);
        let (main, bib) = (dir.path().join("main.tex"), dir.path().join("refs.bib"));
        let mut p = Project::open(&main);
        assert!(p.is_bib(&bib) && !p.is_bib(&main));
        assert!(p.contains(&dir.path().join("ch.tex")) && !p.contains(&bib));
        assert_eq!(
            p.index(&dir.path().join("ch.tex")).unwrap().labels[0].name,
            "c"
        );
        assert!(p.index(&bib).is_none());
        let keys = |p: &Project| p.bib_entries().map(|e| e.key.clone()).collect::<Vec<_>>();
        // The editor's text of the .bib.
        p.update(&bib, "@misc{b, title={B}}");
        assert_eq!(keys(&p), ["b"]);
        assert!(!p.contains(&bib));
        // Back to what is on disk.
        p.reload_bibs();
        assert_eq!(keys(&p), ["a"]);
        std::fs::write(&bib, "@misc{c, title={C}}").unwrap();
        p.reload_bibs();
        assert_eq!(keys(&p), ["c"]);
        assert_eq!(p.root(), main);
        assert_eq!(p.root_dir(), dir.path());
        assert_eq!(p.packages().count(), 0);

        // Parsed elsewhere, then applied: a text parsed as the wrong kind is ignored.
        p.apply(&bib, Parsed::Tex(Box::new(Index::from_text("\\label{x}"))));
        assert!(!p.contains(&bib));
        assert_eq!(keys(&p), ["c"]);
        p.apply(&main, Parsed::Bib(bib::parse("@misc{d, title={D}}")));
        assert!(!p.is_bib(&main));
        p.apply(&bib, Parsed::Bib(bib::parse("@misc{d, title={D}}")));
        assert_eq!(keys(&p), ["d"]);
    }

    #[test]
    fn lists_graphics_paths_once_and_resolves_pictures() {
        let dir = project(&[
            (
                "main.tex",
                "\\graphicspath{{img/}{fig/}}\\input{a}\\newenvironment{e}{}{}",
            ),
            ("a.tex", "\\graphicspath{{fig/}}\\usepackage{x}"),
            ("plot.png", ""),
            ("img/photo.jpeg", ""),
            ("fig/chart.pdf", ""),
            ("fig/chart.png", ""),
            ("fig/raw", ""),
            ("img/both.eps", ""),
            ("both.svg", ""),
        ]);
        let p = Project::open(dir.path().join("main.tex"));
        let gp = p.graphics_paths();
        assert_eq!(gp, ["img/", "fig/"]);
        assert_eq!(p.environments().collect::<Vec<_>>(), ["e"]);
        assert_eq!(p.packages().collect::<Vec<_>>(), ["x"]);
        let d = dir.path();
        let find = |name: &str| resolve_graphics(d, &gp, name);
        assert_eq!(find("plot"), Some(d.join("plot.png")));
        assert_eq!(find(" plot.png "), Some(d.join("plot.png")));
        assert_eq!(find("photo"), Some(d.join("img/photo.jpeg")));
        // Extensions in their order of preference.
        assert_eq!(find("chart"), Some(d.join("fig/chart.pdf")));
        assert_eq!(find("raw"), Some(d.join("fig/raw")));
        // The root folder first.
        assert_eq!(find("both"), Some(d.join("both.svg")));
        assert_eq!(find("./img/../plot"), Some(d.join("plot.png")));
        assert_eq!(find("missing"), None);
        assert_eq!(find("  "), None);
        assert_eq!(resolve_graphics(d, &[], "photo"), None);
    }

    #[test]
    fn resolves_paths_with_their_extension_once() {
        let root = Path::new("/p");
        assert_eq!(resolve_tex(root, "ch/a"), PathBuf::from("/p/ch/a.tex"));
        assert_eq!(
            resolve_tex(root, " ch/a.tex "),
            PathBuf::from("/p/ch/a.tex")
        );
        // TeX adds `.tex` to anything else.
        assert_eq!(
            resolve_tex(root, "foo.sty"),
            PathBuf::from("/p/foo.sty.tex")
        );
        assert_eq!(resolve_tex(root, "a.TEX"), PathBuf::from("/p/a.TEX.tex"));
        assert_eq!(resolve_tex(root, "./x/../b"), PathBuf::from("/p/b.tex"));
        assert_eq!(resolve_bib(root, "refs"), PathBuf::from("/p/refs.bib"));
        assert_eq!(resolve_bib(root, "refs.bib"), PathBuf::from("/p/refs.bib"));
        let abs = std::env::temp_dir().join("abs");
        assert_eq!(
            resolve_bib(root, abs.to_str().unwrap()),
            normalize(&abs.with_extension("bib"))
        );
    }

    #[test]
    fn root_from_a_magic_comment_in_the_first_lines() {
        let file = Path::new("/p/ch/a.tex");
        for magic in [
            "% !TEX root = ../main.tex",
            "%!TeX root=../main.tex",
            "%% !TEX root =  ../main.tex  ",
            "\t% !TEX root = ./../main.tex",
        ] {
            assert_eq!(
                find_root(file, magic),
                PathBuf::from("/p/main.tex"),
                "{magic}"
            );
        }
        // Past the 20th line: not a magic comment any more.
        let late = format!("{}% !TEX root = ../main.tex", "\n".repeat(20));
        assert_eq!(find_root(file, &late), file);
        // Nor outside a comment.
        assert_eq!(find_root(file, "!TEX root = ../main.tex"), file);
        // The file itself, as a root.
        assert_eq!(find_root(file, "\\documentclass{article}"), file);
    }

    #[test]
    fn magic_comments_ignore_case() {
        let file = Path::new("/p/ch/a.tex");
        for magic in ["% !tex root = ../main.tex", "% !TEX ROOT = ../main.tex"] {
            assert_eq!(
                find_root(file, magic),
                PathBuf::from("/p/main.tex"),
                "{magic}"
            );
        }
    }

    #[test]
    fn root_found_up_to_two_folders_above() {
        let dir = project(&[
            ("thesis.tex", "\\documentclass{book}\\input{parts/one/ch}"),
            ("parts/one/ch.tex", "\\section{x}"),
            ("parts/one/other.tex", "\\documentclass{article}"),
            ("parts/latin1.tex", ""),
        ]);
        std::fs::write(
            dir.path().join("parts/latin1.tex"),
            b"\\documentclass caf\xe9",
        )
        .unwrap();
        let ch = dir.path().join("parts/one/ch.tex");
        assert_eq!(
            find_root(&ch, "\\section{x}"),
            dir.path().join("thesis.tex")
        );

        // A document named in capitals (from an old system) is one too.
        let dir = project(&[
            ("THESIS.TEX", "\\documentclass{book}\\input{ch}"),
            ("ch.tex", ""),
        ]);
        let ch = dir.path().join("ch.tex");
        assert_eq!(find_root(&ch, ""), dir.path().join("THESIS.TEX"));

        // Three folders up is too far.
        let dir = project(&[
            ("thesis.tex", "\\documentclass{book}\\input{a/b/c/ch}"),
            ("a/b/c/ch.tex", ""),
        ]);
        let ch = dir.path().join("a/b/c/ch.tex");
        assert_eq!(find_root(&ch, ""), ch);
    }

    #[test]
    fn other_files_belong_to_the_folders_document() {
        let dir = project(&[
            (
                "main.tex",
                "\\documentclass{article}\\bibliography{bib/refs,other}",
            ),
            ("bib/refs.bib", "@book{a,}"),
            ("other.bib", "@book{b,}"),
            ("unused.bib", "@book{c,}"),
            ("notes.txt", "to do"),
            ("ch.tex", "\\section{x}"),
        ]);
        let main = normalize(&dir.path().join("main.tex"));
        for name in ["bib/refs.bib", "other.bib", "unused.bib", "notes.txt"] {
            let file = normalize(&dir.path().join(name));
            assert_eq!(find_root(&file, ""), main, "{name}");
        }
        // A part no document includes is a document of its own (it may be on its way).
        let ch = normalize(&dir.path().join("ch.tex"));
        assert_eq!(find_root(&ch, "\\section{x}"), ch);
        // With no document around, the file itself.
        let alone = project(&[("refs.bib", "")]);
        let bib = alone.path().join("refs.bib");
        assert_eq!(find_root(&bib, ""), bib);
    }

    #[test]
    fn root_search_skips_huge_files() {
        let dir = project(&[
            ("ch.tex", ""),
            ("big.tex", "\\documentclass{book}\\input{ch}"),
        ]);
        // Windows refuses set_len on a file opened only to append.
        let big = std::fs::OpenOptions::new()
            .write(true)
            .open(dir.path().join("big.tex"))
            .unwrap();
        big.set_len(files::MAX_TEXT_BYTES + 1).unwrap();
        let ch = dir.path().join("ch.tex");
        assert_eq!(find_root(&ch, ""), ch);
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

    mod props {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            #[test]
            fn normalize_is_idempotent(path in "(/|)([a-z]{1,3}|\\.|\\.\\.)(/([a-z]{1,3}|\\.|\\.\\.)){0,6}") {
                let once = normalize(Path::new(&path));
                prop_assert_eq!(normalize(&once), once.clone());
                prop_assert!(once.components().all(|c| !matches!(
                    c,
                    std::path::Component::CurDir | std::path::Component::ParentDir
                )));
            }

            #[test]
            fn the_extension_is_added_once(name in "[a-z]{1,4}(\\.[a-z]{1,3})?") {
                let p = resolve_tex(Path::new("/p"), &name);
                prop_assert_eq!(p.extension().unwrap(), "tex");
                let again = resolve_tex(Path::new("/p"), p.to_str().unwrap());
                prop_assert_eq!(again, p);
            }
        }
    }
}
