//! A Vim emulation for an editor that knows nothing about it: keys go in,
//! edits and cursor moves come out ([`Action`]), computed on plain text.
//!
//! Normal, insert, visual and visual-line modes; counts; the operators `d c
//! y > <` with motions and text objects (including LaTeX's `ie`/`ae`
//! environments and `i$`/`a$` inline math); `x X s S r J p P u Ctrl-R . ~ D C
//! Y`; search with `/ ? n N *`; and the ex commands `:w :q :wq :x :<line>`.
//! In insert mode keys are passed to the editor, except Escape.

mod text;

use std::ops::Range;

pub use text::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
    Visual,
    VisualLine,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Normal => "NORMAL",
            Mode::Insert => "INSERT",
            Mode::Visual => "VISUAL",
            Mode::VisualLine => "VISUAL LINE",
        }
    }
}

/// A key press, as Vim sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Ctrl(char),
    Escape,
    Enter,
    Backspace,
    Tab,
    Left,
    Right,
    Up,
    Down,
}

/// What the editor must do, in order. Offsets of an action are in the text
/// as left by the previous ones.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Let the key reach the editor (insert mode).
    Pass,
    Edit {
        range: Range<usize>,
        text: String,
    },
    Cursor(usize),
    /// A visual selection: `range` is what is selected.
    Select(Range<usize>),
    Undo,
    Redo,
    /// Put this text on the system clipboard.
    Yank(String),
    Write,
    Quit,
}

/// What the editor tells about itself.
pub struct Ctx<'a> {
    pub text: &'a str,
    pub cursor: usize,
    /// The system clipboard, pasted when it is not what Vim last yanked.
    pub clipboard: Option<&'a str>,
    /// Lines visible at once (for Ctrl-D and Ctrl-U).
    pub page_lines: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Register {
    text: String,
    linewise: bool,
}

/// How a motion's range is taken by an operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Exclusive,
    Inclusive,
    Linewise,
}

pub struct Vim {
    mode: Mode,
    /// What was typed of the command being entered.
    pending: Vec<Key>,
    register: Register,
    last_find: Option<(char, char)>,
    /// The column `j` and `k` aim for.
    want_col: Option<usize>,
    /// The ends of a visual selection: where it started, and the cursor.
    anchor: usize,
    head: usize,
    /// `:…`, `/…` or `?…` being typed.
    cmdline: Option<(char, String)>,
    search: Option<(String, bool)>,
    /// The keys of the last change, for `.`.
    dot: Vec<Key>,
    /// The keys of the change being made (until back in normal mode).
    recording: Option<Vec<Key>>,
    replaying: bool,
    /// The keys of the command being run.
    last_keys: Vec<Key>,
}

impl Default for Vim {
    fn default() -> Self {
        Self::new()
    }
}

/// Text with a cursor, as edited while replaying (`.`).
struct Sim {
    text: String,
    cursor: usize,
}

impl Sim {
    fn apply(&mut self, actions: &[Action]) {
        for a in actions {
            match a {
                Action::Edit { range, text } => {
                    self.text.replace_range(range.clone(), text);
                    self.cursor = range.start + text.len();
                }
                Action::Cursor(c) | Action::Select(Range { start: c, .. }) => {
                    self.cursor = (*c).min(self.text.len())
                }
                _ => {}
            }
        }
    }
}

impl Vim {
    pub fn new() -> Self {
        Self {
            mode: Mode::Normal,
            pending: Vec::new(),
            register: Register::default(),
            last_find: None,
            want_col: None,
            anchor: 0,
            head: 0,
            cmdline: None,
            search: None,
            dot: Vec::new(),
            recording: None,
            replaying: false,
            last_keys: Vec::new(),
        }
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// What the status bar shows: the command line, or the keys typed so far.
    pub fn status(&self) -> String {
        if let Some((c, s)) = &self.cmdline {
            return format!("{c}{s}");
        }
        self.pending
            .iter()
            .map(|k| match k {
                Key::Char(c) => c.to_string(),
                Key::Ctrl(c) => format!("^{}", c.to_ascii_uppercase()),
                _ => String::new(),
            })
            .collect()
    }

    /// Back to normal mode, forgetting what was being typed.
    pub fn reset(&mut self) {
        self.mode = Mode::Normal;
        self.pending.clear();
        self.cmdline = None;
        self.recording = None;
    }

    /// The range of the normal-mode cursor's block, to draw it.
    pub fn block(&self, t: &str, cursor: usize) -> Option<Range<usize>> {
        if self.mode == Mode::Insert {
            return None;
        }
        let c = clamp_normal(t, cursor.min(t.len()));
        let e = next_char(t, c);
        (e > c && char_at(t, c) != Some('\n')).then_some(c..e)
    }

    pub fn key(&mut self, ctx: &Ctx, key: Key) -> Vec<Action> {
        if let Some(r) = &mut self.recording
            && !self.replaying
        {
            r.push(key);
        }
        let actions = match self.mode {
            Mode::Insert => self.insert_key(ctx, key),
            _ if self.cmdline.is_some() => self.cmdline_key(ctx, key),
            _ => self.command_key(ctx, key),
        };
        if self.mode == Mode::Normal
            && self.pending.is_empty()
            && let Some(keys) = self.recording.take()
            && !self.replaying
        {
            self.dot = keys;
        }
        actions
    }

    fn insert_key(&mut self, ctx: &Ctx, key: Key) -> Vec<Action> {
        match key {
            Key::Escape | Key::Ctrl('[') | Key::Ctrl('c') => {
                self.mode = Mode::Normal;
                let t = ctx.text;
                let c = ctx.cursor.min(t.len());
                let back = if c > line_start(t, c) {
                    prev_char(t, c)
                } else {
                    c
                };
                vec![Action::Cursor(back)]
            }
            _ if self.replaying => {
                let t = ctx.text;
                let c = ctx.cursor.min(t.len());
                match key {
                    Key::Char(ch) => vec![Action::Edit {
                        range: c..c,
                        text: ch.to_string(),
                    }],
                    Key::Enter => vec![Action::Edit {
                        range: c..c,
                        text: "\n".into(),
                    }],
                    Key::Tab => vec![Action::Edit {
                        range: c..c,
                        text: "  ".into(),
                    }],
                    Key::Backspace if c > 0 => vec![Action::Edit {
                        range: prev_char(t, c)..c,
                        text: String::new(),
                    }],
                    _ => Vec::new(),
                }
            }
            _ => vec![Action::Pass],
        }
    }

    fn cmdline_key(&mut self, ctx: &Ctx, key: Key) -> Vec<Action> {
        let Some((kind, mut line)) = self.cmdline.take() else {
            return Vec::new();
        };
        match key {
            Key::Escape | Key::Ctrl('[') | Key::Ctrl('c') => {}
            Key::Backspace => {
                if line.pop().is_some() {
                    self.cmdline = Some((kind, line));
                }
            }
            Key::Char(c) => {
                line.push(c);
                self.cmdline = Some((kind, line));
            }
            Key::Enter => return self.run_cmdline(ctx, kind, &line),
            _ => self.cmdline = Some((kind, line)),
        }
        Vec::new()
    }

    fn run_cmdline(&mut self, ctx: &Ctx, kind: char, line: &str) -> Vec<Action> {
        if kind == ':' {
            let cmd = line.trim();
            if let Ok(n) = cmd.parse::<usize>() {
                let at = first_non_blank(ctx.text, line_offset(ctx.text, n.saturating_sub(1)));
                return self.move_to(ctx, at);
            }
            return match cmd {
                "w" | "write" => vec![Action::Write],
                "q" | "quit" | "q!" => vec![Action::Quit],
                "wq" | "x" | "xit" => vec![Action::Write, Action::Quit],
                _ => Vec::new(),
            };
        }
        if !line.is_empty() {
            self.search = Some((line.to_string(), kind == '?'));
        }
        self.search_next(ctx, false, 1)
    }

    /// `n` / `N` (`reverse`): the next match of the last search.
    fn search_next(&mut self, ctx: &Ctx, reverse: bool, count: usize) -> Vec<Action> {
        let Some((query, backward)) = self.search.clone() else {
            return Vec::new();
        };
        let backward = backward != reverse;
        let mut at = ctx.cursor;
        for _ in 0..count {
            match find(ctx.text, &query, at, backward) {
                Some(p) => at = p,
                None => break,
            }
        }
        if at == ctx.cursor {
            return Vec::new();
        }
        self.move_to(ctx, at)
    }

    fn move_to(&mut self, ctx: &Ctx, at: usize) -> Vec<Action> {
        self.want_col = None;
        if matches!(self.mode, Mode::Visual | Mode::VisualLine) {
            return vec![self.selection(ctx.text, at)];
        }
        vec![Action::Cursor(clamp_normal(ctx.text, at))]
    }

    /// The visual selection from the anchor to `head` (both included).
    fn selection(&mut self, t: &str, head: usize) -> Action {
        self.head = head;
        let (a, h) = (self.anchor.min(head), self.anchor.max(head));
        let range = if self.mode == Mode::VisualLine {
            line_start(t, a)..(line_end(t, h) + 1).min(t.len())
        } else {
            a..next_char(t, h).max(a)
        };
        // The cursor is the head: the editor's selection ends there.
        if head < self.anchor {
            Action::Select(range.end..range.start)
        } else {
            Action::Select(range)
        }
    }

    fn command_key(&mut self, ctx: &Ctx, key: Key) -> Vec<Action> {
        if matches!(key, Key::Escape | Key::Ctrl('[') | Key::Ctrl('c')) {
            self.pending.clear();
            if self.mode != Mode::Normal {
                self.mode = Mode::Normal;
                return vec![Action::Cursor(clamp_normal(ctx.text, ctx.cursor))];
            }
            return Vec::new();
        }
        self.pending.push(key);
        let keys = self.pending.clone();
        match self.parse(&keys) {
            Parse::Incomplete => Vec::new(),
            Parse::Invalid => {
                self.pending.clear();
                Vec::new()
            }
            Parse::Done(cmd) => {
                self.pending.clear();
                self.last_keys = keys;
                self.run(ctx, cmd)
            }
        }
    }

    fn parse(&self, keys: &[Key]) -> Parse {
        let mut i = 0;
        let count1 = take_count(keys, &mut i);
        let Some(&k) = keys.get(i) else {
            return Parse::Incomplete;
        };
        let count1v = count1.unwrap_or(1);
        let visual = matches!(self.mode, Mode::Visual | Mode::VisualLine);
        // Operators.
        if let Key::Char(op @ ('d' | 'c' | 'y' | '>' | '<')) = k {
            if visual {
                return Parse::Done(Cmd::VisualOp(op));
            }
            i += 1;
            let count2 = take_count(keys, &mut i);
            let count = count1v * count2.unwrap_or(1);
            let Some(&next) = keys.get(i) else {
                return Parse::Incomplete;
            };
            if next == Key::Char(op) {
                return Parse::Done(Cmd::OpLines(op, count));
            }
            if let Key::Char(m @ ('i' | 'a')) = next {
                return match keys.get(i + 1) {
                    None => Parse::Incomplete,
                    Some(Key::Char(o)) => Parse::Done(Cmd::OpObject(op, m == 'a', *o)),
                    _ => Parse::Invalid,
                };
            }
            return match parse_motion(&keys[i..], count2.or(count1)) {
                MotionParse::Incomplete => Parse::Incomplete,
                MotionParse::Invalid => Parse::Invalid,
                MotionParse::Done(m) => Parse::Done(Cmd::OpMotion(op, m, count)),
            };
        }
        if visual && let Key::Char(m @ ('i' | 'a')) = k {
            return match keys.get(i + 1) {
                None => Parse::Incomplete,
                Some(Key::Char(o)) => Parse::Done(Cmd::VisualObject(m == 'a', *o)),
                _ => Parse::Invalid,
            };
        }
        if let Key::Char('r') = k {
            return match keys.get(i + 1) {
                None => Parse::Incomplete,
                Some(Key::Char(c)) => Parse::Done(Cmd::Replace(*c, count1v)),
                _ => Parse::Invalid,
            };
        }
        match parse_motion(&keys[i..], count1) {
            MotionParse::Done(m) => return Parse::Done(Cmd::Move(m, count1v)),
            MotionParse::Incomplete => return Parse::Incomplete,
            MotionParse::Invalid => {}
        }
        let simple = match k {
            Key::Char(c) => match c {
                'x' | 'X' | 's' | 'S' | 'D' | 'C' | 'Y' | 'J' | 'p' | 'P' | 'u' | '.' | 'i'
                | 'a' | 'I' | 'A' | 'o' | 'O' | 'v' | 'V' | ':' | '/' | '?' | 'n' | 'N' | '*'
                | '~' => Some(c),
                _ => None,
            },
            Key::Ctrl('r') => Some('R'),
            Key::Ctrl('d') => Some('\u{4}'),
            Key::Ctrl('u') => Some('\u{15}'),
            _ => None,
        };
        match simple {
            Some(c) => Parse::Done(Cmd::Simple(c, count1v)),
            None => Parse::Invalid,
        }
    }

    fn run(&mut self, ctx: &Ctx, cmd: Cmd) -> Vec<Action> {
        let t = ctx.text;
        // In visual mode the editor's cursor is the end of the selection.
        let cur = if matches!(self.mode, Mode::Visual | Mode::VisualLine) {
            self.head.min(t.len())
        } else {
            ctx.cursor.min(t.len())
        };
        match cmd {
            Cmd::Move(m, count) => {
                let Some((to, _)) = self.motion(t, cur, m, count) else {
                    return Vec::new();
                };
                if !matches!(m, Motion::Up | Motion::Down) {
                    self.want_col = None;
                }
                if matches!(self.mode, Mode::Visual | Mode::VisualLine) {
                    return vec![self.selection(t, to)];
                }
                vec![Action::Cursor(clamp_normal(t, to))]
            }
            Cmd::OpMotion(op, m, count) => {
                self.start_change(op);
                let Some((to, kind)) = self.motion(t, cur, m, count) else {
                    self.recording = None;
                    return Vec::new();
                };
                // `cw` changes to the end of the word, like `ce`.
                let (to, kind) = if op == 'c'
                    && matches!(m, Motion::Word(_))
                    && char_at(t, cur).is_some_and(|c| !c.is_whitespace())
                {
                    let big = matches!(m, Motion::Word(true));
                    let mut e = cur;
                    for _ in 0..count {
                        e = word_end(t, e, big);
                    }
                    (e, Kind::Inclusive)
                } else if matches!(m, Motion::Word(_))
                    && to > line_end(t, cur)
                    && line_end(t, cur) > cur
                {
                    // `dw` on a line's last word stops at its end.
                    (line_end(t, cur), Kind::Exclusive)
                } else {
                    (to, kind)
                };
                let range = match kind {
                    Kind::Exclusive => cur.min(to)..cur.max(to),
                    Kind::Inclusive => cur.min(to)..next_char(t, cur.max(to)),
                    Kind::Linewise => lines(t, cur.min(to), cur.max(to)),
                };
                self.operate(ctx, op, range, kind == Kind::Linewise)
            }
            Cmd::OpLines(op, count) => {
                self.start_change(op);
                let last = line_offset(t, line_of(t, cur) + count - 1);
                self.operate(ctx, op, lines(t, cur, last), true)
            }
            Cmd::OpObject(op, around, obj) => {
                self.start_change(op);
                match object(t, cur, obj, around) {
                    Some(range) => self.operate(ctx, op, range, false),
                    None => {
                        self.recording = None;
                        Vec::new()
                    }
                }
            }
            Cmd::VisualOp(op) => {
                let linewise = self.mode == Mode::VisualLine;
                let range = match self.selection(t, cur) {
                    Action::Select(r) => r.start.min(r.end)..r.start.max(r.end),
                    _ => return Vec::new(),
                };
                self.mode = Mode::Normal;
                self.operate(ctx, op, range, linewise)
            }
            Cmd::VisualObject(around, obj) => match object(t, cur, obj, around) {
                Some(r) if !r.is_empty() => {
                    self.anchor = r.start;
                    vec![self.selection(t, prev_char(t, r.end))]
                }
                _ => Vec::new(),
            },
            Cmd::Replace(c, count) => {
                let end = line_end(t, cur);
                let mut e = cur;
                for _ in 0..count {
                    if e >= end {
                        return Vec::new();
                    }
                    e = next_char(t, e);
                }
                self.start_change('r');
                let text: String = std::iter::repeat_n(c, count).collect();
                let last = cur + text.len() - c.len_utf8();
                vec![
                    Action::Edit {
                        range: cur..e,
                        text,
                    },
                    Action::Cursor(last),
                ]
            }
            Cmd::Simple(c, count) => self.simple(ctx, c, count),
        }
    }

    /// Start recording a change for `.`.
    fn start_change(&mut self, op: char) {
        if op != 'y' && !self.replaying {
            self.recording = Some(self.pending_snapshot());
        }
    }

    /// The keys of the command being run (already cleared from `pending`).
    fn pending_snapshot(&self) -> Vec<Key> {
        self.last_keys.clone()
    }

    /// Delete, change, yank or indent `range`.
    fn operate(&mut self, ctx: &Ctx, op: char, range: Range<usize>, linewise: bool) -> Vec<Action> {
        let t = ctx.text;
        let text = t[range.clone()].to_string();
        match op {
            'y' => {
                self.yank(text.clone(), linewise);
                vec![
                    Action::Yank(text),
                    Action::Cursor(clamp_normal(t, range.start)),
                ]
            }
            'd' => {
                self.yank(text.clone(), linewise);
                let mut range = range;
                // The last line's deletion takes the newline before it.
                if linewise
                    && range.end == t.len()
                    && range.start > 0
                    && !t[range.clone()].ends_with('\n')
                {
                    range.start -= 1;
                }
                let after = remove(t, &range);
                let cursor = if linewise {
                    first_non_blank(&after, range.start.min(after.len()))
                } else {
                    clamp_normal(&after, range.start)
                };
                vec![
                    Action::Yank(text),
                    Action::Edit {
                        range,
                        text: String::new(),
                    },
                    Action::Cursor(cursor),
                ]
            }
            'c' => {
                self.yank(text.clone(), linewise);
                self.mode = Mode::Insert;
                let (range, keep) = if linewise {
                    // Keep the first line's indentation, and the newline.
                    let indent: String = t[range.start..]
                        .chars()
                        .take_while(|c| *c == ' ' || *c == '\t')
                        .collect();
                    let end = if t[range.clone()].ends_with('\n') {
                        range.end - 1
                    } else {
                        range.end
                    };
                    (range.start..end, indent)
                } else {
                    (range, String::new())
                };
                let at = range.start + keep.len();
                vec![
                    Action::Yank(text),
                    Action::Edit { range, text: keep },
                    Action::Cursor(at),
                ]
            }
            '>' | '<' => {
                let range = lines(t, range.start, prev_char(t, range.end).max(range.start));
                let block = &t[range.clone()];
                let new: String = block
                    .split_inclusive('\n')
                    .map(|l| {
                        if op == '>' {
                            if l.trim().is_empty() {
                                l.to_string()
                            } else {
                                format!("  {l}")
                            }
                        } else if let Some(r) = l.strip_prefix("  ") {
                            r.to_string()
                        } else {
                            l.strip_prefix(['\t', ' ']).unwrap_or(l).to_string()
                        }
                    })
                    .collect();
                let start = range.start;
                let after_first = first_non_blank(&new, 0);
                vec![
                    Action::Edit { range, text: new },
                    Action::Cursor(start + after_first),
                ]
            }
            _ => Vec::new(),
        }
    }

    fn yank(&mut self, text: String, linewise: bool) {
        self.register = Register { text, linewise };
    }

    /// What to paste: the system clipboard if something else was copied
    /// since Vim's last yank.
    fn paste_register(&self, ctx: &Ctx) -> Register {
        match ctx.clipboard {
            Some(c) if c != self.register.text && !c.is_empty() => Register {
                text: c.to_string(),
                linewise: c.ends_with('\n'),
            },
            _ => self.register.clone(),
        }
    }

    fn simple(&mut self, ctx: &Ctx, c: char, count: usize) -> Vec<Action> {
        let t = ctx.text;
        let visual = matches!(self.mode, Mode::Visual | Mode::VisualLine);
        let cur = if visual {
            self.head.min(t.len())
        } else {
            ctx.cursor.min(t.len())
        };
        let ls = line_start(t, cur);
        let le = line_end(t, cur);
        match c {
            'x' | 'X' | 'D' | 'C' | 's' | 'S' | 'Y' | '~' | 'J' | 'p' | 'P' | 'o' | 'O' | 'i'
            | 'a' | 'I' | 'A'
                if visual && !matches!(c, 'x' | 'J' | '~' | 'p' | 'P') =>
            {
                Vec::new()
            }
            'x' if visual => self.run(ctx, Cmd::VisualOp('d')),
            'x' | 'X' => {
                if le == ls {
                    return Vec::new();
                }
                self.start_change(c);
                let range = if c == 'x' {
                    let mut e = cur;
                    for _ in 0..count {
                        if e < le {
                            e = next_char(t, e);
                        }
                    }
                    cur..e
                } else {
                    let mut s = cur;
                    for _ in 0..count {
                        if s > ls {
                            s = prev_char(t, s);
                        }
                    }
                    s..cur
                };
                if range.is_empty() {
                    self.recording = None;
                    return Vec::new();
                }
                self.operate(ctx, 'd', range, false)
            }
            'D' | 'C' => {
                self.start_change(c);
                let op = if c == 'D' { 'd' } else { 'c' };
                self.operate(ctx, op, cur..le, false)
            }
            's' => {
                self.start_change(c);
                let mut e = cur;
                for _ in 0..count {
                    if e < le {
                        e = next_char(t, e);
                    }
                }
                self.operate(ctx, 'c', cur..e, false)
            }
            'S' => self.run(ctx, Cmd::OpLines('c', count)),
            'Y' => self.run(ctx, Cmd::OpLines('y', count)),
            '~' => {
                if le == ls {
                    return Vec::new();
                }
                self.start_change(c);
                let mut e = cur;
                for _ in 0..count {
                    if e < le {
                        e = next_char(t, e);
                    }
                }
                let toggled: String = t[cur..e]
                    .chars()
                    .flat_map(|ch| {
                        if ch.is_uppercase() {
                            ch.to_lowercase().collect::<Vec<_>>()
                        } else {
                            ch.to_uppercase().collect()
                        }
                    })
                    .collect();
                let after = cur + toggled.len();
                let mut actions = vec![Action::Edit {
                    range: cur..e,
                    text: toggled,
                }];
                let line_end_after = after + (le - e);
                actions.push(Action::Cursor(if after < line_end_after {
                    after
                } else {
                    after.saturating_sub(1).max(cur)
                }));
                actions
            }
            'J' => {
                self.start_change(c);
                let n = count.max(2) - 1;
                let mut end = le;
                for _ in 0..n {
                    if end >= t.len() {
                        break;
                    }
                    end = line_end(t, end + 1);
                }
                if end == le {
                    self.recording = None;
                    return Vec::new();
                }
                let joined: Vec<&str> = t[ls..end].split('\n').collect();
                let mut new = joined[0].trim_end().to_string();
                let mut cursor = new.len();
                for part in &joined[1..] {
                    let part = part.trim_start();
                    cursor = new.len();
                    if !part.is_empty() && !part.starts_with(')') && !new.is_empty() {
                        new.push(' ');
                    }
                    new.push_str(part);
                }
                vec![
                    Action::Edit {
                        range: ls..end,
                        text: new,
                    },
                    Action::Cursor(ls + cursor),
                ]
            }
            'p' | 'P' => {
                let reg = self.paste_register(ctx);
                if reg.text.is_empty() {
                    return Vec::new();
                }
                self.start_change(c);
                let text = reg.text.repeat(count);
                if reg.linewise {
                    let text = if text.ends_with('\n') {
                        text
                    } else {
                        format!("{text}\n")
                    };
                    let at = if c == 'p' {
                        if le >= t.len() {
                            // After the last line: a newline first.
                            let body = text.trim_end_matches('\n').to_string();
                            return vec![
                                Action::Edit {
                                    range: t.len()..t.len(),
                                    text: format!("\n{body}"),
                                },
                                Action::Cursor(first_non_blank(
                                    &format!("{t}\n{body}"),
                                    t.len() + 1,
                                )),
                            ];
                        }
                        le + 1
                    } else {
                        ls
                    };
                    let indent = text
                        .chars()
                        .take_while(|c| *c == ' ' || *c == '\t')
                        .map(char::len_utf8)
                        .sum::<usize>();
                    vec![
                        Action::Edit {
                            range: at..at,
                            text,
                        },
                        Action::Cursor(at + indent),
                    ]
                } else {
                    let at = if c == 'p' && le > ls {
                        next_char(t, cur)
                    } else {
                        cur
                    };
                    let last = at + text.len() - text.chars().next_back().map_or(0, char::len_utf8);
                    vec![
                        Action::Edit {
                            range: at..at,
                            text,
                        },
                        Action::Cursor(last),
                    ]
                }
            }
            'u' => (0..count).map(|_| Action::Undo).collect(),
            'R' => (0..count).map(|_| Action::Redo).collect(),
            '.' => self.repeat(ctx, count),
            'i' | 'a' | 'I' | 'A' => {
                self.start_change(c);
                self.mode = Mode::Insert;
                let at = match c {
                    'i' => cur,
                    'a' => {
                        if le > ls {
                            next_char(t, cur)
                        } else {
                            cur
                        }
                    }
                    'I' => first_non_blank(t, cur),
                    _ => le,
                };
                vec![Action::Cursor(at)]
            }
            'o' | 'O' => {
                self.start_change(c);
                self.mode = Mode::Insert;
                let indent: String = t[ls..]
                    .chars()
                    .take_while(|c| *c == ' ' || *c == '\t')
                    .collect();
                if c == 'o' {
                    let text = format!("\n{indent}");
                    let at = le + text.len();
                    vec![
                        Action::Edit {
                            range: le..le,
                            text,
                        },
                        Action::Cursor(at),
                    ]
                } else {
                    let text = format!("{indent}\n");
                    vec![
                        Action::Edit {
                            range: ls..ls,
                            text,
                        },
                        Action::Cursor(ls + indent.len()),
                    ]
                }
            }
            'v' | 'V' => {
                let mode = if c == 'v' {
                    Mode::Visual
                } else {
                    Mode::VisualLine
                };
                if self.mode == mode {
                    self.mode = Mode::Normal;
                    return vec![Action::Cursor(clamp_normal(t, cur))];
                }
                if !visual {
                    self.anchor = clamp_normal(t, cur);
                }
                self.mode = mode;
                vec![self.selection(t, clamp_normal(t, cur))]
            }
            ':' | '/' | '?' => {
                self.cmdline = Some((c, String::new()));
                Vec::new()
            }
            'n' | 'N' => self.search_next(ctx, c == 'N', count),
            '*' => {
                let Some(w) = word_object(t, cur, false, false) else {
                    return Vec::new();
                };
                let word = t[w.clone()].to_string();
                if word.trim().is_empty() {
                    return Vec::new();
                }
                self.search = Some((format!("\\<{word}\\>"), false));
                let ctx = Ctx {
                    text: t,
                    cursor: w.start,
                    clipboard: None,
                    page_lines: ctx.page_lines,
                };
                self.search_next(&ctx, false, count)
            }
            '\u{4}' | '\u{15}' => {
                let half = (ctx.page_lines / 2).max(1) * count;
                let line = line_of(t, cur);
                let target = if c == '\u{4}' {
                    line + half
                } else {
                    line.saturating_sub(half)
                };
                let at = first_non_blank(t, line_offset(t, target));
                self.move_to(ctx, at)
            }
            _ => Vec::new(),
        }
    }

    /// `.`: the last change again, `count` times.
    fn repeat(&mut self, ctx: &Ctx, count: usize) -> Vec<Action> {
        if self.dot.is_empty() || self.replaying {
            return Vec::new();
        }
        let keys = self.dot.clone();
        let mut sim = Sim {
            text: ctx.text.to_string(),
            cursor: ctx.cursor,
        };
        self.replaying = true;
        let mut yanks = Vec::new();
        for _ in 0..count {
            for &k in &keys {
                let c = Ctx {
                    text: &sim.text,
                    cursor: sim.cursor,
                    clipboard: None,
                    page_lines: ctx.page_lines,
                };
                let actions = self.key(&c, k);
                yanks.extend(actions.iter().filter_map(|a| match a {
                    Action::Yank(y) => Some(y.clone()),
                    _ => None,
                }));
                sim.apply(&actions);
            }
            // A change left in insert mode ends with it.
            if self.mode == Mode::Insert {
                let c = Ctx {
                    text: &sim.text,
                    cursor: sim.cursor,
                    clipboard: None,
                    page_lines: ctx.page_lines,
                };
                let actions = self.key(&c, Key::Escape);
                sim.apply(&actions);
            }
        }
        self.replaying = false;
        self.mode = Mode::Normal;
        // One edit, so that one undo takes it back.
        let old = ctx.text;
        let new = &sim.text;
        let prefix = old
            .char_indices()
            .zip(new.chars())
            .take_while(|((_, a), b)| a == b)
            .last()
            .map_or(0, |((i, a), _)| i + a.len_utf8());
        let suffix = old[prefix..]
            .chars()
            .rev()
            .zip(new[prefix..].chars().rev())
            .take_while(|(a, b)| a == b)
            .map(|(a, _)| a.len_utf8())
            .sum::<usize>();
        let suffix = suffix.min(old.len() - prefix).min(new.len() - prefix);
        let mut actions: Vec<Action> = yanks.into_iter().take(1).map(Action::Yank).collect();
        if old != new.as_str() {
            actions.push(Action::Edit {
                range: prefix..old.len() - suffix,
                text: new[prefix..new.len() - suffix].to_string(),
            });
        }
        actions.push(Action::Cursor(clamp_normal(new, sim.cursor)));
        actions
    }

    /// Where `m` (repeated `count` times) goes from `cur`, and how an
    /// operator takes it.
    fn motion(&mut self, t: &str, cur: usize, m: Motion, count: usize) -> Option<(usize, Kind)> {
        let mut p = cur;
        let kind = match m {
            Motion::Left => {
                let ls = line_start(t, cur);
                for _ in 0..count {
                    if p > ls {
                        p = prev_char(t, p);
                    }
                }
                Kind::Exclusive
            }
            Motion::Right => {
                let le = line_end(t, cur);
                for _ in 0..count {
                    if p < le {
                        p = next_char(t, p);
                    }
                }
                Kind::Exclusive
            }
            Motion::Up | Motion::Down => {
                let col = *self.want_col.get_or_insert_with(|| column(t, cur));
                let line = line_of(t, cur);
                let target = if m == Motion::Up {
                    line.checked_sub(count)?
                } else {
                    let last = line_count(t) - 1;
                    if line == last {
                        return None;
                    }
                    (line + count).min(last)
                };
                p = at_column(t, line_offset(t, target), col, true);
                Kind::Linewise
            }
            Motion::Word(big) => {
                for _ in 0..count {
                    p = next_word(t, p, big);
                }
                Kind::Exclusive
            }
            Motion::WordEnd(big) => {
                for _ in 0..count {
                    p = word_end(t, p, big);
                }
                Kind::Inclusive
            }
            Motion::WordBack(big) => {
                for _ in 0..count {
                    p = prev_word(t, p, big);
                }
                Kind::Exclusive
            }
            Motion::LineStart => {
                p = line_start(t, cur);
                Kind::Exclusive
            }
            Motion::FirstNonBlank => {
                p = first_non_blank(t, cur);
                Kind::Exclusive
            }
            Motion::LineEnd => {
                let mut l = cur;
                for _ in 1..count {
                    let e = line_end(t, l);
                    if e < t.len() {
                        l = e + 1;
                    }
                }
                p = last_char(t, l);
                self.want_col = Some(usize::MAX);
                Kind::Inclusive
            }
            Motion::Line(n) => {
                p = first_non_blank(t, line_offset(t, n));
                Kind::Linewise
            }
            Motion::LastLine(n) => {
                let line = n.unwrap_or(line_count(t)).saturating_sub(1);
                p = first_non_blank(t, line_offset(t, line));
                Kind::Linewise
            }
            Motion::Find(kind, c) => {
                self.last_find = Some((kind, c));
                p = self.find(t, cur, kind, c, count)?;
                if matches!(kind, 'f' | 't') {
                    Kind::Inclusive
                } else {
                    Kind::Exclusive
                }
            }
            Motion::RepeatFind(reverse) => {
                let (kind, c) = self.last_find?;
                let kind = if reverse {
                    match kind {
                        'f' => 'F',
                        'F' => 'f',
                        't' => 'T',
                        _ => 't',
                    }
                } else {
                    kind
                };
                // `;` after `t` must not stay stuck before the same character.
                let from = match kind {
                    't' => next_char(t, cur),
                    'T' => prev_char(t, cur),
                    _ => cur,
                };
                p = self.find(t, from, kind, c, count)?;
                if matches!(kind, 'f' | 't') {
                    Kind::Inclusive
                } else {
                    Kind::Exclusive
                }
            }
            Motion::Bracket => {
                p = matching_bracket(t, cur)?;
                Kind::Inclusive
            }
            Motion::Paragraph(forward) => {
                for _ in 0..count {
                    p = paragraph(t, p, forward);
                }
                Kind::Exclusive
            }
        };
        Some((p, kind))
    }

    fn find(&self, t: &str, from: usize, kind: char, c: char, count: usize) -> Option<usize> {
        let mut p = from;
        let forward = matches!(kind, 'f' | 't');
        for _ in 0..count {
            p = find_in_line(t, p, c, forward, false)?;
        }
        Some(match kind {
            't' => prev_char(t, p),
            'T' => next_char(t, p),
            _ => p,
        })
    }
}

/// The lines from the one of `a` to the one of `b`, newline included.
fn lines(t: &str, a: usize, b: usize) -> Range<usize> {
    let end = line_end(t, b);
    line_start(t, a)..(end + 1).min(t.len())
}

fn remove(t: &str, r: &Range<usize>) -> String {
    format!("{}{}", &t[..r.start], &t[r.end..])
}

/// The text object `obj` (`w`, `(`, `e`, `$`…) at `i`.
fn object(t: &str, i: usize, obj: char, around: bool) -> Option<Range<usize>> {
    match obj {
        'w' => word_object(t, i, false, around),
        'W' => word_object(t, i, true, around),
        '(' | ')' | 'b' => bracket_object(t, i, '(', ')', around),
        '{' | '}' | 'B' => bracket_object(t, i, '{', '}', around),
        '[' | ']' => bracket_object(t, i, '[', ']', around),
        '<' | '>' => bracket_object(t, i, '<', '>', around),
        '"' | '\'' | '`' => quote_object(t, i, obj, around),
        'e' => environment_object(t, i, around),
        '$' => math_object(t, i, around),
        _ => None,
    }
}

/// The next (or previous) match of a search: literal, ignoring case unless
/// it has a capital, `\<word\>` for whole words.
fn find(t: &str, query: &str, from: usize, backward: bool) -> Option<usize> {
    let (needle, whole) = match query
        .strip_prefix("\\<")
        .and_then(|q| q.strip_suffix("\\>"))
    {
        Some(w) => (w, true),
        None => (query, false),
    };
    if needle.is_empty() {
        return None;
    }
    let fold = !needle.chars().any(char::is_uppercase);
    let hay: String;
    let pat: String;
    let (h, n) = if fold {
        hay = t.to_lowercase();
        pat = needle.to_lowercase();
        // Lower-casing may change lengths: search the original then.
        if hay.len() != t.len() {
            (t, needle)
        } else {
            (hay.as_str(), pat.as_str())
        }
    } else {
        (t, needle)
    };
    let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
    let ok = |p: usize| {
        !whole || (!word(t[..p].chars().next_back()) && !word(t[p + n.len()..].chars().next()))
    };
    let matches: Vec<usize> = h
        .match_indices(n)
        .map(|(p, _)| p)
        .filter(|&p| ok(p))
        .collect();
    if backward {
        matches
            .iter()
            .rev()
            .find(|&&p| p < from)
            .or(matches.last())
            .copied()
    } else {
        matches
            .iter()
            .find(|&&p| p > from)
            .or(matches.first())
            .copied()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Motion {
    Left,
    Right,
    Up,
    Down,
    Word(bool),
    WordEnd(bool),
    WordBack(bool),
    LineStart,
    FirstNonBlank,
    LineEnd,
    /// `gg` (0-based line).
    Line(usize),
    /// `G`, to a 1-based line or the last.
    LastLine(Option<usize>),
    Find(char, char),
    RepeatFind(bool),
    Bracket,
    Paragraph(bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cmd {
    Move(Motion, usize),
    OpMotion(char, Motion, usize),
    OpLines(char, usize),
    OpObject(char, bool, char),
    VisualOp(char),
    VisualObject(bool, char),
    Replace(char, usize),
    Simple(char, usize),
}

enum Parse {
    Incomplete,
    Invalid,
    Done(Cmd),
}

enum MotionParse {
    Incomplete,
    Invalid,
    Done(Motion),
}

fn take_count(keys: &[Key], i: &mut usize) -> Option<usize> {
    let mut n: Option<usize> = None;
    while let Some(Key::Char(c)) = keys.get(*i) {
        let Some(d) = c.to_digit(10) else { break };
        // A leading 0 is the motion.
        if d == 0 && n.is_none() {
            break;
        }
        n = Some(
            n.unwrap_or(0)
                .saturating_mul(10)
                .saturating_add(d as usize)
                .min(100_000),
        );
        *i += 1;
    }
    n
}

fn parse_motion(keys: &[Key], count: Option<usize>) -> MotionParse {
    let Some(&k) = keys.first() else {
        return MotionParse::Incomplete;
    };
    let m = match k {
        Key::Char('h') | Key::Left | Key::Backspace => Motion::Left,
        Key::Char('l') | Key::Right | Key::Char(' ') => Motion::Right,
        Key::Char('k') | Key::Up => Motion::Up,
        Key::Char('j') | Key::Down | Key::Enter => Motion::Down,
        Key::Char('w') => Motion::Word(false),
        Key::Char('W') => Motion::Word(true),
        Key::Char('e') => Motion::WordEnd(false),
        Key::Char('E') => Motion::WordEnd(true),
        Key::Char('b') => Motion::WordBack(false),
        Key::Char('B') => Motion::WordBack(true),
        Key::Char('0') => Motion::LineStart,
        Key::Char('^') => Motion::FirstNonBlank,
        Key::Char('$') => Motion::LineEnd,
        Key::Char('G') => Motion::LastLine(count),
        Key::Char('%') => Motion::Bracket,
        Key::Char('}') => Motion::Paragraph(true),
        Key::Char('{') => Motion::Paragraph(false),
        Key::Char(';') => Motion::RepeatFind(false),
        Key::Char(',') => Motion::RepeatFind(true),
        Key::Char('g') => {
            return match keys.get(1) {
                None => MotionParse::Incomplete,
                Some(Key::Char('g')) => {
                    MotionParse::Done(Motion::Line(count.unwrap_or(1).saturating_sub(1)))
                }
                _ => MotionParse::Invalid,
            };
        }
        Key::Char(f @ ('f' | 'F' | 't' | 'T')) => {
            return match keys.get(1) {
                None => MotionParse::Incomplete,
                Some(Key::Char(c)) => MotionParse::Done(Motion::Find(f, *c)),
                _ => MotionParse::Invalid,
            };
        }
        _ => return MotionParse::Invalid,
    };
    MotionParse::Done(m)
}

#[cfg(test)]
mod tests;
