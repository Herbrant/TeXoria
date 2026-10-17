//! Random texts, cursors and key sequences: Vim must never panic, and every offset it hands
//! the editor must be inside the text and on a character boundary.

use proptest::prelude::*;
use tex_vim::*;

/// Characters that matter to Vim's motions and objects, multibyte ones included (the Kelvin
/// sign and `İ` change their byte length when lower-cased).
const CHARS: &[char] = &[
    'a', 'b', 'Z', '_', ' ', '\t', '\n', '\n', '$', '\\', '{', '}', '(', ')', '[', ']', '<', '>',
    '"', '\'', '`', '.', ',', 'é', '€', '😀', '\u{212A}', 'İ',
];

/// Commands, as typed (`<esc>`-style names for special keys).
const VOCAB: &[&str] = &[
    "h", "j", "k", "l", "w", "W", "e", "E", "b", "B", "0", "^", "$", "G", "gg", "%", "{", "}", ";",
    ",", "fa", "t$", "F\\", "Té", "x", "X", "s", "S", "D", "C", "Y", "J", "p", "P", "u", "<c-r>",
    ".", "i", "a", "I", "A", "o", "O", "v", "V", "~", "rX", "ré", "dw", "cw", "yw", "de", "d$",
    "dj", "dk", "dG", "d}", "y%", ">j", "<k", "diw", "daW", "di(", "da{", "di[", "di$", "da$",
    "die", "dae", "di<", "ca>", "ci\"", "yi`", "di'", "dd", "cc", "yy", ">>", "<<", "2", "3", "9",
    "<esc>", "<cr>", "<bs>", "<tab>", "<left>", "<right>", "<up>", "<down>", " ", "/a<cr>",
    "/é<cr>", "?k<cr>", "/İ<cr>", "n", "N", "*", "<c-d>", "<c-u>", ":2<cr>", ":w<cr>", "z", "é",
    "😀", "\n", "q", "iw", "a$", "ie", "U", "r-", "99", "i<", "a\"", "ab", "<c-c>",
];

fn parse_keys(s: &str) -> Vec<Key> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(c) = rest.chars().next() {
        if c == '<'
            && let Some(end) = rest.find('>')
        {
            let key = match &rest[1..end] {
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

/// An editor: its text, its cursor, an undo history and a clipboard.
struct Editor {
    text: String,
    cursor: usize,
    undo: Vec<String>,
    redo: Vec<String>,
    clipboard: Option<String>,
}

impl Editor {
    fn new(text: String, cursor: usize) -> Self {
        Self {
            text,
            cursor,
            undo: Vec::new(),
            redo: Vec::new(),
            clipboard: None,
        }
    }

    /// The nearest character boundary at or before `i`.
    fn floor(&self, i: usize) -> usize {
        let mut i = i.min(self.text.len());
        while !self.text.is_char_boundary(i) {
            i -= 1;
        }
        i
    }

    fn check(&self, what: &str, i: usize) -> Result<(), TestCaseError> {
        prop_assert!(
            i <= self.text.len() && self.text.is_char_boundary(i),
            "{what} {i} out of {:?}",
            self.text
        );
        Ok(())
    }

    fn type_key(&mut self, vim: &mut Vim, key: Key) -> Result<(), TestCaseError> {
        let ctx = Ctx {
            text: &self.text,
            cursor: self.cursor,
            clipboard: self.clipboard.as_deref(),
            page_lines: 4,
        };
        let mode = vim.mode();
        let actions = vim.key(&ctx, key);
        for a in actions {
            match a {
                Action::Pass => {
                    prop_assert_eq!(mode, Mode::Insert);
                    self.pass(key);
                }
                Action::Edit { range, text } => {
                    prop_assert!(range.start <= range.end, "{range:?}");
                    self.check("edit start", range.start)?;
                    self.check("edit end", range.end)?;
                    self.undo.push(self.text.clone());
                    self.redo.clear();
                    self.text.replace_range(range.clone(), &text);
                    self.cursor = range.start + text.len();
                }
                Action::Cursor(c) => {
                    self.check("cursor", c)?;
                    self.cursor = c;
                }
                Action::Select(r) => {
                    self.check("selection start", r.start)?;
                    self.check("selection end", r.end)?;
                    self.cursor = r.end;
                }
                Action::Undo => {
                    if let Some(old) = self.undo.pop() {
                        self.redo.push(std::mem::replace(&mut self.text, old));
                        self.cursor = self.floor(self.cursor);
                    }
                }
                Action::Redo => {
                    if let Some(new) = self.redo.pop() {
                        self.undo.push(std::mem::replace(&mut self.text, new));
                        self.cursor = self.floor(self.cursor);
                    }
                }
                Action::Yank(y) => self.clipboard = Some(y),
                Action::Write | Action::Quit => {}
            }
        }
        Ok(())
    }

    /// What the editor does with a key Vim lets through.
    fn pass(&mut self, key: Key) {
        let c = self.cursor;
        match key {
            Key::Char(ch) => {
                self.text.insert(c, ch);
                self.cursor += ch.len_utf8();
            }
            Key::Enter | Key::Tab => {
                let ch = if key == Key::Enter { '\n' } else { '\t' };
                self.text.insert(c, ch);
                self.cursor += 1;
            }
            Key::Backspace if c > 0 => {
                let p = self.floor(c - 1);
                self.text.replace_range(p..c, "");
                self.cursor = p;
            }
            Key::Left if c > 0 => self.cursor = self.floor(c - 1),
            Key::Right => {
                if let Some(ch) = self.text[c..].chars().next() {
                    self.cursor += ch.len_utf8();
                }
            }
            _ => {}
        }
    }
}

fn text() -> impl Strategy<Value = String> {
    prop::collection::vec(prop::sample::select(CHARS), 0..40).prop_map(|v| v.into_iter().collect())
}

/// A text and a character boundary in it.
fn text_at() -> impl Strategy<Value = (String, usize)> {
    text().prop_flat_map(|t| {
        let n = t.chars().count();
        (Just(t), 0..=n).prop_map(|(t, k)| {
            let i = t.char_indices().nth(k).map_or(t.len(), |(i, _)| i);
            (t, i)
        })
    })
}

fn commands() -> impl Strategy<Value = Vec<&'static str>> {
    // Escape often enough to leave insert mode and the command line.
    let command = prop_oneof![6 => prop::sample::select(VOCAB), 1 => Just("<esc>")];
    prop::collection::vec(command, 0..30)
}

proptest! {
    #[test]
    fn keys_never_break_the_text((t, i) in text_at(), cmds in commands()) {
        let mut vim = Vim::new();
        let mut ed = Editor::new(t, i);
        for cmd in &cmds {
            for key in parse_keys(cmd) {
                ed.type_key(&mut vim, key)?;
                if let Some(b) = vim.block(&ed.text, ed.cursor) {
                    prop_assert!(b.start < b.end);
                    ed.check("block start", b.start)?;
                    ed.check("block end", b.end)?;
                }
            }
        }
        // Two escapes always get back to normal mode, with nothing pending.
        ed.type_key(&mut vim, Key::Escape)?;
        ed.type_key(&mut vim, Key::Escape)?;
        prop_assert_eq!(vim.mode(), Mode::Normal);
        prop_assert_eq!(vim.status(), "");
    }

    #[test]
    fn text_functions_stay_on_boundaries((t, i) in text_at(), big: bool, around: bool) {
        let ok = |what: &str, p: usize| -> Result<(), TestCaseError> {
            prop_assert!(p <= t.len() && t.is_char_boundary(p), "{what}: {p} in {t:?} at {i}");
            Ok(())
        };
        let range = |what: &str, r: Option<std::ops::Range<usize>>| {
            if let Some(r) = r {
                prop_assert!(r.start <= r.end, "{what}: {r:?} in {t:?} at {i}");
                ok(what, r.start)?;
                ok(what, r.end)?;
            }
            Ok(())
        };
        ok("line_start", line_start(&t, i))?;
        ok("line_end", line_end(&t, i))?;
        ok("next_char", next_char(&t, i))?;
        ok("prev_char", prev_char(&t, i))?;
        ok("last_char", last_char(&t, i))?;
        ok("clamp_normal", clamp_normal(&t, i))?;
        ok("first_non_blank", first_non_blank(&t, i))?;
        ok("next_word", next_word(&t, i, big))?;
        ok("word_end", word_end(&t, i, big))?;
        ok("prev_word", prev_word(&t, i, big))?;
        ok("paragraph", paragraph(&t, i, big))?;
        for col in [0, 1, 3, 100] {
            ok("at_column", at_column(&t, line_start(&t, i), col, around))?;
        }
        for c in ['a', '$', 'é', '\n'] {
            if let Some(p) = find_in_line(&t, i, c, big, around) {
                ok("find_in_line", p)?;
            }
        }
        if let Some(p) = matching_bracket(&t, i) {
            ok("matching_bracket", p)?;
        }
        range("word_object", word_object(&t, i, big, around))?;
        for (open, close) in [('(', ')'), ('{', '}'), ('[', ']'), ('<', '>')] {
            range("bracket_object", bracket_object(&t, i, open, close, around))?;
        }
        for q in ['"', '\'', '`'] {
            range("quote_object", quote_object(&t, i, q, around))?;
        }
        range("environment_object", environment_object(&t, i, around))?;
        range("math_object", math_object(&t, i, around))?;

        // Lines and characters agree with each other.
        prop_assert_eq!(line_offset(&t, line_of(&t, i)), line_start(&t, i));
        prop_assert!(line_of(&t, i) < line_count(&t));
        prop_assert_eq!(column(&t, i), t[line_start(&t, i)..i].chars().count());
        if i < t.len() {
            prop_assert_eq!(prev_char(&t, next_char(&t, i)), i);
            prop_assert_eq!(char_at(&t, i), t[i..].chars().next());
        }
        if i > 0 {
            prop_assert_eq!(next_char(&t, prev_char(&t, i)), i);
        }
    }
}
