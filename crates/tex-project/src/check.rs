//! Problems found without building: references to labels and citations that
//! do not exist, duplicate labels, missing files, and flaws of the
//! bibliography.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::PathBuf;

use tex_syntax::IncludeKind;

use crate::bib::BibEntry;
use crate::{Project, resolve_bib, resolve_graphics, resolve_tex};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    Error,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub file: PathBuf,
    /// Bytes of the file the issue is about.
    pub range: Range<usize>,
    /// 0-based row of `range`.
    pub line: u32,
    pub level: Level,
    /// What kind of issue (`undefined-reference`, `bib-missing-field`…).
    pub code: &'static str,
    pub message: String,
    /// The label or citation key concerned, if any.
    pub key: Option<String>,
}

/// Fields an entry type needs (one of `a|b` is enough), as BibTeX and
/// biblatex define them.
const REQUIRED: &[(&str, &[&str])] = &[
    (
        "article",
        &["author", "title", "journal|journaltitle", "year|date"],
    ),
    (
        "book",
        &["author|editor", "title", "publisher", "year|date"],
    ),
    (
        "inbook",
        &["author|editor", "title", "publisher", "year|date"],
    ),
    (
        "incollection",
        &["author", "title", "booktitle", "year|date"],
    ),
    (
        "inproceedings",
        &["author", "title", "booktitle", "year|date"],
    ),
    ("conference", &["author", "title", "booktitle", "year|date"]),
    (
        "phdthesis",
        &["author", "title", "school|institution", "year|date"],
    ),
    (
        "mastersthesis",
        &["author", "title", "school|institution", "year|date"],
    ),
    (
        "thesis",
        &["author", "title", "type", "institution|school", "year|date"],
    ),
    (
        "techreport",
        &["author", "title", "institution", "year|date"],
    ),
    (
        "report",
        &["author", "title", "type", "institution", "year|date"],
    ),
];

/// Every issue of the project, ordered by file and position.
pub fn run(project: &Project) -> Vec<Issue> {
    let mut issues = Vec::new();
    let dir = project.root_dir();
    // Names built from macro arguments (`\ref{#1}`) are not real keys.
    let real = |name: &str| !name.contains(['#', '\\', '*']);

    let mut labels: HashMap<&str, usize> = HashMap::new();
    for (_, index) in project.indexes() {
        for l in &index.labels {
            *labels.entry(&l.name).or_default() += 1;
        }
    }

    let bibs: Vec<(PathBuf, &[BibEntry])> = project
        .bib_files()
        .map(|(p, e)| (p.to_path_buf(), e))
        .collect();
    let bib_missing = bibs.iter().any(|(p, _)| !p.is_file());
    let mut keys: HashSet<&str> = bibs
        .iter()
        .flat_map(|(_, e)| e.iter().map(|e| e.key.as_str()))
        .collect();
    for (_, index) in project.indexes() {
        keys.extend(index.bibitems.iter().map(|b| b.name.as_str()));
    }
    // Citations can be checked only against a bibliography TeXoria can read.
    let check_citations = !bib_missing && !keys.is_empty();
    let graphics_paths = project.graphics_paths();

    for (file, index) in project.indexes() {
        let issue =
            |range: Range<usize>, line, level, code, message: String, key: Option<&str>| Issue {
                file: file.to_path_buf(),
                range,
                line,
                level,
                code,
                message,
                key: key.map(str::to_string),
            };
        for r in &index.references {
            if real(&r.name) && !labels.contains_key(r.name.as_str()) {
                issues.push(issue(
                    r.range.clone(),
                    r.line,
                    Level::Warning,
                    "undefined-reference",
                    format!("No \\label{{{}}} in the project", r.name),
                    Some(&r.name),
                ));
            }
        }
        for l in &index.labels {
            if real(&l.name) && labels.get(l.name.as_str()).is_some_and(|&n| n > 1) {
                issues.push(issue(
                    l.range.clone(),
                    l.line,
                    Level::Warning,
                    "duplicate-label",
                    format!("The label {} is defined more than once", l.name),
                    Some(&l.name),
                ));
            }
        }
        if check_citations {
            for c in &index.citations {
                if real(&c.name) && !keys.contains(c.name.as_str()) {
                    issues.push(issue(
                        c.range.clone(),
                        c.line,
                        Level::Warning,
                        "undefined-citation",
                        format!("No entry {} in the bibliography", c.name),
                        Some(&c.name),
                    ));
                }
            }
        }
        for inc in &index.includes {
            if !real(&inc.path) {
                continue;
            }
            let (found, level, what) = match inc.kind {
                IncludeKind::Tex => (resolve_tex(dir, &inc.path).is_file(), Level::Error, "File"),
                IncludeKind::Bib => (
                    resolve_bib(dir, &inc.path).is_file(),
                    Level::Warning,
                    "Bibliography",
                ),
                IncludeKind::Graphics => (
                    resolve_graphics(dir, &graphics_paths, &inc.path).is_some(),
                    Level::Warning,
                    "Picture",
                ),
            };
            if !found {
                issues.push(issue(
                    inc.range.clone(),
                    inc.line,
                    level,
                    "missing-file",
                    format!("{what} {} not found", inc.path),
                    None,
                ));
            }
        }
    }

    for (file, entries) in &bibs {
        issues.extend(check_bib(file, entries));
    }
    issues.sort_by(|a, b| (&a.file, a.range.start).cmp(&(&b.file, b.range.start)));
    issues
}

/// The flaws of one bibliography file.
pub fn check_bib(file: &std::path::Path, entries: &[BibEntry]) -> Vec<Issue> {
    let mut issues = Vec::new();
    let issue = |range: Range<usize>, line, level, code, message: String, key: &str| Issue {
        file: file.to_path_buf(),
        range,
        line,
        level,
        code,
        message,
        key: Some(key.to_string()),
    };
    let mut count: HashMap<&str, usize> = HashMap::new();
    for e in entries {
        *count.entry(&e.key).or_default() += 1;
    }
    for e in entries {
        if e.truncated {
            issues.push(issue(
                e.key_range.clone(),
                e.line,
                Level::Error,
                "bib-unterminated",
                format!("The entry {} is not closed", e.key),
                &e.key,
            ));
        }
        if count[e.key.as_str()] > 1 {
            issues.push(issue(
                e.key_range.clone(),
                e.line,
                Level::Warning,
                "bib-duplicate-key",
                format!("Another entry has the key {}", e.key),
                &e.key,
            ));
        }
        if let Some((_, fields)) = REQUIRED.iter().find(|(kind, _)| *kind == e.kind) {
            let missing: Vec<&str> = fields
                .iter()
                .filter(|any| {
                    !any.split('|')
                        .any(|f| e.field(f).is_some_and(|v| !v.is_empty()))
                })
                .map(|any| any.split('|').next().unwrap_or(any))
                .collect();
            if !missing.is_empty() && !e.truncated {
                issues.push(issue(
                    e.key_range.clone(),
                    e.line,
                    Level::Warning,
                    "bib-missing-field",
                    format!("@{} {} has no {}", e.kind, e.key, missing.join(", ")),
                    &e.key,
                ));
            }
        }
        if let Some(doi) = e.fields.iter().find(|f| f.name == "doi")
            && !valid_doi(&doi.value)
        {
            issues.push(issue(
                doi.range.clone(),
                doi.line,
                Level::Warning,
                "bib-invalid-doi",
                format!("{} is not a DOI (10.xxxx/…)", doi.value),
                &e.key,
            ));
        }
    }
    issues
}

/// `10.<registrant>/<suffix>`, possibly written as a doi.org URL or with `doi:`.
/// The registrant is dot-separated numbers (`10.1000.10`).
pub fn valid_doi(value: &str) -> bool {
    let v = value.trim();
    let lower = v.to_ascii_lowercase();
    let v = [
        "https://doi.org/",
        "http://doi.org/",
        "https://dx.doi.org/",
        "http://dx.doi.org/",
        "doi:",
    ]
    .iter()
    .find(|p| lower.starts_with(*p))
    .map_or(v, |p| v[p.len()..].trim_start());
    let Some((prefix, suffix)) = v.split_once('/') else {
        return false;
    };
    let Some(registrant) = prefix.strip_prefix("10.") else {
        return false;
    };
    registrant
        .split('.')
        .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
        && !suffix.is_empty()
        && !suffix.contains(char::is_whitespace)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bib;
    use std::path::Path;

    fn codes(issues: &[Issue]) -> Vec<(&'static str, Option<&str>)> {
        issues.iter().map(|i| (i.code, i.key.as_deref())).collect()
    }

    #[test]
    fn the_sample_project_has_no_issues() {
        let root =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/sample/main.tex");
        let p = Project::open(root);
        assert_eq!(run(&p), Vec::new());
    }

    #[test]
    fn finds_reference_and_file_issues() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("main.tex");
        std::fs::write(dir.path().join("refs.bib"), "@misc{known, title={T}}").unwrap();
        std::fs::write(
            &root,
            "\\documentclass{article}\\begin{document}\n\
             \\label{a}\\label{a}\\ref{a}\\ref{nope}\\cite{known,unknown}\n\
             \\input{missing}\\includegraphics{nofig}\n\
             \\newcommand{\\r}[1]{\\ref{#1}}\n\
             \\bibliography{refs}\\end{document}",
        )
        .unwrap();
        let p = Project::open(&root);
        let issues = run(&p);
        assert_eq!(
            codes(&issues),
            vec![
                ("duplicate-label", Some("a")),
                ("duplicate-label", Some("a")),
                ("undefined-reference", Some("nope")),
                ("undefined-citation", Some("unknown")),
                ("missing-file", None),
                ("missing-file", None),
            ]
        );
        let text = std::fs::read_to_string(&root).unwrap();
        assert_eq!(&text[issues[2].range.clone()], "nope");
        assert_eq!(issues[2].line, 1);
        assert_eq!(issues[4].level, Level::Error);
    }

    #[test]
    fn citations_are_not_checked_without_a_readable_bibliography() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("main.tex");
        std::fs::write(&root, "\\cite{x}\\bibliography{gone}").unwrap();
        let issues = run(&Project::open(&root));
        assert_eq!(codes(&issues), vec![("missing-file", None)]);
    }

    #[test]
    fn checks_the_bibliography() {
        let src = "@article{a, title={T}, author={A}, year=2020}\n\
                   @article{a, title={T}, author={A}, journal={J}, year=2020, doi={doi.org/x}}\n\
                   @misc{m, doi={https://doi.org/10.1000/xyz}}\n\
                   @book{b, title={T}";
        let issues = check_bib(Path::new("refs.bib"), &bib::parse(src));
        assert_eq!(
            codes(&issues),
            vec![
                ("bib-duplicate-key", Some("a")),
                ("bib-missing-field", Some("a")),
                ("bib-duplicate-key", Some("a")),
                ("bib-invalid-doi", Some("a")),
                ("bib-unterminated", Some("b")),
            ]
        );
        assert_eq!(issues[1].message, "@article a has no journal");
        assert_eq!(&src[issues[3].range.clone()], "doi.org/x");
    }

    #[test]
    fn tells_dois() {
        assert!(valid_doi("10.1145/3368089.3409696"));
        assert!(valid_doi("https://doi.org/10.1000/xyz"));
        assert!(!valid_doi("10.1145"));
        assert!(!valid_doi("11.1/x"));
        assert!(!valid_doi("10.ab/x"));
    }

    #[test]
    fn tells_dois_written_with_a_prefix() {
        for doi in [
            "doi: 10.1000/xyz",
            "DOI:10.1000/xyz",
            "http://doi.org/10.1000/xyz",
            "http://dx.doi.org/10.1000/xyz",
            "https://dx.doi.org/10.1000/xyz",
            "https://doi.org/ 10.1000/xyz",
            " 10.1000.10/xyz ",
        ] {
            assert!(valid_doi(doi), "{doi}");
        }
        for not in [
            "10../x",
            "10./x",
            "10.1./x",
            "10.1..2/x",
            "doi:",
            "10.1000/",
            "10.1000/a b",
        ] {
            assert!(!valid_doi(not), "{not}");
        }
    }

    #[test]
    fn bibitems_pictures_and_macro_arguments() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("main.tex");
        std::fs::create_dir(dir.path().join("img")).unwrap();
        std::fs::write(dir.path().join("img/plot.png"), "").unwrap();
        std::fs::write(
            &root,
            "\\graphicspath{{img/}}\\includegraphics{plot}\\includegraphics{gone}\n\
             \\begin{thebibliography}{9}\\bibitem{k} K.\\end{thebibliography}\n\
             \\cite{k}\\cite{nope}\\ref{#1}\\ref{\\x}\\label{a*}\\label{a*}\\input{#1}\n\
             \\bibliography{missing}",
        )
        .unwrap();
        let issues = run(&Project::open(&root));
        let got: Vec<_> = issues.iter().map(|i| (i.code, i.level, i.line)).collect();
        // `\bibitem` keys are known, but a missing .bib stops citation checks.
        assert_eq!(
            got,
            [
                ("missing-file", Level::Warning, 0),
                ("missing-file", Level::Warning, 3),
            ]
        );
        assert_eq!(issues[0].message, "Picture gone not found");
        assert_eq!(issues[1].message, "Bibliography missing not found");

        // With only `\bibitem`s, citations are checked against them.
        std::fs::write(&root, "\\bibitem{k} K.\n\\cite{k,nope}").unwrap();
        let issues = run(&Project::open(&root));
        assert_eq!(codes(&issues), [("undefined-citation", Some("nope"))]);
        assert_eq!(issues[0].line, 1);
    }

    #[test]
    fn knows_the_fields_each_entry_type_needs() {
        let src = "@book{b, editor={E}, title={T}, publisher={P}, date={2020}}\n\
                   @inproceedings{i, author={A}, title={T}, year={2020}}\n\
                   @phdthesis{p, author={A}, title={T}, institution={I}, year={2020}}\n\
                   @thesis{t, author={A}, title={T}, institution={I}, year={2020}}\n\
                   @report{r, author={A}, title={T}, type={T}, institution={}, year={2020}}\n\
                   @unknown{u}\n@misc{m}";
        let issues = check_bib(Path::new("refs.bib"), &bib::parse(src));
        let messages: Vec<_> = issues.iter().map(|i| i.message.as_str()).collect();
        assert_eq!(
            messages,
            [
                "@inproceedings i has no booktitle",
                "@thesis t has no type",
                "@report r has no institution",
            ]
        );
        assert_eq!(issues[0].line, 1);
        assert!(issues.iter().all(|i| i.level == Level::Warning));
    }
}
