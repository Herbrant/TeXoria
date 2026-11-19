use super::*;

use std::path::PathBuf;

use test_support::{write, write_files};

const TEMPLATE: &str = "<title>{{title}}</title><meta content=\"{{description}}\">\
<base data-root=\"{{root}}\" data-path=\"{{path}}\"><nav>{{nav}}</nav>{{toc}}\
<main>{{content}}</main>{{pager}}<a href=\"{{ edit }}\">Edit</a>";

const INDEX_MD: &str = "# Docs

## Guide

- [Writing](writing.md): what you can do
- [Git](git.md)

## Development

- [Testing](development/testing.md) and [more](writing.md)

See also [CHANGELOG.md](../CHANGELOG.md).
";

/// A repository with `docs/` (the index, `writing.md`, `git.md`, `development/testing.md`
/// plus `files`) and `CHANGELOG.md`, `crates/tex-log/`.
fn repo(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    write_files(
        dir.path(),
        &[
            ("CHANGELOG.md", "# Changelog\n"),
            ("crates/tex-log/Cargo.toml", ""),
            ("docs/README.md", INDEX_MD),
            (
                "docs/writing.md",
                "# Writing\n\nHow to *write*.\n\n## Vim mode\n\nKeys.\n",
            ),
            ("docs/git.md", "# Git\n\n## One\n\n## Two\n"),
            (
                "docs/development/testing.md",
                "# Testing\n\nSee [Git](../git.md#two).\n",
            ),
        ],
    );
    write_files(dir.path(), files);
    dir
}

/// Build the repository's docs; the pages by path, or the error.
fn build_docs(repo: &Path) -> Result<HashMap<String, String>, String> {
    let template = write(repo, "template.html", TEMPLATE);
    let out = repo.join("out");
    build(&repo.join("docs"), &template, &out).map_err(|e| format!("{e:#}"))?;
    let mut pages = HashMap::new();
    collect(&out, "", &mut pages);
    Ok(pages)
}

fn collect(dir: &Path, prefix: &str, pages: &mut HashMap<String, String>) {
    for entry in fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let rel = format!("{prefix}{}", entry.file_name().to_str().unwrap());
        if entry.file_type().unwrap().is_dir() {
            collect(&entry.path(), &format!("{rel}/"), pages);
        } else {
            pages.insert(rel, fs::read_to_string(entry.path()).unwrap());
        }
    }
}

#[test]
fn slugs_follow_github() {
    assert_eq!(slug("Vim mode"), "vim-mode");
    assert_eq!(slug("Build and preview"), "build-and-preview");
    assert_eq!(slug("latexmkrc"), "latexmkrc");
    assert_eq!(slug("What's new? (v0.7)"), "whats-new-v07");
    assert_eq!(slug("Git → Overleaf"), "git--overleaf");
    assert_eq!(slug("snake_case and-hyphens"), "snake_case-and-hyphens");
    assert_eq!(slug("Éditeur Ünïcode"), "éditeur-ünïcode");
}

#[test]
fn repeated_headings_are_numbered() {
    let ids: Vec<String> = headings("# A\n\n## Notes\n\n## Notes\n\n### Notes\n\n## `code` *x*\n")
        .into_iter()
        .map(|h| h.id)
        .collect();
    assert_eq!(ids, ["a", "notes", "notes-1", "notes-2", "code-x"]);
}

#[test]
fn descriptions_come_from_the_introduction() {
    assert_eq!(
        description("# T\n\nSome `code`\nand **text**.\n\n## H\n\nLater.").as_deref(),
        Some("Some code and text.")
    );
    assert_eq!(description("# T\n\n## H\n\nLater.\n"), None);
    assert_eq!(description("# T\n\n- a list\n"), None);
    let long = format!("# T\n\n{}\n", "word ".repeat(50));
    let short = description(&long).unwrap();
    assert!(short.ends_with("word…"), "{short}");
    assert!(short.chars().count() <= 160);
    assert_eq!(shorten("a, b, c", 7), "a, b, c");
    assert_eq!(shorten("aa, bb, cc", 8), "aa, bb…");
    assert_eq!(shorten("aa bbbb", 5), "aa…");
}

#[test]
fn the_index_gives_the_sections() {
    let sections = nav(INDEX_MD);
    let titles: Vec<&str> = sections.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(titles, ["Guide", "Development"]);
    assert_eq!(
        sections[0].1,
        [
            ("Writing".to_string(), "writing.md".to_string()),
            ("Git".to_string(), "git.md".to_string())
        ]
    );
    // Only the first link of an item; none from a paragraph or before the first section.
    assert_eq!(
        sections[1].1,
        [("Testing".to_string(), "development/testing.md".to_string())]
    );
    assert!(nav("- [A](a.md)\n\n## Empty\n\nText.\n").is_empty());
}

#[test]
fn paths_resolve_within_the_repository() {
    assert_eq!(resolve("docs/a.md", "b.md").as_deref(), Some("docs/b.md"));
    assert_eq!(
        resolve("docs/dev/a.md", "../b.md").as_deref(),
        Some("docs/b.md")
    );
    assert_eq!(resolve("docs/a.md", "./../x/./y/").as_deref(), Some("x/y"));
    assert_eq!(resolve("docs/a.md", "../../x"), None);
    assert_eq!(relative("a.html", "b.html"), "b.html");
    assert_eq!(relative("dev/a.html", "b.html"), "../b.html");
    assert_eq!(relative("a.html", "dev/b.html"), "dev/b.html");
    assert_eq!(relative("dev/a.html", "dev/b.html"), "b.html");
    assert_eq!(relative("dev/a.html", "other/b.html"), "../other/b.html");
    assert_eq!(html_path("README.md"), "index.html");
    assert_eq!(html_path("dev/README.md"), "dev/index.html");
    assert_eq!(html_path("dev/a.md"), "dev/a.html");
}

#[test]
fn urls_are_told_from_paths() {
    for url in ["https://x.org", "mailto:a@b.c", "//cdn.org/x", "a+b.c-d:x"] {
        assert!(is_url(url), "{url}");
    }
    for path in ["a.md", "../a.md#x:y", "#x", "1x:y", "a/b:c", ""] {
        assert!(!is_url(path), "{path}");
    }
}

#[test]
fn the_template_is_filled() {
    let values = [("a", "1".to_string()), ("b", "{{a}}".to_string())];
    assert_eq!(fill("<{{a}}|{{ b }}>", &values).unwrap(), "<1|{{a}}>");
    assert!(fill("{{c}}", &values).unwrap_err().contains("{{c}}"));
    assert!(fill("{{a", &values).unwrap_err().contains("unclosed"));
    assert_eq!(
        escape(r#"<a href="x">Tom & 'Jerry'</a>"#),
        "&lt;a href=&quot;x&quot;&gt;Tom &amp; &#39;Jerry&#39;&lt;/a&gt;"
    );
}

#[test]
fn builds_every_page() {
    let repo = repo(&[]);
    let pages = build_docs(repo.path()).unwrap();
    let mut paths: Vec<&str> = pages.keys().map(String::as_str).collect();
    paths.sort();
    assert_eq!(
        paths,
        [
            "development/testing.html",
            "git.html",
            "index.html",
            "writing.html"
        ]
    );

    let writing = &pages["writing.html"];
    assert!(writing.contains("<title>Writing</title>"), "{writing}");
    assert!(writing.contains(r#"<meta content="How to write.">"#));
    assert!(writing.contains(r#"data-root="../" data-path="docs/writing.html""#));
    assert!(writing.contains(
        r##"<h2 id="vim-mode">Vim mode<a class="anchor" href="#vim-mode" aria-label="Link to this section"></a></h2>"##
    ));
    assert!(writing.contains(r#"<h1 id="writing">Writing</h1>"#));
    assert!(writing.contains(&format!(r#"href="{REPO_URL}/edit/main/docs/writing.md""#)));
    // One `##` heading: no "On this page".
    assert!(!writing.contains("docs-toc"));

    // The sidebar: the index, then the sections, the current page marked.
    assert!(writing.contains(
        "<nav><ul><li><a href=\"index.html\">Overview</a></li></ul>\n\
         <p class=\"docs-nav-title\">Guide</p>\n\
         <ul><li><a href=\"writing.html\" aria-current=\"page\">Writing</a></li>\
         <li><a href=\"git.html\">Git</a></li></ul>\n\
         <p class=\"docs-nav-title\">Development</p>\n\
         <ul><li><a href=\"development/testing.html\">Testing</a></li></ul>\n</nav>"
    ));
    // Previous and next follow the sidebar.
    assert!(writing.contains(
        r#"<a class="prev" href="index.html" rel="prev"><span>Previous</span>Overview</a><a class="next" href="git.html" rel="next"><span>Next</span>Git</a>"#
    ));

    let testing = &pages["development/testing.html"];
    assert!(testing.contains(r#"data-root="../../""#));
    assert!(
        testing.contains(r##"<a href="../git.html#two">Git</a>"##),
        "{testing}"
    );
    assert!(testing.contains(r#"<li><a href="../index.html">Overview</a></li>"#));
    assert!(testing.contains(r#"class="prev" href="../git.html""#));
    assert!(!testing.contains(r#"class="next""#));

    let git = &pages["git.html"];
    assert!(git.contains(r##"<li><a href="#one">One</a></li><li><a href="#two">Two</a></li>"##));
    // A page without an introduction is described by its title.
    assert!(git.contains(r#"<meta content="Git">"#));

    let index = &pages["index.html"];
    assert!(index.contains(r#"data-root="../""#));
    assert!(index.contains(r#"<a href="writing.html">Writing</a>: what you can do"#));
    assert!(index.contains(&format!(r#"<a href="{REPO_URL}/blob/main/CHANGELOG.md">"#)));
    assert!(!index.contains(r#"class="prev""#));
}

#[test]
fn links_are_rewritten() {
    let repo = repo(&[(
        "docs/links.md",
        "# Links\n\n## Here\n\n\
         [web](https://example.org/a.md) [mail](mailto:a@b.c) <a@b.c> [here](#here)\n\
         [index](README.md) [anchor](writing.md#vim-mode) [crate](../crates/tex-log/)\n\
         [file](../crates/tex-log/Cargo.toml#L1) ![logo](../CHANGELOG.md)\n\n\
         | a | b |\n|---|---|\n| 1 | 2 |\n",
    )]);
    write(
        repo.path(),
        "docs/README.md",
        format!("{INDEX_MD}\n## More\n\n- [Links](links.md)\n"),
    );
    let pages = build_docs(repo.path()).unwrap();
    let links = &pages["links.html"];
    for expected in [
        r#"<a href="https://example.org/a.md">web</a>"#,
        r#"<a href="mailto:a@b.c">mail</a>"#,
        r#"<a href="mailto:a@b.c">a@b.c</a>"#,
        r##"<a href="#here">here</a>"##,
        r#"<a href="index.html">index</a>"#,
        r##"<a href="writing.html#vim-mode">anchor</a>"##,
        &format!(r#"<a href="{REPO_URL}/tree/main/crates/tex-log">crate</a>"#),
        &format!(r#"<a href="{REPO_URL}/blob/main/crates/tex-log/Cargo.toml#L1">file</a>"#),
        &format!(r#"<img src="{REPO_URL}/raw/main/CHANGELOG.md" alt="logo" />"#),
        "<div class=\"table\"><table>",
        "</table>\n</div>",
    ] {
        assert!(links.contains(expected), "{expected} in {links}");
    }
}

#[test]
fn broken_links_fail_the_build() {
    let repo = repo(&[(
        "docs/development/testing.md",
        "# Testing\n\n[a](missing.md) [b](../git.md#three) [c](#nowhere) [d](../../../up)\n\
         ![e](missing.png)\n",
    )]);
    let error = build_docs(repo.path()).unwrap_err();
    for expected in [
        "docs/development/testing.md: missing.md: docs/development/missing.md does not exist",
        "docs/development/testing.md: ../git.md#three: docs/git.md has no such heading",
        "docs/development/testing.md: #nowhere: no such heading",
        "docs/development/testing.md: ../../../up: outside the repository",
        "docs/development/testing.md: missing.png: docs/development/missing.png does not exist",
    ] {
        assert!(error.contains(expected), "{expected} in {error}");
    }
}

#[test]
fn every_page_is_listed_once() {
    let repo = repo(&[
        ("docs/extra.md", "# Extra\n"),
        ("docs/notes.txt", "not a page"),
        (
            "docs/README.md",
            "# Docs\n\n## A\n\n- [Writing](writing.md)\n- [Again](writing.md)\n\
             - [Missing](missing.md)\n- [Out](../CHANGELOG.md)\n\
             - [Git](git.md)\n- [Testing](development/testing.md)\n",
        ),
    ]);
    let error = build_docs(repo.path()).unwrap_err();
    for expected in [
        "docs/README.md: writing.md is not a page of docs/, or is listed twice",
        "docs/README.md: missing.md is not a page",
        "docs/README.md: ../CHANGELOG.md is not a page",
        "docs/extra.md is not listed in docs/README.md",
    ] {
        assert!(error.contains(expected), "{expected} in {error}");
    }
}

#[test]
fn bad_inputs_are_errors() {
    let untitled = repo(&[("docs/git.md", "No title.\n")]);
    assert!(
        build_docs(untitled.path())
            .unwrap_err()
            .contains("docs/git.md: no `# title`")
    );

    let no_index = repo(&[]);
    fs::remove_file(no_index.path().join("docs/README.md")).unwrap();
    assert!(
        build_docs(no_index.path())
            .unwrap_err()
            .contains("has no README.md")
    );

    let dir = repo(&[]);
    let docs = dir.path().join("docs");
    let out = dir.path().join("out");
    let bad = write(dir.path(), "bad.html", "{{nope}}");
    let error = build(&docs, &bad, &out).unwrap_err();
    assert!(error.to_string().contains("unknown {{nope}}"), "{error}");
    let error = build(&docs, &dir.path().join("none.html"), &out).unwrap_err();
    assert!(format!("{error:#}").contains("none.html"), "{error:#}");
    let good = write(dir.path(), "good.html", TEMPLATE);
    let error = build(&dir.path().join("nothing"), &good, &out).unwrap_err();
    assert!(format!("{error:#}").contains("nothing"), "{error:#}");

    assert!(
        run(["a".to_string()])
            .unwrap_err()
            .to_string()
            .starts_with("usage:")
    );
}

#[test]
fn run_builds_into_the_output_folder() {
    let repo = repo(&[]);
    let template = write(repo.path(), "template.html", TEMPLATE);
    let out = repo.path().join("site/docs");
    let args =
        [repo.path().join("docs"), template, out.clone()].map(|p| p.to_str().unwrap().to_string());
    run(args).unwrap();
    assert!(out.join("development/testing.html").is_file());
}

/// The repository's own documentation builds with the website's template: no broken links,
/// every page listed in docs/README.md.
#[test]
fn the_repository_docs_build() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let out = tempfile::tempdir().unwrap();
    build(
        &root.join("docs"),
        &root.join("website/doc-template.html"),
        out.path(),
    )
    .unwrap();
    let index = fs::read_to_string(out.path().join("index.html")).unwrap();
    assert!(index.contains(r#"href="development/building.html""#));
    let writing = fs::read_to_string(out.path().join("writing.html")).unwrap();
    assert!(writing.contains(r#"id="vim-mode""#));
}
