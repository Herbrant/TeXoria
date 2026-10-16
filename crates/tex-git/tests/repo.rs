//! Scenarios against real temporary repositories. Skipped (passing) when git
//! is not installed.

use std::path::Path;

use tex_git::{Cancel, CommitOptions, Error, Git, Operation, Repo, SyncOutcome};

/// git with the user's own configuration out of the way.
fn git(home: &Path) -> Option<Git> {
    let found = Git::find()?;
    let config = home.join("gitconfig");
    std::fs::write(
        &config,
        "[user]\n\tname = Test\n\temail = test@example.com\n[init]\n\tdefaultBranch = main\n",
    )
    .unwrap();
    Some(
        Git::at(found.program())
            .env("GIT_CONFIG_GLOBAL", &config)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("HOME", home),
    )
}

fn write(repo: &Repo, rel: &str, text: &str) {
    let p = repo.abs_path(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

fn commit_all(repo: &Repo, msg: &str) {
    repo.stage(&[".".into()]).unwrap();
    repo.commit(msg, CommitOptions::default()).unwrap();
}

#[test]
fn init_stage_commit_log_show() {
    let tmp = tempfile::tempdir().unwrap();
    let Some(git) = git(tmp.path()) else {
        eprintln!("git not found; skipping");
        return;
    };
    let repo = git.init(&tmp.path().join("paper")).unwrap();
    assert!(repo.abs_path(".gitignore").is_file());
    assert!(git.discover(&tmp.path().join("paper")).unwrap().is_some());
    assert!(git.discover(tmp.path()).unwrap().is_none());

    write(&repo, "main.tex", "\\documentclass{article}\n");
    write(&repo, "main.aux", "ignored\n");
    let st = repo.status().unwrap();
    assert_eq!(st.oid, None);
    assert_eq!(st.branch.as_deref(), Some("main"));
    let paths: Vec<_> = st.entries.iter().map(|e| e.path.as_str()).collect();
    assert_eq!(paths, [".gitignore", "main.tex"]);
    assert!(st.entries.iter().all(|e| e.is_untracked()));
    assert!(
        repo.untracked_diff("main.tex")
            .unwrap()
            .contains("+\\documentclass")
    );
    assert!(repo.log(0, 10, None).unwrap().is_empty());

    repo.stage(&["main.tex".into()]).unwrap();
    assert!(
        repo.diff("main.tex", true)
            .unwrap()
            .contains("new file mode")
    );
    repo.unstage(&["main.tex".into()], false).unwrap();
    assert_eq!(repo.status().unwrap().staged().count(), 0);

    assert!(matches!(
        repo.commit(" \n", CommitOptions::default()),
        Err(Error::Invalid(_))
    ));
    commit_all(&repo, "Initial draft");
    let st = repo.status().unwrap();
    assert!(st.is_clean() && st.oid.is_some());

    write(&repo, "main.tex", "\\documentclass{book}\n");
    let st = repo.status().unwrap();
    assert_eq!(st.unstaged().count(), 1);
    let patch = repo.diff("main.tex", false).unwrap();
    assert!(
        patch.contains("-\\documentclass{article}") && patch.contains("+\\documentclass{book}")
    );

    repo.commit(
        "Book class",
        CommitOptions {
            all: true,
            amend: false,
        },
    )
    .unwrap();
    repo.commit(
        "Use the book class",
        CommitOptions {
            all: false,
            amend: true,
        },
    )
    .unwrap();
    assert_eq!(repo.head_message().unwrap(), "Use the book class");

    let log = repo.log(0, 10, None).unwrap();
    assert_eq!(log.len(), 2);
    assert_eq!(log[0].subject, "Use the book class");
    assert_eq!(log[0].author, "Test");
    assert_eq!(repo.log(1, 10, None).unwrap().len(), 1);
    assert_eq!(repo.log(0, 10, Some("main.tex")).unwrap().len(), 2);

    let first = &log[1];
    assert!(
        repo.show(&first.sha)
            .unwrap()
            .contains("+\\documentclass{article}")
    );
    assert_eq!(
        repo.file_at(&first.sha, "main.tex").unwrap(),
        b"\\documentclass{article}\n"
    );
    repo.restore_file(&first.sha, "main.tex").unwrap();
    assert_eq!(
        std::fs::read_to_string(repo.abs_path("main.tex")).unwrap(),
        "\\documentclass{article}\n"
    );
    repo.discard(&["main.tex".into()]).unwrap();
    assert!(repo.status().unwrap().is_clean());

    write(&repo, "notes.txt", "x");
    repo.delete_untracked(&["notes.txt".into()]).unwrap();
    assert!(repo.status().unwrap().is_clean());
}

#[test]
fn branches_switch_and_create() {
    let tmp = tempfile::tempdir().unwrap();
    let Some(git) = git(tmp.path()) else {
        return;
    };
    let repo = git.init(&tmp.path().join("p")).unwrap();
    write(&repo, "a.tex", "a\n");
    commit_all(&repo, "a");

    assert!(matches!(
        repo.create_branch("bad..name"),
        Err(Error::Invalid(_))
    ));
    repo.create_branch("draft").unwrap();
    write(&repo, "a.tex", "draft\n");
    commit_all(&repo, "draft");

    let branches = repo.branches().unwrap();
    let main = branches.iter().find(|b| b.name == "main").unwrap();
    assert!(!main.current);
    assert!(branches.iter().any(|b| b.name == "draft" && b.current));
    repo.switch(main).unwrap();
    assert_eq!(repo.status().unwrap().branch.as_deref(), Some("main"));
    assert_eq!(
        std::fs::read_to_string(repo.abs_path("a.tex")).unwrap(),
        "a\n"
    );
}

#[test]
fn push_pull_and_conflicts() {
    let tmp = tempfile::tempdir().unwrap();
    let Some(git) = git(tmp.path()) else {
        return;
    };
    let cancel = Cancel::new();
    let remote = tmp.path().join("remote.git");
    std::fs::create_dir(&remote).unwrap();
    let bare = std::process::Command::new(git.program())
        .args(["init", "--bare", "--quiet", "-b", "main"])
        .arg(&remote)
        .status()
        .unwrap();
    assert!(bare.success());

    let a = git.init(&tmp.path().join("a")).unwrap();
    write(&a, "main.tex", "one\ntwo\nthree\n");
    commit_all(&a, "first");
    // No remote yet.
    assert!(matches!(
        a.push(&a.status().unwrap(), &cancel),
        Err(Error::NoRemote)
    ));
    a.add_remote("origin", remote.to_str().unwrap()).unwrap();
    a.push(&a.status().unwrap(), &cancel).unwrap();
    let st = a.status().unwrap();
    assert_eq!(st.upstream.as_deref(), Some("origin/main"));
    assert_eq!((st.ahead, st.behind), (0, 0));

    let b = git
        .clone_repo(remote.to_str().unwrap(), &tmp.path().join("b"), &cancel)
        .unwrap();
    assert!(
        git.clone_repo(remote.to_str().unwrap(), &tmp.path().join("b"), &cancel)
            .is_err()
    );
    write(&b, "main.tex", "one\nTWO from b\nthree\n");
    commit_all(&b, "b edits");
    b.push(&b.status().unwrap(), &cancel).unwrap();

    write(&a, "main.tex", "one\nTWO from a\nthree\n");
    commit_all(&a, "a edits");
    assert!(matches!(
        a.push(&a.status().unwrap(), &cancel),
        Err(Error::Rejected { .. })
    ));
    a.fetch(&cancel).unwrap();
    let st = a.status().unwrap();
    assert_eq!((st.ahead, st.behind), (1, 1));

    assert!(a.pull(false, &cancel).is_err());
    let st = a.status().unwrap();
    assert_eq!(st.operation, Operation::Merge);
    assert_eq!(st.conflicts().count(), 1);
    assert!(a.merge_message().is_some_and(|m| m.starts_with("Merge")));
    a.abort(Operation::Merge).unwrap();
    let st = a.status().unwrap();
    assert_eq!(st.operation, Operation::None);
    assert!(st.is_clean());

    // Resolve the conflict this time.
    assert!(a.pull(false, &cancel).is_err());
    write(&a, "main.tex", "one\nTWO from both\nthree\n");
    a.stage(&["main.tex".into()]).unwrap();
    let msg = a.merge_message().unwrap();
    a.commit(&msg, CommitOptions::default()).unwrap();
    assert_eq!(a.status().unwrap().operation, Operation::None);
    a.push(&a.status().unwrap(), &cancel).unwrap();

    b.pull(true, &cancel).unwrap();
    assert_eq!(
        std::fs::read_to_string(b.abs_path("main.tex")).unwrap(),
        "one\nTWO from both\nthree\n"
    );
}

#[test]
fn cancel_stops_network_operations() {
    let tmp = tempfile::tempdir().unwrap();
    let Some(git) = git(tmp.path()) else {
        return;
    };
    let cancel = Cancel::new();
    cancel.cancel();
    let err = git
        .clone_repo(
            "https://example.invalid/x.git",
            &tmp.path().join("x"),
            &cancel,
        )
        .unwrap_err();
    assert!(matches!(err, Error::Cancelled), "{err:?}");
}

#[test]
fn sync_commits_rebases_and_pushes() {
    let tmp = tempfile::tempdir().unwrap();
    let Some(git) = git(tmp.path()) else {
        return;
    };
    let cancel = Cancel::new();
    let remote = tmp.path().join("remote.git");
    let bare = std::process::Command::new(git.program())
        .args(["init", "--bare", "--quiet", "-b", "main"])
        .arg(&remote)
        .status()
        .unwrap();
    assert!(bare.success());

    let a = git.init(&tmp.path().join("a")).unwrap();
    write(&a, "main.tex", "one\n");
    commit_all(&a, "first");
    // Nothing to sync with yet.
    assert!(matches!(a.sync("Sync", &cancel), Err(Error::Invalid(_))));
    a.add_remote("origin", remote.to_str().unwrap()).unwrap();
    a.push(&a.status().unwrap(), &cancel).unwrap();
    assert_eq!(a.remote_url("origin").unwrap().as_deref(), remote.to_str());
    assert_eq!(a.remote_url("nope").unwrap(), None);

    let b = git
        .clone_repo(remote.to_str().unwrap(), &tmp.path().join("b"), &cancel)
        .unwrap();
    write(&b, "b.tex", "from b\n");
    commit_all(&b, "b edits");
    b.push(&b.status().unwrap(), &cancel).unwrap();

    // Uncommitted edits and a new file on a, while the remote moved on.
    a.exclude_build_artifacts().unwrap();
    a.exclude_build_artifacts().unwrap();
    let exclude = std::fs::read_to_string(tmp.path().join("a/.git/info/exclude")).unwrap();
    assert_eq!(exclude.matches("*.aux").count(), 1);
    write(&a, "main.tex", "one\ntwo\n");
    write(&a, "a.tex", "from a\n");
    write(&a, "main.aux", "build artifact\n");
    let done = a.sync("Sync", &cancel).unwrap();
    assert_eq!(
        done,
        SyncOutcome {
            committed: true,
            pulled: true,
            pushed: true
        }
    );
    let st = a.status().unwrap();
    assert!(st.is_clean());
    assert_eq!((st.ahead, st.behind), (0, 0));
    let log: Vec<_> = a
        .log(0, 10, None)
        .unwrap()
        .into_iter()
        .map(|c| c.subject)
        .collect();
    assert_eq!(log, ["Sync", "b edits", "first"]);
    assert!(a.file_at("HEAD", "main.aux").is_err());
    assert_eq!(a.sync("Sync", &cancel).unwrap(), SyncOutcome::default());

    // A commit made by hand is pushed too.
    b.pull(true, &cancel).unwrap();
    write(&b, "b.tex", "from b again\n");
    commit_all(&b, "by hand");
    let done = b.sync("Sync", &cancel).unwrap();
    assert!(!done.committed && !done.pulled && done.pushed);
}

#[test]
fn unreachable_remote() {
    let tmp = tempfile::tempdir().unwrap();
    let Some(git) = git(tmp.path()) else {
        return;
    };
    // Nothing listens on the discard port.
    let err = git
        .clone_repo(
            "https://127.0.0.1:9/x.git",
            &tmp.path().join("x"),
            &Cancel::new(),
        )
        .unwrap_err();
    assert!(matches!(err, Error::Unreachable { .. }), "{err:?}");
}

#[test]
fn excluded_build_artifacts_stay_local() {
    let tmp = tempfile::tempdir().unwrap();
    let Some(git) = git(tmp.path()) else {
        return;
    };
    let repo = git.init(&tmp.path().join("c")).unwrap();
    // Like an Overleaf project: no .gitignore.
    std::fs::remove_file(repo.abs_path(".gitignore")).unwrap();
    write(&repo, "main.aux", "build artifact\n");
    write(&repo, ".texoria/build/main.pdf", "pdf\n");
    assert_eq!(repo.status().unwrap().entries.len(), 2);
    repo.exclude_build_artifacts().unwrap();
    assert!(repo.status().unwrap().is_clean());
    assert!(!repo.abs_path(".gitignore").exists());
}
