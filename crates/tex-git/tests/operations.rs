//! The guards around git's arguments, the operations a merge-like command
//! leaves behind, and how commands are stopped. Skipped (passing) when git is
//! not installed.

mod common;

use std::time::{Duration, Instant};

use common::{Sandbox, commit_all, hook, read, write};
use tex_git::{Branch, Cancel, CommitOptions, Error, Git, Operation};

#[track_caller]
fn invalid<T: std::fmt::Debug>(r: tex_git::Result<T>) {
    assert!(matches!(r, Err(Error::Invalid(_))), "{r:?}");
}

#[test]
fn arguments_are_never_options() {
    let Some(sb) = Sandbox::new() else {
        return;
    };
    let repo = sb.committed("p");
    let canary = sb.path("p/canary");
    for rev in ["", "-p", "--output=canary"] {
        invalid(repo.show(rev));
        invalid(repo.file_at(rev, "main.tex"));
        invalid(repo.restore_file(rev, "main.tex"));
        invalid(repo.switch(&Branch {
            name: rev.into(),
            remote: false,
            current: false,
            upstream: None,
        }));
    }
    for name in ["-x", " --orphan=x", "", "bad..name", "a b"] {
        invalid(repo.create_branch(name));
    }
    for (name, url) in [
        ("", "/x"),
        ("-x", "/x"),
        ("origin", ""),
        ("origin", "--upload-pack=touch canary"),
        (" ", " "),
    ] {
        invalid(repo.add_remote(name, url));
    }
    assert!(repo.remotes().unwrap().is_empty());
    for name in ["", "-v", "--push"] {
        assert_eq!(repo.remote_url(name).unwrap(), None);
    }
    assert!(!canary.exists());
}

#[test]
fn reading_unknown_things() {
    let Some(sb) = Sandbox::new() else {
        return;
    };
    let repo = sb.committed("p");
    assert!(matches!(repo.show("nosuchrev"), Err(Error::Failed { .. })));
    assert!(repo.file_at("HEAD", "nope.tex").is_err());
    let err = repo.untracked_diff("nope.tex").unwrap_err();
    assert!(matches!(err, Error::Spawn(_)), "{err:?}");
    assert_eq!(repo.merge_message(), None);
    assert_eq!(repo.operation(), Operation::None);
    // The log of a file nobody committed.
    assert!(repo.log(0, 10, Some("nope.tex")).unwrap().is_empty());
    assert_eq!(repo.git().program(), sb.git.program());
    let err = sb.git.discover(&sb.path("nowhere")).unwrap_err();
    assert!(matches!(err, Error::Spawn(_)), "{err:?}");
    assert_eq!(
        repo.rel_path(&repo.abs_path("a/b.tex")).as_deref(),
        Some("a/b.tex")
    );

    // A broken index.
    std::fs::write(repo.root().join(".git").join("index"), "garbage").unwrap();
    let err = repo.status().unwrap_err();
    assert!(matches!(err, Error::Failed { .. }), "{err:?}");
}

#[test]
fn untracked_folders_and_large_files_are_binary() {
    let Some(sb) = Sandbox::new() else {
        return;
    };
    let repo = sb.committed("p");
    // A nested repository is listed as a folder.
    sb.git.init(&repo.abs_path("nested")).unwrap();
    let st = repo.status().unwrap();
    assert!(st.entries.iter().any(|e| e.path == "nested/"), "{st:?}");
    let patch = repo.untracked_diff("nested/").unwrap();
    assert!(
        patch.contains("Binary files /dev/null and b/nested/"),
        "{patch}"
    );

    let big = std::fs::File::create(repo.abs_path("big.tex")).unwrap();
    big.set_len((8 << 20) + 1).unwrap();
    drop(big);
    assert!(
        repo.untracked_diff("big.tex")
            .unwrap()
            .contains("Binary files")
    );
    // Up to the limit, text is text.
    std::fs::write(repo.abs_path("big.tex"), "a\n".repeat(1000)).unwrap();
    assert!(repo.untracked_diff("big.tex").unwrap().contains("+a\n"));
}

#[test]
fn identity_is_asked_for_then_set() {
    let Some(sb) = Sandbox::new() else {
        return;
    };
    // No identity, and none guessed from the system.
    let config = sb.path("empty-gitconfig");
    std::fs::write(&config, "[user]\n\tuseConfigOnly = true\n").unwrap();
    let git = sb
        .git
        .clone()
        .env("GIT_CONFIG_GLOBAL", &config)
        .env("EMAIL", "");
    let repo = git.init(&sb.path("p")).unwrap();
    write(&repo, "main.tex", "x\n");
    repo.stage(&["main.tex".into()]).unwrap();
    let err = repo.commit("first", CommitOptions::default()).unwrap_err();
    assert!(matches!(err, Error::NoIdentity), "{err:?}");

    repo.set_identity("Ada", "ada@example.org", false).unwrap();
    repo.commit("first", CommitOptions::default()).unwrap();
    assert_eq!(repo.log(0, 1, None).unwrap()[0].author, "Ada");
    assert!(!std::fs::read_to_string(&config).unwrap().contains("Ada"));

    // The global one goes to the user's configuration.
    let other = git.init(&sb.path("q")).unwrap();
    other.set_identity("Bob", "bob@example.org", true).unwrap();
    let global = std::fs::read_to_string(&config).unwrap();
    assert!(
        global.contains("Bob") && global.contains("bob@example.org"),
        "{global}"
    );
    commit_all(&other, "first");
    assert_eq!(other.log(0, 1, None).unwrap()[0].email, "bob@example.org");
}

#[test]
fn unstaging_after_the_first_commit() {
    let Some(sb) = Sandbox::new() else {
        return;
    };
    let repo = sb.committed("p");
    write(&repo, "main.tex", "changed\n");
    write(&repo, "new.tex", "new\n");
    repo.stage(&["main.tex".into(), "new.tex".into()]).unwrap();
    assert_eq!(repo.status().unwrap().staged().count(), 2);
    repo.unstage(&["main.tex".into(), "new.tex".into()], true)
        .unwrap();
    let st = repo.status().unwrap();
    assert_eq!(st.staged().count(), 0);
    assert_eq!(st.unstaged().count(), 2);
    assert_eq!(read(&repo, "main.tex"), "changed\n");
}

/// `a` and `b` share `main.tex`, then change the same line of it: `a` gets
/// the commit `b` pushed as `origin/main`, and its own commit on top of the
/// common one.
fn diverged(sb: &Sandbox) -> (tex_git::Repo, String) {
    let remote = sb.bare("remote.git");
    let cancel = Cancel::new();
    let a = sb.committed("a");
    a.add_remote("origin", remote.to_str().unwrap()).unwrap();
    a.push(&a.status().unwrap(), &cancel).unwrap();
    let b = sb
        .git
        .clone_repo(remote.to_str().unwrap(), &sb.path("b"), &cancel)
        .unwrap();
    write(&b, "main.tex", "one\nTWO from b\nthree\n");
    commit_all(&b, "b edits");
    b.push(&b.status().unwrap(), &cancel).unwrap();
    write(&a, "main.tex", "one\nTWO from a\nthree\n");
    commit_all(&a, "a edits");
    a.fetch(&cancel).unwrap();
    let theirs = sb.ok(a.root(), &["rev-parse", "origin/main"]);
    (a, theirs)
}

#[test]
fn rebase_stops_on_conflicts_then_continues() {
    let Some(sb) = Sandbox::new() else {
        return;
    };
    let (a, _) = diverged(&sb);
    let cancel = Cancel::new();
    assert!(a.pull(true, &cancel).is_err());
    let st = a.status().unwrap();
    assert_eq!(st.operation, Operation::Rebase);
    assert_eq!(st.branch, None);
    assert_eq!(st.conflicts().count(), 1);
    // Sync waits for the rebase to end.
    let err = a.sync("Sync", &cancel).unwrap_err();
    assert_eq!(err.to_string(), "Finish the rebase first");
    // Not resolved yet.
    let err = a.continue_rebase().unwrap_err();
    assert!(matches!(err, Error::Failed { .. }), "{err:?}");

    a.abort(Operation::Rebase).unwrap();
    assert_eq!(a.status().unwrap().operation, Operation::None);
    assert_eq!(read(&a, "main.tex"), "one\nTWO from a\nthree\n");

    assert!(a.pull(true, &cancel).is_err());
    write(&a, "main.tex", "one\nTWO from both\nthree\n");
    a.stage(&["main.tex".into()]).unwrap();
    a.continue_rebase().unwrap();
    let st = a.status().unwrap();
    assert_eq!(st.operation, Operation::None);
    assert_eq!(st.branch.as_deref(), Some("main"));
    assert_eq!((st.ahead, st.behind), (1, 0));
    let subjects: Vec<_> = a
        .log(0, 10, None)
        .unwrap()
        .into_iter()
        .map(|c| c.subject)
        .collect();
    assert_eq!(subjects, ["a edits", "b edits", "first"]);
}

#[test]
fn cherry_pick_and_revert_in_progress() {
    let Some(sb) = Sandbox::new() else {
        return;
    };
    let (a, theirs) = diverged(&sb);
    let cherry = sb.raw(a.root(), &["cherry-pick", &theirs]);
    assert!(!cherry.status.success());
    let st = a.status().unwrap();
    assert_eq!(st.operation, Operation::CherryPick);
    assert_eq!(st.operation.label(), "Cherry-pick");
    assert_eq!(
        a.sync("Sync", &Cancel::new()).unwrap_err().to_string(),
        "Finish the cherry-pick first"
    );
    a.abort(Operation::CherryPick).unwrap();
    assert_eq!(a.status().unwrap().operation, Operation::None);
    assert!(a.status().unwrap().is_clean());

    // Reverting the commit before the last one conflicts with the last one.
    let first = sb.ok(a.root(), &["rev-parse", "HEAD~1"]);
    write(&a, "main.tex", "one\nTWO again\nthree\n");
    commit_all(&a, "again");
    let revert = sb.raw(a.root(), &["revert", "--no-edit", "HEAD~1"]);
    assert!(!revert.status.success());
    assert_eq!(a.status().unwrap().operation, Operation::Revert);
    a.abort(Operation::Revert).unwrap();
    assert_eq!(a.status().unwrap().operation, Operation::None);
    assert_eq!(read(&a, "main.tex"), "one\nTWO again\nthree\n");
    assert_ne!(first, sb.ok(a.root(), &["rev-parse", "HEAD"]));

    // Nothing to stop.
    a.abort(Operation::None).unwrap();
    assert!(a.abort(Operation::Merge).is_err());
}

#[test]
fn sync_and_push_need_a_branch_without_conflicts() {
    let Some(sb) = Sandbox::new() else {
        return;
    };
    let (a, _) = diverged(&sb);
    let cancel = Cancel::new();

    // Conflicts left by `stash pop`, with no operation in progress.
    write(&a, "main.tex", "one\nTWO stashed\nthree\n");
    sb.ok(a.root(), &["stash"]);
    write(&a, "main.tex", "one\nTWO committed\nthree\n");
    commit_all(&a, "committed");
    assert!(!sb.raw(a.root(), &["stash", "pop"]).status.success());
    let st = a.status().unwrap();
    assert_eq!(st.operation, Operation::None);
    assert_eq!(st.conflicts().count(), 1);
    assert_eq!(
        a.sync("Sync", &cancel).unwrap_err().to_string(),
        "Resolve the conflicts first"
    );
    sb.ok(a.root(), &["reset", "--hard", "--quiet"]);

    sb.ok(a.root(), &["switch", "--quiet", "--detach"]);
    let st = a.status().unwrap();
    assert_eq!(st.branch, None);
    assert_eq!(
        a.sync("Sync", &cancel).unwrap_err().to_string(),
        "Check out a branch to sync it"
    );
    assert_eq!(
        a.push(&st, &cancel).unwrap_err().to_string(),
        "Check out a branch to push it"
    );
}

#[test]
fn a_new_branch_is_published_to_the_first_remote() {
    let Some(sb) = Sandbox::new() else {
        return;
    };
    let remote = sb.bare("remote.git");
    let a = sb.committed("a");
    a.add_remote("upstream", remote.to_str().unwrap()).unwrap();
    a.create_branch("draft").unwrap();
    a.push(&a.status().unwrap(), &Cancel::new()).unwrap();
    assert_eq!(
        a.status().unwrap().upstream.as_deref(),
        Some("upstream/draft")
    );
    let branches = a.branches().unwrap();
    let theirs = branches.iter().find(|b| b.remote).unwrap();
    assert_eq!(theirs.name, "upstream/draft");
    assert_eq!(theirs.local_name(), "draft");

    // A remote branch is checked out as a local one that tracks it.
    let b = sb
        .git
        .clone_repo(remote.to_str().unwrap(), &sb.path("b"), &Cancel::new())
        .unwrap();
    let draft = b
        .branches()
        .unwrap()
        .into_iter()
        .find(|b| b.name == "origin/draft")
        .unwrap();
    b.switch(&draft).unwrap();
    let st = b.status().unwrap();
    assert_eq!(st.branch.as_deref(), Some("draft"));
    assert_eq!(st.upstream.as_deref(), Some("origin/draft"));
}

#[test]
fn discovery_refuses_a_working_tree_elsewhere() {
    let Some(sb) = Sandbox::new() else {
        return;
    };
    let other = sb.committed("other");
    let top = sb.init("top");
    sb.ok(
        top.root(),
        &["config", "core.worktree", other.root().to_str().unwrap()],
    );
    let err = sb.git.discover_at(top.root()).unwrap_err();
    assert!(matches!(err, Error::Invalid(_)), "{err:?}");
    assert!(
        err.to_string().contains("Git found the repository at"),
        "{err}"
    );
}

#[test]
fn init_keeps_an_existing_gitignore() {
    let Some(sb) = Sandbox::new() else {
        return;
    };
    let dir = sb.path("p");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join(".gitignore"), "mine\n").unwrap();
    let repo = sb.git.init(&dir).unwrap();
    assert_eq!(read(&repo, ".gitignore"), "mine\n");
    // Again, in the same folder.
    sb.git.init(&dir).unwrap();
    assert!(sb.git.init(&dir.join(".gitignore").join("x")).is_err());

    // A .gitignore that cannot be written does not stop the repository.
    #[cfg(unix)]
    {
        let dir = sb.path("q");
        std::fs::create_dir(&dir).unwrap();
        std::os::unix::fs::symlink(sb.path("missing/x"), dir.join(".gitignore")).unwrap();
        sb.git.init(&dir).unwrap();
        assert!(!sb.path("missing").exists());
    }
}

#[test]
fn exclude_appends_to_existing_patterns() {
    let Some(sb) = Sandbox::new() else {
        return;
    };
    let repo = sb.init("p");
    let exclude = repo.root().join(".git").join("info").join("exclude");
    std::fs::remove_dir_all(exclude.parent().unwrap()).unwrap();
    repo.exclude_local_files().unwrap();
    assert!(read(&repo, ".git/info/exclude").contains("*.aux\n"));

    std::fs::write(&exclude, "*.aux\n*.log").unwrap();
    repo.exclude_local_files().unwrap();
    let text = read(&repo, ".git/info/exclude");
    assert!(text.starts_with("*.aux\n*.log\n# Local files"), "{text}");
    assert_eq!(text.matches("*.log").count(), 1);
}

// ── Stopping commands ───────────────────────────────────────────────────

/// A repository with a remote whose pushes run `pre-push`.
fn slow_push(sb: &Sandbox, script: &str) -> tex_git::Repo {
    let remote = sb.bare("remote.git");
    let a = sb.committed("a");
    a.add_remote("origin", remote.to_str().unwrap()).unwrap();
    hook(&a, "pre-push", script);
    a
}

#[test]
fn network_commands_time_out() {
    let Some(sb) = Sandbox::new() else {
        return;
    };
    let a = slow_push(&sb, "sleep 30");
    let a = Git::discover(
        &sb.git.clone().network_timeout(Some(Duration::from_secs(1))),
        a.root(),
    )
    .unwrap()
    .unwrap();
    let started = Instant::now();
    let err = a.push(&a.status().unwrap(), &Cancel::new()).unwrap_err();
    assert!(matches!(err, Error::TimedOut(1)), "{err:?}");
    assert!(started.elapsed() < Duration::from_secs(20));
}

#[test]
fn commands_are_cancelled_while_running() {
    let Some(sb) = Sandbox::new() else {
        return;
    };
    let a = slow_push(&sb, "sleep 30");
    let cancel = Cancel::new();
    let stop = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        stop.cancel();
    });
    let started = Instant::now();
    let err = a.push(&a.status().unwrap(), &cancel).unwrap_err();
    assert!(matches!(err, Error::Cancelled), "{err:?}");
    assert!(cancel.is_cancelled());
    assert!(started.elapsed() < Duration::from_secs(20));
    // Nothing was pushed.
    assert_eq!(a.status().unwrap().upstream, None);
}

/// A program git started may keep its output open after git exited (the
/// credential cache daemon does): the result does not wait for it.
#[test]
fn helpers_left_running_do_not_hold_the_result() {
    let Some(sb) = Sandbox::new() else {
        return;
    };
    let a = slow_push(&sb, "(sleep 20; echo late) &\necho early");
    let started = Instant::now();
    a.push(&a.status().unwrap(), &Cancel::new()).unwrap();
    assert!(started.elapsed() < Duration::from_secs(15));
    assert_eq!(a.status().unwrap().upstream.as_deref(), Some("origin/main"));
}

#[test]
fn hooks_that_fail_stop_the_commit() {
    let Some(sb) = Sandbox::new() else {
        return;
    };
    let a = sb.committed("a");
    hook(&a, "pre-commit", "echo 'error: checks failed' >&2\nexit 1");
    write(&a, "main.tex", "changed\n");
    let err = a
        .commit(
            "x",
            CommitOptions {
                all: true,
                amend: false,
            },
        )
        .unwrap_err();
    assert_eq!(err.to_string(), "Checks failed");
    assert!(err.details().is_some_and(|d| d.contains("checks failed")));
}
