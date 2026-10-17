//! Finding agents and running them as isolated child processes.
//!
//! A GUI app does not get the `PATH` of the user's shell (on macOS it is just
//! `/usr/bin:/bin:/usr/sbin:/sbin`), and most agents are Node scripts whose
//! `#!/usr/bin/env node` needs it: the login shell's `PATH` and the usual
//! install locations are searched, and given to the agent.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// How long the login shell may take to report its `PATH`.
#[cfg(unix)]
const SHELL_TIMEOUT: Duration = Duration::from_secs(5);

/// The `PATH` to look for agents in and to run them with.
pub fn search_path() -> OsString {
    static PATH: OnceLock<OsString> = OnceLock::new();
    PATH.get_or_init(|| {
        let mut dirs: Vec<PathBuf> = Vec::new();
        #[cfg(unix)]
        if let Some(path) = login_shell_path() {
            dirs.extend(std::env::split_paths(&path));
        }
        if let Some(path) = std::env::var_os("PATH") {
            dirs.extend(std::env::split_paths(&path));
        }
        dirs.extend(well_known_dirs());
        let mut seen = std::collections::HashSet::new();
        // Agents run in the project: a relative entry (`.`) would let a
        // `node` planted there run instead of the real one.
        dirs.retain(|d| d.is_absolute() && seen.insert(d.clone()));
        std::env::join_paths(dirs).unwrap_or_default()
    })
    .clone()
}

/// `name` on [`search_path`].
pub fn find(name: &str) -> Option<PathBuf> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    which::which_in(name, Some(search_path()), cwd).ok()
}

/// Where package managers and installers put command-line tools.
fn well_known_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from);
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
        let var = |k: &str| std::env::var_os(k).map(PathBuf::from);
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
    use std::io::Read;
    use std::os::unix::ffi::OsStringExt;
    use std::sync::mpsc;

    let shell = std::env::var_os("SHELL")
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/bin/sh".into());
    let mut cmd = Command::new(&shell);
    cmd.args(["-l", "-i", "-c"])
        .arg(format!("printf '{MARK}%s{MARK}' \"$PATH\""))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    isolate(&mut cmd);
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
    let deadline = Instant::now() + SHELL_TIMEOUT;
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
        kill_tree(&mut child);
    }
    match path {
        Some(path) if !path.is_empty() => Some(OsString::from_vec(path)),
        _ => {
            log::warn!("{}: no PATH from the login shell", shell.to_string_lossy());
            None
        }
    }
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

/// Unix: a new session, so nothing waits on the terminal TeXoria was started
/// from, and a process group of its own, so the whole tree can be killed.
/// Windows: no console window.
pub(crate) fn isolate(cmd: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: setsid is async-signal-safe.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
}

/// Agents start helpers (MCP servers, shells, language servers): stop them all.
pub(crate) fn kill_tree(child: &mut Child) {
    #[cfg(unix)]
    {
        let pgid = child.id() as libc::pid_t;
        unsafe {
            libc::kill(-pgid, libc::SIGKILL);
        }
    }
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/T", "/F", "/PID", &child.id().to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_path_keeps_the_process_path() {
        let path = search_path();
        let dirs: Vec<PathBuf> = std::env::split_paths(&path).collect();
        assert!(dirs.iter().all(|d| d.is_absolute()));
        let current = std::env::var_os("PATH").unwrap_or_default();
        for d in std::env::split_paths(&current).filter(|d| d.is_absolute()) {
            assert!(dirs.contains(&d), "{} missing", d.display());
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

    #[test]
    fn newest_node_first() {
        assert!(
            node_version(Path::new("/h/.nvm/versions/node/v22.3.1/bin"))
                > node_version(Path::new("/h/.nvm/versions/node/v9.11.0/bin"))
        );
    }
}
