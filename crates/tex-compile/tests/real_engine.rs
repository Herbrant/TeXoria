//! End-to-end compile of the sample project with whatever engine the host
//! has. Skipped (passes trivially) when no TeX engine is installed.

use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use tex_compile::{Compiler, Event, Job, Status, detect};

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let name = entry.file_name();
        if name == ".texoria" {
            continue;
        }
        let dst = to.join(&name);
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &dst);
        } else {
            std::fs::copy(entry.path(), dst).unwrap();
        }
    }
}

fn vendor_tectonic() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vendor/tectonic")
}

#[test]
fn compiles_sample_project() {
    let Some(engine) = detect(None).into_iter().next() else {
        eprintln!("no TeX engine found; skipping");
        return;
    };
    compile_sample(engine);
}

/// Exercises the bundled Tectonic when `scripts/fetch-tectonic.sh` has run.
/// The first run downloads TeX support files, so it is opt-in.
#[test]
#[ignore = "downloads TeX bundle on first run; run with --ignored"]
fn compiles_sample_with_tectonic() {
    let Some(engine) = detect(Some(&vendor_tectonic()))
        .into_iter()
        .find(|e| matches!(e.kind, tex_compile::EngineKind::Tectonic { .. }))
    else {
        eprintln!("tectonic not available; skipping");
        return;
    };
    compile_sample(engine);
}

fn compile_sample(engine: tex_compile::Engine) {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/sample");
    let dir = tempfile::tempdir().unwrap();
    copy_dir(&src, dir.path());

    let (tx, rx) = mpsc::channel();
    let compiler = Compiler::new(engine, Duration::ZERO, move |e| {
        let _ = tx.send(e);
    });
    compiler.request_now(Job::for_root(dir.path().join("main.tex")));

    let out = loop {
        match rx
            .recv_timeout(Duration::from_secs(120))
            .expect("compile timed out")
        {
            Event::Finished(o) => break o,
            Event::SpawnFailed { error, .. } => panic!("{error}"),
            Event::Started { .. } => {}
        }
    };
    assert_eq!(out.status, Status::Success, "{:#?}", out.diagnostics);
    assert!(out.pdf.is_some());
    assert!(out.synctex.is_some());
    // The sample is meant to be clean: it doubles as the README screenshot.
    assert!(out.diagnostics.is_empty(), "{:#?}", out.diagnostics);
}
