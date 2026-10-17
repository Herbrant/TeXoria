//! Helpers shared by the workspace's tests. Only ever a dev-dependency, and it depends on no
//! other workspace crate (a dev-dependency cycle would build two copies of that crate).

pub mod git;
pub mod http;

use std::path::PathBuf;

/// The shared fixtures folder, `tests/fixtures` at the repository root.
pub fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures")
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
    }

    #[test]
    fn skips_unless_required() {
        // No test sets this one.
        assert!(!required("NOTHING_AT_ALL"));
        skip("NOTHING_AT_ALL", "nothing");
    }
}
