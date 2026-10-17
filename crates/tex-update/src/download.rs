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
use crate::release::{Release, Source};

/// The packages are ~30–60 MB.
const MAX_SIZE: u64 = 1 << 30;
/// For the response to start. ureq has no limit between two reads of the
/// body: a stalled transfer only ends with `timeout_recv_body`.
const STALL: Duration = Duration::from_secs(60);
/// Prefix of the staging folders.
pub(crate) const STAGING: &str = ".texoria-update-";

/// A verified release file, ready to be installed (see [`Update::install`]).
pub struct Update {
    pub version: Version,
    pub(crate) kind: InstallKind,
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
    let staging = staging_dir(kind)?;
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
        staging,
        file,
    })
}

/// A new folder next to what the update replaces. Folders left by an update
/// that was interrupted (the app was killed) are removed first: the caller
/// holds the [`crate::Lock`], so no other running copy is using them.
fn staging_dir(kind: &InstallKind) -> Result<TempDir> {
    let parent = kind
        .staging_parent()
        .ok_or_else(|| anyhow!("this installation cannot update itself"))?;
    remove_stale(&parent);
    tempfile::Builder::new()
        .prefix(STAGING)
        .tempdir_in(&parent)
        .with_context(|| format!("creating a folder in {}", parent.display()))
}

fn remove_stale(parent: &Path) {
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    for e in entries.flatten() {
        if e.file_name().to_string_lossy().starts_with(STAGING)
            && e.file_type().is_ok_and(|t| t.is_dir())
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
    use std::io::BufRead;
    use std::net::TcpListener;

    use super::*;
    use crate::release::tests::keys;

    /// Serve `files` (path → body) over HTTP until the test ends.
    fn serve(files: Vec<(String, Vec<u8>)>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                let mut line = String::new();
                let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
                reader.read_line(&mut line).unwrap();
                loop {
                    let mut h = String::new();
                    reader.read_line(&mut h).unwrap();
                    if h.trim().is_empty() {
                        break;
                    }
                }
                let path = line.split_whitespace().nth(1).unwrap_or("").to_string();
                let resp = match files.iter().find(|(p, _)| *p == path) {
                    Some((_, body)) => {
                        let mut r = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        )
                        .into_bytes();
                        r.extend_from_slice(body);
                        r
                    }
                    None if path.ends_with("/latest") => b"HTTP/1.1 302 Found\r\n\
                        Location: http://example.com/releases/tag/v9.0.0\r\n\
                        Content-Length: 0\r\nConnection: close\r\n\r\n"
                        .to_vec(),
                    None => {
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                            .to_vec()
                    }
                };
                stream.write_all(&resp).unwrap();
            }
        });
        format!("http://{addr}/releases")
    }

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
        let base = serve(vec![
            (
                "/releases/download/v9.0.0/SHA256SUMS".into(),
                sums.into_bytes(),
            ),
            (
                "/releases/download/v9.0.0/SHA256SUMS.minisig".into(),
                sig.into_bytes(),
            ),
            (format!("/releases/download/v9.0.0/{asset}"), body.clone()),
        ]);
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

    #[test]
    fn stale_staging_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let stale = dir.path().join(format!("{STAGING}old"));
        fs::create_dir_all(stale.join("TeXoria.app")).unwrap();
        let kind = InstallKind::AppImage {
            file: dir.path().join("TeXoria.AppImage"),
        };
        let staging = staging_dir(&kind).unwrap();
        assert!(!stale.exists());
        assert!(staging.path().exists());
    }
}
