//! Helpers shared by the workspace's tests. Only ever a dev-dependency, and it depends on no
//! other workspace crate (a dev-dependency cycle would build two copies of that crate).

pub mod alloc;
pub mod git;
pub mod http;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The shared fixtures folder, `tests/fixtures` at the repository root.
pub fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures")
}

/// The repository's `vendor/<name>` folder (`pdfium`, `tectonic`), which `scripts/fetch-*.sh`
/// fill.
pub fn vendor(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../vendor")
        .join(name)
}

/// `vendor/pdfium`, if `scripts/fetch-pdfium.sh` fetched it; else the test [`skip`]s.
pub fn pdfium() -> Option<PathBuf> {
    let dir = vendor("pdfium");
    let found = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .any(|e| {
            let name = e.file_name().to_string_lossy().to_lowercase();
            name.contains("pdfium") && !name.ends_with(".txt")
        });
    if !found {
        skip("PDFIUM", "vendor/pdfium is missing");
        return None;
    }
    Some(dir)
}

/// Write `text` at `rel` under `dir`, with the folders it needs; its path.
#[track_caller]
pub fn write(dir: &Path, rel: &str, text: impl AsRef<[u8]>) -> PathBuf {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, text).unwrap();
    path
}

/// [`write`] each `(rel, text)` of `files` under `dir`.
#[track_caller]
pub fn write_files(dir: &Path, files: &[(&str, &str)]) {
    for (rel, text) in files {
        write(dir, rel, text);
    }
}

/// Copy the folder `from` into `to`, without the build folder (`.texoria`) of a project.
#[track_caller]
pub fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let (name, path) = (entry.file_name(), entry.path());
        if name == ".texoria" {
            continue;
        }
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&path, &to.join(name));
        } else {
            std::fs::copy(&path, to.join(name)).unwrap();
        }
    }
}

/// A program `name` in `dir` (with `.exe` on Windows), made of `contents` (a script, or
/// nothing for a program that `which` finds but that never runs); its path.
#[track_caller]
pub fn executable(dir: &Path, name: &str, contents: &str) -> PathBuf {
    let path = write(
        dir,
        &format!("{name}{}", std::env::consts::EXE_SUFFIX),
        contents,
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    path
}

/// Whether `done` holds within `timeout`, asked again every few milliseconds.
pub fn within(timeout: Duration, mut done: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if done() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// What `found` gives once it gives something, asked again every few milliseconds; `None`
/// if it still gives nothing after `timeout`.
pub fn eventually<T>(timeout: Duration, mut found: impl FnMut() -> Option<T>) -> Option<T> {
    let mut result = None;
    within(timeout, || {
        result = found();
        result.is_some()
    });
    result
}

/// A test that needs something the host may lack (`need`: `TEX`, `TECTONIC`, `PDFIUM`, `GIT`)
/// calls this and returns early: it passes trivially, unless `TEXORIA_REQUIRE_<need>=1` is set
/// (as in CI), where a missing tool is a failure rather than a silent skip.
#[track_caller]
pub fn skip(need: &str, why: &str) {
    if required(need) {
        panic!("{why}, and TEXORIA_REQUIRE_{need} is set");
    }
    eprintln!("{why}; skipping");
}

/// Whether `TEXORIA_REQUIRE_<need>` asks for `need` to be present.
pub fn required(need: &str) -> bool {
    std::env::var_os(format!("TEXORIA_REQUIRE_{need}")).is_some_and(|v| v == "1")
}

/// Text with a `|` cursor marker: the text without it, and the marker's byte offset.
#[track_caller]
pub fn at(marked: &str) -> (String, usize) {
    let offset = marked.find('|').expect("no | cursor marker");
    (marked.replacen('|', "", 1), offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_the_cursor_marker() {
        assert_eq!(at("te|xt"), ("text".to_string(), 2));
        assert_eq!(at("|"), (String::new(), 0));
        assert_eq!(at("é|a|b"), ("éa|b".to_string(), 2));
    }

    #[test]
    fn finds_the_fixtures() {
        assert!(fixtures().join("sample/main.tex").is_file());
        assert!(vendor("pdfium").ends_with("vendor/pdfium"));
    }

    #[test]
    fn writes_copies_and_makes_programs() {
        let dir = std::env::temp_dir().join(format!("test-support-{}", std::process::id()));
        write_files(&dir, &[("a/b.tex", "x"), ("c.tex", "y")]);
        write(&dir, ".texoria/build.log", "z");
        let copy = dir.join("copy");
        copy_dir(&dir.join("a"), &copy);
        assert_eq!(std::fs::read_to_string(copy.join("b.tex")).unwrap(), "x");
        copy_dir(&dir, &dir.with_extension("copy"));
        assert!(!dir.with_extension("copy").join(".texoria").exists());
        let program = executable(&dir, "tool", "");
        assert!(program.is_file());
        for d in [dir.clone(), dir.with_extension("copy")] {
            std::fs::remove_dir_all(d).unwrap();
        }
    }

    #[test]
    fn waits_until_done() {
        let mut n = 0;
        assert!(within(Duration::from_secs(5), || {
            n += 1;
            n == 3
        }));
        assert!(!within(Duration::from_millis(20), || false));
        let mut m = 0;
        let found = eventually(Duration::from_secs(5), || {
            m += 1;
            (m == 3).then_some(m)
        });
        assert_eq!(found, Some(3));
        assert_eq!(eventually(Duration::from_millis(20), || None::<()>), None);
    }

    #[test]
    fn skips_unless_required() {
        // No test sets this one.
        assert!(!required("NOTHING_AT_ALL"));
        skip("NOTHING_AT_ALL", "nothing");
    }
}
