//! Crash-safe file writes.

use std::io::{self, Write};
use std::path::Path;

/// Replace the contents of `path` atomically: write a temporary sibling, flush
/// it to disk and rename it over the original. A crash never leaves a
/// truncated file, and an engine that is reading the file never sees half of
/// it. Symlinks are followed (the link stays a link) and permissions are kept;
/// a new file gets the usual ones (the umask's), not a temporary file's.
pub fn write(path: &Path, contents: &[u8]) -> io::Result<()> {
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let dir = target
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let existing = std::fs::metadata(&target).ok();
    let replace = || -> io::Result<()> {
        let mut builder = tempfile::Builder::new();
        builder.prefix(".texoria-").suffix(".tmp");
        #[cfg(unix)]
        if existing.is_none() {
            use std::os::unix::fs::PermissionsExt;
            // Narrowed by the umask, like any file a program creates.
            builder.permissions(std::fs::Permissions::from_mode(0o666));
        }
        let mut tmp = builder.tempfile_in(dir)?;
        tmp.write_all(contents)?;
        if let Some(meta) = &existing {
            tmp.as_file().set_permissions(meta.permissions())?;
        }
        tmp.as_file().sync_all()?;
        tmp.persist(&target).map_err(|e| e.error)?;
        Ok(())
    };
    replace().or_else(|e| {
        // Windows refuses to replace a file another process holds open (the
        // engine), and the directory may not be writable: write in place.
        log::debug!(
            "atomic write of {} failed ({e}); writing in place",
            target.display()
        );
        std::fs::write(&target, contents)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_contents() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("main.tex");
        write(&p, b"first").unwrap();
        write(&p, b"second").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"second");
        // No temporary files left behind.
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn keeps_permissions_and_symlinks() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.tex");
        let link = dir.path().join("link.tex");
        std::fs::write(&real, "old").unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o640)).unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();

        write(&link, b"new").unwrap();
        assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read(&real).unwrap(), b"new");
        let mode = std::fs::metadata(&real).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o640);
    }

    #[cfg(unix)]
    #[test]
    fn new_files_get_the_usual_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let plain = dir.path().join("plain.tex");
        std::fs::write(&plain, "x").unwrap();
        let new = dir.path().join("new.tex");
        write(&new, b"x").unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&new), mode(&plain));
    }
}
