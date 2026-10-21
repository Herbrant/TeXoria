//! Finding agents and running them as isolated child processes.
//!
//! A GUI app does not get the `PATH` of the user's shell (on macOS it is just
//! `/usr/bin:/bin:/usr/sbin:/sbin`), and most agents are Node scripts whose
//! `#!/usr/bin/env node` needs it: the login shell's `PATH` and the usual
//! install locations are searched, and given to the agent.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::process::{Command, Stdio};
use std::sync::OnceLock;
#[cfg(unix)]
use std::time::{Duration, Instant};

/// How long the login shell may take to report its `PATH`.
#[cfg(unix)]
const SHELL_TIMEOUT: Duration = Duration::from_secs(5);

/// The `PATH` to look for agents in and to run them with.
pub fn search_path() -> OsString {
    static PATH: OnceLock<OsString> = OnceLock::new();
    PATH.get_or_init(|| {
        #[cfg(unix)]
        let login = login_shell_path();
        #[cfg(not(unix))]
        let login = None;
        let var = |k: &str| std::env::var_os(k);
        join_search_path(login, var("PATH"), well_known_dirs(&var))
    })
    .clone()
}

/// The directories of `login` (the login shell's `PATH`), then of `path`,
/// then `known`, as [`tex_sys::paths::absolute_path`] keeps them: agents run
/// in the project, where a relative entry (`.`) would let a `node` planted
/// there run instead of the real one.
fn join_search_path(
    login: Option<OsString>,
    path: Option<OsString>,
    known: Vec<PathBuf>,
) -> OsString {
    let mut dirs: Vec<PathBuf> = Vec::new();
    for path in [login, path].into_iter().flatten() {
        dirs.extend(std::env::split_paths(&path));
    }
    dirs.extend(known);
    tex_sys::paths::absolute_path(dirs)
}

/// `name` on [`search_path`].
pub fn find(name: &str) -> Option<PathBuf> {
    find_in(name, search_path())
}

/// `name` on `path`.
pub(crate) fn find_in(name: &str, path: OsString) -> Option<PathBuf> {
    tex_sys::paths::find_program(name, Some(path), &[]).map(|(program, _)| program)
}

/// Where package managers and installers put command-line tools (`var`
/// reads the environment).
fn well_known_dirs(var: &dyn Fn(&str) -> Option<OsString>) -> Vec<PathBuf> {
    let home = tex_sys::paths::home(var);
    let mut dirs = Vec::new();
    if let Some(h) = &home {
        for d in [
            ".local/bin",
            ".npm-global/bin",
            ".bun/bin",
            ".volta/bin",
            ".deno/bin",
            ".cargo/bin",
            ".claude/local",
            ".opencode/bin",
            ".local/share/pnpm",
            "Library/pnpm",
            ".local/share/mise/shims",
            ".asdf/shims",
        ] {
            dirs.push(h.join(d));
        }
        // nvm: the newest installed Node first.
        let mut nodes: Vec<PathBuf> = std::fs::read_dir(h.join(".nvm/versions/node"))
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path().join("bin"))
            .collect();
        nodes.sort_by_key(|p| std::cmp::Reverse(node_version(p)));
        dirs.extend(nodes);
    }
    if cfg!(target_os = "macos") {
        dirs.push(PathBuf::from("/opt/homebrew/bin"));
    }
    if cfg!(unix) {
        dirs.push(PathBuf::from("/usr/local/bin"));
        dirs.push(PathBuf::from("/usr/bin"));
        dirs.push(PathBuf::from("/bin"));
    }
    if cfg!(windows) {
        let var = |k: &str| var(k).map(PathBuf::from);
        if let Some(appdata) = var("APPDATA") {
            dirs.push(appdata.join("npm"));
        }
        if let Some(local) = var("LOCALAPPDATA") {
            dirs.push(local.join("Programs"));
            dirs.push(local.join("pnpm"));
            dirs.push(local.join("Microsoft/WinGet/Links"));
        }
        if let Some(pf) = var("ProgramFiles") {
            dirs.push(pf.join("nodejs"));
        }
    }
    dirs
}

/// `v22.3.1` in `~/.nvm/versions/node/v22.3.1/bin`, as numbers.
fn node_version(bin: &Path) -> Vec<u32> {
    bin.parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .map(|n| {
            n.trim_start_matches('v')
                .split('.')
                .map(|p| p.parse().unwrap_or(0))
                .collect()
        })
        .unwrap_or_default()
}

/// The `PATH` an interactive login shell sets up (Homebrew, nvm, …).
#[cfg(unix)]
const MARK: &str = "__TEXORIA_PATH__";

#[cfg(unix)]
fn login_shell_path() -> Option<OsString> {
    let shell = std::env::var_os("SHELL")
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/bin/sh".into());
    let path = marked_output(login_shell_command(&shell), SHELL_TIMEOUT);
    if path.is_none() {
        log::warn!("{}: no PATH from the login shell", shell.to_string_lossy());
    }
    path
}

/// `shell`, asked to print its `PATH` between marks.
#[cfg(unix)]
fn login_shell_command(shell: &std::ffi::OsStr) -> Command {
    let mut cmd = Command::new(shell);
    cmd.args(["-l", "-i", "-c"])
        .arg(format!("printf '{MARK}%s{MARK}' \"$PATH\""));
    cmd
}

/// What `cmd` prints between the marks, if it does within `timeout`. The
/// command (and what it started) is stopped then.
#[cfg(unix)]
fn marked_output(mut cmd: Command, timeout: Duration) -> Option<OsString> {
    use std::io::Read;
    use std::os::unix::ffi::OsStringExt;
    use std::sync::mpsc;

    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    tex_sys::process::new_session(&mut cmd);
    let mut child = cmd.spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    // Read as it comes, and stop at the second mark rather than at the end of
    // the output: a job the rc files start in the background keeps the pipe
    // open long after the shell is gone.
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n) = stdout.read(&mut buf) {
            if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    let deadline = Instant::now() + timeout;
    let mut out = Vec::new();
    let path = loop {
        if let Some(path) = between_marks(&out) {
            break Some(path.to_vec());
        }
        match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(chunk) => out.extend(chunk),
            Err(_) => break None,
        }
    };
    if !matches!(child.try_wait(), Ok(Some(_))) {
        tex_sys::process::kill_tree(&mut child);
    }
    path.filter(|p| !p.is_empty()).map(OsString::from_vec)
}

/// What the shell printed between the two marks, once both arrived.
#[cfg(unix)]
fn between_marks(out: &[u8]) -> Option<&[u8]> {
    let mark = MARK.as_bytes();
    let find = |hay: &[u8]| hay.windows(mark.len()).position(|w| w == mark);
    let start = find(out)? + mark.len();
    let len = find(&out[start..])?;
    Some(&out[start..start + len])
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// An empty file `name` in `dir` that `which` takes for a program.
    pub(crate) fn executable(dir: &Path, name: &str) -> PathBuf {
        test_support::executable(dir, name, "")
    }

    #[test]
    fn search_path_keeps_order_and_absolute_dirs_once() {
        let abs = |p: &str| std::env::temp_dir().join(p);
        let join = |ps: &[PathBuf]| std::env::join_paths(ps).unwrap();
        let login = join(&[abs("login"), abs("both")]);
        let path = join(&[
            abs("both"),
            PathBuf::from("."),
            PathBuf::from("rel/bin"),
            abs("env"),
        ]);
        let joined = join_search_path(Some(login), Some(path), vec![abs("known"), abs("env")]);
        let dirs: Vec<PathBuf> = std::env::split_paths(&joined).collect();
        assert_eq!(dirs, [abs("login"), abs("both"), abs("env"), abs("known")]);
        // A dir that cannot be in a `PATH` is left out, not the whole.
        let bad = if cfg!(windows) { "C:\\a\"b" } else { "/a:b" };
        let joined = join_search_path(None, None, vec![PathBuf::from(bad), abs("known")]);
        assert_eq!(
            std::env::split_paths(&joined).collect::<Vec<_>>(),
            [abs("known")]
        );
        // Never empty on Unix: `execvp` reads an empty `PATH` as the current directory.
        let empty = join_search_path(None, None, Vec::new());
        assert_eq!(empty.is_empty(), cfg!(windows));
    }

    #[test]
    fn search_path_keeps_the_process_path() {
        let current = std::env::var_os("PATH").unwrap_or_default();
        let path = join_search_path(None, Some(current.clone()), Vec::new());
        let dirs: Vec<PathBuf> = std::env::split_paths(&path).collect();
        for d in std::env::split_paths(&current).filter(|d| d.is_absolute()) {
            assert!(dirs.contains(&d), "{} missing", d.display());
        }
    }

    #[test]
    fn well_known_dirs_follow_the_home_folder() {
        let home = tempfile::tempdir().unwrap();
        for v in ["v9.11.0", "v22.3.1", "v22.10.0", "weird"] {
            std::fs::create_dir_all(home.path().join(".nvm/versions/node").join(v)).unwrap();
        }
        let h = home.path().as_os_str().to_owned();
        let dirs = well_known_dirs(&|k| (k == "HOME").then(|| h.clone()));
        assert_eq!(dirs[0], home.path().join(".local/bin"));
        let nvm: Vec<&PathBuf> = dirs
            .iter()
            .filter(|d| d.starts_with(home.path().join(".nvm")))
            .collect();
        let node = |v: &str| home.path().join(".nvm/versions/node").join(v).join("bin");
        assert_eq!(
            nvm,
            [
                &node("v22.10.0"),
                &node("v22.3.1"),
                &node("v9.11.0"),
                &node("weird")
            ]
        );
        // USERPROFILE stands in for HOME (Windows).
        let profile = well_known_dirs(&|k| (k == "USERPROFILE").then(|| h.clone()));
        assert!(profile.contains(&home.path().join(".cargo/bin")));
        let homeless = well_known_dirs(&|_| None);
        assert!(homeless.iter().all(|d| !d.starts_with(home.path())));
        if cfg!(target_os = "macos") {
            assert!(homeless.contains(&PathBuf::from("/opt/homebrew/bin")));
        }
        if cfg!(unix) {
            assert_eq!(homeless.last(), Some(&PathBuf::from("/bin")));
        }
    }

    #[cfg(windows)]
    #[test]
    fn well_known_dirs_on_windows() {
        let dirs = well_known_dirs(&|k| match k {
            "APPDATA" => Some(r"C:\Users\me\AppData\Roaming".into()),
            "LOCALAPPDATA" => Some(r"C:\Users\me\AppData\Local".into()),
            "ProgramFiles" => Some(r"C:\Program Files".into()),
            _ => None,
        });
        for d in [
            r"C:\Users\me\AppData\Roaming\npm",
            r"C:\Users\me\AppData\Local\pnpm",
            r"C:\Program Files\nodejs",
        ] {
            assert!(
                dirs.contains(&PathBuf::from(d)),
                "{d} missing from {dirs:?}"
            );
        }
        assert!(well_known_dirs(&|_| None).is_empty());
    }

    #[test]
    fn finds_programs_on_a_given_path() {
        let dir = tempfile::tempdir().unwrap();
        let exe = executable(dir.path(), "my-agent");
        let path = dir.path().as_os_str().to_owned();
        assert_eq!(find_in("my-agent", path.clone()), Some(exe));
        assert_eq!(find_in("not-there", path), None);
    }

    #[test]
    fn newest_node_first() {
        assert!(
            node_version(Path::new("/h/.nvm/versions/node/v22.3.1/bin"))
                > node_version(Path::new("/h/.nvm/versions/node/v9.11.0/bin"))
        );
        assert_eq!(node_version(Path::new("bin")), Vec::<u32>::new());
    }

    proptest::proptest! {
        #[test]
        fn node_versions_never_panic(name in "\\PC{0,12}") {
            let bin = Path::new("/h").join(&name).join("bin");
            let _ = node_version(&bin);
        }
    }

    #[cfg(unix)]
    #[test]
    fn path_is_read_between_the_marks() {
        let out = format!("motd\n{MARK}/usr/bin:/bin{MARK}more");
        assert_eq!(between_marks(out.as_bytes()), Some(&b"/usr/bin:/bin"[..]));
        assert_eq!(between_marks(format!("{MARK}/usr/b").as_bytes()), None);
        assert_eq!(between_marks(b"nothing"), None);
    }

    #[cfg(unix)]
    fn sh(script: &str) -> Command {
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c").arg(script);
        cmd
    }

    #[cfg(unix)]
    #[test]
    fn reads_what_the_shell_prints_between_marks() {
        let timeout = Duration::from_secs(10);
        let printed = sh(&format!("echo motd; printf '{MARK}/a:/b{MARK}'; echo bye"));
        assert_eq!(marked_output(printed, timeout), Some("/a:/b".into()));
        for nothing in ["echo no marks", &format!("printf '{MARK}{MARK}'"), "exit 3"] {
            assert_eq!(marked_output(sh(nothing), timeout), None, "{nothing}");
        }
        assert_eq!(marked_output(Command::new("/no/such/shell"), timeout), None);
    }

    #[cfg(unix)]
    #[test]
    fn does_not_wait_for_slow_shells_or_their_jobs() {
        let start = Instant::now();
        // A job started by the rc files keeps the output open.
        let job = sh(&format!("sleep 30 & printf '{MARK}/a{MARK}'"));
        assert_eq!(
            marked_output(job, Duration::from_secs(10)),
            Some("/a".into())
        );
        let hung = sh(&format!("sleep 30; printf '{MARK}/a{MARK}'"));
        assert_eq!(marked_output(hung, Duration::from_millis(300)), None);
        assert!(start.elapsed() < Duration::from_secs(10));
    }

    #[cfg(unix)]
    #[test]
    fn asks_an_interactive_login_shell() {
        let cmd = login_shell_command(std::ffi::OsStr::new("/bin/zsh"));
        assert_eq!(cmd.get_program(), "/bin/zsh");
        let args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args[..3], ["-l", "-i", "-c"]);
        assert_eq!(args[3], format!("printf '{MARK}%s{MARK}' \"$PATH\""));
    }
}
