//! Shared by the integration tests: git with the user's own configuration out
//! of the way, and temporary repositories to work on.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Output;

use tex_git::{CommitOptions, Git, Repo};

/// A temporary folder (also git's `HOME`) and git isolated in it.
pub struct Sandbox {
    pub tmp: tempfile::TempDir,
    pub git: Git,
    raw: test_support::git::Isolated,
}

impl Sandbox {
    /// `None` (the test then passes trivially) when git is not installed.
    pub fn new() -> Option<Sandbox> {
        let Some(found) = Git::find() else {
            test_support::skip("GIT", "git not found");
            return None;
        };
        let tmp = tempfile::tempdir().unwrap();
        let raw = test_support::git::Isolated::new(found.program(), tmp.path());
        let git = (raw.env().iter()).fold(Git::at(found.program()), |git, (k, v)| git.env(k, v));
        Some(Sandbox { tmp, git, raw })
    }

    pub fn path(&self, rel: &str) -> PathBuf {
        self.tmp.path().join(rel)
    }

    /// A new repository in `name`.
    pub fn init(&self, name: &str) -> Repo {
        self.git.init(&self.path(name)).unwrap()
    }

    /// A new repository in `name` with one commit of `main.tex`.
    pub fn committed(&self, name: &str) -> Repo {
        let repo = self.init(name);
        write(&repo, "main.tex", "one\ntwo\nthree\n");
        commit_all(&repo, "first");
        repo
    }

    /// A new bare repository in `name`, to push to.
    pub fn bare(&self, name: &str) -> PathBuf {
        let path = self.path(name);
        self.ok(
            self.tmp.path(),
            &["init", "--bare", "--quiet", "-b", "main", name],
        );
        path
    }

    /// git run directly in `dir`, for states the crate does not create itself.
    pub fn raw(&self, dir: &Path, args: &[&str]) -> Output {
        self.raw.raw(dir, args)
    }

    /// [`Sandbox::raw`], which must succeed: its standard output.
    #[track_caller]
    pub fn ok(&self, dir: &Path, args: &[&str]) -> String {
        self.raw.ok(dir, args)
    }
}

pub fn write(repo: &Repo, rel: &str, text: &str) {
    test_support::write(repo.root(), rel, text);
}

pub fn read(repo: &Repo, rel: &str) -> String {
    std::fs::read_to_string(repo.abs_path(rel)).unwrap()
}

pub fn commit_all(repo: &Repo, msg: &str) {
    repo.stage(&[".".into()]).unwrap();
    repo.commit(msg, CommitOptions::default()).unwrap();
}

/// A shell script as the repository's hook `name` (git runs hooks with sh on
/// Windows too).
pub fn hook(repo: &Repo, name: &str, script: &str) {
    let path = repo.root().join(".git").join("hooks").join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}
