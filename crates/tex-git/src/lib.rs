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

pub mod overleaf;
mod parse;
mod process;
mod repo;

use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

pub use parse::{Branch, Change, Commit, Entry, Operation, Status};
pub use process::Cancel;
pub use repo::{CommitOptions, Repo, SyncOutcome, clone_dir_name, untracked_patch};

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
    #[error("Cannot reach the remote: check your connection")]
    Unreachable { output: String },
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
            Error::Failed { output, .. }
            | Error::Rejected { output }
            | Error::Auth { output }
            | Error::Unreachable { output } => Some(output),
            _ => None,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// A username and password for the remotes under one URL prefix
/// (`https://host`), given to git in place of the user's credential helpers.
#[derive(Clone, PartialEq, Eq)]
pub struct Credential {
    pub url: String,
    pub username: String,
    pub password: String,
}

impl Credential {
    /// The credential protocol is line based: a line break in a value
    /// would let it add fields of its own.
    fn is_safe(&self) -> bool {
        let clean = |s: &str| !s.is_empty() && !s.chars().any(char::is_control);
        clean(&self.url) && clean(&self.username) && clean(&self.password)
    }
}

impl fmt::Debug for Credential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credential")
            .field("url", &self.url)
            .field("username", &self.username)
            .finish_non_exhaustive()
    }
}

/// A credential helper that answers `get` from the environment, so the
/// password never appears in a command line, a URL or a config file.
const CREDENTIAL_HELPER: &str = "!f() { if test \"$1\" = get; then \
    printf 'username=%s\\npassword=%s\\n' \"$TEXORIA_GIT_USERNAME\" \"$TEXORIA_GIT_PASSWORD\"; fi; }; f";

/// A git executable and the environment it runs with.
#[derive(Debug, Clone)]
pub struct Git {
    program: PathBuf,
    env: Vec<(OsString, OsString)>,
    network_timeout: Option<Duration>,
    credential: Option<Credential>,
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
            credential: None,
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

    /// Credentials for fetch, pull, push and clone from `credential.url`;
    /// other remotes keep using the user's helpers.
    pub fn credential(mut self, credential: Option<Credential>) -> Git {
        self.credential = credential;
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

    /// The repository whose working tree is exactly `top`, which the user
    /// trusts. Git never looks above it (`GIT_CEILING_DIRECTORIES`): a `.git`
    /// that git does not accept there must not hand the project over to a
    /// repository in a parent folder, whose configuration nobody trusted.
    pub fn discover_at(&self, top: &Path) -> Result<Option<Repo>> {
        let mut git = self.clone();
        if let Some(parent) = top.parent()
            && let Ok(ceiling) = std::env::join_paths([parent])
        {
            git = git.env("GIT_CEILING_DIRECTORIES", ceiling);
        }
        let top = canon(&top.to_string_lossy());
        match git.discover(&top)? {
            Some(repo) if repo.root() == top => Ok(Some(repo)),
            Some(repo) => Err(Error::Invalid(format!(
                "Git found the repository at {}, not at {}",
                repo.root().display(),
                top.display()
            ))),
            None => Ok(None),
        }
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
        let target = plain(dest);
        let args: [&std::ffi::OsStr; 4] = [
            "clone".as_ref(),
            "--".as_ref(),
            url.as_ref(),
            target.as_os_str(),
        ];
        self.run(
            parent,
            args,
            process::Options {
                // A large clone takes a while: only an explicit cancel stops it.
                timeout: None,
                cancel: Some(cancel),
                network: true,
                ..Default::default()
            },
        )?;
        self.discover(dest)?
            .ok_or_else(|| Error::Invalid("the cloned repository was not found".into()))
    }

    fn command(&self, dir: &Path) -> Command {
        let mut c = Command::new(&self.program);
        c.current_dir(plain(dir))
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
        if opts.network
            && let Some(c) = &self.credential
        {
            if !c.is_safe() {
                return Err(Error::Invalid("The saved credentials are not valid".into()));
            }
            // An empty value drops the helpers configured so far for this URL.
            let key = format!("credential.{}.helper", c.url);
            cmd.arg("-c")
                .arg(format!("{key}="))
                .arg("-c")
                .arg(format!("{key}={CREDENTIAL_HELPER}"))
                .env("TEXORIA_GIT_USERNAME", &c.username)
                .env("TEXORIA_GIT_PASSWORD", &c.password);
        }
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
    } else if matches!(command.as_str(), "clone" | "fetch" | "pull" | "push")
        && (has("Could not resolve host")
            || has("Couldn't resolve host")
            || has("Failed to connect")
            || has("Couldn't connect to server")
            || has("Network is unreachable")
            || has("Connection timed out")
            || has("Operation timed out")
            || has("Connection refused"))
    {
        Error::Unreachable { output }
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
    plain(&std::fs::canonicalize(&p).unwrap_or(p))
}

/// `\\?\C:\dir` as `C:\dir`. `canonicalize` returns such verbatim paths on
/// Windows, and git does not understand them as a working directory or an
/// argument.
pub(crate) fn plain(path: &Path) -> PathBuf {
    #[cfg(windows)]
    if let Some(rest) = path.to_str().and_then(|s| s.strip_prefix(r"\\?\"))
        && rest.as_bytes().get(1) == Some(&b':')
    {
        return PathBuf::from(rest);
    }
    path.to_path_buf()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_replaces_helpers_for_its_url_only() {
        let Some(found) = Git::find() else {
            return;
        };
        let tmp = tempfile::tempdir().unwrap();
        let config = tmp.path().join("gitconfig");
        // The user's own helper, with a stale password.
        std::fs::write(
            &config,
            "[credential]\n\thelper = \"!f() { echo username=me; echo password=stale; }; f\"\n",
        )
        .unwrap();
        let git = Git::at(found.program())
            .env("GIT_CONFIG_GLOBAL", &config)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("HOME", tmp.path())
            .credential(Some(Credential {
                url: "https://git.overleaf.com".into(),
                username: "git".into(),
                password: "olp_secret".into(),
            }));
        let fill = |host: &str, network: bool| {
            let input = format!("protocol=https\nhost={host}\npath=abc\n\n");
            let out = git
                .run(
                    tmp.path(),
                    ["credential", "fill"],
                    process::Options {
                        stdin: Some(input.as_bytes()),
                        network,
                        ..Default::default()
                    },
                )
                .unwrap();
            String::from_utf8(out).unwrap()
        };
        let out = fill("git.overleaf.com", true);
        assert!(out.contains("username=git\n"), "{out}");
        assert!(out.contains("password=olp_secret\n"), "{out}");
        assert!(fill("github.com", true).contains("password=stale\n"));
        // Only network commands get it.
        assert!(fill("git.overleaf.com", false).contains("password=stale\n"));
    }

    #[test]
    fn rejects_credentials_that_break_the_protocol() {
        let git = Git::at("git").credential(Some(Credential {
            url: "https://git.overleaf.com".into(),
            username: "git".into(),
            password: "olp_x\nquit=1".into(),
        }));
        let err = git
            .run(
                Path::new("."),
                ["fetch"],
                process::Options {
                    network: true,
                    ..Default::default()
                },
            )
            .unwrap_err();
        assert!(matches!(err, Error::Invalid(_)), "{err:?}");
    }

    #[test]
    fn credential_debug_hides_password() {
        let c = Credential {
            url: "https://git.overleaf.com".into(),
            username: "git".into(),
            password: "olp_secret".into(),
        };
        assert!(!format!("{c:?}").contains("olp_secret"));
    }

    #[test]
    fn classifies_unreachable_remotes() {
        let out = "fatal: unable to access 'https://x/': Could not resolve host: x".to_string();
        assert!(matches!(
            classify("fetch".into(), out.clone()),
            Error::Unreachable { .. }
        ));
        assert!(matches!(
            classify("status".into(), out),
            Error::Failed { .. }
        ));
    }
}
