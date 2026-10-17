//! Text edits computed on plain strings, applied to the editor by the
//! workspace.

use std::ops::Range;
use std::path::Path;

use tex_fmt::args::{Args, OptionArgs};

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

/// `text` formatted by tex-fmt: indented environments and groups, without
/// wrapping lines unless the project's `tex-fmt.toml` asks for it.
pub fn format_latex(text: &str, file: &Path, project_dir: Option<&Path>) -> Result<String, String> {
    let config = project_dir
        .map(|d| d.join("tex-fmt.toml"))
        .filter(|p| p.is_file());
    let text = text.to_string();
    let file = file.to_path_buf();
    // tex-fmt panics on what it cannot handle (and on a bad config file).
    std::panic::catch_unwind(move || {
        let args = format_args(config.as_deref());
        let mut logs = Vec::new();
        let out = tex_fmt::format::format_file(&text, &file, &args, &mut logs);
        // Its line ending is the platform's; the editor's is `\n`.
        if text.contains("\r\n") {
            out
        } else {
            out.replace("\r\n", "\n")
        }
    })
    .map_err(|e| {
        e.downcast_ref::<String>()
            .cloned()
            .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap_or_else(|| "tex-fmt could not format the file".into())
    })
}

fn format_args(config: Option<&Path>) -> Args {
    let mut args = OptionArgs {
        // Wrapping rewrites paragraphs: only on request.
        wrap: Some(false),
        ..OptionArgs::default()
    };
    let read = config.and_then(|path| {
        tex_fmt::config::get_config_args(tex_fmt::config::get_config(&OptionArgs {
            config: Some(path.to_path_buf()),
            ..OptionArgs::new()
        }))
    });
    if let Some(c) = read {
        let pick = |v: &mut Vec<String>, from: Vec<String>| {
            if !from.is_empty() {
                v.extend(from);
            }
        };
        args.wrap = c.wrap.or(args.wrap);
        args.wraplen = c.wraplen.or(args.wraplen);
        args.wrapmin = c.wrapmin.or(args.wrapmin);
        args.tabsize = c.tabsize.or(args.tabsize);
        args.tabchar = c.tabchar.or(args.tabchar);
        args.format_tables = c.format_tables.or(args.format_tables);
        pick(&mut args.lists, c.lists);
        pick(&mut args.verbatims, c.verbatims);
        pick(&mut args.no_indent_envs, c.no_indent_envs);
        if !c.wrap_chars.is_empty() {
            args.wrap_chars = c.wrap_chars;
        }
    }
    Args::from(args)
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

    #[test]
    fn formats_latex() {
        let text =
            "\\begin{itemize}\n\\item a\n\\begin{equation}\nx\n\\end{equation}\n\\end{itemize}\n";
        let out = format_latex(text, Path::new("a.tex"), None).unwrap();
        assert_eq!(
            out,
            "\\begin{itemize}\n  \\item a\n    \\begin{equation}\n      x\n    \\end{equation}\n\\end{itemize}\n"
        );
        let long = format!("{}\n", "word ".repeat(40).trim_end());
        assert_eq!(format_latex(&long, Path::new("a.tex"), None).unwrap(), long);
    }

    #[test]
    fn reads_the_project_config() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("tex-fmt.toml"), "tabsize = 4\n").unwrap();
        let out = format_latex(
            "\\begin{center}\nx\n\\end{center}\n",
            Path::new("a.tex"),
            Some(dir.path()),
        )
        .unwrap();
        assert_eq!(out, "\\begin{center}\n    x\n\\end{center}\n");
        std::fs::write(dir.path().join("tex-fmt.toml"), "tabsize = [").unwrap();
        assert!(format_latex("x\n", Path::new("a.tex"), Some(dir.path())).is_err());
    }
}
