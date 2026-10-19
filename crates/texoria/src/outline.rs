//! The outline's groups: each heading holds the deeper items that follow it, and its group is
//! collapsed until opened.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use tex_project::ProjectOutlineItem;
use tex_syntax::OutlineKind;

/// A heading, independently of its line: the outline is rebuilt on every edit.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Key {
    file: PathBuf,
    kind: OutlineKind,
    title: String,
    /// Among the headings of the same file, kind and title.
    nth: usize,
}

/// Which groups are open.
#[derive(Debug, Default)]
pub struct Fold {
    /// Groups are open unless toggled.
    expand_all: bool,
    toggled: HashSet<Key>,
}

impl Fold {
    pub fn is_open(&self, key: &Key) -> bool {
        self.expand_all != self.toggled.contains(key)
    }

    pub fn toggle(&mut self, key: Key) {
        if !self.toggled.remove(&key) {
            self.toggled.insert(key);
        }
    }

    /// Open or close every group, those that appear later included.
    pub fn set_all(&mut self, open: bool) {
        self.expand_all = open;
        self.toggled.clear();
    }
}

/// An outline item, as laid out in the panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// Its index in the outline.
    pub index: usize,
    /// Indentation level, 0 for the outermost items.
    pub depth: u8,
    /// Set for a heading that has items under it.
    pub group: Option<Key>,
    /// Its group is open.
    pub open: bool,
    /// Not inside a closed group.
    pub visible: bool,
    /// The row of the heading it is under.
    parent: Option<usize>,
}

/// One row per outline item.
pub fn layout(outline: &[ProjectOutlineItem], fold: &Fold) -> Vec<Row> {
    let min_depth = outline
        .iter()
        .map(|o| o.item.kind.depth())
        .min()
        .unwrap_or(0);
    // Floats sit one level below the preceding heading.
    let mut heading = None;
    let depths: Vec<u8> = outline
        .iter()
        .map(|o| {
            let kind = o.item.kind;
            let depth = if kind.is_float() {
                heading.map_or(0, |d| d + 1)
            } else {
                heading = Some(kind.depth());
                kind.depth()
            };
            depth.saturating_sub(min_depth)
        })
        .collect();

    let mut seen: HashMap<Key, usize> = HashMap::new();
    let mut rows: Vec<Row> = Vec::with_capacity(outline.len());
    // The headings enclosing the current item, innermost last.
    let mut stack: Vec<usize> = Vec::new();
    for (i, o) in outline.iter().enumerate() {
        let depth = depths[i];
        while stack.last().is_some_and(|&p| rows[p].depth >= depth) {
            stack.pop();
        }
        let parent = stack.last().copied();
        let group = depths.get(i + 1).is_some_and(|&d| d > depth).then(|| {
            let mut key = Key {
                file: o.file.clone(),
                kind: o.item.kind,
                title: o.item.title.clone(),
                nth: 0,
            };
            let n = seen.entry(key.clone()).or_default();
            key.nth = *n;
            *n += 1;
            key
        });
        let open = group.as_ref().is_some_and(|k| fold.is_open(k));
        let visible = parent.is_none_or(|p| rows[p].visible && rows[p].open);
        rows.push(Row {
            index: i,
            depth,
            group,
            open,
            visible,
            parent,
        });
        stack.push(i);
    }
    rows
}

/// The row that stands for item `index`: itself, or the closest visible heading around it.
pub fn shown(rows: &[Row], index: usize) -> Option<usize> {
    let mut row = rows.get(index)?;
    while !row.visible {
        row = &rows[row.parent?];
    }
    Some(row.index)
}

/// Some group on screen is open.
pub fn any_open(rows: &[Row]) -> bool {
    rows.iter().any(|r| r.visible && r.open)
}

#[cfg(test)]
mod tests {
    use super::*;
    use OutlineKind::*;
    use tex_syntax::OutlineItem;

    fn item(file: &str, kind: OutlineKind, title: &str, line: u32) -> ProjectOutlineItem {
        ProjectOutlineItem {
            file: PathBuf::from(file),
            item: OutlineItem {
                kind,
                title: title.into(),
                starred: false,
                line,
                byte: 0,
            },
        }
    }

    fn article() -> Vec<ProjectOutlineItem> {
        vec![
            item("main.tex", Section, "Intro", 1),
            item("main.tex", Subsection, "Aims", 2),
            item("main.tex", Figure, "Plot", 3),
            item("main.tex", Subsubsection, "Detail", 4),
            item("main.tex", Section, "Results", 5),
            item("main.tex", Table, "Data", 6),
            item("main.tex", Section, "End", 7),
        ]
    }

    fn visible(outline: &[ProjectOutlineItem], fold: &Fold) -> Vec<String> {
        layout(outline, fold)
            .into_iter()
            .filter(|r| r.visible)
            .map(|r| outline[r.index].item.title.clone())
            .collect()
    }

    fn group(outline: &[ProjectOutlineItem], index: usize) -> Key {
        layout(outline, &Fold::default())[index]
            .group
            .clone()
            .expect("a group")
    }

    #[test]
    fn items_nest_under_their_headings() {
        let outline = article();
        let rows = layout(&outline, &Fold::default());
        let depths: Vec<u8> = rows.iter().map(|r| r.depth).collect();
        assert_eq!(depths, [0, 1, 2, 2, 0, 1, 0]);
        let parents: Vec<_> = rows.iter().map(|r| r.parent).collect();
        assert_eq!(
            parents,
            [None, Some(0), Some(1), Some(1), None, Some(4), None]
        );
        let groups: Vec<bool> = rows.iter().map(|r| r.group.is_some()).collect();
        assert_eq!(groups, [true, true, false, false, true, false, false]);
    }

    #[test]
    fn groups_start_collapsed() {
        let outline = article();
        let fold = Fold::default();
        assert_eq!(visible(&outline, &fold), ["Intro", "Results", "End"]);
        assert!(!any_open(&layout(&outline, &fold)));
    }

    #[test]
    fn groups_open_one_level_at_a_time() {
        let outline = article();
        let mut fold = Fold::default();
        fold.toggle(group(&outline, 0));
        assert_eq!(
            visible(&outline, &fold),
            ["Intro", "Aims", "Results", "End"]
        );
        assert!(any_open(&layout(&outline, &fold)));
        fold.toggle(group(&outline, 1));
        assert_eq!(
            visible(&outline, &fold),
            ["Intro", "Aims", "Plot", "Detail", "Results", "End"]
        );
        // Closing the outer group hides the open inner one, which stays open.
        fold.toggle(group(&outline, 0));
        assert_eq!(visible(&outline, &fold), ["Intro", "Results", "End"]);
        assert!(!any_open(&layout(&outline, &fold)));
        fold.toggle(group(&outline, 0));
        assert_eq!(visible(&outline, &fold).len(), 6);
    }

    #[test]
    fn expand_and_collapse_all() {
        let outline = article();
        let mut fold = Fold::default();
        fold.set_all(true);
        assert_eq!(visible(&outline, &fold).len(), outline.len());
        fold.toggle(group(&outline, 4));
        assert_eq!(visible(&outline, &fold).len(), outline.len() - 1);
        // A section added later is open too.
        let mut more = outline.clone();
        more.push(item("main.tex", Subsection, "Coda", 8));
        assert_eq!(visible(&more, &fold).len(), more.len() - 1);
        fold.set_all(false);
        assert_eq!(visible(&outline, &fold), ["Intro", "Results", "End"]);
    }

    #[test]
    fn groups_survive_reindexing() {
        let outline = article();
        let mut fold = Fold::default();
        fold.toggle(group(&outline, 4));
        // A heading added above moves every line.
        let mut edited = vec![item("main.tex", Section, "Preface", 0)];
        edited.extend(outline.into_iter().map(|mut o| {
            o.item.line += 3;
            o
        }));
        assert_eq!(
            visible(&edited, &fold),
            ["Preface", "Intro", "Results", "Data", "End"]
        );
    }

    #[test]
    fn same_titles_are_distinct_groups() {
        let outline = vec![
            item("a.tex", Section, "Notes", 0),
            item("a.tex", Subsection, "One", 1),
            item("a.tex", Section, "Notes", 2),
            item("a.tex", Subsection, "Two", 3),
            item("b.tex", Section, "Notes", 0),
            item("b.tex", Subsection, "Three", 1),
        ];
        let mut fold = Fold::default();
        fold.toggle(group(&outline, 2));
        assert_eq!(visible(&outline, &fold), ["Notes", "Notes", "Two", "Notes"]);
    }

    #[test]
    fn levels_are_relative_to_the_outermost() {
        let outline = vec![
            item("main.tex", Figure, "Cover", 0),
            item("main.tex", Chapter, "One", 1),
            item("main.tex", Section, "First", 2),
            item("main.tex", Chapter, "Two", 3),
        ];
        let rows = layout(&outline, &Fold::default());
        let depths: Vec<u8> = rows.iter().map(|r| r.depth).collect();
        assert_eq!(depths, [0, 0, 1, 0]);
        assert_eq!(visible(&outline, &Fold::default()), ["Cover", "One", "Two"]);
        // Only floats: all at the top.
        let floats = [
            item("main.tex", Figure, "A", 0),
            item("main.tex", Table, "B", 1),
        ];
        assert!(
            layout(&floats, &Fold::default())
                .iter()
                .all(|r| r.depth == 0 && r.visible)
        );
        assert!(layout(&[], &Fold::default()).is_empty());
    }

    #[test]
    fn hidden_items_are_shown_by_their_visible_heading() {
        let outline = article();
        let mut fold = Fold::default();
        let rows = layout(&outline, &fold);
        assert_eq!(shown(&rows, 3), Some(0));
        assert_eq!(shown(&rows, 5), Some(4));
        assert_eq!(shown(&rows, 6), Some(6));
        assert_eq!(shown(&rows, 7), None);
        fold.toggle(group(&outline, 0));
        let rows = layout(&outline, &fold);
        assert_eq!(shown(&rows, 3), Some(1));
        assert_eq!(shown(&rows, 1), Some(1));
    }
}
