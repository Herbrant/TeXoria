//! Background compilation scheduler.
//!
//! Requests are debounced, at most one engine process runs at a time and
//! requests arriving during a run are coalesced into a single follow-up run.
//! The running job is not killed by new edits: under continuous typing that
//! would starve the preview, while coalescing keeps the PDF updating at the
//! natural pace of the engine. Explicit [`Compiler::cancel`], an engine
//! change or the run exceeding its timeout kill the process tree.

use std::fs::File;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Child, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use tex_log::{Diagnostic, Severity};

use crate::engine::{Engine, Job};

const POLL: Duration = Duration::from_millis(25);
/// A runaway engine can write gigabytes of log; only this much is parsed.
const MAX_LOG_BYTES: u64 = 32 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// The engine exited cleanly.
    Success,
    /// The engine reported errors (a PDF may still have been produced).
    Failed,
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct Output {
    pub id: u64,
    pub job: Job,
    pub status: Status,
    /// Present if the PDF exists after the run.
    pub pdf: Option<PathBuf>,
    /// The engine wrote the PDF during this run (it may also have left an
    /// older one alone, up to date or not).
    pub pdf_written: bool,
    pub synctex: Option<PathBuf>,
    pub diagnostics: Vec<Diagnostic>,
    /// The document wanted to run programs (see `Job::shell_escape`).
    pub needs_shell_escape: bool,
    pub elapsed: Duration,
}

#[derive(Debug, Clone)]
pub enum Event {
    Started {
        id: u64,
        engine: String,
    },
    Finished(Output),
    /// The engine could not even be launched.
    SpawnFailed {
        id: u64,
        error: String,
    },
}

enum Cmd {
    Request { job: Job, immediate: bool },
    SetEngine(Engine),
    SetDebounce(Duration),
    SetTimeout(Option<Duration>),
    Cancel,
}

pub struct Compiler {
    tx: Sender<Cmd>,
    thread: Option<JoinHandle<()>>,
}

impl Compiler {
    pub fn new(
        engine: Engine,
        debounce: Duration,
        on_event: impl Fn(Event) + Send + 'static,
    ) -> Self {
        let (tx, rx) = mpsc::channel::<Cmd>();
        let thread = std::thread::Builder::new()
            .name("tex-compile".into())
            .spawn(move || Worker::new(engine, debounce, Box::new(on_event)).run(rx))
            .expect("spawn compile thread");
        Compiler {
            tx,
            thread: Some(thread),
        }
    }

    /// Schedule a compile after the debounce interval (live preview).
    pub fn request(&self, job: Job) {
        let _ = self.tx.send(Cmd::Request {
            job,
            immediate: false,
        });
    }

    /// Schedule a compile right away (explicit "Build" command).
    pub fn request_now(&self, job: Job) {
        let _ = self.tx.send(Cmd::Request {
            job,
            immediate: true,
        });
    }

    pub fn set_engine(&self, engine: Engine) {
        let _ = self.tx.send(Cmd::SetEngine(engine));
    }

    pub fn set_debounce(&self, d: Duration) {
        let _ = self.tx.send(Cmd::SetDebounce(d));
    }

    /// Kill runs that take longer than `timeout` (`None`: never).
    pub fn set_timeout(&self, timeout: Option<Duration>) {
        let _ = self.tx.send(Cmd::SetTimeout(timeout));
    }

    pub fn cancel(&self) {
        let _ = self.tx.send(Cmd::Cancel);
    }
}

impl Drop for Compiler {
    fn drop(&mut self) {
        // Dropping the sender disconnects the worker, which kills any child.
        let (tx, _) = mpsc::channel();
        drop(std::mem::replace(&mut self.tx, tx));
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct Running {
    id: u64,
    job: Job,
    child: Child,
    started: Instant,
    started_at: SystemTime,
}

struct Worker {
    engine: Engine,
    debounce: Duration,
    timeout: Option<Duration>,
    on_event: Box<dyn Fn(Event) + Send>,
    next_id: u64,
    pending: Option<(Job, Instant)>,
    running: Option<Running>,
}

impl Worker {
    fn new(engine: Engine, debounce: Duration, on_event: Box<dyn Fn(Event) + Send>) -> Self {
        Worker {
            engine,
            debounce,
            timeout: None,
            on_event,
            next_id: 1,
            pending: None,
            running: None,
        }
    }

    fn run(mut self, rx: mpsc::Receiver<Cmd>) {
        loop {
            let now = Instant::now();
            let mut timeout = match &self.pending {
                Some((_, at)) if self.running.is_none() => at.saturating_duration_since(now),
                _ => Duration::from_secs(3600),
            };
            if self.running.is_some() {
                timeout = timeout.min(POLL);
            }

            match rx.recv_timeout(timeout) {
                Ok(cmd) => self.handle(cmd),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    self.kill_running();
                    return;
                }
            }

            self.poll_running();
            if self.running.is_none()
                && let Some((_, at)) = &self.pending
                && *at <= Instant::now()
            {
                let (job, _) = self.pending.take().unwrap();
                self.start(job);
            }
        }
    }

    fn handle(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Request { job, immediate } => {
                let at = if immediate {
                    Instant::now()
                } else {
                    Instant::now() + self.debounce
                };
                self.pending = Some((job, at));
            }
            Cmd::SetEngine(engine) => {
                if engine != self.engine {
                    // Output of the old engine is irrelevant now; rerun.
                    if let Some(job) = self.kill_running() {
                        self.pending.get_or_insert((job, Instant::now()));
                    }
                    self.engine = engine;
                }
            }
            Cmd::SetDebounce(d) => self.debounce = d,
            Cmd::SetTimeout(t) => self.timeout = t,
            Cmd::Cancel => {
                self.pending = None;
                self.kill_running();
            }
        }
    }

    fn start(&mut self, job: Job) {
        let id = self.next_id;
        self.next_id += 1;

        if let Err(e) = std::fs::create_dir_all(&job.out_dir) {
            (self.on_event)(Event::SpawnFailed {
                id,
                error: format!("cannot create build dir: {e}"),
            });
            return;
        }
        ignore_build_dir(&job);
        // Engine chatter goes to a file: draining pipes would need extra
        // threads, and a full pipe blocks the engine.
        let transcript = job.out_dir.join("texoria-run.txt");
        let stdio = File::create(&transcript).and_then(|f| Ok((f.try_clone()?, f)));
        let mut cmd = self.engine.command(&job);
        cmd.stdin(Stdio::null());
        match stdio {
            Ok((out, err)) => {
                cmd.stdout(out).stderr(err);
            }
            Err(_) => {
                cmd.stdout(Stdio::null()).stderr(Stdio::null());
            }
        }
        isolate(&mut cmd);

        log::info!("compile #{id}: {:?}", cmd);
        match cmd.spawn() {
            Ok(child) => {
                (self.on_event)(Event::Started {
                    id,
                    engine: self.engine.label(),
                });
                self.running = Some(Running {
                    id,
                    job,
                    child,
                    started: Instant::now(),
                    started_at: SystemTime::now(),
                });
            }
            Err(e) => {
                (self.on_event)(Event::SpawnFailed {
                    id,
                    error: format!("failed to launch {}: {e}", self.engine.label()),
                });
            }
        }
    }

    fn poll_running(&mut self) {
        let Some(run) = self.running.as_mut() else {
            return;
        };
        match run.child.try_wait() {
            Ok(None) => {
                if let Some(limit) = self.timeout
                    && run.started.elapsed() > limit
                {
                    // An infinite loop in the document: new edits would only
                    // queue behind it, so stop it.
                    let mut run = self.running.take().unwrap();
                    kill_tree(&mut run.child);
                    let _ = run.child.wait();
                    self.finish_with(run, Status::Failed, Some(limit));
                }
            }
            Ok(Some(status)) => {
                let run = self.running.take().unwrap();
                let s = if status.success() {
                    Status::Success
                } else {
                    Status::Failed
                };
                self.finish(run, s);
            }
            Err(e) => {
                log::warn!("waiting on compile #{}: {e}", run.id);
                let run = self.running.take().unwrap();
                self.finish(run, Status::Failed);
            }
        }
    }

    fn finish(&self, run: Running, status: Status) {
        self.finish_with(run, status, None);
    }

    /// `timed_out`: the run was killed after this long.
    fn finish_with(&self, run: Running, mut status: Status, timed_out: Option<Duration>) {
        let job = run.job;
        let job_shell_escape = job.shell_escape;
        // A cancelled run has nothing to say: its log, if any, is partial or
        // an earlier run's.
        let mut log = Vec::new();
        if status != Status::Cancelled
            && let Ok(f) = File::open(job.artifact("log"))
        {
            let _ = f.take(MAX_LOG_BYTES).read_to_end(&mut log);
        }
        let log_text = String::from_utf8_lossy(&log);
        let mut diagnostics = tex_log::parse_bytes(&log, job.work_dir());
        if let Some(limit) = timed_out {
            diagnostics.insert(
                0,
                Diagnostic {
                    severity: Severity::Error,
                    file: None,
                    line: None,
                    message: format!(
                        "The build stopped after {}s (infinite loop?)",
                        limit.as_secs()
                    ),
                },
            );
        }
        // latexmk exits 0 on some error paths (and Tectonic on recoverable
        // ones); trust the log over the exit code.
        if status == Status::Success && diagnostics.iter().any(|d| d.severity == Severity::Error) {
            status = Status::Failed;
        }
        let existing = |p: PathBuf| p.is_file().then_some(p);
        let pdf = existing(job.artifact("pdf"));
        // File times may be coarser than the clock.
        let since = run.started_at - Duration::from_secs(2);
        let pdf_written = pdf
            .as_ref()
            .and_then(|p| p.metadata().ok()?.modified().ok())
            .is_some_and(|t| t >= since);
        let out = Output {
            id: run.id,
            pdf,
            pdf_written,
            synctex: existing(job.artifact("synctex.gz"))
                .or_else(|| existing(job.artifact("synctex"))),
            job,
            status,
            diagnostics,
            needs_shell_escape: !job_shell_escape && tex_log::needs_shell_escape(&log_text),
            elapsed: run.started.elapsed(),
        };
        (self.on_event)(Event::Finished(out));
    }

    /// Kill the running process tree; returns its job if there was one.
    fn kill_running(&mut self) -> Option<Job> {
        let mut run = self.running.take()?;
        kill_tree(&mut run.child);
        let _ = run.child.wait();
        let job = run.job.clone();
        self.finish(run, Status::Cancelled);
        Some(job)
    }
}

/// Keep `.texoria/` out of version control without touching the project's
/// own `.gitignore`: git skips a directory whose `.gitignore` ignores `*`.
fn ignore_build_dir(job: &Job) {
    let Some(dir) = job.out_dir.parent().filter(|d| d.ends_with(".texoria")) else {
        return;
    };
    let ignore = dir.join(".gitignore");
    if !ignore.exists()
        && let Err(e) = std::fs::write(&ignore, "# Created by TeXoria: build artifacts.\n*\n")
    {
        log::debug!("writing {}: {e}", ignore.display());
    }
}

/// Put the child in its own process group (Unix) / hide its console
/// (Windows) so the whole tree can be killed and no window flashes.
fn isolate(cmd: &mut std::process::Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
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
        // latexmk spawns pdflatex/bibtex: signal the whole group.
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
        let _ = std::process::Command::new(taskkill)
            .args(["/T", "/F", "/PID", &child.id().to_string()])
            .creation_flags(CREATE_NO_WINDOW)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
}
