//! Text edits computed on plain strings, applied to the editor by the
//! workspace.

use std::ops::Range;

/// A replacement of `range` with `text`, and the selection to set after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub range: Range<usize>,
    pub text: String,
    pub selection: Range<usize>,
}

/// Comment the lines touched by `selection` out with `% `, or uncomment
/// them if every non-blank one already is.
pub fn toggle_comment(text: &str, selection: Range<usize>) -> Edit {
    let start = text[..selection.start].rfind('\n').map_or(0, |i| i + 1);
    // A selection of whole lines ends at the start of the next one.
    let last = if selection.end > selection.start && text[..selection.end].ends_with('\n') {
        selection.end - 1
    } else {
        selection.end
    };
    let end = text[last..].find('\n').map_or(text.len(), |i| last + i);
    let block = &text[start..end];
    let lines: Vec<&str> = block.split('\n').collect();
    let content = |l: &str| !l.trim().is_empty();
    let commented = lines
        .iter()
        .filter(|l| content(l))
        .all(|l| l.trim_start().starts_with('%'));
    let any = lines.iter().any(|l| content(l));

    let new: Vec<String> = if commented && any {
        lines
            .iter()
            .map(|l| {
                let indent = l.len() - l.trim_start().len();
                let rest = &l[indent..];
                match rest.strip_prefix('%') {
                    Some(r) => format!("{}{}", &l[..indent], r.strip_prefix(' ').unwrap_or(r)),
                    None => l.to_string(),
                }
            })
            .collect()
    } else {
        let indent = lines
            .iter()
            .filter(|l| content(l))
            .map(|l| l.len() - l.trim_start().len())
            .min()
            .unwrap_or(0);
        lines
            .iter()
            .map(|l| {
                if !content(l) && any {
                    l.to_string()
                } else {
                    let at = indent.min(l.len());
                    format!("{}% {}", &l[..at], &l[at..])
                }
            })
            .collect()
    };
    let new = new.join("\n");
    let selection = if selection.is_empty() {
        // Keep the cursor on the same character of its line.
        let delta = new.len() as isize - block.len() as isize;
        let column = (selection.start - start) as isize;
        let moved = (column + delta).clamp(0, new.find('\n').unwrap_or(new.len()) as isize);
        let at = start + moved as usize;
        at..at
    } else {
        start..start + new.len()
    };
    Edit {
        range: start..end,
        text: new,
        selection,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(text: &str, e: &Edit) -> String {
        format!(
            "{}{}{}",
            &text[..e.range.start],
            e.text,
            &text[e.range.end..]
        )
    }

    #[test]
    fn comments_and_uncomments_a_line() {
        let text = "a\n  \\section{x}\nb";
        let e = toggle_comment(text, 6..6);
        let out = apply(text, &e);
        assert_eq!(out, "a\n  % \\section{x}\nb");
        assert_eq!(e.selection, 8..8);
        let back = toggle_comment(&out, e.selection.clone());
        assert_eq!(apply(&out, &back), text);
    }

    #[test]
    fn comments_a_block_at_its_indentation() {
        let text = "  one\n\n    two\nthree";
        let e = toggle_comment(text, 0..13);
        assert_eq!(apply(text, &e), "  % one\n\n  %   two\nthree");
        assert_eq!(e.selection, 0..e.text.len());
    }

    #[test]
    fn mixed_lines_are_commented() {
        let text = "% a\nb";
        let e = toggle_comment(text, 0..5);
        assert_eq!(apply(text, &e), "% % a\n% b");
    }

    #[test]
    fn whole_line_selection_stops_at_its_newline() {
        let text = "a\nb\nc";
        let e = toggle_comment(text, 0..4);
        assert_eq!(apply(text, &e), "% a\n% b\nc");
    }

    #[test]
    fn uncomments_without_space() {
        let text = "%a\n  %b";
        let e = toggle_comment(text, 0..text.len());
        assert_eq!(apply(text, &e), "a\n  b");
    }

    #[test]
    fn empty_line() {
        let e = toggle_comment("", 0..0);
        assert_eq!(e.text, "% ");
        assert_eq!(e.selection, 2..2);
    }
}
