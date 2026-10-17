//! Positions in a text: lines, characters, words, brackets, and the text
//! objects of LaTeX (environments, inline math). Offsets are bytes, always
//! on character boundaries.

use std::ops::Range;

pub fn line_start(t: &str, i: usize) -> usize {
    t[..i].rfind('\n').map_or(0, |p| p + 1)
}

/// The end of the line of `i`, before its newline.
pub fn line_end(t: &str, i: usize) -> usize {
    t[i..].find('\n').map_or(t.len(), |p| i + p)
}

pub fn next_char(t: &str, i: usize) -> usize {
    t[i..].chars().next().map_or(i, |c| i + c.len_utf8())
}

pub fn prev_char(t: &str, i: usize) -> usize {
    t[..i].chars().next_back().map_or(i, |c| i - c.len_utf8())
}

pub fn char_at(t: &str, i: usize) -> Option<char> {
    t[i..].chars().next()
}

/// The last character of the line of `i` (where the normal-mode cursor may
/// be), or its start if it is empty.
pub fn last_char(t: &str, i: usize) -> usize {
    let (s, e) = (line_start(t, i), line_end(t, i));
    if e > s { prev_char(t, e) } else { s }
}

/// Keep a normal-mode cursor off the newline.
pub fn clamp_normal(t: &str, i: usize) -> usize {
    i.min(last_char(t, i.min(t.len())))
}

/// 0-based line of `i`.
pub fn line_of(t: &str, i: usize) -> usize {
    t[..i].matches('\n').count()
}

pub fn line_count(t: &str) -> usize {
    t.matches('\n').count() + 1
}

/// The start of 0-based line `n` (the last line if past the end).
pub fn line_offset(t: &str, n: usize) -> usize {
    if n == 0 {
        return 0;
    }
    t.match_indices('\n')
        .nth(n - 1)
        .map_or_else(|| line_start(t, t.len()), |(p, _)| p + 1)
}

/// Column of `i` in characters.
pub fn column(t: &str, i: usize) -> usize {
    t[line_start(t, i)..i].chars().count()
}

/// The offset at character column `col` of the line starting at `start`,
/// or its last character.
pub fn at_column(t: &str, start: usize, col: usize, normal: bool) -> usize {
    let end = line_end(t, start);
    let mut i = start;
    for _ in 0..col {
        if i >= end {
            break;
        }
        i = next_char(t, i);
    }
    if normal && i >= end && end > start {
        prev_char(t, end)
    } else {
        i.min(end)
    }
}

/// The first non-blank character of the line of `i`.
pub fn first_non_blank(t: &str, i: usize) -> usize {
    let s = line_start(t, i);
    let e = line_end(t, i);
    t[s..e]
        .char_indices()
        .find(|(_, c)| !c.is_whitespace())
        .map_or(e, |(p, _)| s + p)
}

/// Character classes for word motions: blank, word, punctuation.
fn class(c: char, big: bool) -> u8 {
    if c.is_whitespace() {
        0
    } else if big || c.is_alphanumeric() || c == '_' {
        1
    } else {
        2
    }
}

/// `w` / `W`: the start of the next word (an empty line counts as one).
pub fn next_word(t: &str, i: usize, big: bool) -> usize {
    let mut chars = t[i..].char_indices().map(|(p, c)| (i + p, c)).peekable();
    let Some((_, first)) = chars.next() else {
        return t.len();
    };
    let start_class = class(first, big);
    let mut prev = first;
    for (p, c) in chars {
        let k = class(c, big);
        if prev == '\n' && c == '\n' {
            return p;
        }
        if k != 0 && (k != start_class || class(prev, big) == 0) {
            return p;
        }
        prev = c;
    }
    t.len()
}

/// `e` / `E`: the end of this or the next word.
pub fn word_end(t: &str, i: usize, big: bool) -> usize {
    let mut p = next_char(t, i);
    while let Some(c) = char_at(t, p) {
        if class(c, big) != 0 {
            break;
        }
        p = next_char(t, p);
    }
    let Some(c) = char_at(t, p) else {
        return prev_char(t, t.len());
    };
    let k = class(c, big);
    loop {
        let n = next_char(t, p);
        match char_at(t, n) {
            Some(c) if class(c, big) == k => p = n,
            _ => return p,
        }
    }
}

/// `b` / `B`: the start of this or the previous word.
pub fn prev_word(t: &str, i: usize, big: bool) -> usize {
    let mut p = i;
    // Back over blanks.
    loop {
        if p == 0 {
            return 0;
        }
        let q = prev_char(t, p);
        let c = char_at(t, q).unwrap_or(' ');
        if class(c, big) != 0 {
            p = q;
            break;
        }
        p = q;
    }
    let k = class(char_at(t, p).unwrap_or(' '), big);
    while p > 0 {
        let q = prev_char(t, p);
        if class(char_at(t, q).unwrap_or(' '), big) != k {
            break;
        }
        p = q;
    }
    p
}

/// `}`: the next blank line (or the end); `{`: the previous one.
pub fn paragraph(t: &str, i: usize, forward: bool) -> usize {
    let blank = |s: usize| t[s..line_end(t, s)].trim().is_empty();
    let mut line = line_start(t, i);
    // From blank lines, the paragraph after (or before) them counts.
    let mut seen_text = !blank(line);
    loop {
        if forward {
            let e = line_end(t, line);
            if e >= t.len() {
                return t.len();
            }
            line = e + 1;
        } else {
            if line == 0 {
                return 0;
            }
            line = line_start(t, line - 1);
        }
        if blank(line) {
            if seen_text {
                return line;
            }
        } else {
            seen_text = true;
        }
    }
}

/// `f` / `t` (forward) and `F` / `T` (backward) on the current line.
pub fn find_in_line(t: &str, i: usize, target: char, forward: bool, till: bool) -> Option<usize> {
    if forward {
        let start = next_char(t, i);
        let end = line_end(t, i);
        let p = start + t[start.min(end)..end].find(target)?;
        Some(if till { prev_char(t, p) } else { p })
    } else {
        let start = line_start(t, i);
        let p = start + t[start..i].rfind(target)?;
        Some(if till { next_char(t, p) } else { p })
    }
}

/// Whether the character at `i` is escaped (`\{`, `\$`).
fn escaped(t: &str, i: usize) -> bool {
    let backslashes = t[..i].chars().rev().take_while(|&c| c == '\\').count();
    backslashes % 2 == 1
}

/// `%`: the bracket matching the one at, or after, `i` on its line.
pub fn matching_bracket(t: &str, i: usize) -> Option<usize> {
    let end = line_end(t, i);
    let (at, c) = t[i..end]
        .char_indices()
        .map(|(p, c)| (i + p, c))
        .find(|(_, c)| "()[]{}".contains(*c))?;
    let (open, close, forward) = match c {
        '(' => ('(', ')', true),
        '[' => ('[', ']', true),
        '{' => ('{', '}', true),
        ')' => ('(', ')', false),
        ']' => ('[', ']', false),
        _ => ('{', '}', false),
    };
    if forward {
        return closing(t, at, open, close);
    }
    let mut depth = 0i32;
    for (p, c) in t[..=at].char_indices().rev() {
        if escaped(t, p) {
            continue;
        }
        if c == close {
            depth += 1;
        } else if c == open {
            depth -= 1;
            if depth == 0 {
                return Some(p);
            }
        }
    }
    None
}

/// The `close` matching the `open` at `at`, unless escaped.
fn closing(t: &str, at: usize, open: char, close: char) -> Option<usize> {
    let mut depth = 0i32;
    for (p, c) in t[at..].char_indices().map(|(p, c)| (at + p, c)) {
        if escaped(t, p) {
            continue;
        }
        if c == open {
            depth += 1;
        } else if c == close {
            depth -= 1;
            if depth == 0 {
                return Some(p);
            }
        }
    }
    None
}

/// The word (or blank run) at `i`; `around` adds the blanks after it (or
/// before it at the end of a line).
pub fn word_object(t: &str, i: usize, big: bool, around: bool) -> Option<Range<usize>> {
    let c = char_at(t, i)?;
    if c == '\n' {
        return None;
    }
    let k = class(c, big);
    let same = |p: usize| char_at(t, p).is_some_and(|c| c != '\n' && class(c, big) == k);
    let mut s = i;
    while s > 0 && same(prev_char(t, s)) {
        s = prev_char(t, s);
    }
    let mut e = i;
    while same(e) {
        e = next_char(t, e);
    }
    if around && k != 0 {
        let blank = |p: usize| char_at(t, p).is_some_and(|c| c == ' ' || c == '\t');
        let mut e2 = e;
        while blank(e2) {
            e2 = next_char(t, e2);
        }
        if e2 > e {
            e = e2;
        } else {
            while s > 0 && blank(prev_char(t, s)) {
                s = prev_char(t, s);
            }
        }
    }
    Some(s..e)
}

/// The innermost `open`…`close` pair around `i`: inside it, or with the
/// brackets.
pub fn bracket_object(
    t: &str,
    i: usize,
    open: char,
    close: char,
    around: bool,
) -> Option<Range<usize>> {
    let mut depth = 0i32;
    let mut start = None;
    // On the opening bracket itself.
    let from = if char_at(t, i) == Some(open) && !escaped(t, i) {
        next_char(t, i)
    } else {
        i
    };
    for (p, c) in t[..from].char_indices().rev() {
        if escaped(t, p) {
            continue;
        }
        if c == close {
            depth += 1;
        } else if c == open {
            if depth == 0 {
                start = Some(p);
                break;
            }
            depth -= 1;
        }
    }
    let start = start?;
    let end = closing(t, start, open, close)?;
    if end < i {
        return None;
    }
    Some(if around {
        start..next_char(t, end)
    } else {
        next_char(t, start)..end
    })
}

/// The quotes around `i` on its line.
pub fn quote_object(t: &str, i: usize, q: char, around: bool) -> Option<Range<usize>> {
    let (s, e) = (line_start(t, i), line_end(t, i));
    let quotes: Vec<usize> = t[s..e]
        .char_indices()
        .filter(|&(p, c)| c == q && !escaped(t, s + p))
        .map(|(p, _)| s + p)
        .collect();
    let pair = quotes
        .chunks(2)
        .find(|w| w.len() == 2 && w[0] <= i && i <= w[1])?;
    Some(if around {
        pair[0]..pair[1] + q.len_utf8()
    } else {
        pair[0] + q.len_utf8()..pair[1]
    })
}

/// `ie` / `ae`: the innermost `\begin{…}`…`\end{…}` around `i`.
pub fn environment_object(t: &str, i: usize, around: bool) -> Option<Range<usize>> {
    // (start of `\begin`, end of its `{name}`, name)
    let mut stack: Vec<(usize, usize, &str)> = Vec::new();
    let mut best: Option<(usize, usize, usize, usize)> = None;
    let mut p = 0;
    while let Some(off) = t[p..].find('\\') {
        let at = p + off;
        p = at + 1;
        let rest = &t[at..];
        let (is_begin, tag) = if rest.starts_with("\\begin{") {
            (true, "\\begin{")
        } else if rest.starts_with("\\end{") {
            (false, "\\end{")
        } else {
            continue;
        };
        let name_start = at + tag.len();
        let Some(close) = t[name_start..].find('}') else {
            continue;
        };
        let name = &t[name_start..name_start + close];
        let after = name_start + close + 1;
        if is_begin {
            stack.push((at, after, name));
        } else if let Some(pos) = stack.iter().rposition(|(_, _, n)| *n == name) {
            let (bs, be, _) = stack[pos];
            stack.truncate(pos);
            if bs <= i && i < after && best.is_none_or(|b| bs > b.0) {
                best = Some((bs, be, at, after));
            }
        }
    }
    let (bs, be, es, ee) = best?;
    if around {
        return Some(bs..ee);
    }
    // Inside: from the line after `\begin{…}` to the end of the line before `\end`.
    let inner_start = if t[be..].starts_with('\n') {
        be + 1
    } else {
        be
    };
    let mut inner_end = es;
    let before_end = line_start(t, es);
    if t[before_end..es].trim().is_empty() && before_end > inner_start {
        inner_end = before_end;
    }
    Some(inner_start..inner_end.max(inner_start))
}

/// `i$` / `a$`: the inline formula (`$…$` or `\(…\)`) around `i`.
pub fn math_object(t: &str, i: usize, around: bool) -> Option<Range<usize>> {
    let dollars: Vec<usize> = t
        .char_indices()
        .filter(|&(p, c)| c == '$' && !escaped(t, p))
        .map(|(p, _)| p)
        .collect();
    // Pair them up, `$$` counting as one delimiter.
    let mut delims = Vec::new();
    let mut k = 0;
    while k < dollars.len() {
        let double = k + 1 < dollars.len() && dollars[k + 1] == dollars[k] + 1;
        let len = if double { 2 } else { 1 };
        delims.push((dollars[k], len));
        k += len;
    }
    for pair in delims.chunks(2) {
        if let [(s, sl), (e, el)] = pair
            && *s <= i
            && i < e + el
        {
            return Some(if around { *s..e + el } else { s + sl..*e });
        }
    }
    // Up to the character at `i` included (the cursor may be on the `(` of `\(`).
    let open = t[..next_char(t, i.min(t.len()))].rfind("\\(")?;
    let close = open + t[open..].find("\\)")?;
    (close + 2 > i).then(|| {
        if around {
            open..close + 2
        } else {
            open + 2..close
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines() {
        let t = "ab\n\ncd";
        assert_eq!(line_start(t, 5), 4);
        assert_eq!(line_end(t, 0), 2);
        assert_eq!(last_char(t, 0), 1);
        assert_eq!(last_char(t, 3), 3);
        assert_eq!(line_offset(t, 2), 4);
        assert_eq!(line_offset(t, 9), 4);
        assert_eq!(at_column(t, 4, 5, true), 5);
        assert_eq!(first_non_blank("  x", 0), 2);
    }

    #[test]
    fn words() {
        let t = "foo.bar baz\n\nqux";
        assert_eq!(next_word(t, 0, false), 3);
        assert_eq!(next_word(t, 0, true), 8);
        assert_eq!(next_word(t, 8, false), 12);
        assert_eq!(word_end(t, 0, false), 2);
        assert_eq!(word_end(t, 2, false), 3);
        assert_eq!(prev_word(t, 8, false), 4);
        assert_eq!(prev_word(t, 13, false), 8);
        assert_eq!(word_object(t, 9, false, false), Some(8..11));
        assert_eq!(word_object("a bc d", 2, false, true), Some(2..5));
    }

    #[test]
    fn finds_and_matches() {
        let t = "f(a, \\{b) x";
        assert_eq!(find_in_line(t, 0, ',', true, false), Some(3));
        assert_eq!(find_in_line(t, 0, ',', true, true), Some(2));
        assert_eq!(find_in_line(t, 10, '(', false, false), Some(1));
        assert_eq!(matching_bracket(t, 0), Some(8));
        assert_eq!(matching_bracket(t, 8), Some(1));
        assert_eq!(bracket_object(t, 3, '(', ')', false), Some(2..8));
        assert_eq!(bracket_object(t, 3, '(', ')', true), Some(1..9));
        assert_eq!(
            quote_object("say \"hi there\" ok", 6, '"', false),
            Some(5..13)
        );
    }

    #[test]
    fn paragraphs() {
        let t = "a\nb\n\nc\n\nd";
        assert_eq!(paragraph(t, 0, true), 4);
        assert_eq!(paragraph(t, 4, true), 7);
        assert_eq!(paragraph(t, 9, false), 7);
    }

    #[test]
    fn latex_objects() {
        let t = "\\begin{itemize}\n  \\item a $x+1$\n\\end{itemize}\n";
        let i = t.find("item a").unwrap();
        let around = environment_object(t, i, true).unwrap();
        assert_eq!(
            &t[around],
            "\\begin{itemize}\n  \\item a $x+1$\n\\end{itemize}"
        );
        let inner = environment_object(t, i, false).unwrap();
        assert_eq!(&t[inner], "  \\item a $x+1$\n");
        let x = t.find('x').unwrap();
        assert_eq!(&t[math_object(t, x, false).unwrap()], "x+1");
        assert_eq!(&t[math_object(t, x, true).unwrap()], "$x+1$");
        let nested = "\\begin{a}\\begin{b}x\\end{b}\\end{a}";
        let x = nested.find('x').unwrap();
        assert_eq!(&nested[environment_object(nested, x, false).unwrap()], "x");
        let paren = "see \\(y\\) now";
        assert_eq!(&paren[math_object(paren, 6, false).unwrap()], "y");
    }

    #[test]
    fn unbalanced_environments() {
        // An unclosed name, an `\end` without its `\begin`, a `\begin` without its `\end`.
        assert_eq!(environment_object("\\begin{x", 3, false), None);
        assert_eq!(environment_object("a \\end{y} b", 0, true), None);
        assert_eq!(environment_object("\\begin{a}x", 9, true), None);
        let t = "\\begin{a}\\end{b}x\\end{a}";
        assert_eq!(environment_object(t, 16, true), Some(0..t.len()));
        // Other commands are skipped.
        let t = "\\begin{a}\\emph{x}\\end{a}";
        assert_eq!(&t[environment_object(t, 12, false).unwrap()], "\\emph{x}");
    }

    #[test]
    fn math_object_without_formula() {
        // No `$` pair: the `\(` lookup must not slice an empty text or a multibyte character.
        assert_eq!(math_object("", 0, false), None);
        assert_eq!(math_object("é", 0, true), None);
        assert_eq!(math_object("a€b", 1, false), None);
        let t = "\\(é\\)";
        assert_eq!(math_object(t, 2, false), Some(2..4));
        assert_eq!(math_object(t, 1, true), Some(0..6));
        // At the end of the text, after a closed formula.
        assert_eq!(math_object(t, t.len(), false), None);
    }
}
