//! Git through the system `git` executable.
//!
//! The CLI (rather than a library) keeps the user's setup working as is:
//! credential helpers, ssh-agent, signing and hooks. Every command runs
//! without a terminal (`GIT_TERMINAL_PROMPT=0`, no controlling TTY), so an
//! operation that would need to ask for a password fails instead of hanging.
//!
//! A repository's own configuration can run programs even for a read-only
//! `git status` (`core.fsmonitor`, filter drivers, …): callers must only use a
//! [`Repo`] for projects the user trusts.

mod parse;
mod process;
mod repo;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

pub use parse::{Branch, Change, Commit, Entry, Operation, Status};
pub use process::Cancel;
pub use repo::{CommitOptions, Repo, clone_dir_name, untracked_patch};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("cannot run git: {0}")]
    Spawn(#[source] std::io::Error),
    #[error("{}", parse::summary(.output).unwrap_or_else(|| format!("git {} failed", .command)))]
    Failed { command: String, output: String },
    #[error("Git needs your name and email to record commits")]
    NoIdentity,
    #[error("The remote has changes you don't have yet: pull first")]
    Rejected { output: String },
    #[error("Authentication failed: set up a credential helper or an SSH key for this remote")]
    Auth { output: String },
    #[error("No remote repository is configured")]
    NoRemote,
    #[error("Cancelled")]
    Cancelled,
    #[error("Timed out after {0} s")]
    TimedOut(u64),
    #[error("{0}")]
    Invalid(String),
}

impl Error {
    /// git's full output, for a "details" view.
    pub fn details(&self) -> Option<&str> {
        match self {
            Error::Failed { output, .. } | Error::Rejected { output } | Error::Auth { output } => {
                Some(output)
            }
            _ => None,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// A git executable and the environment it runs with.
#[derive(Debug, Clone)]
pub struct Git {
    program: PathBuf,
    env: Vec<(OsString, OsString)>,
    network_timeout: Option<Duration>,
}

impl Git {
    /// Locate git: `PATH`, then well-known install dirs a GUI app's `PATH`
    /// may lack. Runs it once to check that it works.
    pub fn find() -> Option<Git> {
        let program = which::which("git").ok().or_else(|| {
            let exe = format!("git{}", std::env::consts::EXE_SUFFIX);
            fallback_dirs()
                .into_iter()
                .map(|d| d.join(&exe))
                .find(|p| p.is_file())
        })?;
        if cfg!(target_os = "macos") && program == Path::new("/usr/bin/git") && !has_clt() {
            // The stub would pop up the Command Line Tools installer.
            return None;
        }
        let git = Git::at(program);
        git.version().ok()?;
        Some(git)
    }

    pub fn at(program: impl Into<PathBuf>) -> Git {
        Git {
            program: program.into(),
            env: Vec::new(),
            network_timeout: Some(Duration::from_secs(120)),
        }
    }

    /// An extra environment variable for every command (tests isolate the
    /// user's global configuration this way).
    pub fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Git {
        self.env.push((key.into(), value.into()));
        self
    }

    /// Kill fetch/pull/push after this long (`None`: never).
    pub fn network_timeout(mut self, timeout: Option<Duration>) -> Git {
        self.network_timeout = timeout;
        self
    }

    pub fn program(&self) -> &Path {
        &self.program
    }

    pub fn version(&self) -> Result<String> {
        let out = self.run(Path::new("."), ["--version"], Default::default())?;
        Ok(String::from_utf8_lossy(&out).trim().to_string())
    }

    /// The repository containing `dir`, or `None` if it is not in one.
    pub fn discover(&self, dir: &Path) -> Result<Option<Repo>> {
        let out = self.run(
            dir,
            ["rev-parse", "--show-toplevel", "--absolute-git-dir"],
            Default::default(),
        );
        let out = match out {
            Ok(out) => out,
            Err(Error::Failed { output, .. })
                if output.contains("not a git repository")
                    || output.contains("must be run in a work tree") =>
            {
                return Ok(None);
            }
            Err(e) => return Err(e),
        };
        let text = String::from_utf8_lossy(&out);
        let mut lines = text.lines();
        let (Some(root), Some(git_dir)) = (lines.next(), lines.next()) else {
            return Ok(None);
        };
        Ok(Some(Repo::new(self.clone(), canon(root), canon(git_dir))))
    }

    /// `git init` in `dir` (created if needed), adding a LaTeX `.gitignore`
    /// unless there is one.
    pub fn init(&self, dir: &Path) -> Result<Repo> {
        std::fs::create_dir_all(dir).map_err(Error::Spawn)?;
        self.run(dir, ["init", "--quiet"], Default::default())?;
        let ignore = dir.join(".gitignore");
        if !ignore.exists()
            && let Err(e) = std::fs::write(&ignore, repo::LATEX_GITIGNORE)
        {
            log::warn!("writing {}: {e}", ignore.display());
        }
        self.discover(dir)?
            .ok_or_else(|| Error::Invalid("the new repository was not found".into()))
    }

    /// `git clone <url> <dest>`.
    pub fn clone_repo(&self, url: &str, dest: &Path, cancel: &Cancel) -> Result<Repo> {
        let url = url.trim();
        if url.is_empty() || url.starts_with('-') {
            return Err(Error::Invalid("Not a repository URL".into()));
        }
        if dest.exists() {
            return Err(Error::Invalid(format!("{} already exists", dest.display())));
        }
        let parent = dest
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let args: [&std::ffi::OsStr; 4] = [
            "clone".as_ref(),
            "--".as_ref(),
            url.as_ref(),
            dest.as_os_str(),
        ];
        self.run(
            parent,
            args,
            process::Options {
                // A large clone takes a while: only an explicit cancel stops it.
                timeout: None,
                cancel: Some(cancel),
                ..Default::default()
            },
        )?;
        self.discover(dest)?
            .ok_or_else(|| Error::Invalid("the cloned repository was not found".into()))
    }

    fn command(&self, dir: &Path) -> Command {
        let mut c = Command::new(&self.program);
        c.current_dir(dir)
            .args([
                "--no-pager",
                "-c",
                "color.ui=false",
                "-c",
                "core.quotepath=false",
            ])
            .env("GIT_TERMINAL_PROMPT", "0")
            // Paths come from git itself: `[` or `*` in a name is literal.
            .env("GIT_LITERAL_PATHSPECS", "1")
            // Errors are matched (and shown) in English.
            .env("LC_MESSAGES", "C")
            .env("LANGUAGE", "C")
            .env_remove("LC_ALL")
            // Inherited from a parent git process, these would redirect us.
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE");
        for (k, v) in &self.env {
            c.env(k, v);
        }
        c
    }

    fn run<I, S>(&self, dir: &Path, args: I, opts: process::Options) -> Result<Vec<u8>>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        let mut cmd = self.command(dir);
        let args: Vec<OsString> = args.into_iter().map(|a| a.as_ref().to_owned()).collect();
        cmd.args(&args);
        let out = process::run(cmd, opts)?;
        if out.success {
            return Ok(out.stdout);
        }
        // Some failures ("nothing to commit") are only reported on stdout.
        let mut output = out.stderr;
        if output.trim().is_empty() {
            output = String::from_utf8_lossy(&out.stdout).into_owned();
        }
        let command = args
            .iter()
            .map(|a| a.to_string_lossy())
            .find(|a| !a.starts_with('-'))
            .unwrap_or_default()
            .into_owned();
        Err(classify(command, output))
    }
}

fn classify(command: String, output: String) -> Error {
    let has = |s: &str| output.contains(s);
    if has("Author identity unknown")
        || has("Please tell me who you are")
        || has("empty ident name")
    {
        Error::NoIdentity
    } else if command == "push"
        && (has("[rejected]") || has("non-fast-forward") || has("fetch first"))
    {
        Error::Rejected { output }
    } else if has("Authentication failed")
        || has("could not read Username")
        || has("could not read Password")
        || has("terminal prompts disabled")
        || has("Permission denied (publickey")
    {
        Error::Auth { output }
    } else {
        Error::Failed { command, output }
    }
}

fn canon(path: &str) -> PathBuf {
    let p = PathBuf::from(path);
    std::fs::canonicalize(&p).unwrap_or(p)
}

fn fallback_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if cfg!(target_os = "macos") {
        dirs.push(PathBuf::from("/opt/homebrew/bin"));
    }
    if cfg!(unix) {
        dirs.push(PathBuf::from("/usr/local/bin"));
        dirs.push(PathBuf::from("/usr/bin"));
    }
    if cfg!(windows) {
        for var in ["ProgramFiles", "ProgramW6432", "LOCALAPPDATA"] {
            if let Some(base) = std::env::var_os(var) {
                let base = PathBuf::from(base);
                dirs.push(base.join("Git").join("cmd"));
                dirs.push(base.join("Programs").join("Git").join("cmd"));
            }
        }
    }
    dirs
}

/// Whether the macOS developer tools are installed (`xcode-select -p` only
/// prints a path, it never prompts).
fn has_clt() -> bool {
    Command::new("/usr/bin/xcode-select")
        .arg("-p")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}
