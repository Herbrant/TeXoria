use super::*;

/// The text and cursor after typing `keys` in normal mode, from `start`
/// (`|` marks the cursor), as `|`-marked text.
fn run(start: &str, keys: &str) -> String {
    let mut vim = Vim::new();
    run_with(&mut vim, start, keys)
}

fn run_with(vim: &mut Vim, start: &str, keys: &str) -> String {
    let cursor = start.find('|').expect("cursor");
    let mut text = start.replacen('|', "", 1);
    let mut cursor = cursor;
    let mut clipboard: Option<String> = None;
    for key in parse_keys(keys) {
        let ctx = Ctx {
            text: &text,
            cursor,
            clipboard: clipboard.as_deref(),
            page_lines: 10,
        };
        for a in vim.key(&ctx, key) {
            match a {
                Action::Pass => {
                    // What an editor does with a typed key.
                    if let Key::Char(c) = key {
                        text.insert(cursor, c);
                        cursor += c.len_utf8();
                    } else if key == Key::Enter {
                        text.insert(cursor, '\n');
                        cursor += 1;
                    }
                }
                Action::Edit { range, text: new } => {
                    text.replace_range(range.clone(), &new);
                    cursor = range.start + new.len();
                }
                Action::Cursor(c) => cursor = c,
                Action::Select(r) => cursor = r.end.min(text.len()),
                Action::Yank(y) => clipboard = Some(y),
                _ => {}
            }
        }
    }
    let mut out = text;
    out.insert(cursor.min(out.len()), '|');
    out
}

/// `<esc>`, `<cr>`, `<c-r>`… and plain characters.
fn parse_keys(s: &str) -> Vec<Key> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(c) = rest.chars().next() {
        if c == '<'
            && let Some(end) = rest.find('>')
        {
            let name = &rest[1..end];
            let key = match name {
                "esc" => Some(Key::Escape),
                "cr" => Some(Key::Enter),
                "bs" => Some(Key::Backspace),
                n if n.starts_with("c-") => n[2..].chars().next().map(Key::Ctrl),
                _ => None,
            };
            if let Some(k) = key {
                out.push(k);
                rest = &rest[end + 1..];
                continue;
            }
        }
        out.push(Key::Char(c));
        rest = &rest[c.len_utf8()..];
    }
    out
}

#[test]
fn moves() {
    assert_eq!(run("|foo bar baz", "w"), "foo |bar baz");
    assert_eq!(run("|foo bar baz", "2w"), "foo bar |baz");
    assert_eq!(run("foo bar |baz", "b"), "foo |bar baz");
    assert_eq!(run("|foo bar", "e"), "fo|o bar");
    assert_eq!(run("|foo bar", "$"), "foo ba|r");
    assert_eq!(run("  foo |bar", "0"), "|  foo bar");
    assert_eq!(run("  foo |bar", "^"), "  |foo bar");
    assert_eq!(run("|a(b)c", "f)"), "a(b|)c");
    assert_eq!(run("|a(b)c", "t)"), "a(|b)c");
    assert_eq!(run("|a,b,c", "f,;"), "a,b|,c");
    assert_eq!(run("|a(b)c", "f(%"), "a(b|)c");
    assert_eq!(run("|ab\ncd\nef", "G"), "ab\ncd\n|ef");
    assert_eq!(run("ab\ncd\n|ef", "gg"), "|ab\ncd\nef");
    assert_eq!(run("ab\ncd\n|ef", "2G"), "ab\n|cd\nef");
}

#[test]
fn keeps_the_column() {
    assert_eq!(run("abc|d\nx\nabcd", "jj"), "abcd\nx\nabc|d");
    assert_eq!(run("abc|d\nx\nabcd", "j"), "abcd\n|x\nabcd");
    assert_eq!(run("a|b\ncd", "k"), "a|b\ncd");
}

#[test]
fn deletes_changes_and_yanks() {
    assert_eq!(run("|foo bar", "dw"), "|bar");
    assert_eq!(run("foo |bar", "dw"), "foo| ");
    assert_eq!(run("|foo bar baz", "d2w"), "|baz");
    assert_eq!(run("|foo bar", "cwxy<esc>"), "x|y bar");
    assert_eq!(run("a\n|b\nc", "dd"), "a\n|c");
    assert_eq!(run("a\nb\n|c", "dd"), "a\n|b");
    assert_eq!(run("|a\nb\nc", "2dd"), "|c");
    assert_eq!(run("|a\nb", "yyp"), "a\n|a\nb");
    assert_eq!(run("|a\nb", "yyP"), "|a\na\nb");
    assert_eq!(run("|foo", "x"), "|oo");
    assert_eq!(run("f|oo", "X"), "|oo");
    assert_eq!(run("|foo bar", "D"), "|");
    assert_eq!(run("a |b c", "dfc"), "a| ");
    assert_eq!(run("|ab", "ylp"), "a|ab");
    assert_eq!(run("|abc", "rX"), "|Xbc");
    assert_eq!(run("|abc", "~"), "A|bc");
}

#[test]
fn toggles_case_up_to_a_multibyte_end() {
    // At the end of the line the cursor stays on the last toggled character.
    assert_eq!(run("|é", "~"), "|É");
    assert_eq!(run("a|é\nb", "~"), "a|É\nb");
    assert_eq!(run("|aé", "3~"), "A|É");
    assert_eq!(run("|éa", "~"), "É|a");
    // `İ` lower-cases to two characters.
    assert_eq!(run("|İ", "~"), "i|\u{307}");
}

#[test]
fn text_objects() {
    assert_eq!(run("f(a, |b) x", "di("), "f(|) x");
    assert_eq!(run("f(a, |b) x", "da("), "f| x");
    assert_eq!(run("say \"h|i\" ok", "ci\"yo<esc>"), "say \"y|o\" ok");
    assert_eq!(run("a fo|o b", "daw"), "a |b");
    assert_eq!(
        run("\\begin{x}\n  a|b\n\\end{x}\n", "die"),
        "\\begin{x}\n|\\end{x}\n"
    );
    assert_eq!(run("see $x+|1$ now", "di$"), "see $|$ now");
    assert_eq!(run("see $x+|1$ now", "da$"), "see | now");
}

#[test]
fn math_object_outside_a_formula() {
    assert_eq!(run("|", "di$"), "|");
    assert_eq!(run("|é", "di$"), "|é");
    assert_eq!(run("a |€ b", "ca$"), "a |€ b");
}

#[test]
fn inserts_and_opens_lines() {
    assert_eq!(run("|ab", "ix<esc>"), "|xab");
    assert_eq!(run("|ab", "ax<esc>"), "a|xb");
    assert_eq!(run("|ab", "Ax<esc>"), "ab|x");
    assert_eq!(run("  |ab", "Ix<esc>"), "  |xab");
    assert_eq!(run("  |ab", "ox<esc>"), "  ab\n  |x");
    assert_eq!(run("  |ab", "Ox<esc>"), "  |x\n  ab");
    assert_eq!(run("|ab\ncd", "J"), "ab| cd");
}

#[test]
fn indents() {
    assert_eq!(run("|a\nb", ">>"), "  |a\nb");
    assert_eq!(run("|a\nb", "2>>"), "  |a\n  b");
    assert_eq!(run("  |a", "<<"), "|a");
}

#[test]
fn indents_up_to_a_multibyte_end() {
    // The range ends after a multibyte character: its last line is found from that character.
    assert_eq!(run("a\n|é", ">k"), "  |a\n  é");
    assert_eq!(run("|a\n  é", "<j"), "|a\né");
    assert_eq!(run("|aé", "v$>"), "  |aé");
}

#[test]
fn repeats_changes() {
    assert_eq!(run("|a b c d", "dw."), "|c d");
    assert_eq!(run("|a b c", "dw2."), "|");
    assert_eq!(run("|ab\ncd", "Ax<esc>j."), "abx\ncd|x");
    assert_eq!(run("|foo foo", "cwbar<esc>w."), "bar ba|r");
}

#[test]
fn visual_mode() {
    assert_eq!(run("|foo bar", "vwd"), "|ar");
    assert_eq!(run("a\n|b\nc\nd", "Vjd"), "a\n|d");
    assert_eq!(run("|foo bar", "veyP"), "fo|ofoo bar");
    assert_eq!(run("f(|ab)", "vi(d"), "f(|)");
}

#[test]
fn searches() {
    assert_eq!(run("|a foo b Foo", "/foo<cr>"), "a |foo b Foo");
    assert_eq!(run("|a foo b foo", "/foo<cr>n"), "a foo b |foo");
    assert_eq!(run("|a foo b foo", "/foo<cr>nn"), "a |foo b foo");
    assert_eq!(run("a foo b |foo", "?foo<cr>"), "a |foo b foo");
    assert_eq!(run("|foo food foo", "*"), "foo food |foo");
}

#[test]
fn ex_commands() {
    let mut vim = Vim::new();
    let ctx = Ctx {
        text: "a\nb\nc",
        cursor: 0,
        clipboard: None,
        page_lines: 10,
    };
    for k in parse_keys(":3") {
        assert!(vim.key(&ctx, k).is_empty());
    }
    assert_eq!(vim.status(), ":3");
    assert_eq!(vim.key(&ctx, Key::Enter), vec![Action::Cursor(4)]);
    for k in parse_keys(":wq") {
        vim.key(&ctx, k);
    }
    assert_eq!(vim.key(&ctx, Key::Enter), vec![Action::Write, Action::Quit]);
}

#[test]
fn undo_and_modes() {
    let mut vim = Vim::new();
    let ctx = Ctx {
        text: "ab",
        cursor: 0,
        clipboard: None,
        page_lines: 10,
    };
    assert_eq!(vim.key(&ctx, Key::Char('u')), vec![Action::Undo]);
    assert_eq!(vim.key(&ctx, Key::Ctrl('r')), vec![Action::Redo]);
    vim.key(&ctx, Key::Char('i'));
    assert_eq!(vim.mode(), Mode::Insert);
    assert_eq!(vim.key(&ctx, Key::Char('z')), vec![Action::Pass]);
    assert_eq!(vim.block("ab", 0), None);
    vim.key(&ctx, Key::Escape);
    assert_eq!(vim.mode(), Mode::Normal);
    assert_eq!(vim.block("ab", 0), Some(0..1));
    assert_eq!(vim.block("", 0), None);
}

#[test]
fn pastes_the_system_clipboard() {
    let mut vim = Vim::new();
    let ctx = Ctx {
        text: "ab",
        cursor: 0,
        clipboard: Some("XY"),
        page_lines: 10,
    };
    assert_eq!(
        vim.key(&ctx, Key::Char('p')),
        vec![
            Action::Edit {
                range: 1..1,
                text: "XY".into()
            },
            Action::Cursor(2)
        ]
    );
}

/// A context on `text` at `cursor`.
fn ctx(text: &str, cursor: usize) -> Ctx<'_> {
    Ctx {
        text,
        cursor,
        clipboard: None,
        page_lines: 10,
    }
}

#[test]
fn visual_mode_survives_a_text_that_changed() {
    // Started past the end of the text it is now (an undo shrank it).
    let mut vim = Vim::new();
    vim.key(&ctx("aé€b", 6), Key::Char('v'));
    let a = vim.key(&ctx("a€", 4), Key::Char('h'));
    // The head is before the anchor: the selection goes backwards.
    assert_eq!(a, vec![Action::Select(Range { start: 4, end: 1 })]);
    // Ends inside what is now a multibyte character.
    let mut vim = Vim::new();
    vim.key(&ctx("aé", 1), Key::Char('v'));
    let a = vim.key(&ctx("é", 1), Key::Char('l'));
    assert_eq!(a, vec![Action::Select(0..2)]);
    let mut vim = Vim::new();
    vim.key(&ctx("ab\ncd", 4), Key::Char('V'));
    let a = vim.key(&ctx("é", 1), Key::Char('d'));
    assert_eq!(
        a[1],
        Action::Edit {
            range: 0..2,
            text: String::new()
        }
    );
    assert_eq!(vim.mode(), Mode::Normal);
}

#[test]
fn searches_ignoring_the_case_of_any_letter() {
    assert_eq!(run("|a Éte", "/éte<cr>"), "a |Éte");
    // The Kelvin sign lower-cases to `k`, one byte instead of three.
    assert_eq!(run("x |a \u{212A}", "/k<cr>"), "x a |\u{212A}");
    assert_eq!(run("|\u{212A}x ab", "/ab<cr>"), "\u{212A}x |ab");
    // `İ` grows by one byte: with two of them the lengths add up again.
    assert_eq!(run("|\u{212A}abİİ", "/ab<cr>"), "\u{212A}|abİİ");
    assert_eq!(run("|\u{212A}abİİ ab", "/ab<cr>n"), "\u{212A}abİİ |ab");
    assert_eq!(run("|İİ\u{212A}x", "/x<cr>"), "İİ\u{212A}|x");
    // Whole words are checked in the original text.
    assert_eq!(run("xİ |ab ab", "*"), "xİ ab |ab");
    assert_eq!(run("|ab İab ab", "*"), "ab İab |ab");
}

#[test]
fn angle_bracket_objects() {
    assert_eq!(run("<(|x)>", "di<"), "<|>");
    assert_eq!(run("a <b |c> d", "da>"), "a | d");
    assert_eq!(run("<a <b> |c>", "ci<z<esc>"), "<|z>");
    assert_eq!(run("<a [|b> c]", "di<"), "<|> c]");
    // Not inside a pair: nothing happens.
    assert_eq!(run("a|b <c>", "di<"), "a|b <c>");
    assert_eq!(run("<a|b", "di<"), "<a|b");
    // An escaped bracket does not count.
    assert_eq!(run("{a \\} |b}", "di{"), "{|}");
}
