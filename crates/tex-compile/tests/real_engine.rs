//! End-to-end compile of the sample project with whatever engine the host
//! has. Skipped (passes trivially) when no TeX engine is installed, unless
//! `TEXORIA_REQUIRE_TEX=1` (`TEXORIA_REQUIRE_TECTONIC=1` for Tectonic).

use std::sync::mpsc;
use std::time::Duration;

use tex_compile::{Compiler, Event, Job, Status, detect};

fn vendor_tectonic() -> std::path::PathBuf {
    test_support::vendor("tectonic")
}

#[test]
fn compiles_sample_project() {
    let Some(engine) = detect(None).into_iter().next() else {
        test_support::skip("TEX", "no TeX engine found");
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
        test_support::skip("TECTONIC", "tectonic not available");
        return;
    };
    compile_sample(engine);
}

fn compile_sample(engine: tex_compile::Engine) {
    let src = test_support::fixtures().join("sample");
    let dir = tempfile::tempdir().unwrap();
    test_support::copy_dir(&src, dir.path());

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
