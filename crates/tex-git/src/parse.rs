//! Parsers for git's machine-readable output.

/// How a file differs on one side (index or working tree) of `git status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Unmodified,
    Modified,
    Added,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
    Untracked,
}

impl Change {
    fn from_code(c: u8) -> Change {
        match c {
            b'M' => Change::Modified,
            b'A' => Change::Added,
            b'D' => Change::Deleted,
            b'R' => Change::Renamed,
            b'C' => Change::Copied,
            b'T' => Change::TypeChanged,
            _ => Change::Unmodified,
        }
    }

    /// The one-letter code shown next to a file.
    pub fn letter(self) -> &'static str {
        match self {
            Change::Unmodified => "",
            Change::Modified => "M",
            Change::Added => "A",
            Change::Deleted => "D",
            Change::Renamed => "R",
            Change::Copied => "C",
            Change::TypeChanged => "T",
            Change::Untracked => "U",
        }
    }
}

/// A file listed by `git status`. Paths are relative to the repository root,
/// with `/` separators.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    /// The source of a rename or copy.
    pub orig_path: Option<String>,
    pub index: Change,
    pub worktree: Change,
    /// Unmerged: both sides changed it during a merge, rebase or cherry-pick.
    pub conflicted: bool,
}

impl Entry {
    /// Has changes in the index (part of the next commit).
    pub fn is_staged(&self) -> bool {
        !self.conflicted && self.index != Change::Unmodified
    }

    /// Has changes in the working tree not in the index (including untracked).
    pub fn is_unstaged(&self) -> bool {
        !self.conflicted && self.worktree != Change::Unmodified
    }

    pub fn is_untracked(&self) -> bool {
        self.worktree == Change::Untracked
    }
}

/// A merge-like operation stopped half-way (usually on conflicts).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Operation {
    #[default]
    None,
    Merge,
    Rebase,
    CherryPick,
    Revert,
}

impl Operation {
    pub fn label(self) -> &'static str {
        match self {
            Operation::None => "",
            Operation::Merge => "Merge",
            Operation::Rebase => "Rebase",
            Operation::CherryPick => "Cherry-pick",
            Operation::Revert => "Revert",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Status {
    /// The current branch; `None` when HEAD is detached.
    pub branch: Option<String>,
    /// The commit HEAD points at; `None` before the first commit.
    pub oid: Option<String>,
    pub upstream: Option<String>,
    /// Commits not on the upstream yet.
    pub ahead: u32,
    /// Commits on the upstream not merged yet.
    pub behind: u32,
    pub entries: Vec<Entry>,
    pub operation: Operation,
}

impl Status {
    pub fn conflicts(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|e| e.conflicted)
    }

    pub fn staged(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|e| e.is_staged())
    }

    pub fn unstaged(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|e| e.is_unstaged())
    }

    pub fn is_clean(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Parse `git status --porcelain=v2 --branch -z`.
pub(crate) fn status(out: &[u8]) -> Status {
    let mut status = Status::default();
    let mut records = out
        .split(|&b| b == 0)
        .map(|r| String::from_utf8_lossy(r).into_owned());
    while let Some(rec) = records.next() {
        if let Some(header) = rec.strip_prefix("# ") {
            let (key, value) = header.split_once(' ').unwrap_or((header, ""));
            match key {
                "branch.oid" if value != "(initial)" => status.oid = Some(value.to_string()),
                "branch.head" if value != "(detached)" => status.branch = Some(value.to_string()),
                "branch.upstream" => status.upstream = Some(value.to_string()),
                "branch.ab" => {
                    for part in value.split(' ') {
                        if let Some(n) = part.strip_prefix('+') {
                            status.ahead = n.parse().unwrap_or(0);
                        } else if let Some(n) = part.strip_prefix('-') {
                            status.behind = n.parse().unwrap_or(0);
                        }
                    }
                }
                _ => {}
            }
            continue;
        }
        let kind = rec.as_bytes().first().copied();
        let entry = match kind {
            // 1 XY sub mH mI mW hH hI path
            Some(b'1') => rec.splitn(9, ' ').nth(8).map(|path| {
                let xy = rec.as_bytes();
                Entry {
                    path: path.to_string(),
                    orig_path: None,
                    index: Change::from_code(xy[2]),
                    worktree: Change::from_code(xy[3]),
                    conflicted: false,
                }
            }),
            // 2 XY sub mH mI mW hH hI Xscore path NUL origPath
            Some(b'2') => rec.splitn(10, ' ').nth(9).map(|path| {
                let xy = rec.as_bytes();
                Entry {
                    path: path.to_string(),
                    orig_path: records.next(),
                    index: Change::from_code(xy[2]),
                    worktree: Change::from_code(xy[3]),
                    conflicted: false,
                }
            }),
            // u XY sub m1 m2 m3 mW h1 h2 h3 path
            Some(b'u') => rec.splitn(11, ' ').nth(10).map(|path| Entry {
                path: path.to_string(),
                orig_path: None,
                index: Change::Modified,
                worktree: Change::Modified,
                conflicted: true,
            }),
            Some(b'?') => rec.strip_prefix("? ").map(|path| Entry {
                path: path.to_string(),
                orig_path: None,
                index: Change::Unmodified,
                worktree: Change::Untracked,
                conflicted: false,
            }),
            _ => None,
        };
        status.entries.extend(entry);
    }
    status
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    pub sha: String,
    pub short: String,
    pub author: String,
    pub email: String,
    /// Author time, seconds since the Unix epoch.
    pub time: i64,
    pub parents: Vec<String>,
    pub subject: String,
}

/// The `--format` matching [`log`]: fields separated by US, commits by NUL
/// (`-z`), the free-form subject last.
pub(crate) const LOG_FORMAT: &str = "--format=%H%x1f%h%x1f%an%x1f%ae%x1f%at%x1f%P%x1f%s";

pub(crate) fn log(out: &[u8]) -> Vec<Commit> {
    String::from_utf8_lossy(out)
        .split('\0')
        .filter_map(|rec| {
            let rec = rec.trim_start_matches('\n');
            let mut f = rec.splitn(7, '\x1f');
            Some(Commit {
                sha: f.next().filter(|s| !s.is_empty())?.to_string(),
                short: f.next()?.to_string(),
                author: f.next()?.to_string(),
                email: f.next()?.to_string(),
                time: f.next()?.parse().unwrap_or(0),
                parents: f.next()?.split_whitespace().map(str::to_string).collect(),
                subject: f.next()?.trim_end().to_string(),
            })
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branch {
    /// `main`, or `origin/main` for a remote-tracking branch.
    pub name: String,
    pub remote: bool,
    pub current: bool,
    pub upstream: Option<String>,
}

impl Branch {
    /// The local name a remote-tracking branch is checked out as.
    pub fn local_name(&self) -> &str {
        if self.remote {
            self.name.split_once('/').map_or(&self.name, |(_, b)| b)
        } else {
            &self.name
        }
    }
}

pub(crate) const BRANCH_FORMAT: &str = "--format=%(HEAD)%00%(refname)%00%(upstream:short)";

pub(crate) fn branches(out: &[u8]) -> Vec<Branch> {
    String::from_utf8_lossy(out)
        .lines()
        .filter_map(|line| {
            let mut f = line.split('\0');
            let head = f.next()?;
            let refname = f.next()?;
            let upstream = f.next().filter(|u| !u.is_empty()).map(str::to_string);
            let (name, remote) = match refname.strip_prefix("refs/heads/") {
                Some(n) => (n, false),
                None => {
                    let n = refname.strip_prefix("refs/remotes/")?;
                    // `origin/HEAD` is an alias, not a branch.
                    if n.ends_with("/HEAD") || !n.contains('/') {
                        return None;
                    }
                    (n, true)
                }
            };
            Some(Branch {
                name: name.to_string(),
                remote,
                current: head == "*",
                upstream,
            })
        })
        .collect()
}

/// The first useful line of an error message, without git's `fatal:` prefix.
pub(crate) fn summary(text: &str) -> Option<String> {
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with("hint:"))?;
    let line = ["fatal: ", "error: ", "remote: "]
        .iter()
        .find_map(|p| line.strip_prefix(p))
        .unwrap_or(line);
    let mut s = line.to_string();
    if let Some(first) = s.get_mut(..1) {
        first.make_ascii_uppercase();
    }
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_status_v2() {
        let out = b"# branch.oid 1234abcd\0# branch.head main\0# branch.upstream origin/main\0\
# branch.ab +2 -1\0\
1 .M N... 100644 100644 100644 aaaa bbbb chapters/intro.tex\0\
1 A. N... 000000 100644 100644 0000 cccc new file.tex\0\
2 R. N... 100644 100644 100644 dddd eeee R100 b.tex\0a.tex\0\
u UU N... 100644 100644 100644 100644 f1 f2 f3 main.tex\0\
? notes.txt\0";
        let s = status(out);
        assert_eq!(s.branch.as_deref(), Some("main"));
        assert_eq!(s.oid.as_deref(), Some("1234abcd"));
        assert_eq!(s.upstream.as_deref(), Some("origin/main"));
        assert_eq!((s.ahead, s.behind), (2, 1));
        assert_eq!(s.entries.len(), 5);

        let e = &s.entries[0];
        assert_eq!(e.path, "chapters/intro.tex");
        assert_eq!(
            (e.index, e.worktree),
            (Change::Unmodified, Change::Modified)
        );
        assert!(e.is_unstaged() && !e.is_staged());

        assert_eq!(s.entries[1].path, "new file.tex");
        assert!(s.entries[1].is_staged());

        let r = &s.entries[2];
        assert_eq!(r.path, "b.tex");
        assert_eq!(r.orig_path.as_deref(), Some("a.tex"));
        assert_eq!(r.index, Change::Renamed);

        let u = &s.entries[3];
        assert!(u.conflicted && !u.is_staged() && !u.is_unstaged());
        assert_eq!(s.conflicts().count(), 1);

        assert!(s.entries[4].is_untracked());
        assert_eq!(s.staged().count(), 2);
        assert_eq!(s.unstaged().count(), 2);
    }

    #[test]
    fn parses_initial_and_detached() {
        let s = status(b"# branch.oid (initial)\0# branch.head main\0");
        assert_eq!(s.oid, None);
        assert_eq!(s.branch.as_deref(), Some("main"));
        assert!(s.is_clean());

        let s = status(b"# branch.oid abc\0# branch.head (detached)\0");
        assert_eq!(s.branch, None);
        assert_eq!(s.upstream, None);
    }

    #[test]
    fn parses_log() {
        let out = "aaaa\x1faa\x1fAda\x1fada@x\x1f1700000000\x1fbbbb\x1fFix: a \x1f in the subject\0\
\ncccc\x1fcc\x1fBob\x1fbob@x\x1f1600000000\x1f\x1fInitial\n\0";
        let commits = log(out.as_bytes());
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].sha, "aaaa");
        assert_eq!(commits[0].parents, vec!["bbbb"]);
        assert_eq!(commits[0].subject, "Fix: a \x1f in the subject");
        assert_eq!(commits[0].time, 1_700_000_000);
        assert!(commits[1].parents.is_empty());
        assert_eq!(commits[1].subject, "Initial");
    }

    #[test]
    fn parses_branches() {
        let out = "*\0refs/heads/main\0origin/main\n \0refs/heads/draft\0\n \0refs/remotes/origin/HEAD\0\n \0refs/remotes/origin/main\0\n \0refs/remotes/origin/feature/x\0\n";
        let b = branches(out.as_bytes());
        assert_eq!(b.len(), 4);
        assert!(b[0].current && !b[0].remote);
        assert_eq!(b[0].upstream.as_deref(), Some("origin/main"));
        assert_eq!(b[1].name, "draft");
        assert_eq!(b[1].upstream, None);
        assert!(b[2].remote);
        assert_eq!(b[3].name, "origin/feature/x");
        assert_eq!(b[3].local_name(), "feature/x");
    }

    #[test]
    fn letters_and_labels() {
        let letters: String = [
            Change::Unmodified,
            Change::Modified,
            Change::Added,
            Change::Deleted,
            Change::Renamed,
            Change::Copied,
            Change::TypeChanged,
            Change::Untracked,
        ]
        .map(Change::letter)
        .concat();
        assert_eq!(letters, "MADRCTU");
        assert_eq!(Change::from_code(b'?'), Change::Unmodified);
        let labels = [
            Operation::None,
            Operation::Merge,
            Operation::Rebase,
            Operation::CherryPick,
            Operation::Revert,
        ]
        .map(Operation::label);
        assert_eq!(labels, ["", "Merge", "Rebase", "Cherry-pick", "Revert"]);
        assert_eq!(Operation::default(), Operation::None);
    }

    #[test]
    fn summarizes_errors() {
        assert_eq!(
            summary("hint: foo\nfatal: not a git repository\n").as_deref(),
            Some("Not a git repository")
        );
        assert_eq!(summary("\n\n"), None);
    }

    mod props {
        use proptest::prelude::*;

        use super::*;

        fn change(c: u8) -> Change {
            Change::from_code(c)
        }

        /// A path as git prints it with `-z`: anything but NUL.
        fn path() -> impl Strategy<Value = String> {
            "[^\0]{1,24}"
        }

        #[derive(Debug, Clone)]
        enum Rec {
            Ordinary(u8, u8, String),
            Renamed(u8, String, String),
            Unmerged(String),
            Untracked(String),
        }

        fn rec() -> impl Strategy<Value = Rec> {
            let code = prop::sample::select(b".MADRCT".to_vec());
            prop_oneof![
                (code.clone(), code.clone(), path()).prop_map(|(x, y, p)| Rec::Ordinary(x, y, p)),
                (prop::sample::select(b"RC".to_vec()), path(), path())
                    .prop_map(|(x, p, o)| Rec::Renamed(x, p, o)),
                path().prop_map(Rec::Unmerged),
                path().prop_map(Rec::Untracked),
            ]
        }

        proptest! {
            #[test]
            fn status_never_panics(out in prop::collection::vec(any::<u8>(), 0..512)) {
                let _ = status(&out);
            }

            #[test]
            fn status_of_records_never_panics(
                recs in prop::collection::vec("[12u?#] [ -~]{0,40}", 0..8)
            ) {
                let _ = status(recs.join("\0").as_bytes());
            }

            #[test]
            fn log_never_panics(out in prop::collection::vec(any::<u8>(), 0..512)) {
                let _ = log(&out);
            }

            #[test]
            fn branches_never_panic(out in prop::collection::vec(any::<u8>(), 0..512)) {
                let _ = branches(&out);
            }

            #[test]
            fn summary_never_panics(text in "\\PC*") {
                if let Some(s) = summary(&text) {
                    prop_assert!(!s.is_empty());
                    prop_assert!(!s.starts_with("fatal: ") && !s.starts_with("hint:"));
                }
            }

            #[test]
            fn status_round_trip(
                head in prop::option::of("[a-z][a-z/_-]{0,12}"),
                oid in prop::option::of("[0-9a-f]{40}"),
                upstream in prop::option::of("[a-z]{1,8}/[a-z]{1,8}"),
                ab in (0u32..100_000, 0u32..100_000),
                recs in prop::collection::vec(rec(), 0..8),
            ) {
                let mut out: Vec<String> = vec![
                    format!("# branch.oid {}", oid.as_deref().unwrap_or("(initial)")),
                    format!("# branch.head {}", head.as_deref().unwrap_or("(detached)")),
                ];
                if let Some(u) = &upstream {
                    out.push(format!("# branch.upstream {u}"));
                    out.push(format!("# branch.ab +{} -{}", ab.0, ab.1));
                }
                let mut want = Vec::new();
                for r in &recs {
                    let (line, entry) = match r {
                        Rec::Ordinary(x, y, p) => (
                            format!(
                                "1 {}{} N... 100644 100644 100644 aaaa bbbb {p}",
                                *x as char, *y as char
                            ),
                            Entry {
                                path: p.clone(),
                                orig_path: None,
                                index: change(*x),
                                worktree: change(*y),
                                conflicted: false,
                            },
                        ),
                        Rec::Renamed(x, p, o) => (
                            format!(
                                "2 {}. N... 100644 100644 100644 aaaa bbbb {}100 {p}\0{o}",
                                *x as char, *x as char
                            ),
                            Entry {
                                path: p.clone(),
                                orig_path: Some(o.clone()),
                                index: change(*x),
                                worktree: Change::Unmodified,
                                conflicted: false,
                            },
                        ),
                        Rec::Unmerged(p) => (
                            format!("u UU N... 100644 100644 100644 100644 a b c {p}"),
                            Entry {
                                path: p.clone(),
                                orig_path: None,
                                index: Change::Modified,
                                worktree: Change::Modified,
                                conflicted: true,
                            },
                        ),
                        Rec::Untracked(p) => (
                            format!("? {p}"),
                            Entry {
                                path: p.clone(),
                                orig_path: None,
                                index: Change::Unmodified,
                                worktree: Change::Untracked,
                                conflicted: false,
                            },
                        ),
                    };
                    out.push(line);
                    want.push(entry);
                }
                let mut bytes = out.join("\0").into_bytes();
                bytes.push(0);
                let s = status(&bytes);
                prop_assert_eq!(&s.branch, &head);
                prop_assert_eq!(&s.oid, &oid);
                prop_assert_eq!((s.ahead, s.behind), if upstream.is_some() { ab } else { (0, 0) });
                prop_assert_eq!(&s.upstream, &upstream);
                prop_assert_eq!(s.is_clean(), want.is_empty());
                prop_assert_eq!(s.conflicts().count(), want.iter().filter(|e| e.conflicted).count());
                prop_assert_eq!(s.entries, want);
                prop_assert_eq!(s.operation, Operation::None);
            }

            #[test]
            fn log_round_trip(
                commits in prop::collection::vec(
                    (
                        "[0-9a-f]{40}",
                        "[^\0\x1f]{0,12}",
                        "[^\0\x1f]{0,12}",
                        any::<i64>(),
                        prop::collection::vec("[0-9a-f]{40}", 0..3),
                        "([^\0]{0,20}[^\0\\s])?",
                    ),
                    0..6,
                )
            ) {
                let out: String = commits
                    .iter()
                    .map(|(sha, author, email, time, parents, subject)| {
                        format!(
                            "\n{sha}\x1f{}\x1f{author}\x1f{email}\x1f{time}\x1f{}\x1f{subject}\0",
                            &sha[..7],
                            parents.join(" ")
                        )
                    })
                    .collect();
                let got = log(out.as_bytes());
                prop_assert_eq!(got.len(), commits.len());
                for (c, (sha, author, email, time, parents, subject)) in got.iter().zip(&commits) {
                    prop_assert_eq!(&c.sha, sha);
                    prop_assert_eq!(&c.short, &sha[..7]);
                    prop_assert_eq!(&c.author, author);
                    prop_assert_eq!(&c.email, email);
                    prop_assert_eq!(c.time, *time);
                    prop_assert_eq!(&c.parents, parents);
                    prop_assert_eq!(&c.subject, subject);
                }
            }

            #[test]
            fn branches_round_trip(
                refs in prop::collection::vec(
                    (any::<bool>(), any::<bool>(), "[a-z][a-z0-9/_-]{0,12}", prop::option::of("[a-z]{1,6}/[a-z]{1,6}")),
                    0..8,
                )
            ) {
                let out: String = refs
                    .iter()
                    .map(|(current, remote, name, upstream)| {
                        let refname = if *remote {
                            format!("refs/remotes/origin/{name}")
                        } else {
                            format!("refs/heads/{name}")
                        };
                        let head = if *current { "*" } else { " " };
                        format!("{head}\0{refname}\0{}\n", upstream.as_deref().unwrap_or(""))
                    })
                    .collect();
                let want: Vec<Branch> = refs
                    .iter()
                    .filter(|(_, remote, name, _)| !(*remote && (name == "HEAD" || name.ends_with("/HEAD"))))
                    .map(|(current, remote, name, upstream)| Branch {
                        name: if *remote { format!("origin/{name}") } else { name.clone() },
                        remote: *remote,
                        current: *current,
                        upstream: upstream.clone(),
                    })
                    .collect();
                let got = branches(out.as_bytes());
                for b in &got {
                    if b.remote {
                        prop_assert_eq!(format!("origin/{}", b.local_name()), b.name.clone());
                    } else {
                        prop_assert_eq!(b.local_name(), b.name.as_str());
                    }
                }
                prop_assert_eq!(got, want);
            }
        }
    }
}
