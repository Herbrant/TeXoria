//! Builds the website's documentation pages from the Markdown of `docs/`, which stays their only
//! source (it reads well on GitHub too). `docs/README.md` is the index, and its lists of links,
//! under a `## Section` each, are the sidebar: every page must be listed there. Headings get
//! GitHub's ids, so the same anchors work in both places. Links between pages become links
//! between the HTML pages, links to the rest of the repository go to GitHub, and a link to a
//! missing file or heading fails the build. Run by `scripts/build-website.sh`.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use anyhow::{Context as _, Result, anyhow, bail};
use pulldown_cmark::{Event, HeadingLevel, LinkType, Options, Parser, Tag, TagEnd};

/// The repository: links outside `docs/` and "Edit this page" go there.
pub const REPO_URL: &str = "https://github.com/Herbrant/TeXoria";

/// The index page, under `docs/`.
const INDEX: &str = "README.md";

/// The template's placeholders, `{{name}}`.
const PLACEHOLDERS: [&str; 9] = [
    "title",
    "description",
    "root",
    "path",
    "nav",
    "toc",
    "content",
    "pager",
    "edit",
];

/// `site-docs <docs dir> <template> <out dir>`.
pub fn run(args: impl IntoIterator<Item = String>) -> Result<()> {
    let args: Vec<String> = args.into_iter().collect();
    let [docs, template, out] = args.as_slice() else {
        bail!("usage: site-docs <docs dir> <template> <out dir>");
    };
    build(Path::new(docs), Path::new(template), Path::new(out))
}

/// Write the HTML page of every Markdown file of `docs` (in a repository, its parent) into
/// `out`, with the same layout (`README.md` becomes `index.html`), through the template.
pub fn build(docs: &Path, template: &Path, out: &Path) -> Result<()> {
    let template =
        fs::read_to_string(template).with_context(|| format!("reading {}", template.display()))?;
    let site = Site::load(docs, template)?;
    let mut pages = Vec::new();
    let mut errors = Vec::new();
    for i in 0..site.pages.len() {
        match site.page_html(i) {
            Ok(html) => pages.push((html_path(&site.pages[i].rel), html)),
            Err(found) => {
                let rel = &site.pages[i].rel;
                errors.extend(found.into_iter().map(|e| format!("docs/{rel}: {e}")));
            }
        }
    }
    if !errors.is_empty() {
        bail!("{}", errors.join("\n"));
    }
    for (path, html) in pages {
        let file = out.join(path);
        if let Some(dir) = file.parent() {
            fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        fs::write(&file, html).with_context(|| format!("writing {}", file.display()))?;
    }
    Ok(())
}

struct Page {
    /// The path under `docs/`, with `/`: `writing.md`, `development/testing.md`.
    rel: String,
    source: String,
    headings: Vec<Heading>,
    /// The first `#` heading.
    title: String,
    /// The first paragraph before the first `##`, else the title.
    description: String,
    /// Its name in the sidebar: the text of its link in the index.
    label: String,
}

struct Heading {
    level: HeadingLevel,
    text: String,
    id: String,
}

struct Section {
    title: String,
    /// Indexes in `Site::pages`.
    pages: Vec<usize>,
}

struct Site {
    /// The repository.
    root: std::path::PathBuf,
    /// The index first, then the sidebar's order (which "Previous" and "Next" follow).
    pages: Vec<Page>,
    nav: Vec<Section>,
    by_rel: HashMap<String, usize>,
    template: String,
}

impl Site {
    fn load(docs: &Path, template: String) -> Result<Self> {
        fill(&template, &PLACEHOLDERS.map(|name| (name, String::new()))).map_err(|e| anyhow!(e))?;
        let mut files = Vec::new();
        markdown_files(docs, "", &mut files)?;
        let mut found = HashMap::new();
        for rel in files {
            let path = docs.join(&rel);
            let source =
                fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
            let page = Page::new(rel.clone(), source).with_context(|| format!("docs/{rel}"))?;
            found.insert(rel, page);
        }

        let mut index = found
            .remove(INDEX)
            .with_context(|| format!("{} has no {INDEX}", docs.display()))?;
        index.label = "Overview".to_string();
        let sections = nav(&index.source);
        let mut pages = vec![index];
        let mut nav = Vec::new();
        let mut errors = Vec::new();
        for (title, links) in sections {
            let mut section = Section {
                title,
                pages: Vec::new(),
            };
            for (text, dest) in links {
                let rel = resolve(&format!("docs/{INDEX}"), &dest)
                    .and_then(|path| path.strip_prefix("docs/").map(str::to_string));
                match rel.and_then(|rel| found.remove(&rel)) {
                    Some(mut page) => {
                        page.label = text;
                        section.pages.push(pages.len());
                        pages.push(page);
                    }
                    None => errors.push(format!(
                        "docs/{INDEX}: {dest} is not a page of docs/, or is listed twice"
                    )),
                }
            }
            nav.push(section);
        }
        let mut unlisted: Vec<_> = found.into_keys().collect();
        unlisted.sort();
        for rel in unlisted {
            errors.push(format!("docs/{rel} is not listed in docs/{INDEX}"));
        }
        if !errors.is_empty() {
            bail!("{}", errors.join("\n"));
        }

        let by_rel = pages
            .iter()
            .enumerate()
            .map(|(i, p)| (p.rel.clone(), i))
            .collect();
        Ok(Self {
            root: docs.parent().unwrap_or(Path::new(".")).to_path_buf(),
            pages,
            nav,
            by_rel,
            template,
        })
    }

    fn page_html(&self, i: usize) -> Result<String, Vec<String>> {
        let page = &self.pages[i];
        let path = html_path(&page.rel);
        let content = self.render(page)?;
        let values = [
            ("title", escape(&page.title)),
            ("description", escape(&page.description)),
            ("root", "../".repeat(path.matches('/').count() + 1)),
            ("path", escape(&format!("docs/{path}"))),
            ("nav", self.nav_html(i)),
            ("toc", toc_html(page)),
            ("content", content),
            ("pager", self.pager_html(i)),
            (
                "edit",
                escape(&format!("{REPO_URL}/edit/main/docs/{}", page.rel)),
            ),
        ];
        fill(&self.template, &values).map_err(|e| vec![e])
    }

    /// The page's Markdown as HTML: headings with their ids and an anchor, links rewritten,
    /// tables in a scrolling box.
    fn render(&self, page: &Page) -> Result<String, Vec<String>> {
        let mut errors = Vec::new();
        let mut headings = page.headings.iter();
        let mut heading = None;
        let mut events = Vec::new();
        for event in Parser::new_ext(&page.source, options()) {
            match event {
                Event::Start(Tag::Heading {
                    level,
                    classes,
                    attrs,
                    ..
                }) => {
                    let h = headings
                        .next()
                        .expect("the same parse finds the same headings");
                    heading = Some(h);
                    let id = Some(h.id.clone().into());
                    events.push(Event::Start(Tag::Heading {
                        level,
                        id,
                        classes,
                        attrs,
                    }));
                }
                Event::End(TagEnd::Heading(level)) => {
                    if let Some(h) = heading.take()
                        && level != HeadingLevel::H1
                    {
                        let anchor = format!(
                            r##"<a class="anchor" href="#{}" aria-label="Link to this section"></a>"##,
                            escape(&h.id)
                        );
                        events.push(Event::InlineHtml(anchor.into()));
                    }
                    events.push(Event::End(TagEnd::Heading(level)));
                }
                Event::Start(Tag::Link {
                    link_type,
                    dest_url,
                    title,
                    id,
                }) if link_type != LinkType::Email => {
                    let dest_url = match self.link(page, &dest_url, false) {
                        Ok(dest) => dest.into(),
                        Err(e) => {
                            errors.push(e);
                            dest_url
                        }
                    };
                    events.push(Event::Start(Tag::Link {
                        link_type,
                        dest_url,
                        title,
                        id,
                    }));
                }
                Event::Start(Tag::Image {
                    link_type,
                    dest_url,
                    title,
                    id,
                }) => {
                    let dest_url = match self.link(page, &dest_url, true) {
                        Ok(dest) => dest.into(),
                        Err(e) => {
                            errors.push(e);
                            dest_url
                        }
                    };
                    events.push(Event::Start(Tag::Image {
                        link_type,
                        dest_url,
                        title,
                        id,
                    }));
                }
                Event::Start(Tag::Table(alignments)) => {
                    events.push(Event::Html(r#"<div class="table">"#.into()));
                    events.push(Event::Start(Tag::Table(alignments)));
                }
                end @ Event::End(TagEnd::Table) => {
                    events.push(end);
                    events.push(Event::Html("</div>\n".into()));
                }
                event => events.push(event),
            }
        }
        if !errors.is_empty() {
            return Err(errors);
        }
        let mut html = String::new();
        pulldown_cmark::html::push_html(&mut html, events.into_iter());
        Ok(html)
    }

    /// Where a link of `page` to `dest` goes on the website: another page's HTML, GitHub for
    /// the rest of the repository (its raw file for an `image`), unchanged for a URL. An error
    /// if it leads to no file or no heading.
    fn link(&self, page: &Page, dest: &str, image: bool) -> Result<String, String> {
        if is_url(dest) {
            return Ok(dest.to_string());
        }
        let (path, fragment) = match dest.split_once('#') {
            Some((path, fragment)) => (path, Some(fragment)),
            None => (dest, None),
        };
        if path.is_empty() {
            let fragment = fragment.unwrap_or_default();
            return match page.has_heading(fragment) {
                true => Ok(dest.to_string()),
                false => Err(format!("{dest}: no such heading")),
            };
        }
        let target = resolve(&format!("docs/{}", page.rel), path)
            .ok_or_else(|| format!("{dest}: outside the repository"))?;
        let other = target
            .strip_prefix("docs/")
            .and_then(|rel| self.by_rel.get(rel));
        let mut href = match other {
            Some(&i) => {
                let other = &self.pages[i];
                if let Some(fragment) = fragment
                    && !other.has_heading(fragment)
                {
                    return Err(format!("{dest}: docs/{} has no such heading", other.rel));
                }
                relative(&html_path(&page.rel), &html_path(&other.rel))
            }
            None => {
                let file = self.root.join(&target);
                let kind = if !file.exists() {
                    return Err(format!("{dest}: {target} does not exist"));
                } else if image {
                    "raw"
                } else if file.is_dir() {
                    "tree"
                } else {
                    "blob"
                };
                format!("{REPO_URL}/{kind}/main/{target}")
            }
        };
        if let Some(fragment) = fragment {
            href.push('#');
            href.push_str(fragment);
        }
        Ok(href)
    }

    /// The sidebar's lists, `current`'s link marked.
    fn nav_html(&self, current: usize) -> String {
        let from = html_path(&self.pages[current].rel);
        let item = |i: usize| {
            let page = &self.pages[i];
            let href = relative(&from, &html_path(&page.rel));
            let mark = if i == current {
                r#" aria-current="page""#
            } else {
                ""
            };
            format!(
                r#"<li><a href="{}"{mark}>{}</a></li>"#,
                escape(&href),
                escape(&page.label)
            )
        };
        let mut html = format!("<ul>{}</ul>\n", item(0));
        for section in &self.nav {
            html.push_str(&format!(
                "<p class=\"docs-nav-title\">{}</p>\n<ul>",
                escape(&section.title)
            ));
            for &i in &section.pages {
                html.push_str(&item(i));
            }
            html.push_str("</ul>\n");
        }
        html
    }

    /// Links to the pages before and after `i` in the sidebar.
    fn pager_html(&self, i: usize) -> String {
        let from = html_path(&self.pages[i].rel);
        let link = |j: usize, class: &str, text: &str| {
            let page = &self.pages[j];
            format!(
                r#"<a class="{class}" href="{}" rel="{class}"><span>{text}</span>{}</a>"#,
                escape(&relative(&from, &html_path(&page.rel))),
                escape(&page.label)
            )
        };
        let mut html = String::from(r#"<nav class="pager" aria-label="Previous and next pages">"#);
        if i > 0 {
            html.push_str(&link(i - 1, "prev", "Previous"));
        }
        if i + 1 < self.pages.len() {
            html.push_str(&link(i + 1, "next", "Next"));
        }
        html.push_str("</nav>");
        html
    }
}

impl Page {
    fn new(rel: String, source: String) -> Result<Self> {
        let headings = headings(&source);
        let title = headings
            .iter()
            .find(|h| h.level == HeadingLevel::H1)
            .map(|h| h.text.clone())
            .context("no `# title`")?;
        let description = description(&source).unwrap_or_else(|| title.clone());
        Ok(Self {
            rel,
            source,
            headings,
            title,
            description,
            label: String::new(),
        })
    }

    fn has_heading(&self, id: &str) -> bool {
        self.headings.iter().any(|h| h.id == id)
    }
}

/// The "On this page" list of the `##` headings, if there are at least two.
fn toc_html(page: &Page) -> String {
    let items: Vec<String> = page
        .headings
        .iter()
        .filter(|h| h.level == HeadingLevel::H2)
        .map(|h| {
            format!(
                r##"<li><a href="#{}">{}</a></li>"##,
                escape(&h.id),
                escape(&h.text)
            )
        })
        .collect();
    if items.len() < 2 {
        return String::new();
    }
    format!(
        "<nav class=\"docs-toc\" aria-label=\"On this page\">\n\
         <p class=\"docs-nav-title\">On this page</p>\n<ul>{}</ul>\n</nav>",
        items.concat()
    )
}

fn options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES
}

/// The headings of a page, with their text and their ids.
fn headings(source: &str) -> Vec<Heading> {
    let mut slugger = Slugger::default();
    let mut found = Vec::new();
    let mut current: Option<(HeadingLevel, String)> = None;
    for event in Parser::new_ext(source, options()) {
        match event {
            Event::Start(Tag::Heading { level, .. }) => current = Some((level, String::new())),
            Event::End(TagEnd::Heading(_)) => {
                if let Some((level, text)) = current.take() {
                    let id = slugger.id(&text);
                    found.push(Heading { level, text, id });
                }
            }
            Event::Text(text) | Event::Code(text) => {
                if let Some((_, heading)) = &mut current {
                    heading.push_str(&text);
                }
            }
            _ => {}
        }
    }
    found
}

/// The text of the first paragraph before the first `##` heading, shortened for a
/// `<meta name="description">`.
fn description(source: &str) -> Option<String> {
    let mut text = None;
    for event in Parser::new_ext(source, options()) {
        match (&mut text, event) {
            (None, Event::Start(Tag::Heading { level, .. })) if level != HeadingLevel::H1 => {
                return None;
            }
            (None, Event::Start(Tag::Paragraph)) => text = Some(String::new()),
            (Some(text), Event::End(TagEnd::Paragraph)) => return Some(shorten(text, 160)),
            (Some(text), Event::Text(t) | Event::Code(t)) => text.push_str(&t),
            (Some(text), Event::SoftBreak | Event::HardBreak) => text.push(' '),
            _ => {}
        }
    }
    None
}

/// `text` cut at a word to at most `max` characters, with an ellipsis if it was cut.
fn shorten(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let cut: String = text.chars().take(max - 1).collect();
    // Drop the last word unless it ends right there.
    let cut = match text[cut.len()..].starts_with(' ') {
        true => cut.as_str(),
        false => cut.rsplit_once(' ').map_or(cut.as_str(), |(head, _)| head),
    };
    format!("{}…", cut.trim_end_matches([',', ';', ':', '.', ' ']))
}

/// A heading's id as GitHub makes it: lowercase, without punctuation, spaces as hyphens.
pub fn slug(text: &str) -> String {
    text.trim()
        .to_lowercase()
        .chars()
        .filter_map(|c| match c {
            ' ' => Some('-'),
            '-' | '_' => Some(c),
            c if c.is_alphanumeric() => Some(c),
            _ => None,
        })
        .collect()
}

/// Unique ids in a page: GitHub numbers repeated ones `-1`, `-2`…
#[derive(Default)]
struct Slugger {
    seen: HashMap<String, usize>,
}

impl Slugger {
    fn id(&mut self, text: &str) -> String {
        let base = slug(text);
        let count = self.seen.entry(base.clone()).or_default();
        let id = match *count {
            0 => base,
            n => format!("{base}-{n}"),
        };
        *count += 1;
        id
    }
}

/// The index's `## Section`s, each with the (text, destination) of the first link of its list
/// items.
fn nav(index: &str) -> Vec<(String, Vec<(String, String)>)> {
    let mut sections: Vec<(String, Vec<(String, String)>)> = Vec::new();
    let mut heading = None;
    let mut link: Option<(String, String)> = None;
    let mut items = 0;
    let mut linked = false;
    for event in Parser::new_ext(index, options()) {
        match event {
            Event::Start(Tag::Heading {
                level: HeadingLevel::H2,
                ..
            }) => {
                heading = Some(String::new());
            }
            Event::End(TagEnd::Heading(HeadingLevel::H2)) => {
                if let Some(title) = heading.take() {
                    sections.push((title, Vec::new()));
                }
            }
            Event::Start(Tag::Item) => {
                items += 1;
                linked = false;
            }
            Event::End(TagEnd::Item) => items -= 1,
            Event::Start(Tag::Link { dest_url, .. })
                if items > 0 && !linked && !sections.is_empty() =>
            {
                link = Some((String::new(), dest_url.to_string()));
                linked = true;
            }
            Event::End(TagEnd::Link) => {
                if let (Some(link), Some((_, links))) = (link.take(), sections.last_mut()) {
                    links.push(link);
                }
            }
            Event::Text(text) | Event::Code(text) => {
                if let Some(heading) = &mut heading {
                    heading.push_str(&text);
                }
                if let Some((link, _)) = &mut link {
                    link.push_str(&text);
                }
            }
            _ => {}
        }
    }
    sections.retain(|(_, links)| !links.is_empty());
    sections
}

/// Whether `dest` is a URL (`https:`, `mailto:`, `//host`) rather than a path.
fn is_url(dest: &str) -> bool {
    dest.starts_with("//")
        || dest.split_once(':').is_some_and(|(scheme, _)| {
            scheme.starts_with(|c: char| c.is_ascii_alphabetic())
                && scheme
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
        })
}

/// The repository path (with `/`) that the relative `target` designates from the file `from`,
/// or `None` if it leaves the repository.
fn resolve(from: &str, target: &str) -> Option<String> {
    let mut parts: Vec<&str> = from.split('/').collect();
    parts.pop();
    for part in target.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            part => parts.push(part),
        }
    }
    Some(parts.join("/"))
}

/// The relative URL from the page at `from` to the one at `to` (both under the same folder).
fn relative(from: &str, to: &str) -> String {
    let mut dirs: Vec<&str> = from.split('/').collect();
    dirs.pop();
    let to: Vec<&str> = to.split('/').collect();
    let common = dirs
        .iter()
        .zip(&to[..to.len() - 1])
        .take_while(|(a, b)| a == b)
        .count();
    format!(
        "{}{}",
        "../".repeat(dirs.len() - common),
        to[common..].join("/")
    )
}

/// The HTML page of a Markdown file: `writing.md` → `writing.html`, `README.md` → `index.html`.
fn html_path(rel: &str) -> String {
    let stem = rel.strip_suffix(".md").unwrap_or(rel);
    match stem.rsplit_once('/') {
        Some((dir, "README")) => format!("{dir}/index.html"),
        None if stem == "README" => "index.html".to_string(),
        _ => format!("{stem}.html"),
    }
}

/// The `.md` files under `dir`, as paths under it with `/`.
fn markdown_files(dir: &Path, prefix: &str, files: &mut Vec<String>) -> Result<()> {
    let entries = fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))?;
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let name = name
            .to_str()
            .with_context(|| format!("{name:?} is not UTF-8"))?;
        let rel = match prefix {
            "" => name.to_string(),
            prefix => format!("{prefix}/{name}"),
        };
        if entry.file_type()?.is_dir() {
            markdown_files(&entry.path(), &rel, files)?;
        } else if name.ends_with(".md") {
            files.push(rel);
        }
    }
    Ok(())
}

/// `template` with each `{{name}}` replaced by its value; an error for an unknown name.
fn fill(template: &str, values: &[(&str, String)]) -> Result<String, String> {
    let mut html = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        html.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let end = after.find("}}").ok_or("the template has an unclosed {{")?;
        let name = after[..end].trim();
        let (_, value) = values
            .iter()
            .find(|(key, _)| *key == name)
            .ok_or_else(|| format!("the template has an unknown {{{{{name}}}}}"))?;
        html.push_str(value);
        rest = &after[end + 2..];
    }
    html.push_str(rest);
    Ok(html)
}

/// `text` escaped for HTML text and attribute values.
fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            c => escaped.push(c),
        }
    }
    escaped
}

#[cfg(test)]
mod tests;
