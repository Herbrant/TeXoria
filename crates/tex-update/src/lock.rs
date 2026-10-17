//! One update at a time among the running copies of TeXoria.
//!
//! The copy that downloads an update holds an OS lock on a file until the
//! update is installed, dropped or has failed; the others see it busy and
//! leave it alone. The system releases the lock when its process ends, so a
//! copy that crashed or was killed never blocks the next update.

use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// The lock of this user's updates (per-user temporary folder on macOS and
/// Windows).
pub fn default_path() -> PathBuf {
    std::env::temp_dir().join("texoria-update.lock")
}

/// Held while this copy downloads or installs an update.
#[derive(Debug)]
pub struct Lock {
    _file: File,
}

impl Lock {
    /// The lock, or `None` if another copy holds it.
    pub fn try_acquire() -> Result<Option<Self>> {
        Self::try_acquire_at(&default_path())
    }

    pub fn try_acquire_at(path: &Path) -> Result<Option<Self>> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)
            .with_context(|| format!("opening {}", path.display()))?;
        match file.try_lock() {
            Ok(()) => Ok(Some(Self { _file: file })),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Error(e)) => {
                Err(e).with_context(|| format!("locking {}", path.display()))
            }
        }
    }

    /// Whether another copy holds the lock. An error counts as free: it must
    /// not keep this copy from updating.
    pub fn busy() -> bool {
        Self::busy_at(&default_path())
    }

    pub fn busy_at(path: &Path) -> bool {
        matches!(Self::try_acquire_at(path), Ok(None))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_holder_at_a_time() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("update.lock");
        assert!(!Lock::busy_at(&path));
        let held = Lock::try_acquire_at(&path).unwrap().unwrap();
        // Another open file, as in another process.
        assert!(Lock::try_acquire_at(&path).unwrap().is_none());
        assert!(Lock::busy_at(&path));
        drop(held);
        assert!(!Lock::busy_at(&path));
        assert!(Lock::try_acquire_at(&path).unwrap().is_some());
    }

    #[test]
    fn a_lock_that_cannot_be_opened_counts_as_free() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing").join("update.lock");
        let err = Lock::try_acquire_at(&path).unwrap_err();
        assert!(err.to_string().contains("opening"), "{err}");
        assert!(!Lock::busy_at(&path));
        // A folder in its place.
        assert!(Lock::try_acquire_at(dir.path()).is_err());
        assert!(default_path().ends_with("texoria-update.lock"));
    }

    /// Set in the copy of the test program that holds the lock.
    const HOLD: &str = "TEXORIA_TEST_HOLD_LOCK";

    /// Another process holds the lock until it ends, crashed or not. The test
    /// program runs this test again as that process.
    #[test]
    fn held_by_another_process() {
        if let Some(path) = std::env::var_os(HOLD) {
            let path = PathBuf::from(path);
            let _held = Lock::try_acquire_at(&path).unwrap().unwrap();
            std::fs::write(path.with_extension("ready"), "").unwrap();
            std::thread::sleep(std::time::Duration::from_secs(60));
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("update.lock");
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "lock::tests::held_by_another_process",
                "--test-threads=1",
            ])
            .env(HOLD, &path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let ready = path.with_extension("ready");
        let started = std::time::Instant::now();
        while !ready.exists() {
            assert!(
                started.elapsed().as_secs() < 30,
                "the other process never took the lock"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(Lock::busy_at(&path));
        assert!(Lock::try_acquire_at(&path).unwrap().is_none());
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(!Lock::busy_at(&path));
        assert!(Lock::try_acquire_at(&path).unwrap().is_some());
    }
}
