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
    progress: impl FnMut(u64, Option<u64>),
) -> Result<Update> {
    download_at_most(source, release, kind, cancel, progress, MAX_SIZE)
}

/// [`download`], refusing more than `max_size` bytes.
fn download_at_most(
    source: &Source,
    release: &Release,
    kind: &InstallKind,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64, Option<u64>),
    max_size: u64,
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
    let reader = resp.body_mut().with_config().limit(max_size).reader();
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

    /// An AppImage install in a temporary folder, and a server for release
    /// 9.0.0 whose list says the AppImage is `listed` and which answers it
    /// with `served`.
    struct Fixture {
        dir: tempfile::TempDir,
        kind: InstallKind,
        source: Source,
        release: Release,
        _server: test_support::http::Server,
    }

    fn fixture(listed: &[u8], served: Response) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("TeXoria.AppImage");
        fs::write(&file, b"old").unwrap();
        let kind = InstallKind::AppImage { file };
        let version = Version::new(9, 0, 0);
        let asset = kind.asset(&version, std::env::consts::ARCH).unwrap();
        let sums = format!("{}  {asset}\n", sha256(listed));
        let (public, sign) = keys();
        let sig = sign(sums.as_bytes(), "texoria 9.0.0");
        let path = format!("/releases/download/v9.0.0/{asset}");
        let server = serve_paths(vec![(&path, served)]);
        let source = Source::with_key(&format!("{}/releases", server.url()), &public).unwrap();
        let key = minisign_verify::PublicKey::decode(&public).unwrap();
        let release = Release::verify(version, sums.as_bytes(), &sig, &key).unwrap();
        Fixture {
            dir,
            kind,
            source,
            release,
            _server: server,
        }
    }

    impl Fixture {
        fn download(&self, cancel: &AtomicBool) -> Result<Update> {
            download(&self.source, &self.release, &self.kind, cancel, |_, _| {})
        }

        /// Only the installed AppImage: nothing left of a failed download.
        #[track_caller]
        fn assert_clean(&self) {
            let left: Vec<_> = fs::read_dir(self.dir.path())
                .unwrap()
                .flatten()
                .map(|e| e.file_name())
                .collect();
            assert_eq!(left, ["TeXoria.AppImage"]);
            assert_eq!(
                fs::read(self.dir.path().join("TeXoria.AppImage")).unwrap(),
                b"old"
            );
        }
    }

    #[test]
    fn failed_downloads_leave_nothing_behind() {
        let no = AtomicBool::new(false);
        let f = fixture(b"good", Response::ok("tampered"));
        let err = f.download(&no).unwrap_err();
        assert!(format!("{err:#}").contains("SHA-256 mismatch"), "{err:#}");
        f.assert_clean();

        let f = fixture(b"good", Response::not_found());
        let err = f.download(&no).unwrap_err();
        assert!(format!("{err:#}").contains("downloading http"), "{err:#}");
        f.assert_clean();

        // Bigger than any release file.
        let f = fixture(b"good", Response::ok("good"));
        let err = download_at_most(&f.source, &f.release, &f.kind, &no, |_, _| {}, 3).unwrap_err();
        assert!(format!("{err:#}").contains("limit"), "{err:#}");
        f.assert_clean();

        // Cut short.
        let f = fixture(b"good", Response::ok("go").content_length(4));
        assert!(f.download(&no).is_err());
        f.assert_clean();
    }

    #[test]
    fn downloads_can_be_cancelled() {
        let body = vec![7u8; 1 << 20];
        let f = fixture(&body, Response::ok(body.clone()));
        let cancel = AtomicBool::new(true);
        let err = f.download(&cancel).unwrap_err();
        assert!(format!("{err:#}").contains("canceled"), "{err:#}");
        f.assert_clean();

        // Half-way.
        let cancel = AtomicBool::new(false);
        let mut calls = 0;
        let err = download(&f.source, &f.release, &f.kind, &cancel, |done, total| {
            calls += 1;
            assert_eq!(total, Some(body.len() as u64));
            assert!(done < body.len() as u64);
            cancel.store(true, Ordering::Relaxed);
        })
        .unwrap_err();
        assert!(format!("{err:#}").contains("canceled"), "{err:#}");
        assert_eq!(calls, 1);
        f.assert_clean();

        let update = f.download(&AtomicBool::new(false)).unwrap();
        assert_eq!(fs::read(&update.file).unwrap(), body);
        assert!(format!("{update:?}").contains("9.0.0"));
        drop(update);
        f.assert_clean();
    }

    #[test]
    fn nothing_to_download() {
        let f = fixture(b"x", Response::ok("x"));
        let no = AtomicBool::new(false);
        for kind in [
            InstallKind::Managed(crate::Manager::System),
            InstallKind::Manual("no"),
            InstallKind::Dev,
        ] {
            let err = download(&f.source, &f.release, &kind, &no, |_, _| {}).unwrap_err();
            assert!(err.to_string().contains("cannot update itself"), "{err}");
            assert!(staging_dir(&kind).is_err());
        }
        // Not in the release.
        let tarball = InstallKind::Tarball {
            prefix: f.dir.path().into(),
        };
        let err = download(&f.source, &f.release, &tarball, &no, |_, _| {}).unwrap_err();
        assert!(
            err.to_string()
                .contains("release 9.0.0 has no texoria-9.0.0-linux-"),
            "{err}"
        );
        f.assert_clean();

        // A folder that is not there.
        let gone = InstallKind::AppImage {
            file: f.dir.path().join("gone/TeXoria.AppImage"),
        };
        let err = staging_dir(&gone).unwrap_err();
        assert!(err.to_string().contains("creating a folder in"), "{err}");
    }

    #[test]
    fn read_only_trees_become_removable() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join(format!("{STAGING}x")).join("TeXoria.app");
        fs::create_dir_all(app.join("Contents")).unwrap();
        let file = app.join("Contents/Info.plist");
        fs::write(&file, "x").unwrap();
        let mut perms = fs::metadata(&file).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(&file, perms).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for d in [app.join("Contents"), app.clone()] {
                fs::set_permissions(&d, fs::Permissions::from_mode(0o555)).unwrap();
            }
        }
        make_writable(&app).unwrap();
        assert!(!fs::metadata(&file).unwrap().permissions().readonly());
        fs::remove_dir_all(&app).unwrap();
        assert!(make_writable(&app).is_err());
    }

    /// A link is left as it is, and so is what it points to.
    #[cfg(unix)]
    #[test]
    fn links_are_not_followed_when_making_writable() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let outside = dir.path().join("outside");
        fs::write(&outside, "x").unwrap();
        fs::set_permissions(&outside, fs::Permissions::from_mode(0o444)).unwrap();
        let tree = dir.path().join("tree");
        fs::create_dir(&tree).unwrap();
        std::os::unix::fs::symlink(&outside, tree.join("link")).unwrap();
        std::os::unix::fs::symlink(dir.path(), tree.join("up")).unwrap();
        make_writable(&tree).unwrap();
        assert_eq!(
            fs::metadata(&outside).unwrap().permissions().mode() & 0o777,
            0o444
        );
    }

    /// A stale folder whose files are read-only is removed too.
    #[test]
    fn stale_read_only_staging_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let stale = dir.path().join(format!("{STAGING}old"));
        fs::create_dir_all(&stale).unwrap();
        let file = stale.join("setup.exe");
        fs::write(&file, "x").unwrap();
        let mut perms = fs::metadata(&file).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(&file, perms).unwrap();
        // Not a staging folder.
        fs::write(dir.path().join(format!("{STAGING}file")), "x").unwrap();
        fs::create_dir(dir.path().join("other")).unwrap();
        remove_stale(dir.path(), Duration::ZERO);
        assert!(!stale.exists());
        assert!(dir.path().join(format!("{STAGING}file")).is_file());
        assert!(dir.path().join("other").is_dir());
        remove_stale(&dir.path().join("missing"), Duration::ZERO);
    }
}
