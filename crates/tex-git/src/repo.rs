//! Operations on one repository.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use crate::parse::{self, Branch, Commit, Operation, Status};
use crate::process::{Cancel, Options};
use crate::{Error, Git, Result};

/// Written by [`Git::init`] when the project has no `.gitignore` yet.
pub(crate) const LATEX_GITIGNORE: &str = "\
# LaTeX build artifacts
*.aux
*.bbl
*.bcf
*.blg
*.fdb_latexmk
*.fls
*.lof
*.log
*.lot
*.nav
*.out
*.run.xml
*.snm
*.synctex.gz
*.toc
*.xdv
.texoria/
";

/// Files that only make sense on this computer: system and editor files,
/// and caches of packages that run external tools.
const LOCAL_FILES: &str = "\
# System and editor files
.DS_Store
Thumbs.db
desktop.ini
*~
*.swp
*.swo
.#*
.vscode/
.idea/
# Package caches
_minted*/
svg-inkscape/
*.synctex(busy)
";

/// Untracked files larger than this are shown as binary.
const MAX_TEXT_PREVIEW: u64 = 8 << 20;

#[derive(Debug, Clone, Copy, Default)]
pub struct CommitOptions {
    pub amend: bool,
    /// Stage every tracked change first (`git commit -a`).
    pub all: bool,
}

/// What [`Repo::sync`] did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SyncOutcome {
    pub committed: bool,
    pub pulled: bool,
    pub pushed: bool,
}

#[derive(Debug, Clone)]
pub struct Repo {
    git: Git,
    root: PathBuf,
    git_dir: PathBuf,
}

impl Repo {
    pub(crate) fn new(git: Git, root: PathBuf, git_dir: PathBuf) -> Repo {
        Repo { git, root, git_dir }
    }

    /// The same repository with [`Git::credential`] set.
    pub fn with_credential(mut self, credential: Option<crate::Credential>) -> Repo {
        self.git = self.git.credential(credential);
        self
    }

    /// The git executable this repository is driven with.
    pub fn git(&self) -> &Git {
        &self.git
    }

    /// The top-level directory of the working tree.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The absolute path of a repository-relative path from git's output.
    pub fn abs_path(&self, rel: &str) -> PathBuf {
        let mut p = self.root.clone();
        p.extend(rel.split('/').filter(|c| !c.is_empty()));
        p
    }

    /// The repository-relative, `/`-separated form of an absolute path.
    pub fn rel_path(&self, abs: &Path) -> Option<String> {
        let abs = crate::plain(abs);
        let rel = abs.strip_prefix(&self.root).ok()?;
        let parts: Vec<_> = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        Some(parts.join("/"))
    }

    fn run<I, S>(&self, args: I) -> Result<Vec<u8>>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.git.run(&self.root, args, Options::default())
    }

    fn run_network<I, S>(&self, args: I, cancel: &Cancel) -> Result<Vec<u8>>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.git.run(
            &self.root,
            args,
            Options {
                timeout: self.git.network_timeout,
                cancel: Some(cancel),
                network: true,
                ..Default::default()
            },
        )
    }

    fn with_paths(args: &[&str], paths: &[String]) -> Vec<OsString> {
        let mut v: Vec<OsString> = args.iter().map(OsString::from).collect();
        v.push("--".into());
        v.extend(paths.iter().map(OsString::from));
        v
    }

    // ── Reading ──────────────────────────────────────────────────────────

    pub fn status(&self) -> Result<Status> {
        let mut cmd = self.git.command(&self.root);
        // A status refresh must not compete with a commit for index.lock.
        cmd.env("GIT_OPTIONAL_LOCKS", "0").args([
            "status",
            "--porcelain=v2",
            "--branch",
            "-z",
            "--untracked-files=all",
        ]);
        let out = crate::process::run(cmd, Options::default())?;
        if !out.success {
            return Err(crate::classify("status".into(), out.stderr));
        }
        let mut status = parse::status(&out.stdout);
        status.operation = self.operation();
        Ok(status)
    }

    /// The merge-like operation in progress, from the files git leaves in
    /// its directory while one is stopped.
    pub fn operation(&self) -> Operation {
        let d = &self.git_dir;
        if d.join("rebase-merge").is_dir() || d.join("rebase-apply").is_dir() {
            Operation::Rebase
        } else if d.join("MERGE_HEAD").is_file() {
            Operation::Merge
        } else if d.join("CHERRY_PICK_HEAD").is_file() {
            Operation::CherryPick
        } else if d.join("REVERT_HEAD").is_file() {
            Operation::Revert
        } else {
            Operation::None
        }
    }

    /// The message git prepared for the commit concluding a merge, without
    /// its comment lines.
    pub fn merge_message(&self) -> Option<String> {
        let text = std::fs::read_to_string(self.git_dir.join("MERGE_MSG")).ok()?;
        let msg: Vec<&str> = text.lines().filter(|l| !l.starts_with('#')).collect();
        Some(msg.join("\n").trim().to_string()).filter(|m| !m.is_empty())
    }

    /// The patch of a tracked file: working tree vs index, or index vs HEAD
    /// when `staged`.
    pub fn diff(&self, path: &str, staged: bool) -> Result<String> {
        let mut args = vec![
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "-M",
            "--src-prefix=a/",
            "--dst-prefix=b/",
        ];
        if staged {
            args.push("--cached");
        }
        let out = self.run(Self::with_paths(&args, &[path.to_string()]))?;
        Ok(String::from_utf8_lossy(&out).into_owned())
    }

    /// The patch adding an untracked file.
    pub fn untracked_diff(&self, path: &str) -> Result<String> {
        let abs = self.abs_path(path);
        let meta = std::fs::metadata(&abs).map_err(Error::Spawn)?;
        let bytes = if meta.is_file() && meta.len() <= MAX_TEXT_PREVIEW {
            std::fs::read(&abs).map_err(Error::Spawn)?
        } else {
            vec![0]
        };
        Ok(untracked_patch(path, &bytes))
    }

    /// Commits, newest first, `skip`ping the first ones. With `path`, only
    /// those touching it.
    pub fn log(&self, skip: usize, limit: usize, path: Option<&str>) -> Result<Vec<Commit>> {
        let mut args: Vec<OsString> = vec![
            "log".into(),
            "-z".into(),
            parse::LOG_FORMAT.into(),
            format!("--skip={skip}").into(),
            format!("--max-count={limit}").into(),
        ];
        if let Some(p) = path {
            args.push("--".into());
            args.push(p.into());
        }
        match self.run(args) {
            Ok(out) => Ok(parse::log(&out)),
            // No commits yet.
            Err(Error::Failed { output, .. })
                if output.contains("does not have any commits")
                    || output.contains("bad default revision") =>
            {
                Ok(Vec::new())
            }
            Err(e) => Err(e),
        }
    }

    /// The patch a commit introduced (against its first parent).
    pub fn show(&self, sha: &str) -> Result<String> {
        check_rev(sha)?;
        let out = self.run([
            "diff-tree",
            "-p",
            "-M",
            "--root",
            "-m",
            "--first-parent",
            "--no-commit-id",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "--src-prefix=a/",
            "--dst-prefix=b/",
            sha,
        ])?;
        Ok(String::from_utf8_lossy(&out).into_owned())
    }

    /// A file's contents at a commit.
    pub fn file_at(&self, sha: &str, path: &str) -> Result<Vec<u8>> {
        check_rev(sha)?;
        self.run(["cat-file", "blob", &format!("{sha}:{path}")])
    }

    pub fn head_message(&self) -> Result<String> {
        let out = self.run(["log", "-1", "--format=%B"])?;
        Ok(String::from_utf8_lossy(&out).trim().to_string())
    }

    pub fn branches(&self) -> Result<Vec<Branch>> {
        let out = self.run([
            "for-each-ref",
            parse::BRANCH_FORMAT,
            "refs/heads",
            "refs/remotes",
        ])?;
        Ok(parse::branches(&out))
    }

    pub fn remotes(&self) -> Result<Vec<String>> {
        let out = self.run(["remote"])?;
        Ok(String::from_utf8_lossy(&out)
            .lines()
            .map(str::to_string)
            .collect())
    }

    /// The URL of remote `name`, or `None` if there is no such remote.
    pub fn remote_url(&self, name: &str) -> Result<Option<String>> {
        if name.is_empty() || name.starts_with('-') {
            return Ok(None);
        }
        match self.run(["remote", "get-url", name]) {
            Ok(out) => Ok(Some(String::from_utf8_lossy(&out).trim().to_string())),
            Err(Error::Failed { output, .. }) if output.contains("No such remote") => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// The commit `rev` names, or `None` if it does not exist.
    fn resolve(&self, rev: &str) -> Result<Option<String>> {
        match self.run(["rev-parse", "--verify", "--quiet", rev]) {
            Ok(out) => Ok(Some(String::from_utf8_lossy(&out).trim().to_string())),
            Err(Error::Failed { .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    // ── Changing the working tree and index ─────────────────────────────

    /// Keep LaTeX build artifacts, system and editor files out of commits
    /// through `.git/info/exclude`, for projects where a `.gitignore` would
    /// be unwelcome (it would be pushed to the other collaborators, on
    /// Overleaf say). Adds only the patterns that are missing.
    pub fn exclude_local_files(&self) -> Result<()> {
        let path = self.git_dir.join("info").join("exclude");
        let mut text = std::fs::read_to_string(&path).unwrap_or_default();
        let present: Vec<&str> = text.lines().map(str::trim).collect();
        let missing: Vec<&str> = LATEX_GITIGNORE
            .lines()
            .chain(LOCAL_FILES.lines())
            .filter(|l| !l.starts_with('#') && !l.is_empty() && !present.contains(l))
            .collect();
        if missing.is_empty() {
            return Ok(());
        }
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str("# Local files, kept out of commits by TeXoria\n");
        for pattern in missing {
            text.push_str(pattern);
            text.push('\n');
        }
        let write = |path: &Path| {
            std::fs::create_dir_all(path.parent().unwrap_or(path))?;
            std::fs::write(path, &text)
        };
        write(&path).map_err(|e| Error::Invalid(format!("writing {}: {e}", path.display())))
    }

    pub fn stage(&self, paths: &[String]) -> Result<()> {
        self.run(Self::with_paths(&["add", "-A"], paths)).map(drop)
    }

    pub fn unstage(&self, paths: &[String], has_head: bool) -> Result<()> {
        let args: &[&str] = if has_head {
            &["restore", "--staged"]
        } else {
            // Nothing to restore from before the first commit.
            &["rm", "--cached", "--quiet", "-r"]
        };
        self.run(Self::with_paths(args, paths)).map(drop)
    }

    /// Throw away working-tree changes to tracked files.
    pub fn discard(&self, paths: &[String]) -> Result<()> {
        self.run(Self::with_paths(&["restore", "--worktree"], paths))
            .map(drop)
    }

    /// Delete untracked files. Nothing is deleted if one of `paths` is not a
    /// file of the working tree.
    pub fn delete_untracked(&self, paths: &[String]) -> Result<()> {
        let files = paths
            .iter()
            .map(|p| {
                self.deletable(p)
                    .ok_or_else(|| Error::Invalid(format!("refusing to delete {p}")))
            })
            .collect::<Result<Vec<_>>>()?;
        for file in files {
            std::fs::remove_file(file).map_err(Error::Spawn)?;
        }
        Ok(())
    }

    /// The working-tree file `rel` names, if it is one: only plain names
    /// (nothing absolute, no `..`, which `\` also separates on Windows),
    /// nothing in a `.git` folder, and no folder on the way that is a link
    /// to elsewhere.
    fn deletable(&self, rel: &str) -> Option<PathBuf> {
        let mut path = self.root.clone();
        let mut names = Path::new(rel).components().peekable();
        names.peek()?;
        while let Some(c) = names.next() {
            let std::path::Component::Normal(name) = c else {
                return None;
            };
            if is_git_dir(&name.to_string_lossy()) {
                return None;
            }
            path.push(name);
            if names.peek().is_some() && !std::fs::symlink_metadata(&path).ok()?.is_dir() {
                return None;
            }
        }
        Some(path)
    }

    pub fn commit(&self, message: &str, opts: CommitOptions) -> Result<()> {
        if message.trim().is_empty() {
            return Err(Error::Invalid("Write a commit message first".into()));
        }
        let mut args = vec!["commit", "--quiet", "--cleanup=strip", "-F", "-"];
        if opts.amend {
            args.push("--amend");
        }
        if opts.all {
            args.push("--all");
        }
        self.git
            .run(
                &self.root,
                args,
                Options {
                    stdin: Some(message.as_bytes()),
                    // Hooks or signing may hang waiting for input.
                    timeout: self.git.network_timeout,
                    ..Default::default()
                },
            )
            .map(drop)
    }

    pub fn set_identity(&self, name: &str, email: &str, global: bool) -> Result<()> {
        for (key, value) in [("user.name", name), ("user.email", email)] {
            let mut args = vec!["config"];
            if global {
                args.push("--global");
            }
            args.extend([key, value]);
            self.run(args)?;
        }
        Ok(())
    }

    /// Restore the working-tree copy of `path` to its version at `sha`.
    pub fn restore_file(&self, sha: &str, path: &str) -> Result<()> {
        check_rev(sha)?;
        self.run(Self::with_paths(
            &["restore", &format!("--source={sha}"), "--worktree"],
            &[path.to_string()],
        ))
        .map(drop)
    }

    // ── Branches ─────────────────────────────────────────────────────────

    /// Check out a branch; a remote-tracking one gets a local branch.
    pub fn switch(&self, branch: &Branch) -> Result<()> {
        check_rev(&branch.name)?;
        if branch.remote {
            self.run(["switch", "--track", &branch.name]).map(drop)
        } else {
            self.run(["switch", &branch.name]).map(drop)
        }
    }

    pub fn create_branch(&self, name: &str) -> Result<()> {
        let name = name.trim();
        if name.starts_with('-') || self.run(["check-ref-format", "--branch", name]).is_err() {
            return Err(Error::Invalid(format!(
                "“{name}” is not a valid branch name"
            )));
        }
        self.run(["switch", "-c", name]).map(drop)
    }

    pub fn add_remote(&self, name: &str, url: &str) -> Result<()> {
        let (name, url) = (name.trim(), url.trim());
        if name.is_empty() || name.starts_with('-') || url.is_empty() || url.starts_with('-') {
            return Err(Error::Invalid("Enter a remote name and URL".into()));
        }
        self.run(["remote", "add", name, url]).map(drop)
    }

    // ── Remotes ──────────────────────────────────────────────────────────

    pub fn fetch(&self, cancel: &Cancel) -> Result<()> {
        self.run_network(["fetch", "--prune", "--quiet"], cancel)
            .map(drop)
    }

    pub fn pull(&self, rebase: bool, cancel: &Cancel) -> Result<()> {
        let mode = if rebase { "--rebase" } else { "--no-rebase" };
        self.run_network(["pull", "--no-edit", "--quiet", mode], cancel)
            .map(drop)
    }

    /// Push the current branch, publishing it (and setting its upstream)
    /// if it has none yet.
    pub fn push(&self, status: &Status, cancel: &Cancel) -> Result<()> {
        if status.upstream.is_some() {
            return self.run_network(["push", "--quiet"], cancel).map(drop);
        }
        let remotes = self.remotes()?;
        let remote = remotes
            .iter()
            .find(|r| *r == "origin")
            .or(remotes.first())
            .ok_or(Error::NoRemote)?;
        if status.branch.is_none() {
            return Err(Error::Invalid("Check out a branch to push it".into()));
        }
        self.run_network(["push", "--quiet", "-u", remote, "HEAD"], cancel)
            .map(drop)
    }

    /// Commit every change with `message`, rebase onto the upstream and
    /// push: one step that leaves the branch equal to its upstream.
    pub fn sync(&self, message: &str, cancel: &Cancel) -> Result<SyncOutcome> {
        let status = self.status()?;
        if status.operation != Operation::None {
            return Err(Error::Invalid(format!(
                "Finish the {} first",
                status.operation.label().to_lowercase()
            )));
        }
        if status.conflicts().next().is_some() {
            return Err(Error::Invalid("Resolve the conflicts first".into()));
        }
        if status.branch.is_none() {
            return Err(Error::Invalid("Check out a branch to sync it".into()));
        }
        if status.upstream.is_none() {
            return Err(Error::Invalid(
                "The branch has no upstream to sync with".into(),
            ));
        }
        let mut outcome = SyncOutcome::default();
        if !status.is_clean() {
            self.stage(&[".".to_string()])?;
            self.commit(message, CommitOptions::default())?;
            outcome.committed = true;
        }
        let upstream = self.resolve("@{upstream}")?;
        self.pull(true, cancel)?;
        outcome.pulled = self.resolve("@{upstream}")? != upstream;
        if self.status()?.ahead > 0 {
            self.run_network(["push", "--quiet"], cancel)?;
            outcome.pushed = true;
        }
        Ok(outcome)
    }

    /// Stop the merge, rebase, cherry-pick or revert in progress.
    pub fn abort(&self, op: Operation) -> Result<()> {
        let cmd = match op {
            Operation::None => return Ok(()),
            Operation::Merge => "merge",
            Operation::Rebase => "rebase",
            Operation::CherryPick => "cherry-pick",
            Operation::Revert => "revert",
        };
        self.run([cmd, "--abort"]).map(drop)
    }

    /// Resume a rebase once its conflicts are staged.
    pub fn continue_rebase(&self) -> Result<()> {
        let mut cmd = self.git.command(&self.root);
        // Keep the commit messages instead of opening an editor.
        cmd.env("GIT_EDITOR", "true").args(["rebase", "--continue"]);
        let out = crate::process::run(cmd, Options::default())?;
        if out.success {
            Ok(())
        } else {
            Err(crate::classify("rebase".into(), out.stderr))
        }
    }
}

/// A revision from our own UI must never be taken as an option.
fn check_rev(rev: &str) -> Result<()> {
    if rev.is_empty() || rev.starts_with('-') {
        Err(Error::Invalid(format!("invalid revision {rev:?}")))
    } else {
        Ok(())
    }
}

/// Whether a file name may stand for `.git` on some file system: in any case
/// (macOS, Windows), with the trailing dots and spaces Windows drops, as an
/// NTFS stream (`.git::$INDEX_ALLOCATION`) or a short name (`GIT~1`).
pub fn is_git_dir(name: &str) -> bool {
    let name = name.trim_end_matches(['.', ' ']).to_ascii_lowercase();
    name == ".git" || name.starts_with(".git:") || name == "git~1"
}

/// A git-style patch adding `path` with `contents`, for untracked files.
pub fn untracked_patch(path: &str, contents: &[u8]) -> String {
    let mut out = format!("diff --git a/{path} b/{path}\nnew file mode 100644\n");
    let probe = &contents[..contents.len().min(8000)];
    match std::str::from_utf8(contents) {
        Ok(text) if !probe.contains(&0) => {
            if text.is_empty() {
                return out;
            }
            out.push_str(&format!("--- /dev/null\n+++ b/{path}\n"));
            let diff = similar::TextDiff::from_lines("", text);
            for hunk in diff.unified_diff().context_radius(3).iter_hunks() {
                out.push_str(&hunk.to_string());
            }
        }
        _ => out.push_str(&format!("Binary files /dev/null and b/{path} differ\n")),
    }
    out
}

/// The directory name `git clone` would pick for `url`.
pub fn clone_dir_name(url: &str) -> Option<String> {
    let url = url.trim().trim_end_matches('/');
    let last = url.rsplit(['/', ':', '\\']).next()?;
    let name = last.strip_suffix(".git").unwrap_or(last);
    (!name.is_empty() && name != "." && name != "..").then(|| name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn untracked_text_patch() {
        let p = untracked_patch("ch/intro.tex", b"a\nb");
        assert!(p.starts_with("diff --git a/ch/intro.tex b/ch/intro.tex\nnew file mode 100644\n"));
        assert!(p.contains("--- /dev/null\n+++ b/ch/intro.tex\n@@ -0,0 +1,2 @@\n+a\n+b"));
        assert!(p.contains("\\ No newline at end of file"));
    }

    #[test]
    fn untracked_binary_and_empty_patch() {
        assert!(untracked_patch("f.pdf", b"%PDF\0\x01").contains("Binary files /dev/null"));
        assert!(!untracked_patch("e.tex", b"").contains("@@"));
    }

    #[test]
    fn clone_names() {
        assert_eq!(
            clone_dir_name("https://github.com/u/paper.git").as_deref(),
            Some("paper")
        );
        assert_eq!(
            clone_dir_name("git@github.com:u/thesis").as_deref(),
            Some("thesis")
        );
        assert_eq!(
            clone_dir_name("https://git.overleaf.com/64ab12/").as_deref(),
            Some("64ab12")
        );
        assert_eq!(clone_dir_name(""), None);
    }

    #[test]
    fn names_of_the_git_folder() {
        for name in [
            ".git",
            ".GIT",
            ".Git.",
            ".git ",
            ".git. .",
            ".git::$INDEX_ALLOCATION",
            "GIT~1",
        ] {
            assert!(is_git_dir(name), "{name}");
        }
        for name in [".gitignore", ".github", "git", "a.git", ".git-x", "x.git."] {
            assert!(!is_git_dir(name), "{name}");
        }
    }

    #[test]
    fn paths_round_trip() {
        let repo = Repo::new(
            Git::at("git"),
            PathBuf::from("/r"),
            PathBuf::from("/r/.git"),
        );
        let abs = repo.abs_path("a/b c.tex");
        assert_eq!(abs, Path::new("/r").join("a").join("b c.tex"));
        assert_eq!(repo.rel_path(&abs).as_deref(), Some("a/b c.tex"));
        assert_eq!(repo.rel_path(Path::new("/elsewhere")), None);
    }

    #[test]
    fn untracked_patch_details() {
        // Text that is not UTF-8 is binary; like git, only the first 8000
        // bytes are looked at for a NUL.
        assert!(untracked_patch("a.tex", b"caf\xe9\n").contains("Binary files"));
        let mut late = vec![b'a'; 8000];
        late.push(0);
        assert!(untracked_patch("a.tex", &late).contains("@@ -0,0 +1 @@"));
        late.insert(0, 0);
        assert!(untracked_patch("a.tex", &late).contains("Binary files"));
        let p = untracked_patch("a.tex", b"x\ny\n");
        assert!(p.ends_with("+x\n+y\n"), "{p}");
        assert!(!p.contains("No newline"));
    }

    #[test]
    fn more_clone_names() {
        for (url, want) in [
            ("https://example.org/a/b.git/", Some("b")),
            ("  /srv/git/paper.git  ", Some("paper")),
            (r"C:\repos\paper", Some("paper")),
            ("file:///srv/x", Some("x")),
            ("host:paper.git", Some("paper")),
            ("https://example.org/..", None),
            ("https://example.org/.git", None),
            ("https://example.org/..git", None),
            ("/", None),
        ] {
            assert_eq!(clone_dir_name(url).as_deref(), want, "{url}");
        }
    }

    #[test]
    fn rel_path_of_the_root_and_outside() {
        let repo = Repo::new(
            Git::at("git"),
            PathBuf::from("/r"),
            PathBuf::from("/r/.git"),
        );
        assert_eq!(repo.rel_path(Path::new("/r")).as_deref(), Some(""));
        assert_eq!(repo.rel_path(Path::new("/rr/a")), None);
        assert_eq!(repo.abs_path(""), Path::new("/r"));
        assert_eq!(repo.abs_path("a//b/"), Path::new("/r").join("a").join("b"));
        assert_eq!(repo.git().program(), Path::new("git"));
        assert_eq!(repo.root(), Path::new("/r"));
        let with = repo.with_credential(Some(crate::Credential {
            url: "https://x".into(),
            username: "u".into(),
            password: "p".into(),
        }));
        assert!(with.git().credential.is_some());
    }

    mod props {
        use proptest::prelude::*;

        use super::*;

        proptest! {
            #[test]
            fn clone_names_are_single_names(url in "\\PC{0,40}") {
                if let Some(name) = clone_dir_name(&url) {
                    prop_assert!(!name.is_empty() && name != "." && name != "..");
                    prop_assert!(!name.contains(['/', '\\', ':']));
                }
            }

            #[test]
            fn untracked_patches_never_panic(
                path in "[^\0\n]{1,20}",
                contents in prop::collection::vec(any::<u8>(), 0..300),
            ) {
                let p = untracked_patch(&path, &contents);
                let head = format!("diff --git a/{path} b/{path}\nnew file mode 100644\n");
                prop_assert!(p.starts_with(&head));
            }

            #[test]
            fn text_patches_add_every_line(lines in prop::collection::vec("[a-z ]{0,10}", 1..20)) {
                let text = lines.join("\n") + "\n";
                let p = untracked_patch("a.tex", text.as_bytes());
                let added: Vec<&str> = p.lines().filter_map(|l| l.strip_prefix('+')).skip(1).collect();
                prop_assert_eq!(added, lines);
            }

            #[test]
            fn paths_round_trip(parts in prop::collection::vec("[a-zA-Z0-9 _é-][a-zA-Z0-9 ._é-]{0,8}", 1..5)) {
                let repo = Repo::new(Git::at("git"), PathBuf::from("/r"), PathBuf::from("/r/.git"));
                let rel = parts.join("/");
                let abs = repo.abs_path(&rel);
                prop_assert!(abs.starts_with("/r"));
                prop_assert_eq!(repo.rel_path(&abs), Some(rel));
            }

            #[test]
            fn only_plain_names_are_deletable(rel in "[a-zA-Z./\\\\:~ ]{0,16}") {
                let repo = Repo::new(Git::at("git"), PathBuf::from("/nonexistent"), PathBuf::from("/nonexistent/.git"));
                if let Some(path) = repo.deletable(&rel) {
                    prop_assert!(path.starts_with("/nonexistent"));
                    let names: Vec<_> = path.strip_prefix("/nonexistent").unwrap().components().collect();
                    // No folder on the way exists here: a single name only.
                    prop_assert_eq!(names.len(), 1);
                    let std::path::Component::Normal(name) = names[0] else {
                        return Err(TestCaseError::fail(format!("{path:?}")));
                    };
                    prop_assert!(!is_git_dir(&name.to_string_lossy()));
                }
            }
        }
    }
}
