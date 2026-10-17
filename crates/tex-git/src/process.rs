//! Running `git` as an isolated child process.

use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::Error;

const POLL: Duration = Duration::from_millis(20);
/// How long to wait for the pipes to close once git has exited. A helper it
/// spawned (the credential cache daemon) may inherit them and live on.
const DRAIN_GRACE: Duration = Duration::from_secs(2);

/// Stops a running command from another thread.
#[derive(Debug, Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

#[derive(Default)]
pub(crate) struct Options<'a> {
    pub stdin: Option<&'a [u8]>,
    pub timeout: Option<Duration>,
    pub cancel: Option<&'a Cancel>,
    /// The command talks to a remote: [`crate::Credential`]s apply.
    pub network: bool,
}

pub(crate) struct Output {
    pub success: bool,
    pub stdout: Vec<u8>,
    pub stderr: String,
}

pub(crate) fn run(mut cmd: Command, opts: Options) -> Result<Output, Error> {
    if opts.cancel.is_some_and(Cancel::is_cancelled) {
        return Err(Error::Cancelled);
    }
    cmd.stdin(if opts.stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    })
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    isolate(&mut cmd);
    // Not `{cmd:?}`: it lists the environment, credentials included.
    log::debug!(
        "{} {:?}",
        cmd.get_program().to_string_lossy(),
        cmd.get_args().collect::<Vec<_>>()
    );
    let mut child = cmd.spawn().map_err(Error::Spawn)?;

    // Pipes are drained on their own threads: a full pipe would block git.
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());
    if let (Some(input), Some(mut pipe)) = (opts.stdin, child.stdin.take()) {
        let input = input.to_vec();
        std::thread::spawn(move || {
            let _ = pipe.write_all(&input);
        });
    }

    let status = if opts.timeout.is_none() && opts.cancel.is_none() {
        child.wait().map_err(Error::Spawn)?
    } else {
        let started = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {}
                Err(e) => {
                    kill_tree(&mut child);
                    return Err(Error::Spawn(e));
                }
            }
            if opts.cancel.is_some_and(Cancel::is_cancelled) {
                kill_tree(&mut child);
                return Err(Error::Cancelled);
            }
            if let Some(limit) = opts.timeout
                && started.elapsed() > limit
            {
                kill_tree(&mut child);
                return Err(Error::TimedOut(limit.as_secs()));
            }
            std::thread::sleep(POLL);
        }
    };

    Ok(Output {
        success: status.success(),
        stdout: stdout.finish(),
        stderr: String::from_utf8_lossy(&stderr.finish()).into_owned(),
    })
}

struct Drain {
    buf: Arc<Mutex<Vec<u8>>>,
    done: Receiver<()>,
}

impl Drain {
    fn finish(self) -> Vec<u8> {
        let _ = self.done.recv_timeout(DRAIN_GRACE);
        std::mem::take(&mut *self.buf.lock().unwrap())
    }
}

fn drain(pipe: Option<impl Read + Send + 'static>) -> Drain {
    let buf = Arc::new(Mutex::new(Vec::new()));
    let (tx, done) = mpsc::channel();
    if let Some(mut pipe) = pipe {
        let buf = buf.clone();
        std::thread::spawn(move || {
            let mut chunk = [0u8; 16 * 1024];
            loop {
                match pipe.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => buf.lock().unwrap().extend_from_slice(&chunk[..n]),
                }
            }
            let _ = tx.send(());
        });
    }
    Drain { buf, done }
}

/// Unix: a new session, so ssh or a credential helper finds no terminal and
/// fails instead of waiting on the one TeXoria was started from, and its own
/// process group, so the whole tree can be killed. Windows: no console window.
fn isolate(cmd: &mut Command) {
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

fn kill_tree(child: &mut Child) {
    #[cfg(unix)]
    {
        // git spawns ssh and remote helpers: signal the whole group.
        let pgid = child.id() as libc::pid_t;
        unsafe {
            libc::kill(-pgid, libc::SIGKILL);
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
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
            .creation_flags(CREATE_NO_WINDOW)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}
