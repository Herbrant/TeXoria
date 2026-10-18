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

fn read(dir: &Path, rel: &str) -> String {
    std::fs::read_to_string(dir.join(rel)).unwrap()
}

#[gpui::test]
fn stages_commits_and_discards(cx: &mut TestAppContext) {
    let Some(s) = GitSandbox::new() else { return };
    let dir = committed(&s, "repo");
    std::fs::write(dir.join("main.tex"), "changed\n").unwrap();
    std::fs::write(dir.join("a.tex"), "a\n").unwrap();
    let p = Panel::on(cx, &s, &dir);

    p.execute(cx, GitOp::Stage(vec!["a.tex".into()]));
    p.read(cx, |g, _| {
        let staged: Vec<_> = g.status().staged().map(|e| e.path.as_str()).collect();
        assert_eq!(staged, ["a.tex"]);
    });
    p.execute(cx, GitOp::Unstage(vec!["a.tex".into()]));
    assert_eq!(p.read(cx, |g, _| g.status().staged().count()), 0);

    // Nothing staged: Commit All.
    p.update(cx, |g, window, cx| {
        g.message
            .update(cx, |m, cx| m.set_value("Both", window, cx));
        g.commit(window, cx);
    });
    let Some(op @ GitOp::Commit { all: true, .. }) = p.requests().pop() else {
        panic!("{:?}", p.requests());
    };
    p.execute(cx, op);
    p.read(cx, |g, cx| {
        assert!(g.status().is_clean());
        assert_eq!(g.history[0].subject, "Both");
        assert_eq!(g.history.len(), 2);
        // The message box is emptied for the next commit.
        assert_eq!(g.message.read(cx).value().to_string(), "");
    });

    // Amending starts from the last message.
    p.update(cx, |g, _, cx| g.toggle_amend(true, cx));
    cx.run_until_parked();
    p.draw(cx);
    p.read(cx, |g, cx| {
        assert_eq!(g.message.read(cx).value().to_string(), "Both")
    });
    p.update(cx, |g, window, cx| g.commit(window, cx));
    let Some(op @ GitOp::Commit { amend: true, .. }) = p.requests().pop() else {
        panic!("{:?}", p.requests());
    };
    p.execute(cx, op);
    p.read(cx, |g, _| {
        assert_eq!(g.history.len(), 2);
        assert!(!g.amend);
    });

    // An empty message is refused.
    let before = p.requests().len();
    std::fs::write(dir.join("main.tex"), "again\n").unwrap();
    p.update(cx, |g, _, cx| g.refresh(cx));
    cx.run_until_parked();
    p.update(cx, |g, window, cx| g.commit(window, cx));
    assert_eq!(p.requests().len(), before);
    assert_eq!(p.notifications(cx), 1);

    // Discard: tracked files go back, untracked ones are deleted.
    std::fs::write(dir.join("b.tex"), "b\n").unwrap();
    p.update(cx, |g, _, cx| g.refresh(cx));
    cx.run_until_parked();
    p.draw(cx);
    let entries = p.read(cx, |g, _| {
        g.status().unstaged().cloned().collect::<Vec<_>>()
    });
    assert_eq!(entries.len(), 2);
    p.update(cx, |g, window, cx| g.confirm_discard(entries, window, cx));
    p.confirm(cx);
    let Some(op @ GitOp::Discard { .. }) = p.requests().pop() else {
        panic!("{:?}", p.requests());
    };
    let GitOp::Discard { tracked, untracked } = &op else {
        unreachable!()
    };
    assert_eq!(
        (&tracked[..], &untracked[..]),
        (&["main.tex".to_string()][..], &["b.tex".to_string()][..])
    );
    let changed = p.count(|e| matches!(e, Seen::WorktreeChanged));
    p.execute(cx, op);
    assert_eq!(read(&dir, "main.tex"), "changed\n");
    assert!(!dir.join("b.tex").exists());
    assert!(p.read(cx, |g, _| g.status().is_clean()));
    assert_eq!(p.count(|e| matches!(e, Seen::WorktreeChanged)), changed + 1);
}

#[gpui::test]
fn branches_remotes_and_identity(cx: &mut TestAppContext) {
    let Some(s) = GitSandbox::new() else { return };
    let (dir, bare) = published(&s, "repo");
    let p = Panel::on(cx, &s, &dir);

    p.execute(cx, GitOp::CreateBranch("draft".into()));
    assert_eq!(
        p.read(cx, |g, _| g.status().branch.clone()).as_deref(),
        Some("draft")
    );
    let main = p.read(cx, |g, _| {
        g.branches.iter().find(|b| b.name == "main").cloned()
    });
    p.execute(cx, GitOp::Switch(main.unwrap()));
    assert_eq!(
        p.read(cx, |g, _| g.status().branch.clone()).as_deref(),
        Some("main")
    );

    // A branch only on the remote is checked out with a local branch tracking it.
    s.run(&dir, &["push", "--quiet", "origin", "HEAD:remote-only"]);
    p.execute(cx, GitOp::Fetch);
    let remote = p.read(cx, |g, _| {
        g.branches
            .iter()
            .find(|b| b.name == "origin/remote-only")
            .cloned()
    });
    p.execute(cx, GitOp::Switch(remote.unwrap()));
    p.read(cx, |g, _| {
        assert_eq!(g.status().branch.as_deref(), Some("remote-only"));
        assert_eq!(g.status().upstream.as_deref(), Some("origin/remote-only"));
    });

    p.execute(
        cx,
        GitOp::AddRemote {
            name: "mirror".into(),
            url: bare.to_string_lossy().into_owned(),
        },
    );
    assert_eq!(p.read(cx, |g, _| g.remotes.clone()), ["mirror", "origin"]);

    p.execute(
        cx,
        GitOp::SetIdentity {
            name: "Ada".into(),
            email: "ada@example.com".into(),
            global: false,
        },
    );
    assert_eq!(s.run(&dir, &["config", "--local", "user.name"]), "Ada");

    // A failure is reported, and the panel is ready for the next one.
    p.execute(cx, GitOp::CreateBranch("-bad".into()));
    assert_eq!(p.notifications(cx), 1);
    p.draw(cx);
}

#[gpui::test]
fn pulls_and_pushes(cx: &mut TestAppContext) {
    let Some(s) = GitSandbox::new() else { return };
    let (dir, bare) = published(&s, "repo");
    let p = Panel::on(cx, &s, &dir);

    push_from_elsewhere(&s, &bare, "intro.tex", "intro\n");
    p.update(cx, |g, _, cx| g.pull(cx));
    let Some(op @ GitOp::Pull) = p.requests().pop() else {
        panic!("{:?}", p.requests());
    };
    p.execute(cx, op);
    assert_eq!(read(&dir, "intro.tex"), "intro\n");
    assert!(p.count(|e| matches!(e, Seen::WorktreeChanged)) >= 1);

    std::fs::write(dir.join("main.tex"), "mine\n").unwrap();
    s.run(&dir, &["commit", "--quiet", "-am", "mine"]);
    p.update(cx, |g, _, cx| g.refresh(cx));
    cx.run_until_parked();
    assert_eq!(p.read(cx, |g, _| g.status().ahead), 1);
    p.update(cx, |g, window, cx| g.push(window, cx));
    let Some(op @ GitOp::Push) = p.requests().pop() else {
        panic!("{:?}", p.requests());
    };
    p.execute(cx, op);
    assert_eq!(p.read(cx, |g, _| g.status().ahead), 0);
    assert_eq!(s.run(&bare, &["log", "-1", "--format=%s"]), "mine");

    // A new branch is published to `origin` with its upstream set.
    p.execute(cx, GitOp::CreateBranch("draft".into()));
    p.execute(cx, GitOp::Push);
    assert_eq!(
        p.read(cx, |g, _| g.status().upstream.clone()).as_deref(),
        Some("origin/draft")
    );
    // Pull, Push, Push: one notification each.
    assert_eq!(p.notifications(cx), 3);
}

#[gpui::test]
fn push_without_a_remote_asks_for_one(cx: &mut TestAppContext) {
    let Some(s) = GitSandbox::new() else { return };
    let dir = committed(&s, "repo");
    let p = Panel::on(cx, &s, &dir);
    // No upstream: nothing to pull from.
    p.update(cx, |g, _, cx| g.pull(cx));
    assert!(p.requests().is_empty());

    p.update(cx, |g, window, cx| g.push(window, cx));
    assert!(p.has_dialog(cx));
    // The name field is filled in with `origin`; the URL has the focus.
    p.fill_and_confirm(cx, "https://example.com/paper.git");
    let Some(GitOp::AddRemote { name, url }) = p.requests().pop() else {
        panic!("{:?}", p.requests());
    };
    assert_eq!(
        (name.as_str(), url.as_str()),
        ("origin", "https://example.com/paper.git")
    );
}

#[gpui::test]
fn stops_on_conflicts_and_aborts(cx: &mut TestAppContext) {
    let Some(s) = GitSandbox::new() else { return };
    let (dir, bare) = published(&s, "repo");
    let p = Panel::on(cx, &s, &dir);
    push_from_elsewhere(&s, &bare, "main.tex", "theirs\n");
    std::fs::write(dir.join("main.tex"), "ours\n").unwrap();
    s.run(&dir, &["commit", "--quiet", "-am", "ours"]);

    p.execute(cx, GitOp::Pull);
    p.draw(cx);
    p.read(cx, |g, cx| {
        assert_eq!(g.status().operation, Operation::Merge);
        assert_eq!(g.status().conflicts().count(), 1);
        assert_eq!(g.commit_mode(), None);
        // git's merge message is offered for the commit that concludes it.
        assert!(
            g.message.read(cx).value().starts_with("Merge branch"),
            "{}",
            g.message.read(cx).value()
        );
        assert_eq!(
            g.file_marks().get(&dir.join("main.tex")),
            Some(&GitMark::Conflict)
        );
    });
    assert_eq!(p.notifications(cx), 1);

    p.execute(cx, GitOp::Abort(Operation::Merge));
    p.read(cx, |g, _| {
        assert_eq!(g.status().operation, Operation::None);
        assert!(g.status().is_clean());
    });
    assert_eq!(read(&dir, "main.tex"), "ours\n");

    // With rebase: resolve, mark as resolved, continue.
    p.update(cx, |g, _, _| g.configure(true, None, Vec::new()));
    p.execute(cx, GitOp::Pull);
    assert_eq!(p.read(cx, |g, _| g.status().operation), Operation::Rebase);
    std::fs::write(dir.join("main.tex"), "both\n").unwrap();
    p.execute(cx, GitOp::Stage(vec!["main.tex".into()]));
    p.draw(cx);
    p.execute(cx, GitOp::ContinueRebase);
    p.read(cx, |g, _| {
        assert_eq!(g.status().operation, Operation::None);
        assert_eq!(g.status().ahead, 1);
        assert_eq!(g.status().behind, 0);
    });
}

#[gpui::test]
fn syncs_what_was_reviewed(cx: &mut TestAppContext) {
    let Some(s) = GitSandbox::new() else { return };
    let (dir, bare) = published(&s, "repo");
    let p = Panel::on(cx, &s, &dir);
    push_from_elsewhere(&s, &bare, "intro.tex", "intro\n");
    std::fs::write(dir.join("main.tex"), "mine\n").unwrap();
    std::fs::write(dir.join("fig.tex"), "fig\n").unwrap();
    p.update(cx, |g, _, cx| g.refresh(cx));
    cx.run_until_parked();
    let review = p.read(cx, |g, _| SyncReview::of(g.status()));
    assert_eq!(review.added, ["fig.tex"]);

    // A file that appeared since the review stops the Sync before it commits anything.
    std::fs::write(dir.join("late.tex"), "late\n").unwrap();
    let sync = |review: &SyncReview, message: Option<&str>| GitOp::Sync {
        message: message.map(str::to_string),
        review: review.clone(),
    };
    p.execute(cx, sync(&review, None));
    assert_eq!(p.notifications(cx), 1);
    assert_eq!(p.read(cx, |g, _| g.history.len()), 1);
    std::fs::remove_file(dir.join("late.tex")).unwrap();

    p.execute(cx, sync(&review, None));
    assert_eq!(p.notifications(cx), 2);
    assert_eq!(read(&dir, "intro.tex"), "intro\n");
    assert_eq!(s.run(&bare, &["log", "-1", "--format=%s"]), SYNC_MESSAGE);
    p.read(cx, |g, _| {
        assert!(g.status().is_clean());
        assert_eq!((g.status().ahead, g.status().behind), (0, 0));
    });

    // With a typed message, which is then cleared.
    std::fs::write(dir.join("main.tex"), "again\n").unwrap();
    p.update(cx, |g, window, cx| {
        g.message
            .update(cx, |m, cx| m.set_value("Typed", window, cx))
    });
    p.execute(cx, sync(&SyncReview::default(), Some("Typed")));
    assert_eq!(s.run(&bare, &["log", "-1", "--format=%s"]), "Typed");
    assert_eq!(
        p.read(cx, |g, cx| g.message.read(cx).value().to_string()),
        ""
    );

    // Nothing to do.
    p.execute(cx, sync(&SyncReview::default(), None));
    assert_eq!(p.notifications(cx), 4);
}

#[gpui::test]
fn initializes_a_repository(cx: &mut TestAppContext) {
    let Some(s) = GitSandbox::new() else { return };
    let dir = s.dir.join("paper");
    std::fs::create_dir_all(&dir).unwrap();
    let p = Panel::open(cx);
    p.update(cx, |g, _, cx| {
        g.use_git(s.git.clone());
        g.set_project(Some(dir.clone()), false, cx);
    });
    cx.run_until_parked();
    assert!(matches!(
        p.read(cx, |g, _| g.state.clone()),
        RepoState::NotRepo
    ));
    p.draw(cx);
    p.execute(cx, GitOp::Init);
    assert!(dir.join(".git").is_dir());
    assert!(dir.join(".gitignore").is_file());
    assert!(p.read(cx, |g, _| g.repo().is_some()));
    assert_eq!(p.count(|e| matches!(e, Seen::Trust)), 1);
    p.draw(cx);
}

#[gpui::test]
fn one_operation_at_a_time(cx: &mut TestAppContext) {
    let Some(s) = GitSandbox::new() else { return };
    let dir = committed(&s, "repo");
    std::fs::write(dir.join("a.tex"), "a\n").unwrap();
    let p = Panel::on(cx, &s, &dir);
    p.update(cx, |g, window, cx| {
        g.execute(GitOp::Stage(vec!["a.tex".into()]), window, cx);
        assert_eq!(g.busy, Some("Stage"));
        g.execute(GitOp::Unstage(vec!["a.tex".into()]), window, cx);
    });
    p.draw(cx);
    cx.run_until_parked();
    // The second was refused, with a word why.
    assert_eq!(p.notifications(cx), 1);
    assert_eq!(p.read(cx, |g, _| g.status().staged().count()), 1);
}

#[gpui::test]
fn asks_who_is_committing(cx: &mut TestAppContext) {
    let Some(s) = GitSandbox::new() else { return };
    let dir = committed(&s, "repo");
    // An empty name in the repository's configuration hides the sandbox's identity.
    s.run(&dir, &["config", "user.name", ""]);
    std::fs::write(dir.join("a.tex"), "a\n").unwrap();
    let p = Panel::on(cx, &s, &dir);
    let commit = GitOp::Commit {
        message: "m".into(),
        amend: false,
        all: true,
    };
    p.execute(cx, commit);
    assert!(p.has_dialog(cx));
    assert!(matches!(
        p.read(cx, |g, _| g.pending.clone()),
        Some(GitOp::Commit { .. })
    ));
    // An empty form stays open.
    p.confirm(cx);
    assert!(p.has_dialog(cx));
    p.update(cx, |_, window, cx| window.close_dialog(cx));

    // Once the identity is saved, the commit is asked for again.
    p.execute(
        cx,
        GitOp::SetIdentity {
            name: "Ada".into(),
            email: "ada@example.com".into(),
            global: false,
        },
    );
    assert!(matches!(&p.requests()[..], [GitOp::Commit { .. }]));
    assert!(p.read(cx, |g, _| g.pending.is_none()));
}

#[gpui::test]
fn shows_why_git_is_off(cx: &mut TestAppContext) {
    let Some(s) = GitSandbox::new() else { return };
    let dir = committed(&s, "repo");
    let sub = dir.join("chapters");
    std::fs::create_dir_all(&sub).unwrap();
    let p = Panel::open(cx);
    p.draw(cx);
    assert!(matches!(
        p.read(cx, |g, _| g.state.clone()),
        RepoState::NoProject
    ));

    // Untrusted: git does not run, and the panel says where the repository is.
    p.update(cx, |g, _, cx| {
        g.use_git(s.git.clone());
        g.set_project(Some(sub.clone()), false, cx);
    });
    cx.run_until_parked();
    assert!(matches!(p.read(cx, |g, _| g.state.clone()), RepoState::Untrusted(top) if top == dir));
    assert!(p.read(cx, |g, _| g.file_marks().is_empty()));
    p.draw(cx);

    // Trusted, from a subfolder.
    p.update(cx, |g, _, cx| g.set_project(Some(sub.clone()), true, cx));
    assert!(matches!(
        p.read(cx, |g, _| g.state.clone()),
        RepoState::Searching
    ));
    p.draw(cx);
    cx.run_until_parked();
    assert!(p.read(cx, |g, _| g.repo().is_some()));
    // The same project again only refreshes.
    let generation = p.read(cx, |g, _| g.generation);
    p.update(cx, |g, _, cx| g.set_project(Some(sub.clone()), true, cx));
    assert_eq!(p.read(cx, |g, _| g.generation), generation);

    // A `.git` that git places elsewhere is not handed the project.
    let odd = committed(&s, "odd");
    s.run(&odd, &["config", "core.worktree", &s.dir.to_string_lossy()]);
    p.update(cx, |g, _, cx| g.set_project(Some(odd.clone()), true, cx));
    cx.run_until_parked();
    assert!(matches!(
        p.read(cx, |g, _| g.state.clone()),
        RepoState::Failed(_)
    ));
    p.draw(cx);

    // No git at all.
    p.update(cx, |g, _, cx| {
        g.git = Some(None);
        g.set_project(Some(dir.clone()), false, cx);
        g.set_project(Some(dir.clone()), true, cx);
    });
    cx.run_until_parked();
    assert!(matches!(
        p.read(cx, |g, _| g.state.clone()),
        RepoState::NoGit
    ));
    p.draw(cx);

    p.update(cx, |g, _, cx| g.set_project(None, true, cx));
    assert!(matches!(
        p.read(cx, |g, _| g.state.clone()),
        RepoState::NoProject
    ));
    assert_eq!(p.read(cx, |g, _| g.change_count()), 0);
}

#[gpui::test]
fn draws_changes_and_history(cx: &mut TestAppContext) {
    let Some(s) = GitSandbox::new() else { return };
    let dir = committed(&s, "repo");
    std::fs::write(dir.join("chapter.tex"), "chapter\n").unwrap();
    std::fs::write(dir.join("gone.tex"), "gone\n").unwrap();
    s.run(&dir, &["add", "-A"]);
    for i in 0..3 {
        std::fs::write(dir.join("main.tex"), format!("{i}\n")).unwrap();
        s.run(&dir, &["commit", "--quiet", "-am", &format!("commit {i}")]);
    }
    std::fs::write(dir.join("main.tex"), "staged\n").unwrap();
    s.run(&dir, &["add", "main.tex"]);
    std::fs::write(dir.join("main.tex"), "and more\n").unwrap();
    std::fs::create_dir_all(dir.join("ch")).unwrap();
    std::fs::write(dir.join("ch/new.tex"), "new\n").unwrap();
    s.run(&dir, &["rm", "--quiet", "chapter.tex"]);
    std::fs::remove_file(dir.join("gone.tex")).unwrap();
    s.run(&dir, &["checkout", "--quiet", "--detach"]);
    let p = Panel::on(cx, &s, &dir);
    p.update(cx, |g, _, cx| g.toggle_amend(false, cx));
    p.draw(cx);
    p.update(cx, |g, _, cx| {
        g.view = View::History;
        g.error = Some("index.lock exists".into());
        cx.notify();
    });
    p.draw(cx);
    p.read(cx, |g, _| {
        assert_eq!(g.history.len(), 4);
        assert_eq!(g.status().branch, None);
        // main.tex staged and changed again, chapter.tex deleted from the index, gone.tex
        // from the disk, ch/new.tex new.
        assert_eq!(g.status().staged().count(), 2);
        assert_eq!(g.status().unstaged().count(), 3);
    });

    // More history, a page at a time: here, nothing more.
    p.update(cx, |g, _, cx| {
        g.history_done = false;
        g.load_more_history(cx);
    });
    cx.run_until_parked();
    p.read(cx, |g, _| {
        assert_eq!(g.history.len(), 4);
        assert!(g.history_done);
    });
}

#[gpui::test]
fn reports_failures_by_kind(cx: &mut TestAppContext) {
    let Some(s) = GitSandbox::new() else { return };
    let dir = committed(&s, "repo");
    let p = Panel::on(cx, &s, &dir);
    let sync = || GitOp::Sync {
        message: None,
        review: SyncReview::default(),
    };
    let finish = |cx: &mut TestAppContext, op: GitOp, result: tex_git::Result<Outcome>| {
        p.update(cx, |g, window, cx| g.finish(op, result, window, cx));
        cx.run_until_parked();
    };
    // Offline: the changes stay here, with a warning.
    let output = String::from("fatal: unable to access");
    finish(
        cx,
        sync(),
        Err(Error::Unreachable {
            output: output.clone(),
        }),
    );
    finish(cx, sync(), Err(Error::TimedOut(60)));
    assert_eq!(p.notifications(cx), 2);
    // Cancelled by the user: nothing to say.
    finish(cx, GitOp::Fetch, Err(Error::Cancelled));
    assert_eq!(p.notifications(cx), 2);
    // Anything else, with git's output a click away.
    let failed = Error::Failed {
        command: "fetch".into(),
        output,
    };
    finish(cx, GitOp::Fetch, Err(failed));
    // Not an Overleaf project: an authentication failure is a plain error.
    let auth = Error::Auth {
        output: "Authentication failed".into(),
    };
    finish(cx, GitOp::Push, Err(auth));
    assert_eq!(p.notifications(cx), 4);
    assert!(!p.has_dialog(cx));
    p.draw(cx);
}

const OVERLEAF: &str = "https://git.overleaf.com";

/// A repository whose `origin` is an Overleaf project. Nothing here reaches it: only the
/// remote's URL is read.
fn overleaf_repo(s: &GitSandbox) -> PathBuf {
    let dir = committed(s, "overleaf");
    s.run(
        &dir,
        &[
            "remote",
            "add",
            "origin",
            &format!("{OVERLEAF}/abcdef123456"),
        ],
    );
    dir
}

#[gpui::test]
fn recognizes_overleaf_projects(cx: &mut TestAppContext) {
    let Some(s) = GitSandbox::new() else { return };
    let dir = overleaf_repo(&s);
    let p = Panel::on(cx, &s, &dir);
    cx.run_until_parked();
    p.read(cx, |g, _| {
        assert_eq!(g.overleaf.as_deref(), Some(OVERLEAF));
        // The keychain has no token.
        assert_eq!(g.credential, None);
    });
    // Build files are kept out of the project for everyone, not by a `.gitignore`.
    let exclude = read(&dir, ".git/info/exclude");
    assert!(exclude.contains("*.aux"), "{exclude}");
    p.draw(cx);

    // One branch: the one cloned, which tracks the project.
    assert!(p.read(cx, |g, _| g.check_overleaf_branch()).is_err());
    p.update(cx, |g, window, cx| {
        g.sync(window, cx);
        g.push(window, cx);
        g.prompt_new_branch(window, cx);
    });
    assert!(p.requests().is_empty());
    assert!(!p.has_dialog(cx));
    assert_eq!(p.notifications(cx), 3);

    p.update(cx, |g, _, _| {
        g.status.upstream = Some("origin/master".into())
    });
    assert_eq!(p.read(cx, |g, _| g.check_overleaf_branch()), Ok(()));
    p.update(cx, |g, window, cx| g.sync(window, cx));
    assert!(
        matches!(&p.requests()[..], [GitOp::Sync { message: None, review }] if review.is_empty())
    );

    // New files are shown before they reach the collaborators.
    std::fs::write(dir.join("fig.tex"), "fig\n").unwrap();
    p.update(cx, |g, _, cx| g.refresh(cx));
    cx.run_until_parked();
    p.update(cx, |g, window, cx| {
        g.status.upstream = Some("origin/main".into());
        g.message
            .update(cx, |m, cx| m.set_value("Figure", window, cx));
        g.sync(window, cx);
    });
    assert!(p.has_dialog(cx));
    assert_eq!(p.requests().len(), 1);
    p.confirm(cx);
    let Some(GitOp::Sync { message, review }) = p.requests().pop() else {
        panic!("{:?}", p.requests());
    };
    assert_eq!(message.as_deref(), Some("Figure"));
    assert_eq!(review.added, ["fig.tex"]);
}

#[gpui::test]
fn saves_an_overleaf_token_once_it_works(cx: &mut TestAppContext) {
    let Some(s) = GitSandbox::new() else { return };
    let dir = overleaf_repo(&s);
    let p = Panel::on(cx, &s, &dir);
    let auth = || {
        Err(Error::Auth {
            output: "Authentication failed".into(),
        })
    };
    let stored = |cx: &mut TestAppContext| {
        let task = cx.update(|cx| overleaf::load_token(OVERLEAF, cx));
        testing::resolve(cx, task)
    };

    // Overleaf wants a token: ask for one, then try again with it.
    p.update(cx, |g, window, cx| {
        g.finish(GitOp::Push, auth(), window, cx)
    });
    assert!(p.has_dialog(cx));
    p.fill_and_confirm(cx, "not a token");
    // Refused, the dialog stays.
    assert!(p.has_dialog(cx));
    assert!(p.requests().is_empty());
    p.update(cx, |_, window, cx| window.close_dialog(cx));

    p.update(cx, |g, window, cx| {
        g.finish(GitOp::Push, auth(), window, cx)
    });
    p.fill_and_confirm(cx, "olp_first0token");
    assert!(matches!(&p.requests()[..], [GitOp::Push]));
    p.read(cx, |g, _| {
        let c = g.credential.as_ref().unwrap();
        assert_eq!(
            (c.url.as_str(), c.password.as_str()),
            (OVERLEAF, "olp_first0token")
        );
        assert!(g.credential_unsaved);
    });
    // Not saved until Overleaf accepted it.
    assert_eq!(stored(cx), None);
    p.update(cx, |g, window, cx| {
        g.finish(GitOp::Push, Ok(Outcome::Done), window, cx)
    });
    cx.run_until_parked();
    assert!(!p.read(cx, |g, _| g.credential_unsaved));
    assert_eq!(stored(cx).as_deref(), Some("olp_first0token"));

    // A refusal keeps the saved token (it may concern this project only), and the new
    // token is not saved when the retry fails.
    p.update(cx, |g, window, cx| {
        g.finish(GitOp::Push, auth(), window, cx)
    });
    p.fill_and_confirm(cx, "olp_second0token");
    p.update(cx, |g, window, cx| {
        g.finish(GitOp::Push, auth(), window, cx)
    });
    cx.run_until_parked();
    assert_eq!(stored(cx).as_deref(), Some("olp_first0token"));
    p.update(cx, |_, window, cx| window.close_dialog(cx));

    // Another project's token reads the saved one.
    p.update(cx, |g, _, cx| {
        g.overleaf = None;
        g.set_overleaf(Some(OVERLEAF.into()), cx);
    });
    cx.run_until_parked();
    let password = p.read(cx, |g, _| g.credential.as_ref().map(|c| c.password.clone()));
    assert_eq!(password.as_deref(), Some("olp_first0token"));
}
