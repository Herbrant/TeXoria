//! Background compilation scheduler.
//!
//! Requests are debounced, at most one engine process runs at a time and
//! requests arriving during a run are coalesced into a single follow-up run.
//! The running job is not killed by new edits: under continuous typing that
//! would starve the preview, while coalescing keeps the PDF updating at the
//! natural pace of the engine. Explicit [`Compiler::cancel`] or an engine
//! change kill the process tree.

use std::fs::File;
use std::path::PathBuf;
use std::process::{Child, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use tex_log::{Diagnostic, Severity};

use crate::engine::{Engine, Job};

const POLL: Duration = Duration::from_millis(25);

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
    pub synctex: Option<PathBuf>,
    pub diagnostics: Vec<Diagnostic>,
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

    /// Schedule a compile right away (explicit "Typeset" command).
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
}

struct Worker {
    engine: Engine,
    debounce: Duration,
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
            Ok(None) => {}
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

    fn finish(&self, run: Running, mut status: Status) {
        let job = run.job;
        let log_text = std::fs::read(job.artifact("log"))
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default();
        let diagnostics = if status == Status::Cancelled {
            Vec::new()
        } else {
            tex_log::parse(&log_text, job.work_dir())
        };
        // latexmk exits 0 on some error paths (and Tectonic on recoverable
        // ones); trust the log over the exit code.
        if status == Status::Success && diagnostics.iter().any(|d| d.severity == Severity::Error) {
            status = Status::Failed;
        }
        let existing = |p: PathBuf| p.is_file().then_some(p);
        let out = Output {
            id: run.id,
            pdf: existing(job.artifact("pdf")),
            synctex: existing(job.artifact("synctex.gz"))
                .or_else(|| existing(job.artifact("synctex"))),
            job,
            status,
            diagnostics,
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
        let _ = std::process::Command::new("taskkill")
            .args(["/T", "/F", "/PID", &child.id().to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::engine::EngineKind;
    use std::sync::{Arc, Mutex};

    fn fake_engine(script: &str) -> Engine {
        Engine {
            kind: EngineKind::Custom {
                program: "/bin/sh".into(),
                args: vec!["-c".into(), script.into()],
            },
            extra_path: vec![],
        }
    }

    fn collect() -> (Arc<Mutex<Vec<Event>>>, impl Fn(Event) + Send + 'static) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = events.clone();
        (events, move |e| sink.lock().unwrap().push(e))
    }

    fn finished(events: &Mutex<Vec<Event>>) -> Vec<Output> {
        events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                Event::Finished(o) => Some(o.clone()),
                _ => None,
            })
            .collect()
    }

    fn wait_for(events: &Mutex<Vec<Event>>, n: usize) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while finished(events).len() < n {
            assert!(Instant::now() < deadline, "timed out");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn debounce_coalesces_requests() {
        let dir = tempfile::tempdir().unwrap();
        let job = Job::for_root(dir.path().join("main.tex"));
        let (events, sink) = collect();
        let compiler = Compiler::new(
            fake_engine("mkdir -p .texoria/build && echo '%PDF' > .texoria/build/main.pdf"),
            Duration::from_millis(80),
            sink,
        );
        for _ in 0..10 {
            compiler.request(job.clone());
            std::thread::sleep(Duration::from_millis(5));
        }
        wait_for(&events, 1);
        std::thread::sleep(Duration::from_millis(200));
        let done = finished(&events);
        assert_eq!(done.len(), 1, "requests should coalesce");
        assert_eq!(done[0].status, Status::Success);
        assert_eq!(done[0].pdf.as_deref(), Some(job.artifact("pdf").as_path()));
    }

    #[test]
    fn requests_during_run_are_queued_not_killed() {
        let dir = tempfile::tempdir().unwrap();
        let job = Job::for_root(dir.path().join("main.tex"));
        let (events, sink) = collect();
        let compiler = Compiler::new(fake_engine("sleep 0.3"), Duration::from_millis(10), sink);
        compiler.request_now(job.clone());
        std::thread::sleep(Duration::from_millis(100));
        compiler.request(job.clone());
        compiler.request(job.clone());
        wait_for(&events, 2);
        let done = finished(&events);
        assert!(done.iter().all(|o| o.status == Status::Success));
        assert!(done[0].elapsed >= Duration::from_millis(250));
    }

    #[test]
    fn cancel_kills_process_tree() {
        let dir = tempfile::tempdir().unwrap();
        let job = Job::for_root(dir.path().join("main.tex"));
        let (events, sink) = collect();
        // Child shell spawns a grandchild that would outlive a plain kill().
        let compiler = Compiler::new(fake_engine("sh -c 'sleep 30'; true"), Duration::ZERO, sink);
        compiler.request_now(job);
        std::thread::sleep(Duration::from_millis(150));
        let t = Instant::now();
        compiler.cancel();
        wait_for(&events, 1);
        assert!(t.elapsed() < Duration::from_secs(2));
        assert_eq!(finished(&events)[0].status, Status::Cancelled);
    }

    #[test]
    fn errors_in_log_mark_failure() {
        let dir = tempfile::tempdir().unwrap();
        let job = Job::for_root(dir.path().join("main.tex"));
        let (events, sink) = collect();
        let script = "mkdir -p .texoria/build && printf '(./main.tex\\n! Undefined control sequence.\\nl.3 \\\\foo\\n\\n)\\n' > .texoria/build/main.log";
        let compiler = Compiler::new(fake_engine(script), Duration::ZERO, sink);
        compiler.request_now(job);
        wait_for(&events, 1);
        let out = &finished(&events)[0];
        assert_eq!(out.status, Status::Failed);
        assert_eq!(out.diagnostics.len(), 1);
        assert_eq!(out.diagnostics[0].line, Some(3));
        assert_eq!(
            out.diagnostics[0].file.as_deref(),
            Some(dir.path().join("main.tex").as_path())
        );
    }
}
