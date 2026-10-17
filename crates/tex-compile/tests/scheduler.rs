//! The scheduler driving `fake-engine` (tests/bin/fake_engine.rs), a
//! stand-in for a TeX engine that behaves the same on every platform.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use tex_compile::{Compiler, Engine, EngineKind, Event, Job, Output, Severity, Status};

fn fake(steps: &[&str]) -> Engine {
    Engine {
        kind: EngineKind::Custom {
            program: env!("CARGO_BIN_EXE_fake-engine").into(),
            args: steps.iter().map(|s| s.to_string()).collect(),
        },
        extra_path: vec![],
    }
}

/// The events a compiler sends, as they come.
#[derive(Clone, Default)]
struct Events(Arc<Mutex<Vec<Event>>>);

impl Events {
    fn sink(&self) -> impl Fn(Event) + Send + 'static {
        let events = self.0.clone();
        move |e| events.lock().unwrap().push(e)
    }

    fn all(&self) -> Vec<Event> {
        self.0.lock().unwrap().clone()
    }

    fn finished(&self) -> Vec<Output> {
        self.all()
            .into_iter()
            .filter_map(|e| match e {
                Event::Finished(o) => Some(o),
                _ => None,
            })
            .collect()
    }

    /// Wait for the `n`th event of the kind `pick` keeps, and return it.
    fn wait<T>(&self, n: usize, pick: impl Fn(&Event) -> Option<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(found) = self.all().iter().filter_map(&pick).nth(n - 1) {
                return found;
            }
            assert!(Instant::now() < deadline, "timed out: {:#?}", self.all());
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn wait_finished(&self, n: usize) -> Output {
        self.wait(n, |e| match e {
            Event::Finished(o) => Some(o.clone()),
            _ => None,
        })
    }

    fn wait_started(&self, n: usize) -> u64 {
        self.wait(n, |e| match e {
            Event::Started { id, .. } => Some(*id),
            _ => None,
        })
    }
}

fn project() -> (tempfile::TempDir, Job) {
    let dir = tempfile::tempdir().unwrap();
    let job = Job::for_root(dir.path().join("main.tex"));
    (dir, job)
}

/// Run `engine` once on `job` and return what it gave.
fn run_once(engine: Engine, job: &Job) -> Output {
    let events = Events::default();
    let compiler = Compiler::new(engine, Duration::ZERO, events.sink());
    compiler.request_now(job.clone());
    events.wait_finished(1)
}

/// A process writing `file` (see `grandchild` in fake_engine.rs), to tell
/// whether it is still alive.
struct Heartbeat(PathBuf);

impl Heartbeat {
    fn new(dir: &Path) -> Self {
        Heartbeat(dir.join("heartbeat"))
    }

    fn step(&self) -> String {
        format!("grandchild={}", self.0.display())
    }

    fn len(&self) -> u64 {
        std::fs::metadata(&self.0).map_or(0, |m| m.len())
    }

    fn wait_started(&self) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while self.len() == 0 {
            assert!(Instant::now() < deadline, "the grandchild never started");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[track_caller]
    fn assert_stopped(&self) {
        // Let a process that is being killed finish dying.
        std::thread::sleep(Duration::from_millis(200));
        let before = self.len();
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(self.len(), before, "the grandchild is still running");
    }
}

#[test]
fn debounce_coalesces_requests() {
    let (dir, job) = project();
    let events = Events::default();
    let compiler = Compiler::new(fake(&["pdf"]), Duration::from_millis(80), events.sink());
    for _ in 0..10 {
        compiler.request(job.clone());
        std::thread::sleep(Duration::from_millis(5));
    }
    let done = events.wait_finished(1);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(events.finished().len(), 1, "requests should coalesce");
    assert_eq!(done.status, Status::Success);
    assert_eq!(done.pdf.as_deref(), Some(job.artifact("pdf").as_path()));
    assert!(done.pdf_written);
    assert_eq!(done.synctex, None);
    let ignore = std::fs::read_to_string(dir.path().join(".texoria/.gitignore")).unwrap();
    assert!(ignore.lines().any(|l| l == "*"));
}

#[test]
fn set_debounce_applies_to_the_next_requests() {
    let (_dir, job) = project();
    let events = Events::default();
    let compiler = Compiler::new(fake(&[]), Duration::ZERO, events.sink());
    compiler.set_debounce(Duration::from_millis(400));
    let t = Instant::now();
    compiler.request(job.clone());
    events.wait_finished(1);
    assert!(t.elapsed() >= Duration::from_millis(400));
    // `request_now` does not wait.
    compiler.request_now(job);
    events.wait_finished(2);
}

#[test]
fn tells_a_pdf_left_from_before() {
    let (_dir, job) = project();
    std::fs::create_dir_all(&job.out_dir).unwrap();
    let old = std::fs::File::create(job.artifact("pdf")).unwrap();
    old.set_modified(SystemTime::now() - Duration::from_secs(3600))
        .unwrap();
    let done = run_once(fake(&[]), &job);
    assert!(done.pdf.is_some());
    assert!(!done.pdf_written);
}

#[test]
fn requests_during_run_are_queued_not_killed() {
    let (_dir, job) = project();
    let events = Events::default();
    let compiler = Compiler::new(
        fake(&["sleep=300"]),
        Duration::from_millis(10),
        events.sink(),
    );
    compiler.request_now(job.clone());
    events.wait_started(1);
    compiler.request(job.clone());
    compiler.request(job.clone());
    events.wait_finished(2);
    std::thread::sleep(Duration::from_millis(100));
    let done = events.finished();
    assert_eq!(done.len(), 2, "the queued requests are one run");
    assert!(done.iter().all(|o| o.status == Status::Success));
    assert!(done[0].elapsed >= Duration::from_millis(250));
}

#[test]
fn ids_increase_and_match() {
    let (_dir, job) = project();
    let events = Events::default();
    let compiler = Compiler::new(fake(&[]), Duration::ZERO, events.sink());
    for n in 1..=3 {
        compiler.request_now(job.clone());
        events.wait_finished(n);
    }
    let ids: Vec<(u64, u64)> = events
        .finished()
        .iter()
        .enumerate()
        .map(|(i, o)| (o.id, events.wait_started(i + 1)))
        .collect();
    assert_eq!(ids, [(1, 1), (2, 2), (3, 3)]);
    let started = events.all().into_iter().find_map(|e| match e {
        Event::Started { engine, .. } => Some(engine),
        _ => None,
    });
    let name = format!("fake-engine{}", std::env::consts::EXE_SUFFIX);
    assert_eq!(started, Some(name));
}

#[test]
fn runs_in_the_root_files_folder() {
    let (dir, job) = project();
    let cwd = dir.path().join("cwd.txt");
    let done = run_once(fake(&[&format!("cwd={}", cwd.display())]), &job);
    assert_eq!(done.status, Status::Success);
    let ran_in = PathBuf::from(std::fs::read_to_string(cwd).unwrap());
    assert_eq!(
        ran_in.canonicalize().unwrap(),
        dir.path().canonicalize().unwrap()
    );
}

#[test]
fn cancel_kills_process_tree() {
    let (dir, job) = project();
    let heartbeat = Heartbeat::new(dir.path());
    let events = Events::default();
    let compiler = Compiler::new(
        fake(&[&heartbeat.step(), "sleep=30000"]),
        Duration::ZERO,
        events.sink(),
    );
    compiler.request_now(job);
    heartbeat.wait_started();
    let t = Instant::now();
    compiler.cancel();
    let done = events.wait_finished(1);
    assert!(t.elapsed() < Duration::from_secs(5));
    assert_eq!(done.status, Status::Cancelled);
    assert!(done.diagnostics.is_empty());
    heartbeat.assert_stopped();
}

#[test]
fn cancel_with_nothing_running() {
    let (_dir, job) = project();
    let events = Events::default();
    let compiler = Compiler::new(fake(&[]), Duration::from_millis(300), events.sink());
    compiler.cancel();
    // A pending request is dropped too.
    compiler.request(job.clone());
    compiler.cancel();
    std::thread::sleep(Duration::from_millis(500));
    assert!(events.all().is_empty(), "{:#?}", events.all());
    compiler.request_now(job);
    assert_eq!(events.wait_finished(1).status, Status::Success);
}

#[test]
fn timeout_kills_runaway_engine() {
    let (dir, job) = project();
    let heartbeat = Heartbeat::new(dir.path());
    let events = Events::default();
    let compiler = Compiler::new(
        fake(&[&heartbeat.step(), "sleep=30000"]),
        Duration::ZERO,
        events.sink(),
    );
    compiler.set_timeout(Some(Duration::from_secs(1)));
    let t = Instant::now();
    compiler.request_now(job);
    let out = events.wait_finished(1);
    assert!(t.elapsed() < Duration::from_secs(10));
    assert_eq!(out.status, Status::Failed);
    assert_eq!(out.diagnostics[0].severity, Severity::Error);
    assert_eq!(
        out.diagnostics[0].message,
        "The build stopped after 1s (infinite loop?)"
    );
    heartbeat.assert_stopped();

    // No limit: it runs on.
    compiler.set_timeout(None);
}

#[test]
fn dropping_the_compiler_kills_the_run() {
    let (dir, job) = project();
    let heartbeat = Heartbeat::new(dir.path());
    let events = Events::default();
    let compiler = Compiler::new(
        fake(&[&heartbeat.step(), "sleep=30000"]),
        Duration::ZERO,
        events.sink(),
    );
    compiler.request_now(job);
    heartbeat.wait_started();
    let t = Instant::now();
    drop(compiler);
    assert!(t.elapsed() < Duration::from_secs(5));
    heartbeat.assert_stopped();
    assert_eq!(events.finished()[0].status, Status::Cancelled);
}

#[test]
fn changing_the_engine_reruns_with_the_new_one() {
    let (dir, job) = project();
    let events = Events::default();
    let compiler = Compiler::new(fake(&["sleep=30000"]), Duration::ZERO, events.sink());
    compiler.request_now(job.clone());
    events.wait_started(1);
    let marker = dir.path().join("second.txt");
    compiler.set_engine(fake(&[&format!("cwd={}", marker.display())]));
    let first = events.wait_finished(1);
    assert_eq!(first.status, Status::Cancelled);
    let second = events.wait_finished(2);
    assert_eq!(second.status, Status::Success);
    assert_eq!(second.id, 2);
    assert!(marker.is_file(), "the new engine ran");
}

#[test]
fn setting_the_same_engine_changes_nothing() {
    let (_dir, job) = project();
    let events = Events::default();
    let engine = fake(&["sleep=300"]);
    let compiler = Compiler::new(engine.clone(), Duration::ZERO, events.sink());
    compiler.request_now(job.clone());
    events.wait_started(1);
    compiler.set_engine(engine);
    assert_eq!(events.wait_finished(1).status, Status::Success);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(events.finished().len(), 1);

    // With nothing running, a new engine is only used for the next run.
    compiler.set_engine(fake(&["exit=1"]));
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(events.finished().len(), 1);
    compiler.request_now(job);
    assert_eq!(events.wait_finished(2).status, Status::Failed);
}

#[test]
fn errors_in_log_mark_failure() {
    let (dir, job) = project();
    let out = run_once(
        fake(&["log=(./main.tex\\n! Undefined control sequence.\\nl.3 \\foo\\n\\n)\\n"]),
        &job,
    );
    assert_eq!(out.status, Status::Failed);
    assert_eq!(out.diagnostics.len(), 1);
    assert_eq!(out.diagnostics[0].line, Some(3));
    assert_eq!(
        out.diagnostics[0].file.as_deref(),
        Some(dir.path().join("main.tex").as_path())
    );
}

#[test]
fn nonzero_exit_is_a_failure() {
    let (_dir, job) = project();
    let out = run_once(fake(&["pdf", "exit=3"]), &job);
    assert_eq!(out.status, Status::Failed);
    assert!(out.diagnostics.is_empty());
    assert!(out.pdf_written);
}

#[test]
fn warnings_alone_are_a_success() {
    let (_dir, job) = project();
    let log = "log=(./main.tex\\nLaTeX Warning: Reference `x' undefined on input line 2.\\n\\n)\\n";
    let out = run_once(fake(&[log]), &job);
    assert_eq!(out.status, Status::Success);
    assert_eq!(out.diagnostics.len(), 1);
    assert_eq!(out.diagnostics[0].severity, Severity::Warning);
}

#[test]
fn finds_synctex_compressed_or_not() {
    let (_dir, job) = project();
    let out = run_once(fake(&["synctex"]), &job);
    assert_eq!(out.synctex, Some(job.artifact("synctex")));
    let out = run_once(fake(&["synctex-gz"]), &job);
    assert_eq!(out.synctex, Some(job.artifact("synctex.gz")));
}

#[test]
fn tells_when_the_document_needs_shell_escape() {
    let (_dir, mut job) = project();
    let log = "log=! Package minted Error: You must invoke LaTeX with the -shell-escape flag.\\n";
    let out = run_once(fake(&[log]), &job);
    assert!(out.needs_shell_escape);
    // Not when it was on already: the error is something else.
    job.shell_escape = true;
    let out = run_once(fake(&[log]), &job);
    assert!(!out.needs_shell_escape);
}

#[test]
fn a_log_left_alone_is_read_again() {
    // latexmk leaves the log alone when nothing changed since a failed run,
    // and exits with an error: that log is still the document's.
    let (_dir, job) = project();
    run_once(fake(&["log=! Oops.\\nl.1 x\\n"]), &job);
    let out = run_once(fake(&["exit=12"]), &job);
    assert_eq!(out.status, Status::Failed);
    assert_eq!(out.diagnostics.len(), 1, "{:#?}", out.diagnostics);
    assert_eq!(out.diagnostics[0].message, "Oops");
}

#[test]
fn huge_logs_are_cut() {
    let (_dir, job) = project();
    let error = "! Late error.\\nl.1 x\\n";
    let out = run_once(fake(&[&format!("big-log=1000:{error}")]), &job);
    assert_eq!(out.diagnostics.len(), 1);
    // Past the first 32 MiB: not read.
    let out = run_once(fake(&[&format!("big-log={}:{error}", 32 << 20)]), &job);
    assert!(out.diagnostics.is_empty(), "{:#?}", out.diagnostics);
    assert_eq!(out.status, Status::Success);
}

#[test]
fn engine_output_goes_to_the_transcript() {
    let (_dir, job) = project();
    run_once(fake(&["print=hello from the engine"]), &job);
    let transcript = std::fs::read_to_string(job.out_dir.join("texoria-run.txt")).unwrap();
    assert!(transcript.contains("hello from the engine"), "{transcript}");
}

#[test]
fn runs_without_a_transcript() {
    let (_dir, job) = project();
    // Something that cannot be opened as a file is where the transcript goes.
    std::fs::create_dir_all(job.out_dir.join("texoria-run.txt")).unwrap();
    let out = run_once(fake(&["print=lost", "pdf"]), &job);
    assert_eq!(out.status, Status::Success);
    assert!(out.pdf_written);
}

#[test]
fn spawn_failures() {
    let (dir, job) = project();
    let events = Events::default();
    let missing = Engine {
        kind: EngineKind::Custom {
            program: dir.path().join("no-such-engine"),
            args: vec![],
        },
        extra_path: vec![],
    };
    let compiler = Compiler::new(missing, Duration::ZERO, events.sink());
    compiler.request_now(job.clone());
    let failed = |n| {
        events.wait(n, |e| match e {
            Event::SpawnFailed { id, error } => Some((*id, error.clone())),
            _ => None,
        })
    };
    let (id, error) = failed(1);
    assert_eq!(id, 1);
    assert!(
        error.starts_with("failed to launch no-such-engine"),
        "{error}"
    );

    // The build folder cannot be created under a file.
    compiler.set_engine(fake(&[]));
    let blocker = dir.path().join("file");
    std::fs::write(&blocker, "").unwrap();
    let mut job = job;
    job.out_dir = blocker.join("build");
    compiler.request_now(job);
    let (id, error) = failed(2);
    assert_eq!(id, 2);
    assert!(error.starts_with("cannot create build dir"), "{error}");
    assert!(events.finished().is_empty());
}

#[test]
fn build_dir_outside_texoria_gets_no_gitignore() {
    let (dir, mut job) = project();
    job.out_dir = dir.path().join("out");
    let out_dir = format!("out={}", job.out_dir.display());
    let out = run_once(fake(&[&out_dir, "pdf"]), &job);
    assert_eq!(out.pdf, Some(dir.path().join("out/main.pdf")));
    assert!(!dir.path().join(".gitignore").exists());
    assert!(!dir.path().join("out/.gitignore").exists());
}

#[test]
fn an_existing_gitignore_is_kept() {
    let (dir, job) = project();
    let ignore = dir.path().join(".texoria/.gitignore");
    std::fs::create_dir_all(ignore.parent().unwrap()).unwrap();
    std::fs::write(&ignore, "mine\n").unwrap();
    run_once(fake(&[]), &job);
    assert_eq!(std::fs::read_to_string(ignore).unwrap(), "mine\n");
}

#[test]
fn a_cancelled_run_reports_nothing_from_the_log() {
    let (_dir, job) = project();
    let log = "log=! Package minted Error: You must invoke LaTeX with the -shell-escape flag.\\n";
    assert!(run_once(fake(&[log]), &job).needs_shell_escape);
    let events = Events::default();
    let compiler = Compiler::new(fake(&["sleep=30000"]), Duration::ZERO, events.sink());
    compiler.request_now(job);
    events.wait_started(1);
    compiler.cancel();
    let out = events.wait_finished(1);
    assert_eq!(out.status, Status::Cancelled);
    assert!(out.diagnostics.is_empty());
    assert!(!out.needs_shell_escape);
}
