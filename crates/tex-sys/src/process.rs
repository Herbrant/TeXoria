//! Child processes in a process group of their own, so that the whole tree they start can be
//! killed, and without a console window on Windows.

use std::process::{Child, Command};

/// `CREATE_NO_WINDOW`: a console program started by a GUI app opens no window.
#[cfg(windows)]
pub const NO_WINDOW: u32 = 0x0800_0000;

/// Unix: a new session, so nothing waits on the terminal TeXoria was started from (ssh or a
/// credential helper fails instead), and a process group of its own, so the whole tree can be
/// killed ([`kill_tree`]). Windows: no console window.
pub fn new_session(cmd: &mut Command) {
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
    no_window(cmd);
}

/// Unix: a process group of its own (in TeXoria's session), so the whole tree can be killed
/// ([`kill_tree`]). Windows: no console window.
pub fn own_group(cmd: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    no_window(cmd);
}

/// Windows: no console window. Nothing elsewhere.
pub fn no_window(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = cmd;
}

/// Kill `child` and the processes it started (its process group on Unix, see [`new_session`]
/// and [`own_group`]; its process tree on Windows), then wait for it.
pub fn kill_tree(child: &mut Child) {
    #[cfg(unix)]
    {
        let pgid = child.id() as libc::pid_t;
        unsafe {
            libc::kill(-pgid, libc::SIGKILL);
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        use std::process::Stdio;
        // By its full path, never one found elsewhere, and without a console.
        let taskkill = std::env::var_os("SystemRoot")
            .map(|root| {
                std::path::PathBuf::from(root)
                    .join("System32")
                    .join("taskkill.exe")
            })
            .unwrap_or_else(|| "taskkill.exe".into());
        let _ = Command::new(taskkill)
            .args(["/T", "/F", "/PID", &child.id().to_string()])
            .creation_flags(NO_WINDOW)
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

    #[cfg(unix)]
    use std::time::{Duration, Instant};

    /// Whether process `pid` is gone (or a zombie nobody waits for).
    #[cfg(unix)]
    fn gone(pid: libc::pid_t) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            // SAFETY: signal 0 only checks that the process exists.
            if unsafe { libc::kill(pid, 0) } == -1 {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    /// Start `/bin/sh` with a grandchild, isolated by `isolate`; the child and the grandchild's
    /// PIDs.
    #[cfg(unix)]
    fn tree(isolate: fn(&mut Command)) -> (Child, libc::pid_t) {
        use std::io::BufRead;
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", "sleep 30 & echo $!; wait"])
            .stdout(std::process::Stdio::piped());
        isolate(&mut cmd);
        let mut child = cmd.spawn().unwrap();
        let mut line = String::new();
        std::io::BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        (child, line.trim().parse().unwrap())
    }

    #[cfg(unix)]
    #[test]
    fn kills_a_new_session() {
        let (mut child, grandchild) = tree(new_session);
        let pid = child.id() as libc::pid_t;
        // A session (and process group) of its own.
        assert_eq!(unsafe { libc::getsid(pid) }, pid);
        assert_eq!(unsafe { libc::getpgid(grandchild) }, pid);
        kill_tree(&mut child);
        assert!(child.try_wait().unwrap().is_some());
        assert!(gone(grandchild), "the grandchild survived");
    }

    #[cfg(unix)]
    #[test]
    fn kills_a_group() {
        let (mut child, grandchild) = tree(own_group);
        let pid = child.id() as libc::pid_t;
        // A group of its own, in our session.
        assert_eq!(unsafe { libc::getpgid(grandchild) }, pid);
        assert_eq!(unsafe { libc::getsid(pid) }, unsafe { libc::getsid(0) });
        kill_tree(&mut child);
        assert!(child.try_wait().unwrap().is_some());
        assert!(gone(grandchild), "the grandchild survived");
    }

    #[cfg(windows)]
    #[test]
    fn kills_the_whole_tree() {
        for isolate in [new_session, own_group] {
            let mut cmd = Command::new("cmd.exe");
            cmd.args(["/C", "ping -n 30 127.0.0.1 >NUL"]);
            isolate(&mut cmd);
            let mut child = cmd.spawn().unwrap();
            kill_tree(&mut child);
            assert!(child.try_wait().unwrap().is_some());
        }
    }

    #[test]
    fn no_window_leaves_the_command_alone() {
        let mut cmd = Command::new("x");
        no_window(&mut cmd);
        assert_eq!(cmd.get_program(), "x");
    }
}
