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
                    } else if key == Key::Tab {
                        text.insert_str(cursor, "  ");
                        cursor += 2;
                    } else if key == Key::Backspace && cursor > 0 {
                        let before = prev_char(&text, cursor);
                        text.replace_range(before..cursor, "");
                        cursor = before;
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
                "tab" => Some(Key::Tab),
                "left" => Some(Key::Left),
                "right" => Some(Key::Right),
                "up" => Some(Key::Up),
                "down" => Some(Key::Down),
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

#[test]
fn huge_counts_are_bounded() {
    // `100000p` of a large register would build gigabytes of text.
    let mut vim = Vim::new();
    let big = "x".repeat(10_000);
    let c = Ctx {
        text: "ab",
        cursor: 0,
        clipboard: Some(&big),
        page_lines: 10,
    };
    let mut pasted = 0;
    for k in parse_keys("100000p") {
        for a in vim.key(&c, k) {
            if let Action::Edit { text, .. } = a {
                pasted = text.len();
            }
        }
    }
    assert!(pasted > 0 && pasted <= MAX_PASTE, "{pasted}");
    assert_eq!(pasted % big.len(), 0);

    // `.` is replayed a bounded number of times, each on a copy of the text.
    let out = run("|ab", "ix<esc>100000.");
    assert_eq!(out.matches('x').count(), MAX_REPEAT + 1);
}

#[test]
fn visual_case_and_replace() {
    assert_eq!(run("|abc def", "vl~"), "|ABc def");
    assert_eq!(run("ab|c\nx", "vh~"), "a|BC\nx");
    assert_eq!(run("|ab\ncd\nef", "Vj~"), "|AB\nCD\nef");
    assert_eq!(run("|ABC", "vlu"), "|abC");
    assert_eq!(run("|abc", "vlU"), "|ABc");
    assert_eq!(run("|abc", "vlrx"), "|xxc");
    assert_eq!(run("|ab\ncd\ne", "Vjrx"), "|xx\nxx\ne");
    assert_eq!(run("|éab", "vlrü"), "|üüb");
    let mut vim = Vim::new();
    run_with(&mut vim, "|ab", "vl~");
    assert_eq!(vim.mode(), Mode::Normal);
}

#[test]
fn visual_join() {
    assert_eq!(run("|a\nb\nc\nd", "vjjJ"), "a b| c\nd");
    assert_eq!(run("|a\nb\nc", "vJ"), "a| b\nc");
    assert_eq!(run("|a\nb\nc", "VjJ"), "a| b\nc");
    assert_eq!(run("a\n|b", "vkJ"), "a| b");
}

#[test]
fn visual_put() {
    // `p` puts the replaced text in the register, `P` keeps it.
    assert_eq!(run("|foo bar", "yiwwviwp"), "foo fo|o");
    assert_eq!(run("|foo bar", "yiwwviwp0viwp"), "ba|r foo");
    assert_eq!(run("|foo bar", "yiwwviwP0viwP"), "fo|o foo");
    assert_eq!(run("|a b", "yiwwv2p"), "a a|a");
    // Lines into characters, characters into lines, lines into lines.
    assert_eq!(run("|a\nxyz", "yyjlvp"), "a\nx\n|a\nz");
    assert_eq!(run("|ab\ncd", "yiwjVp"), "ab\n|ab");
    assert_eq!(run("|ab\ncd\nef", "yyjVp"), "ab\n|ab\nef");
    assert_eq!(run("|ab\ncd\nef", "yiwjVp"), "ab\n|ab\nef");
    // Nothing to put: still selecting.
    let mut vim = Vim::new();
    run_with(&mut vim, "|ab", "vp");
    assert_eq!(vim.mode(), Mode::Visual);
}

#[test]
fn more_motions() {
    assert_eq!(run("|a.b c.d", "W"), "a.b |c.d");
    assert_eq!(run("|a.b c.d", "E"), "a.|b c.d");
    assert_eq!(run("a.b c.|d", "B"), "a.b |c.d");
    assert_eq!(run("|a\nb\n\nc", "}"), "a\nb\n|\nc");
    assert_eq!(run("a\n\nb\n|c", "{"), "a\n|\nb\nc");
    assert_eq!(run("|a\n\nb\n\nc", "2}"), "a\n\nb\n|\nc");
    assert_eq!(run("ab,c|d", "F,"), "ab|,cd");
    assert_eq!(run("ab,c|d", "T,"), "ab,|cd");
    assert_eq!(run("|a,b,c,d", "2f,,"), "a|,b,c,d");
    assert_eq!(run("|a,b,c", "t,;"), "a,|b,c");
    assert_eq!(run("a,b,c,|d", "T,,"), "a,b,c,|d");
    assert_eq!(run("a,b,c,|d", "F,;"), "a,b|,c,d");
    assert_eq!(run("a,b,c,|d", "F,,"), "a,b,c|,d");
    assert_eq!(run("a,b,|c,d", "T,;"), "a,|b,c,d");
    assert_eq!(run("a,b,|c", "F,d;"), "a|,c");
    assert_eq!(run("|a,b,c", "f,d;"), "a|c");
    // No find yet: `;` does nothing.
    assert_eq!(run("a|,b", ";"), "a|,b");
    assert_eq!(run("|ab\ncd\nef", "2$"), "ab\nc|d\nef");
    assert_eq!(run("|ab\ncd", "9$"), "ab\nc|d");
    assert_eq!(run("|abc", "<right><right><left>"), "a|bc");
    assert_eq!(run("|abc", "  <bs>"), "a|bc");
    assert_eq!(run("a|b\ncd", "<down>"), "ab\nc|d");
    assert_eq!(run("ab\nc|d", "<up>"), "a|b\ncd");
    assert_eq!(run("a|b\ncd", "<cr>"), "ab\nc|d");
    assert_eq!(run("|ab\ncd", "3l"), "a|b\ncd");
    assert_eq!(run("ab|c", "9h"), "|abc");
    assert_eq!(run("a|b\ncd", "5j"), "ab\nc|d");
    assert_eq!(run("ab\nc|d", "k5k"), "a|b\ncd");
    assert_eq!(run("|(a)", "%"), "(a|)");
    assert_eq!(run("|ab", "%"), "|ab");
    assert_eq!(run("|ab", "zq"), "|ab");
}

#[test]
fn pages() {
    let text: String = (0..30).map(|i| format!("l{i}\n")).collect();
    let at = |line: usize| {
        let mut t = text.clone();
        t.insert(line_offset(&text, line), '|');
        t
    };
    // Half of the 10 visible lines.
    assert_eq!(run(&at(0), "<c-d>"), at(5));
    assert_eq!(run(&at(0), "2<c-d>"), at(10));
    assert_eq!(run(&at(12), "<c-u>"), at(7));
    assert_eq!(run(&at(3), "<c-u>"), at(0));
    assert_eq!(
        run(&at(0), "v<c-d>d"),
        at(0).replace(&text[..line_offset(&text, 5) + 1], "")
    );
}

#[test]
fn more_searches() {
    assert_eq!(run("|a foo b foo", "/foo<cr>N"), "a foo b |foo");
    assert_eq!(run("a foo b |foo c foo", "?foo<cr>n"), "a foo b foo c |foo");
    assert_eq!(run("a foo b |foo c foo", "?foo<cr>N"), "a foo b |foo c foo");
    // An empty search repeats the last one.
    assert_eq!(run("|a foo b foo", "/foo<cr>/<cr>"), "a foo b |foo");
    // Nothing to repeat yet, or nothing found.
    assert_eq!(run("|a foo", "n"), "|a foo");
    assert_eq!(run("|a foo", "/<cr>"), "|a foo");
    assert_eq!(run("|a foo", "/bar<cr>"), "|a foo");
    assert_eq!(run("|a foo", "/\\<\\><cr>"), "|a foo");
    // Capitals match the case exactly.
    assert_eq!(run("|a foo Foo", "/Foo<cr>"), "a foo |Foo");
    assert_eq!(run("|a foo food foo", "/\\<foo\\><cr>n"), "a foo food |foo");
    assert_eq!(run("|a foo foo", "2n"), "|a foo foo");
    assert_eq!(run("|a foo b foo c", "/foo<cr>2n"), "a |foo b foo c");
    // `*` on a blank or past the text.
    assert_eq!(run("a| b", "*"), "a| b");
    assert_eq!(run("a\n|", "*"), "a\n|");
    assert_eq!(run("|é x é", "*"), "é x |é");
}

#[test]
fn command_line() {
    let mut vim = Vim::new();
    let c = ctx("a\nb\nc", 0);
    let typed = |vim: &mut Vim, keys: &str| {
        let mut out = Vec::new();
        for k in parse_keys(keys) {
            out = vim.key(&c, k);
        }
        out
    };
    for (cmd, want) in [
        (":w<cr>", vec![Action::Write]),
        (":write<cr>", vec![Action::Write]),
        (":q<cr>", vec![Action::Quit]),
        (":q!<cr>", vec![Action::Quit]),
        (":quit<cr>", vec![Action::Quit]),
        (":x<cr>", vec![Action::Write, Action::Quit]),
        (":xit<cr>", vec![Action::Write, Action::Quit]),
        (": wq <cr>", vec![Action::Write, Action::Quit]),
        (":nope<cr>", vec![]),
        (":2<cr>", vec![Action::Cursor(2)]),
        (":0<cr>", vec![Action::Cursor(0)]),
        (":99<cr>", vec![Action::Cursor(4)]),
        (":wx<bs><cr>", vec![Action::Write]),
        (":w<esc>", vec![]),
        (":w<c-c>", vec![]),
    ] {
        assert_eq!(typed(&mut vim, cmd), want, "{cmd}");
        assert_eq!(vim.status(), "", "{cmd}");
    }
    typed(&mut vim, ":ab<tab><left>");
    assert_eq!(vim.status(), ":ab");
    typed(&mut vim, "<bs>");
    assert_eq!(vim.status(), ":a");
    // Backspace on an empty line closes it.
    typed(&mut vim, "<bs><bs>");
    assert_eq!(vim.status(), "");
    assert_eq!(typed(&mut vim, "j"), vec![Action::Cursor(2)]);
}

#[test]
fn status_and_reset() {
    let mut vim = Vim::default();
    let c = ctx("ab", 0);
    for k in parse_keys("2d") {
        vim.key(&c, k);
    }
    assert_eq!(vim.status(), "2d");
    vim.key(&c, Key::Escape);
    assert_eq!(vim.status(), "");
    vim.key(&c, Key::Char('/'));
    vim.key(&c, Key::Char('x'));
    assert_eq!(vim.status(), "/x");
    vim.reset();
    assert_eq!(vim.status(), "");
    vim.key(&c, Key::Char('i'));
    assert_eq!(vim.mode(), Mode::Insert);
    vim.reset();
    assert_eq!(vim.mode(), Mode::Normal);
    // A change interrupted by reset is not repeated.
    assert_eq!(vim.key(&c, Key::Char('.')), vec![]);
    assert_eq!(Mode::Normal.label(), "NORMAL");
    assert_eq!(Mode::Insert.label(), "INSERT");
    assert_eq!(Mode::Visual.label(), "VISUAL");
    assert_eq!(Mode::VisualLine.label(), "VISUAL LINE");
}

#[test]
fn invalid_keys_are_dropped() {
    for keys in [
        "di<c-d>", "r<c-d>", "g<c-d>", "gx", "f<c-d>", "d<c-d>", "<c-x>", "diz", "U",
    ] {
        let mut vim = Vim::new();
        assert_eq!(run_with(&mut vim, "a|b", keys), "a|b", "{keys}");
        assert_eq!(vim.status(), "", "{keys}");
    }
    let mut vim = Vim::new();
    assert_eq!(run_with(&mut vim, "a|b", "vi<c-d>"), "ab|");
    assert_eq!(vim.mode(), Mode::Visual);
}

#[test]
fn block_cursor() {
    let vim = Vim::new();
    assert_eq!(vim.block("é", 0), Some(0..2));
    assert_eq!(vim.block("aé", 9), Some(1..3));
    assert_eq!(vim.block("a€\nb", 4), Some(1..4));
    assert_eq!(vim.block("a\n\nb", 2), None);
}

#[test]
fn more_operators() {
    assert_eq!(run("|foo bar", "ywP"), "foo| foo bar");
    assert_eq!(run("a|b\ncd", "yj"), "|ab\ncd");
    assert_eq!(run("|foo bar", "y$P"), "foo ba|rfoo bar");
    assert_eq!(run("  a|b\nc", "ccx<esc>"), "  |x\nc");
    assert_eq!(run("a\n  |b", "ccx<esc>"), "a\n  |x");
    assert_eq!(run("  |ab\nc", "Sx<esc>"), "  |x\nc");
    assert_eq!(run("a|bc", "Cx<esc>"), "a|x");
    assert_eq!(run("|abc", "2sx<esc>"), "|xc");
    assert_eq!(run("|ab", "9sx<esc>"), "|x");
    assert_eq!(run("|a\nb", ">j"), "  |a\n  b");
    assert_eq!(run("  a\n  |b", "<k"), "|a\nb");
    assert_eq!(run("\t|a", "<<"), "|a");
    assert_eq!(run(" |a", "<<"), "|a");
    assert_eq!(run("|a\n\nb", "3>>"), "  |a\n\n  b");
    assert_eq!(run("|ab", "yl3p"), "aaa|ab");
    assert_eq!(run("|ab", "yl3P"), "aa|aab");
    assert_eq!(run("a\n|b", "yyp"), "a\nb\n|b");
    assert_eq!(run("|  a\nb", "yyjp"), "  a\nb\n  |a");
    assert_eq!(run("|  a\nb", "yyjP"), "  a\n  |a\nb");
    assert_eq!(run("|a\n\nb", "ylj2p"), "a\na|a\nb");
    assert_eq!(run("|ab", "p"), "|ab");
    assert_eq!(run("|ab", "3rx"), "|ab");
    assert_eq!(run("|abc", "2rx"), "x|xc");
    assert_eq!(run("a\n|\nb", "x"), "a\n|\nb");
    assert_eq!(run("|ab", "X"), "|ab");
    assert_eq!(run("|ab", "9x"), "|");
    assert_eq!(run("ab|c", "9X"), "|c");
    assert_eq!(run("a|b", "dk"), "a|b");
    assert_eq!(run("a\n|b", "dk"), "|");
    assert_eq!(run("|a\nb\nc", "dG"), "|");
    assert_eq!(run("a\n|b\nc", "dgg"), "|c");
    assert_eq!(run("|ab cd", "d$"), "|");
    assert_eq!(run("|ab cd", "de"), "| cd");
    assert_eq!(run("|ab cd", "dE"), "| cd");
    assert_eq!(run("|ab  \ncd", "dw"), "|\ncd");
    assert_eq!(run("|  ab", "cwx<esc>"), "|xab");
    assert_eq!(run("|a.b c", "cWx<esc>"), "|x c");
    assert_eq!(run("a(|b)", "d%"), "a|)");
    assert_eq!(run("a|(b)", "d%"), "|a");
    assert_eq!(run("|a b", "dfz"), "|a b");
    assert_eq!(run("|a\nb", "dd"), "|b");
    assert_eq!(run("|a", "dd"), "|");
    assert_eq!(run("|a\nb", "5dd"), "|");
}

#[test]
fn joins() {
    assert_eq!(run("|a\nb\nc", "3J"), "a b| c");
    assert_eq!(run("|a\nb", "J"), "a| b");
    assert_eq!(run("|a", "J"), "|a");
    assert_eq!(run("a\n|b", "J"), "a\n|b");
    assert_eq!(run("|f(a\n)", "J"), "f(a|)");
    assert_eq!(run("|a  \n   b", "J"), "a| b");
    assert_eq!(run("|\nb", "J"), "|b");
    assert_eq!(run("|a\n\nb", "J"), "a|\nb");
    assert_eq!(run("|a\nb\nc", "9J"), "a b| c");
}

#[test]
fn more_repeats() {
    assert_eq!(run("|a\nb", "ox<esc>j."), "a\nx\nb\n|x");
    assert_eq!(run("|a\nb", "Ox<esc>j."), "x\n|x\na\nb");
    assert_eq!(run("|abcd", "rxl."), "x|xcd");
    assert_eq!(run("|ab", "ylp."), "aa|ab");
    assert_eq!(run("|abc", "~."), "AB|c");
    assert_eq!(run("|a b c", "D."), "|");
    assert_eq!(run("|abc", "x."), "|c");
    assert_eq!(run("|a\nb\nc", ">>j."), "  a\n  |b\nc");
    // Insertions are replayed with their Tab and Backspace.
    assert_eq!(run("|ab", "ixy<bs><tab><esc>."), "x x |  ab");
    assert_eq!(run("|ab", "ix<cr>y<esc>."), "x\nx\n|yyab");
    // A failed change is not recorded; a yank is not a change.
    assert_eq!(run("|a b c", "dwdi(."), "|c");
    assert_eq!(run("|a b", "dwyl."), "|");
    assert_eq!(run("|ab", "di(."), "|ab");
    assert_eq!(run("|ab", "."), "|ab");
    // The yanks of a repeated change reach the register.
    assert_eq!(run("|a b c", "dw.P"), "b| c");
}

#[test]
fn more_visual() {
    assert_eq!(run("a b|cd e", "viwd"), "a | e");
    assert_eq!(run("a (b|c) e", "va(d"), "a | e");
    assert_eq!(run("|a\nb\nc", "Vj>"), "  |a\n  b\nc");
    assert_eq!(run("  |a\n  b\nc", "Vj<"), "|a\nb\nc");
    assert_eq!(run("|abc", "vlx"), "|c");
    assert_eq!(run("|abc", "vly"), "|abc");
    assert_eq!(run("|abc", "vlcx<esc>"), "|xc");
    assert_eq!(run("|ab ab", "v/ab<cr>d"), "|b");
    assert_eq!(run("|a\nb\nc", "vjVd"), "|c");
    assert_eq!(run("|a\nb\nc", "Vjvd"), "|\nc");
    assert_eq!(run("|ab", "vlX<esc>"), "a|b");
    assert_eq!(run("a|b", "vhd"), "|");
    assert_eq!(run("|a\nb", "Vjd"), "|");
    assert_eq!(run("a\n|b", "Vkd"), "|");
    assert_eq!(run("ab|", "vd"), "|a");
    // An empty object selects nothing.
    assert_eq!(run("a(|)b", "vi(d"), "a(|b");
    let mut vim = Vim::new();
    assert_eq!(run_with(&mut vim, "a|b", "vv"), "a|b");
    assert_eq!(vim.mode(), Mode::Normal);
    assert_eq!(run_with(&mut vim, "a|b", "VV"), "a|b");
    assert_eq!(vim.mode(), Mode::Normal);
    run_with(&mut vim, "a|b", "vV");
    assert_eq!(vim.mode(), Mode::VisualLine);
    run_with(&mut vim, "a|b", "<c-[>");
    assert_eq!(vim.mode(), Mode::Normal);
}

#[test]
fn more_text_objects() {
    assert_eq!(run("a `b|c` d", "di`"), "a `|` d");
    assert_eq!(run("'a|b'", "ci'x<esc>"), "'|x'");
    assert_eq!(run("a b.|c d", "diW"), "a | d");
    assert_eq!(run("a b.|c d", "daW"), "a |d");
    assert_eq!(run("x\n\\begin{a}\n|y\n\\end{a}\nz", "dae"), "x\n|\nz");
    assert_eq!(run("a \\(b|c\\) d", "di$"), "a \\(|\\) d");
    assert_eq!(run("a \\(b|c\\) d", "da$"), "a | d");
    assert_eq!(run("a [b|c] d", "da]"), "a | d");
    assert_eq!(run("a {b|c} d", "diB"), "a {|} d");
    assert_eq!(run("a (b|c) d", "dib"), "a (|) d");
    assert_eq!(run("a \"b|c\" d", "da\""), "a | d");
    assert_eq!(run("a |b", "di\""), "a |b");
}

#[test]
fn change_word_stops_at_the_end_of_the_word() {
    // On a word's last character `cw` changes only that character, like Vim.
    assert_eq!(run("|a b", "cwx<esc>"), "|x b");
    assert_eq!(run("fo|o bar", "cwx<esc>"), "fo|x bar");
    assert_eq!(run("|a b c", "c2wx<esc>"), "|x c");
    assert_eq!(run("|ab cd ef", "c2wx<esc>"), "|x ef");
    assert_eq!(run("a|.b", "cWx<esc>"), "a|x");
    assert_eq!(run("|é b", "cwx<esc>"), "|x b");
}
