//! Paths in the form TeXoria shows, stores and compares them.

use std::path::{Path, PathBuf};

/// `path` resolved (links, `..`), in its [`plain`] form; `path` itself if it does not exist.
pub fn canonical(path: &Path) -> PathBuf {
    plain(&std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()))
}

/// On Windows, a verbatim path as `canonicalize` returns it (`\\?\C:\dir`,
/// `\\?\UNC\server\share`) in its usual spelling (`C:\dir`, `\\server\share`): the one users
/// know, that other programs take, and that settings written before kept.
pub fn plain(path: &Path) -> PathBuf {
    if cfg!(windows)
        && let Some(s) = path.to_str().and_then(unverbatim)
    {
        return PathBuf::from(s);
    }
    path.to_path_buf()
}

/// `\\?\C:\dir` as `C:\dir` and `\\?\UNC\server\share` as `\\server\share`.
fn unverbatim(path: &str) -> Option<String> {
    let rest = path.strip_prefix(r"\\?\")?;
    if let Some(unc) = rest.strip_prefix(r"UNC\") {
        return Some(format!(r"\\{unc}"));
    }
    let drive = rest.as_bytes();
    (drive.len() >= 2 && drive[0].is_ascii_alphabetic() && drive[1] == b':')
        .then(|| rest.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verbatim_paths_lose_their_prefix() {
        assert_eq!(
            unverbatim(r"\\?\C:\Users\me\paper").as_deref(),
            Some(r"C:\Users\me\paper")
        );
        assert_eq!(
            unverbatim(r"\\?\UNC\server\share\x").as_deref(),
            Some(r"\\server\share\x")
        );
        // Other verbatim forms have no plain spelling.
        assert_eq!(unverbatim(r"\\?\Volume{1234}\x"), None);
        assert_eq!(unverbatim(r"C:\dir"), None);
        assert_eq!(unverbatim("/home/me"), None);
    }

    #[test]
    fn canonical_paths_are_plain_and_resolved() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("a")).unwrap();
        let p = canonical(&dir.path().join("a/../a"));
        assert!(p.is_absolute());
        assert!(!p.to_string_lossy().starts_with(r"\\?\"), "{}", p.display());
        assert!(p.ends_with("a"));
        assert!(!p.components().any(|c| c.as_os_str() == ".."));
        // A path that does not exist stays as it is.
        let missing = dir.path().join("missing");
        assert_eq!(canonical(&missing), plain(&missing));
    }

    #[cfg(windows)]
    #[test]
    fn plain_on_windows() {
        assert_eq!(plain(Path::new(r"\\?\C:\x")), Path::new(r"C:\x"));
    }

    #[cfg(unix)]
    #[test]
    fn plain_leaves_unix_names_alone() {
        // A legal (if odd) file name on Unix.
        assert_eq!(plain(Path::new(r"\\?\C:\x")), Path::new(r"\\?\C:\x"));
    }
}
