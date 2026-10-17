//! Download a release file next to what it replaces, checked against the
//! signed `SHA256SUMS`.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use semver::Version;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

use crate::install::InstallKind;
use crate::lock::Lock;
use crate::release::{Release, Source};

/// The packages are ~30–60 MB.
const MAX_SIZE: u64 = 1 << 30;
/// For the response to start. ureq has no limit between two reads of the
/// body: a stalled transfer only ends with `timeout_recv_body`.
const STALL: Duration = Duration::from_secs(60);
/// Prefix of the staging folders.
pub(crate) const STAGING: &str = ".texoria-update-";
/// In each staging folder, locked by the copy that uses it.
const IN_USE: &str = ".texoria-update.lock";
/// A staging folder without its lock (made by an older version, or by a copy
/// stopped before it could lock it) is left alone for this long.
const UNLOCKED_AGE: Duration = Duration::from_secs(60 * 60);

/// A verified release file, ready to be installed (see [`Update::install`]).
pub struct Update {
    pub version: Version,
    pub(crate) kind: InstallKind,
    /// Keeps the other running copies from removing `staging`. Before it:
    /// dropped first (Windows cannot delete an open file).
    pub(crate) in_use: Option<Lock>,
    /// Removed with what is in it unless the update is installed.
    pub(crate) staging: TempDir,
    pub(crate) file: PathBuf,
}

impl std::fmt::Debug for Update {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Update")
            .field("version", &self.version)
            .field("file", &self.file)
            .finish()
    }
}

/// Download the file of `release` that updates `kind`. `progress` gets the
/// bytes received and the total, if known; setting `cancel` stops the download.
pub fn download(
    source: &Source,
    release: &Release,
    kind: &InstallKind,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64, Option<u64>),
) -> Result<Update> {
    let name = kind
        .asset(&release.version, std::env::consts::ARCH)
        .ok_or_else(|| anyhow!("this installation cannot update itself"))?;
    let expected = release
        .sha256(&name)
        .ok_or_else(|| anyhow!("release {} has no {name}", release.version))?;
    let (staging, in_use) = staging_dir(kind)?;
    let file = staging.path().join(&name);

    let url = source.asset_url(&release.version, &name);
    log::info!("downloading {url}");
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(30)))
        .timeout_recv_response(Some(STALL))
        .timeout_recv_body(Some(Duration::from_secs(30 * 60)))
        .user_agent(crate::USER_AGENT)
        .build()
        .into();
    let mut resp = agent
        .get(&url)
        .call()
        .with_context(|| format!("downloading {url}"))?;
    let total = resp.body().content_length();
    let reader = resp.body_mut().with_config().limit(MAX_SIZE).reader();
    let out = fs::File::create(&file).with_context(|| format!("creating {}", file.display()))?;
    copy_verified(reader, out, expected, cancel, |n| progress(n, total))
        .with_context(|| format!("downloading {url}"))?;
    Ok(Update {
        version: release.version.clone(),
        kind: kind.clone(),
        in_use: Some(in_use),
        staging,
        file,
    })
}

/// A new folder next to what the update replaces, and its lock. Folders
/// left by an update that was interrupted (the app was killed) are removed
/// first; another running copy may be downloading too (the [`crate::Lock`]
/// of updates is not always taken), so only those whose lock is free.
fn staging_dir(kind: &InstallKind) -> Result<(TempDir, Lock)> {
    let parent = kind
        .staging_parent()
        .ok_or_else(|| anyhow!("this installation cannot update itself"))?;
    remove_stale(&parent, UNLOCKED_AGE);
    let dir = tempfile::Builder::new()
        .prefix(STAGING)
        .tempdir_in(&parent)
        .with_context(|| format!("creating a folder in {}", parent.display()))?;
    let lock = Lock::try_acquire_at(&dir.path().join(IN_USE))?
        .ok_or_else(|| anyhow!("{} is in use", dir.path().display()))?;
    Ok((dir, lock))
}

/// Remove the staging folders no running copy uses: those whose lock is
/// free, and those without one older than `unlocked_age`.
fn remove_stale(parent: &Path, unlocked_age: Duration) {
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    let unused = |dir: &Path| {
        let lock = dir.join(IN_USE);
        if lock.exists() {
            // Released at once: no copy locks a folder it did not make.
            matches!(Lock::try_acquire_at(&lock), Ok(Some(_)))
        } else {
            fs::metadata(dir)
                .and_then(|m| m.modified())
                .is_ok_and(|t| t.elapsed().is_ok_and(|age| age >= unlocked_age))
        }
    };
    for e in entries.flatten() {
        if e.file_name().to_string_lossy().starts_with(STAGING)
            && e.file_type().is_ok_and(|t| t.is_dir())
            && unused(&e.path())
        {
            // An old app inside may hold files without write permission.
            let _ = make_writable(&e.path());
            let _ = fs::remove_dir_all(e.path());
        }
    }
}

/// Undo the read-only bits a copied app bundle may have, so it can be deleted.
pub(crate) fn make_writable(path: &Path) -> std::io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        return Ok(());
    }
    let mut perms = meta.permissions();
    if perms.readonly() {
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        fs::set_permissions(path, perms)?;
    }
    if meta.is_dir() {
        for e in fs::read_dir(path)? {
            make_writable(&e?.path())?;
        }
    }
    Ok(())
}

/// Copy `reader` into `out`, failing unless its SHA-256 is `expected`.
fn copy_verified(
    mut reader: impl Read,
    mut out: fs::File,
    expected: &str,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64),
) -> Result<()> {
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 << 10];
    let mut done = 0u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            bail!("canceled");
        }
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        out.write_all(&buf[..n])?;
        done += n as u64;
        progress(done);
    }
    out.sync_all()?;
    let actual: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if actual != expected {
        bail!("SHA-256 mismatch: expected {expected}, got {actual}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use test_support::http::{Response, serve_paths};

    use super::*;
    use crate::release::tests::keys;

    fn sha256(data: &[u8]) -> String {
        Sha256::digest(data)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    #[test]
    fn downloads_a_signed_release() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("TeXoria.AppImage");
        fs::write(&file, b"old").unwrap();
        let kind = InstallKind::AppImage { file: file.clone() };
        let version = Version::new(9, 0, 0);
        let asset = kind.asset(&version, std::env::consts::ARCH).unwrap();
        let body = b"new AppImage".to_vec();
        let sums = format!("{}  {asset}\n", sha256(&body));
        let (public, sign) = keys();
        let sig = sign(sums.as_bytes(), "texoria 9.0.0");
        let asset_path = format!("/releases/download/v9.0.0/{asset}");
        let server = serve_paths(vec![
            ("/releases/download/v9.0.0/SHA256SUMS", Response::ok(sums)),
            (
                "/releases/download/v9.0.0/SHA256SUMS.minisig",
                Response::ok(sig),
            ),
            (&asset_path, Response::ok(body.clone())),
            (
                "/releases/latest",
                Response::redirect("http://example.com/releases/tag/v9.0.0"),
            ),
        ]);
        let base = format!("{}/releases", server.url());
        let source = Source::with_key(&base, &public).unwrap();

        assert_eq!(source.latest_version().unwrap(), version);
        let release = source.release(&version).unwrap();
        let mut seen = 0;
        let update = download(&source, &release, &kind, &AtomicBool::new(false), |n, _| {
            seen = n
        })
        .unwrap();
        assert_eq!(seen, body.len() as u64);
        assert_eq!(fs::read(&update.file).unwrap(), body);
        assert!(update.staging.path().starts_with(dir.path()));
        let agent = server.requests()[0]
            .header("user-agent")
            .map(str::to_string);
        assert_eq!(agent.as_deref(), Some(crate::USER_AGENT));

        // A file that does not match the list is refused.
        let other = Release::verify(
            version.clone(),
            format!("{}  {asset}\n", sha256(b"x")).as_bytes(),
            &sign(
                format!("{}  {asset}\n", sha256(b"x")).as_bytes(),
                "texoria 9.0.0",
            ),
            &minisign_verify::PublicKey::decode(&public).unwrap(),
        )
        .unwrap();
        let err = download(&source, &other, &kind, &AtomicBool::new(false), |_, _| {});
        assert!(format!("{:#}", err.unwrap_err()).contains("SHA-256 mismatch"));
    }

    /// Two running copies may download at the same time (the app goes on
    /// when the update lock cannot be taken): neither removes the other's.
    #[test]
    fn staging_in_use_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let kind = InstallKind::AppImage {
            file: dir.path().join("TeXoria.AppImage"),
        };
        let (first, lock) = staging_dir(&kind).unwrap();
        let (second, _held) = staging_dir(&kind).unwrap();
        assert!(first.path().is_dir());
        // The first copy quit (or crashed) without removing its folder.
        drop(lock);
        let first = first.keep();
        let _third = staging_dir(&kind).unwrap();
        assert!(!first.exists());
        assert!(second.path().is_dir());
    }

    #[test]
    fn unlocked_staging_is_removed_once_old() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join(format!("{STAGING}old"));
        fs::create_dir_all(old.join("TeXoria.app")).unwrap();
        remove_stale(dir.path(), UNLOCKED_AGE);
        assert!(old.is_dir());
        remove_stale(dir.path(), Duration::ZERO);
        assert!(!old.exists());
    }

    #[test]
    fn stale_staging_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let stale = dir.path().join(format!("{STAGING}old"));
        fs::create_dir_all(stale.join("TeXoria.app")).unwrap();
        fs::write(stale.join(IN_USE), "").unwrap();
        let kind = InstallKind::AppImage {
            file: dir.path().join("TeXoria.AppImage"),
        };
        let (staging, _lock) = staging_dir(&kind).unwrap();
        assert!(!stale.exists());
        assert!(staging.path().exists());
    }
}
