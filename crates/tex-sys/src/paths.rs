//! Paths as other programs take them, the user's home folder, and the `PATH` programs are
//! looked for in and run with.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Reads an environment variable: the process's ([`process_env`]), or another one in tests.
pub type Env<'a> = &'a dyn Fn(&str) -> Option<OsString>;

/// The process's environment variable `key`.
pub fn process_env(key: &str) -> Option<OsString> {
    std::env::var_os(key)
}

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

/// [`plain`] for a drive path (`\\?\C:\dir` as `C:\dir`) only: a verbatim UNC path stays as it
/// is (git and the installer get no other spelling of it).
pub fn plain_drive(path: &Path) -> PathBuf {
    if cfg!(windows)
        && let Some(s) = path.to_str().and_then(unverbatim)
        && !s.starts_with(r"\\")
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

/// `path` without its `.` and `..`, read as names (links are not followed): `a/../b` is `b`,
/// `/..` is `/`, and a relative path keeps the `..` that climb above it.
pub fn lexical(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => match out.components().next_back() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                Some(Component::RootDir | Component::Prefix(_)) => {}
                _ => out.push(".."),
            },
            other => out.push(other),
        }
    }
    out
}

/// The user's home folder: `HOME`, else `USERPROFILE` (Windows); an empty value is no folder.
pub fn home(env: Env) -> Option<PathBuf> {
    ["HOME", "USERPROFILE"]
        .into_iter()
        .find_map(|k| env(k).filter(|v| !v.is_empty()))
        .map(PathBuf::from)
}

/// A `PATH` of `dirs`, in order and each once, without relative entries: those name the
/// current directory on Unix, which may be a project where a program of the same name was
/// planted. A dir that cannot be in a `PATH` (its name has the separator) is left out rather
/// than failing the whole.
pub fn absolute_path(dirs: impl IntoIterator<Item = PathBuf>) -> OsString {
    let mut seen = std::collections::HashSet::new();
    let all: Vec<PathBuf> = dirs
        .into_iter()
        .filter(|d| d.is_absolute() && std::env::join_paths([d]).is_ok())
        .filter(|d| seen.insert(d.clone()))
        .collect();
    if all.is_empty() && cfg!(unix) {
        // An empty `PATH` is one empty entry, which `execvp` reads as the
        // current directory.
        return OsString::from("/usr/bin:/bin");
    }
    std::env::join_paths(all).unwrap_or_default()
}

/// Look for the program `name` on `path` (a `PATH`) first, then in the `fallback` dirs.
/// Returns the program and, if found in a fallback dir, that dir (to add to the `PATH` it
/// runs with).
pub fn find_program(
    name: &str,
    path: Option<OsString>,
    fallback: &[PathBuf],
) -> Option<(PathBuf, Option<PathBuf>)> {
    let cwd = std::env::current_dir().unwrap_or_default();
    if let Ok(p) = which::which_in(name, path, cwd) {
        return Some((p, None));
    }
    let exe = format!("{name}{}", std::env::consts::EXE_SUFFIX);
    fallback
        .iter()
        .map(|d| d.join(&exe))
        .find(|p| p.is_file())
        .map(|p| {
            let dir = p.parent().map(Path::to_path_buf);
            (p, dir)
        })
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
        assert_eq!(
            plain(Path::new(r"\\?\UNC\server\share")),
            Path::new(r"\\server\share")
        );
        assert_eq!(plain_drive(Path::new(r"\\?\C:\x")), Path::new(r"C:\x"));
        let unc = Path::new(r"\\?\UNC\server\share");
        assert_eq!(plain_drive(unc), unc);
        assert_eq!(plain_drive(Path::new(r"C:\dir")), Path::new(r"C:\dir"));
    }

    #[cfg(unix)]
    #[test]
    fn plain_leaves_unix_names_alone() {
        // A legal (if odd) file name on Unix.
        for plain in [plain, plain_drive] {
            assert_eq!(plain(Path::new(r"\\?\C:\x")), Path::new(r"\\?\C:\x"));
            assert_eq!(plain(Path::new("/a/b")), Path::new("/a/b"));
        }
    }

    #[test]
    fn lexical_paths() {
        for (path, lexical_path) in [
            ("/p/./a/./b/../c.tex", "/p/a/c.tex"),
            ("/../../x.tex", "/x.tex"),
            ("../x.tex", "../x.tex"),
            ("../a/../../x.tex", "../../x.tex"),
            ("a/b/../c.tex", "a/c.tex"),
            ("a/../..", ".."),
            ("./", ""),
        ] {
            assert_eq!(lexical(Path::new(path)), Path::new(lexical_path), "{path}");
        }
    }

    proptest::proptest! {
        #[test]
        fn lexical_paths_are_stable(
            path in "(/|)([a-z]{1,3}|\\.|\\.\\.)(/([a-z]{1,3}|\\.|\\.\\.)){0,6}"
        ) {
            use std::path::Component;
            let once = lexical(Path::new(&path));
            proptest::prop_assert_eq!(lexical(&once), once.clone());
            // No `.`, and `..` only first, climbing above a relative path.
            let names: Vec<_> = once.components().collect();
            proptest::prop_assert!(!names.contains(&Component::CurDir));
            let ups = names.iter().take_while(|c| **c == Component::ParentDir).count();
            proptest::prop_assert!(!names[ups..].contains(&Component::ParentDir));
            proptest::prop_assert!(ups == 0 || !once.is_absolute());
        }
    }

    #[test]
    fn home_is_home_or_the_user_profile() {
        let env = |vars: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                vars.iter()
                    .find(|(key, _)| *key == k)
                    .map(|(_, v)| OsString::from(v))
            }
        };
        assert_eq!(home(&env(&[("HOME", "/h")])), Some(PathBuf::from("/h")));
        let both = env(&[("USERPROFILE", "/p"), ("HOME", "/h")]);
        assert_eq!(home(&both), Some(PathBuf::from("/h")));
        let empty = env(&[("HOME", ""), ("USERPROFILE", "/p")]);
        assert_eq!(home(&empty), Some(PathBuf::from("/p")));
        assert_eq!(home(&env(&[("HOME", "")])), None);
        assert_eq!(home(&env(&[])), None);
        assert!(process_env("TEXORIA_NO_SUCH_VARIABLE").is_none());
    }

    #[test]
    fn path_keeps_absolute_dirs_in_order_once() {
        let abs = |p: &str| std::env::temp_dir().join(p);
        let dirs = [
            abs("a"),
            PathBuf::from("."),
            PathBuf::from("rel/bin"),
            abs("b"),
            abs("a"),
        ];
        let path = absolute_path(dirs);
        let got: Vec<PathBuf> = std::env::split_paths(&path).collect();
        assert_eq!(got, [abs("a"), abs("b")]);
    }

    #[test]
    fn path_survives_a_dir_that_cannot_be_joined() {
        let bad = if cfg!(windows) { "C:\\a\"b" } else { "/a:b" };
        let good = std::env::temp_dir();
        let joined = absolute_path([PathBuf::from(bad), good.clone()]);
        let dirs: Vec<PathBuf> = std::env::split_paths(&joined).collect();
        assert_eq!(dirs, vec![good], "{joined:?}");
        // Never empty on Unix: an empty `PATH` is one empty entry, which
        // `execvp` reads as the current directory.
        for empty in [vec![], vec![PathBuf::new()], vec![PathBuf::from(".")]] {
            let path = absolute_path(empty);
            assert_eq!(path.is_empty(), cfg!(windows), "{path:?}");
            if !path.is_empty() {
                assert!(std::env::split_paths(&path).all(|d| d.is_absolute()));
            }
        }
    }

    /// An empty file that `which` takes for a program.
    fn program(dir: &Path, name: &str) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&path, "").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }

    #[test]
    fn programs_on_the_path_then_in_fallback_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let on_path = program(&dir.path().join("bin"), "tool");
        let fallback = dir.path().join("fallback");
        let other = program(&fallback, "other");
        let path = std::env::join_paths([dir.path().join("bin")]).unwrap();
        let dirs = [dir.path().join("missing"), fallback.clone()];
        assert_eq!(
            find_program("tool", Some(path.clone()), &dirs),
            Some((on_path, None))
        );
        assert_eq!(
            find_program("other", Some(path.clone()), &dirs),
            Some((other, Some(fallback)))
        );
        assert_eq!(find_program("nothing", Some(path), &dirs), None);
    }
}
