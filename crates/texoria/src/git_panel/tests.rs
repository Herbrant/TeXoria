use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui_kit::base::actions::Confirm;
use gpui_kit::{TestAppContext, WindowOptions};
use tex_git::{Change, Entry, Status};

use super::*;
use crate::testing::{self, GitSandbox};
// `super::*` brings gpui's `test` attribute, which would shadow the standard one, and
// gpui-kit's `gpui` alias, which the `#[gpui::test]` macro's paths must not find twice.
use core::prelude::v1::test;
use gpui_kit::gpui;

fn entry(path: &str, index: Change, worktree: Change) -> Entry {
    Entry {
        path: path.into(),
        orig_path: None,
        index,
        worktree,
        conflicted: false,
    }
}

fn conflicted(path: &str) -> Entry {
    Entry {
        conflicted: true,
        ..entry(path, Change::Modified, Change::Modified)
    }
}

#[test]
fn sync_review_lists_new_and_deleted_files() {
    use Change::*;
    let mut renamed = entry("new.tex", Renamed, Unmodified);
    renamed.orig_path = Some("old.tex".into());
    let status = Status {
        entries: vec![
            entry("main.tex", Unmodified, Modified),
            entry("fig.png", Unmodified, Untracked),
            entry("staged.tex", Added, Unmodified),
            entry("gone.tex", Unmodified, Deleted),
            entry("removed.tex", Deleted, Unmodified),
            renamed,
        ],
        ..Default::default()
    };
    let review = SyncReview::of(&status);
    assert_eq!(review.added, ["fig.png", "new.tex", "staged.tex"]);
    assert_eq!(review.deleted, ["gone.tex", "old.tex", "removed.tex"]);

    let edits_only = Status {
        entries: vec![entry("main.tex", Modified, Modified)],
        ..Default::default()
    };
    assert!(SyncReview::of(&edits_only).is_empty());

    // A file that appeared after the review was not agreed to.
    assert!(SyncReview::of(&edits_only).within(&review));
    assert!(review.within(&review));
    assert!(!review.within(&SyncReview::default()));
}

#[test]
fn sync_summaries() {
    let review = |added: &[&str], deleted: &[&str]| SyncReview {
        added: added.iter().map(|s| s.to_string()).collect(),
        deleted: deleted.iter().map(|s| s.to_string()).collect(),
    };
    let tail = " on Overleaf, for every collaborator of the project.";
    assert_eq!(
        sync_summary(&review(&["a"], &[])),
        format!("1 new file will be added{tail}")
    );
    assert_eq!(
        sync_summary(&review(&[], &["a", "b"])),
        format!("2 files will be deleted{tail}")
    );
    assert_eq!(
        sync_summary(&review(&["a", "b"], &["c"])),
        format!("2 new files will be added and 1 file deleted{tail}")
    );
}

#[test]
fn picks_the_tracked_remote() {
    let remotes = |r: &[&str]| r.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let tracking = |u: &str| Status {
        upstream: Some(u.into()),
        ..Default::default()
    };
    let rs = remotes(&["origin", "overleaf", "over"]);
    assert_eq!(
        main_remote(&tracking("overleaf/main"), &rs),
        Some("overleaf")
    );
    assert_eq!(main_remote(&Status::default(), &rs), Some("origin"));
    let rs = remotes(&["overleaf"]);
    assert_eq!(main_remote(&Status::default(), &rs), Some("overleaf"));
    let rs = remotes(&["a", "b"]);
    assert_eq!(main_remote(&Status::default(), &rs), None);
    // `orig` is a prefix of the upstream's remote, not its remote.
    let rs = remotes(&["orig", "b"]);
    assert_eq!(main_remote(&tracking("origin/main"), &rs), None);
}

#[test]
fn dates() {
    assert_eq!(civil_from_days(0), (1970, 1, 1));
    assert_eq!(civil_from_days(19_723), (2024, 1, 1));
    assert_eq!(civil_from_days(19_782), (2024, 2, 29));
    assert_eq!(civil_from_days(-1), (1969, 12, 31));
}

#[test]
fn relative_times() {
    let now = 19_753 * 86_400 + 4_000;
    let ago = |secs: i64| relative_time_at(now, now - secs);
    assert_eq!(ago(0), "just now");
    assert_eq!(ago(59), "just now");
    // A commit dated in the future (a clock off).
    assert_eq!(ago(-3_600), "just now");
    assert_eq!(ago(60), "1 min ago");
    assert_eq!(ago(3_599), "59 min ago");
    assert_eq!(ago(3_600), "1 h ago");
    assert_eq!(ago(86_399), "23 h ago");
    assert_eq!(ago(86_400), "1 day ago");
    assert_eq!(ago(2 * 86_400), "2 days ago");
    assert_eq!(ago(604_799), "6 days ago");
    // A week or more: the commit's date.
    assert_eq!(ago(30 * 86_400), "2024-01-01");
    assert_eq!(relative_time(0), "1970-01-01");
}

#[test]
fn names_and_plurals() {
    assert_eq!(file_name("chapters/intro.tex"), "intro.tex");
    assert_eq!(file_name("main.tex"), "main.tex");
    assert_eq!(plural(0), "s");
    assert_eq!(plural(1), "");
    assert_eq!(plural(2), "s");
}

#[test]
fn operation_banners() {
    assert_eq!(
        operation_text(Operation::Merge, 1),
        "Merge in progress: 1 conflict left."
    );
    assert_eq!(
        operation_text(Operation::Rebase, 3),
        "Rebase in progress: 3 conflicts left."
    );
    assert_eq!(
        operation_text(Operation::Rebase, 0),
        "Rebase in progress: conflicts resolved."
    );
    assert_eq!(
        operation_text(Operation::CherryPick, 0),
        "Cherry-pick in progress: commit to finish."
    );
}

#[test]
fn what_each_operation_does() {
    let ops = [
        (GitOp::Stage(vec![]), "Stage", false, false),
        (GitOp::Unstage(vec![]), "Unstage", false, false),
        (
            GitOp::Discard {
                tracked: vec![],
                untracked: vec![],
            },
            "Discard",
            false,
            true,
        ),
        (
            GitOp::Commit {
                message: "m".into(),
                amend: false,
                all: false,
            },
            "Commit",
            false,
            false,
        ),
        (GitOp::Fetch, "Fetch", true, false),
        (GitOp::Pull, "Pull", true, true),
        (GitOp::Push, "Push", true, false),
        (
            GitOp::Sync {
                message: None,
                review: SyncReview::default(),
            },
            "Sync",
            true,
            true,
        ),
        (GitOp::Abort(Operation::Merge), "Abort", false, true),
        (GitOp::ContinueRebase, "Continue rebase", false, true),
        (
            GitOp::Switch(Branch {
                name: "main".into(),
                remote: false,
                current: false,
                upstream: None,
            }),
            "Switch branch",
            false,
            true,
        ),
        (
            GitOp::CreateBranch("b".into()),
            "Create branch",
            false,
            false,
        ),
        (
            GitOp::AddRemote {
                name: "origin".into(),
                url: "u".into(),
            },
            "Add remote",
            false,
            false,
        ),
        (
            GitOp::SetIdentity {
                name: "n".into(),
                email: "e".into(),
                global: false,
            },
            "Save identity",
            false,
            false,
        ),
        (GitOp::Init, "Initialize repository", false, false),
    ];
    for (op, label, network, worktree) in ops {
        assert_eq!(op.label(), label);
        assert_eq!(op.is_network(), network, "{label}");
        assert_eq!(op.touches_worktree(), worktree, "{label}");
    }
}

#[test]
fn finds_enclosing_repo() {
    let dir = tempfile::tempdir().unwrap();
    let sub = dir.path().join("paper/figures");
    std::fs::create_dir_all(&sub).unwrap();
    assert_eq!(repo_top(&sub), None);
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    assert_eq!(repo_top(&sub).as_deref(), Some(dir.path()));
    std::fs::write(
        dir.path().join("paper/.git"),
        "gitdir: ../.git/modules/paper\n",
    )
    .unwrap();
    assert_eq!(repo_top(&sub), Some(dir.path().join("paper")));
    // Tests run in crates/texoria, two folders below this repository's .git: a relative
    // path is never resolved against the current folder.
    assert_eq!(repo_top(Path::new("../..")), None);
    assert_eq!(repo_top(Path::new("")), None);
}

// ── A panel in a window ─────────────────────────────────────────────────

/// What the panel emitted.
#[derive(Debug)]
enum Seen {
    Request(GitOp),
    OpenDiff,
    WorktreeChanged,
    StatusChanged,
    Trust,
}

type Log = Rc<RefCell<Vec<Seen>>>;

struct Panel {
    window: AnyWindowHandle,
    panel: Entity<GitPanel>,
    log: Log,
}

impl Panel {
    fn open(cx: &mut TestAppContext) -> Panel {
        testing::init(cx);
        let (window, panel) = cx.update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| GitPanel::new(window, cx))
            })
            .expect("open test window")
        });
        let log: Log = Rc::default();
        let seen = log.clone();
        cx.update(|cx| {
            cx.subscribe(&panel, move |_, ev: &GitPanelEvent, _| {
                seen.borrow_mut().push(match ev {
                    GitPanelEvent::Request(op) => Seen::Request(op.clone()),
                    GitPanelEvent::OpenDiff(_) => Seen::OpenDiff,
                    GitPanelEvent::WorktreeChanged => Seen::WorktreeChanged,
                    GitPanelEvent::StatusChanged => Seen::StatusChanged,
                    GitPanelEvent::Trust => Seen::Trust,
                })
            })
            .detach()
        });
        Panel { window, panel, log }
    }

    /// A panel on the trusted repository `dir`, driven by the sandbox's git.
    fn on(cx: &mut TestAppContext, s: &GitSandbox, dir: &Path) -> Panel {
        let p = Panel::open(cx);
        p.update(cx, |g, _, cx| {
            g.use_git(s.git.clone());
            g.set_project(Some(dir.to_path_buf()), true, cx);
        });
        cx.run_until_parked();
        p
    }

    fn update<R>(
        &self,
        cx: &mut TestAppContext,
        f: impl FnOnce(&mut GitPanel, &mut Window, &mut Context<GitPanel>) -> R,
    ) -> R {
        let panel = self.panel.clone();
        cx.update_window(self.window, |_, window, cx| {
            panel.update(cx, |g, cx| f(g, window, cx))
        })
        .unwrap()
    }

    fn read<R>(&self, cx: &mut TestAppContext, f: impl FnOnce(&GitPanel, &App) -> R) -> R {
        self.panel.read_with(cx, |g, cx| f(g, cx))
    }

    /// Run `op` as the workspace does once buffers are saved, and wait for it.
    fn execute(&self, cx: &mut TestAppContext, op: GitOp) {
        self.update(cx, |g, window, cx| g.execute(op, window, cx));
        cx.run_until_parked();
        assert_eq!(self.read(cx, |g, _| g.busy), None);
    }

    /// Lay the window out and paint it, as a frame on screen would.
    fn draw(&self, cx: &mut TestAppContext) {
        cx.update_window(self.window, |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
    }

    fn requests(&self) -> Vec<GitOp> {
        let log = self.log.borrow();
        (log.iter())
            .filter_map(|s| match s {
                Seen::Request(op) => Some(op.clone()),
                _ => None,
            })
            .collect()
    }

    fn count(&self, f: impl Fn(&Seen) -> bool) -> usize {
        self.log.borrow().iter().filter(|s| f(s)).count()
    }

    fn notifications(&self, cx: &mut TestAppContext) -> usize {
        cx.update_window(self.window, |_, window, cx| window.notifications(cx).len())
            .unwrap()
    }

    fn has_dialog(&self, cx: &mut TestAppContext) -> bool {
        cx.update_window(self.window, |_, window, cx| window.has_active_dialog(cx))
            .unwrap()
    }

    /// Type `text` in the dialog's focused field and press its OK button.
    fn fill_and_confirm(&self, cx: &mut TestAppContext, text: &str) {
        self.draw(cx);
        if !text.is_empty() {
            cx.simulate_input(self.window, text);
        }
        self.confirm(cx);
    }

    fn confirm(&self, cx: &mut TestAppContext) {
        self.draw(cx);
        cx.dispatch_action(self.window, Confirm { secondary: false });
        cx.run_until_parked();
    }
}

/// A repository in `s.dir/name` with `main.tex` committed.
fn committed(s: &GitSandbox, name: &str) -> PathBuf {
    let dir = s.dir.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    s.run(&dir, &["init", "--quiet"]);
    std::fs::write(dir.join("main.tex"), "one\ntwo\nthree\n").unwrap();
    s.run(&dir, &["add", "-A"]);
    s.run(&dir, &["commit", "--quiet", "-m", "first"]);
    dir
}

/// [`committed`], with its `main` pushed to a bare repository, its `origin`.
fn published(s: &GitSandbox, name: &str) -> (PathBuf, PathBuf) {
    let dir = committed(s, name);
    let bare = s.dir.join(format!("{name}.git"));
    s.run(
        &s.dir,
        &["init", "--bare", "--quiet", &format!("{name}.git")],
    );
    s.run(&bare, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    s.run(&dir, &["remote", "add", "origin", &bare.to_string_lossy()]);
    s.run(&dir, &["push", "--quiet", "-u", "origin", "main"]);
    (dir, bare)
}

/// Another clone of `bare` commits `text` to `file` and pushes it.
fn push_from_elsewhere(s: &GitSandbox, bare: &Path, file: &str, text: &str) {
    let other = s
        .dir
        .join(format!("other-{}", s.dir.read_dir().unwrap().count()));
    s.run(
        &s.dir,
        &[
            "clone",
            "--quiet",
            &bare.to_string_lossy(),
            &other.to_string_lossy(),
        ],
    );
    std::fs::write(other.join(file), text).unwrap();
    s.run(&other, &["add", "-A"]);
    s.run(
        &other,
        &["commit", "--quiet", "-m", &format!("edit {file}")],
    );
    s.run(&other, &["push", "--quiet"]);
}

#[gpui::test]
fn asks_through_dialogs(cx: &mut TestAppContext) {
    let Some(s) = GitSandbox::new() else { return };
    let dir = committed(&s, "repo");
    let p = Panel::on(cx, &s, &dir);
    p.update(cx, |g, window, cx| g.prompt_new_branch(window, cx));
    assert!(p.has_dialog(cx));
    p.fill_and_confirm(cx, "draft");
    assert!(!p.has_dialog(cx));
    assert!(
        matches!(&p.requests()[..], [GitOp::CreateBranch(b)] if b == "draft"),
        "{:?}",
        p.requests()
    );
}

#[gpui::test]
fn commit_button_follows_the_status(cx: &mut TestAppContext) {
    let p = Panel::open(cx);
    let set = |cx: &mut TestAppContext, entries: Vec<Entry>, amend: bool| {
        p.update(cx, |g, _, _| {
            g.status.entries = entries;
            g.amend = amend;
            (g.commit_mode(), g.commit_button())
        })
    };
    use Change::*;
    assert_eq!(
        set(cx, vec![], false),
        (None, ("Commit", "Nothing to commit"))
    );
    assert_eq!(
        set(cx, vec![], true),
        (Some(false), ("Amend", "Commit the staged changes (⌘↵)"))
    );
    let (mode, (label, _)) = set(cx, vec![entry("a.tex", Unmodified, Modified)], false);
    assert_eq!((mode, label), (Some(true), "Commit All"));
    let (mode, (label, _)) = set(cx, vec![entry("a.tex", Modified, Modified)], false);
    assert_eq!((mode, label), (Some(false), "Commit"));
    assert_eq!(
        set(
            cx,
            vec![conflicted("a.tex"), entry("b.tex", Modified, Unmodified)],
            false
        ),
        (None, ("Commit", "Resolve the conflicts first"))
    );
}

#[gpui::test]
fn pull_and_push_buttons(cx: &mut TestAppContext) {
    let p = Panel::open(cx);
    let labels = |cx: &mut TestAppContext, upstream: bool, remotes: &[&str], ahead, behind| {
        p.update(cx, |g, _, _| {
            g.status.upstream = upstream.then(|| "origin/main".to_string());
            g.status.ahead = ahead;
            g.status.behind = behind;
            g.remotes = remotes.iter().map(|r| r.to_string()).collect();
            g.pull_push_labels()
        })
    };
    let s = |a: &str, b: &str| (a.to_string(), b.to_string());
    assert_eq!(labels(cx, false, &[], 0, 0), s("", "Add Remote…"));
    assert_eq!(labels(cx, false, &["origin"], 0, 0), s("", "Publish"));
    assert_eq!(labels(cx, true, &["origin"], 0, 0), s("", ""));
    assert_eq!(labels(cx, true, &["origin"], 2, 3), s("3", "2"));
}

#[gpui::test]
fn reads_the_repository(cx: &mut TestAppContext) {
    let Some(s) = GitSandbox::new() else { return };
    let (dir, bare) = published(&s, "repo");
    push_from_elsewhere(&s, &bare, "intro.tex", "intro\n");
    std::fs::create_dir_all(dir.join("ch")).unwrap();
    std::fs::write(dir.join("ch/new.tex"), "new\n").unwrap();
    std::fs::write(dir.join("main.tex"), "one\n").unwrap();
    let p = Panel::on(cx, &s, &dir);
    assert!(p.count(|e| matches!(e, Seen::StatusChanged)) >= 1);
    p.read(cx, |g, _| {
        assert!(g.repo().is_some());
        assert_eq!(g.status().branch.as_deref(), Some("main"));
        assert_eq!(g.status().upstream.as_deref(), Some("origin/main"));
        assert_eq!(g.change_count(), 2);
        assert_eq!(g.remotes, ["origin"]);
        assert!(g.branches.iter().any(|b| b.name == "main" && b.current));
        assert!(
            g.branches
                .iter()
                .any(|b| b.name == "origin/main" && b.remote)
        );
        assert_eq!(g.history.len(), 1);
        assert_eq!(g.history[0].subject, "first");
        assert!(g.history_done);
        let marks = g.file_marks();
        assert_eq!(
            marks.get(&dir.join("ch").join("new.tex")),
            Some(&GitMark::Added)
        );
        assert_eq!(marks.get(&dir.join("main.tex")), Some(&GitMark::Modified));
    });
    p.draw(cx);

    // The other clone's commit, once fetched.
    p.execute(cx, GitOp::Fetch);
    assert_eq!(p.read(cx, |g, _| g.status().behind), 1);
    assert_eq!(p.notifications(cx), 0);
}
