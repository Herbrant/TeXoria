//! Put a downloaded release in place of the running copy, and start it once
//! this process has exited.
//!
//! The running app is replaced, not modified: a new bundle or file is
//! prepared next to the old one and renamed over it, so a failure before the
//! rename leaves the old version untouched. A running program does not mind
//! its files being renamed or deleted on macOS and Linux; on Windows the
//! installer does the work after the app has quit.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, anyhow, bail};

use crate::download::{Update, make_writable};
use crate::install::InstallKind;

/// How to start the new version once this process has exited.
#[derive(Debug, Clone)]
pub struct Relaunch {
    program: PathBuf,
    args: Vec<OsString>,
    /// Whether [`Relaunch::spawn`]'s arguments reach the new app.
    forwards_args: bool,
}

impl Update {
    /// Install the update. Blocking: on macOS it copies the app out of the
    /// disk image.
    pub fn install(self) -> Result<Relaunch> {
        let Update {
            kind,
            staging,
            file,
            version,
        } = self;
        log::info!("installing TeXoria {version}");
        let relaunch = match &kind {
            InstallKind::MacApp { bundle } => {
                install_mac(&file, bundle, staging.path())?;
                Relaunch {
                    program: "/usr/bin/open".into(),
                    args: vec![bundle.into()],
                    forwards_args: true,
                }
            }
            InstallKind::AppImage { file: target } => {
                replace_file(&file, target)?;
                Relaunch {
                    program: target.clone(),
                    args: Vec::new(),
                    forwards_args: true,
                }
            }
            InstallKind::Tarball { prefix } => {
                install_tarball(&file, prefix, staging.path())?;
                Relaunch {
                    program: prefix.join("bin/texoria"),
                    args: Vec::new(),
                    forwards_args: true,
                }
            }
            InstallKind::Windows { dir } => {
                // The installer must outlive the app (the system cleans its
                // temporary folder).
                let _ = staging.keep();
                // Passive (a progress bar, no questions), restart the app, no
                // new shortcuts, same folder. `/D=` must come last, unquoted.
                let mut args: Vec<OsString> = vec!["/P".into(), "/R".into(), "/NS".into()];
                let mut d = OsString::from("/D=");
                d.push(dir);
                args.push(d);
                return Ok(Relaunch {
                    program: file,
                    args,
                    forwards_args: false,
                });
            }
            InstallKind::Managed(_) | InstallKind::Manual(_) | InstallKind::Dev => {
                bail!("this installation cannot update itself")
            }
        };
        // What is left is the download and the old version.
        let _ = make_writable(staging.path());
        drop(staging);
        Ok(relaunch)
    }
}

impl Relaunch {
    /// Start the new version (with `args`, where possible) once this process
    /// has exited. The caller should quit right after.
    pub fn spawn(&self, args: impl IntoIterator<Item = OsString>) -> Result<()> {
        let mut extra: Vec<OsString> = Vec::new();
        if self.forwards_args {
            extra.extend(args);
        }
        self.spawn_with(extra)
    }

    #[cfg(unix)]
    fn spawn_with(&self, extra: Vec<OsString>) -> Result<()> {
        use std::os::unix::process::CommandExt;

        let mut args = self.args.clone();
        if !extra.is_empty() {
            if self.program == Path::new("/usr/bin/open") {
                args.push("--args".into());
            }
            args.extend(extra);
        }
        let script = r#"pid=$1; shift
while kill -0 "$pid" 2>/dev/null; do sleep 0.1; done
exec "$@""#;
        Command::new("/bin/sh")
            .arg("-c")
            .arg(script)
            .arg("sh")
            .arg(std::process::id().to_string())
            .arg(&self.program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .with_context(|| format!("starting {}", self.program.display()))?;
        Ok(())
    }

    #[cfg(windows)]
    fn spawn_with(&self, _extra: Vec<OsString>) -> Result<()> {
        use std::os::windows::process::CommandExt;

        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        let mut cmd = Command::new(&self.program);
        let (last, rest) = self.args.split_last().expect("installer arguments");
        cmd.args(rest);
        // NSIS reads `/D=` raw: quoting would become part of the path.
        cmd.raw_arg(last);
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
            .spawn()
            .with_context(|| format!("starting {}", self.program.display()))?;
        Ok(())
    }
}

/// Copy the app out of the disk image, check its signature, and swap it in.
fn install_mac(dmg: &Path, bundle: &Path, staging: &Path) -> Result<()> {
    let mnt = tempfile::tempdir()?;
    run(Command::new("hdiutil")
        .args([
            "attach",
            "-nobrowse",
            "-readonly",
            "-noautoopen",
            "-quiet",
            "-mountpoint",
        ])
        .arg(mnt.path())
        .arg(dmg))?;
    let new = staging.join("TeXoria.app");
    let copied = run(Command::new("ditto")
        .arg(mnt.path().join("TeXoria.app"))
        .arg(&new));
    if run(Command::new("hdiutil")
        .args(["detach", "-quiet"])
        .arg(mnt.path()))
    .is_err()
    {
        let _ = run(Command::new("hdiutil")
            .args(["detach", "-quiet", "-force"])
            .arg(mnt.path()));
    }
    copied?;
    run(Command::new("codesign")
        .args(["--verify", "--deep", "--strict"])
        .arg(&new))?;
    // Not notarized: Gatekeeper would refuse a quarantined copy. A download
    // by the app is not quarantined, but be sure.
    let _ = Command::new("xattr")
        .args(["-dr", "com.apple.quarantine"])
        .arg(&new)
        .output();
    swap(bundle, &new, &staging.join("old.app"))
}

/// Replace `target` with the downloaded `file` (an AppImage).
fn replace_file(file: &Path, target: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(file, fs::Permissions::from_mode(0o755))?;
    }
    fs::rename(file, target).with_context(|| format!("replacing {}", target.display()))
}

/// `install.sh`'s layout under `prefix`: `bin/texoria`, `lib/texoria/`
/// (PDFium, Tectonic) and `share/` (icons, licenses). The launcher in
/// `share/applications` was rewritten by `install.sh` for the prefix: it stays.
fn install_tarball(tarball: &Path, prefix: &Path, staging: &Path) -> Result<()> {
    let x = staging.join("x");
    let gz = flate2::read::GzDecoder::new(fs::File::open(tarball)?);
    tar::Archive::new(gz)
        .unpack(&x)
        .context("unpacking the update")?;
    let usr = x.join("usr");
    let (bin, lib) = (usr.join("bin/texoria"), usr.join("lib/texoria"));
    if !bin.is_file() || !lib.is_dir() {
        bail!("the update does not have the expected files");
    }

    let target_lib = prefix.join("lib/texoria");
    let old_lib = staging.join("old-lib");
    fs::create_dir_all(prefix.join("lib"))?;
    let had_lib = target_lib.exists();
    if had_lib {
        fs::rename(&target_lib, &old_lib)?;
    }
    let restore = || {
        if had_lib {
            let _ = fs::rename(&target_lib, staging.join("new-lib"));
            let _ = fs::rename(&old_lib, &target_lib);
        }
    };
    if let Err(e) = fs::rename(&lib, &target_lib) {
        restore();
        return Err(e).context("replacing the libraries");
    }
    if let Err(e) = fs::rename(&bin, prefix.join("bin/texoria")) {
        restore();
        return Err(e).context("replacing the program");
    }
    let share = usr.join("share");
    if share.is_dir() {
        copy_tree(&share, &prefix.join("share"), &|p| {
            p.starts_with(share.join("applications"))
        })
        .context("copying the icons")?;
    }
    Ok(())
}

/// Copy the files under `from` into `to`, replacing them, except `skip`ped ones.
fn copy_tree(from: &Path, to: &Path, skip: &dyn Fn(&Path) -> bool) -> Result<()> {
    fs::create_dir_all(to)?;
    for e in fs::read_dir(from)? {
        let e = e?;
        let path = e.path();
        if skip(&path) {
            continue;
        }
        let dest = to.join(e.file_name());
        if e.file_type()?.is_dir() {
            copy_tree(&path, &dest, skip)?;
        } else {
            fs::copy(&path, &dest)?;
        }
    }
    Ok(())
}

/// Move `current` to `aside` and `new` to `current`, putting `current` back
/// if the second rename fails.
fn swap(current: &Path, new: &Path, aside: &Path) -> Result<()> {
    fs::rename(current, aside).with_context(|| format!("moving {}", current.display()))?;
    if let Err(e) = fs::rename(new, current) {
        let _ = fs::rename(aside, current);
        return Err(e).with_context(|| format!("replacing {}", current.display()));
    }
    Ok(())
}

fn run(cmd: &mut Command) -> Result<()> {
    let program = cmd.get_program().to_string_lossy().into_owned();
    let out = cmd
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("running {program}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(anyhow!("{program} failed: {}", err.trim()));
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use semver::Version;

    use super::*;

    fn update(kind: InstallKind, file: &str, content: &[u8]) -> Update {
        let parent = kind.staging_parent().unwrap();
        let staging = tempfile::Builder::new()
            .prefix(crate::download::STAGING)
            .tempdir_in(parent)
            .unwrap();
        let file = staging.path().join(file);
        fs::write(&file, content).unwrap();
        Update {
            version: Version::new(9, 0, 0),
            kind,
            staging,
            file,
        }
    }

    #[test]
    fn replaces_an_appimage() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("TeXoria.AppImage");
        fs::write(&target, b"old").unwrap();
        let kind = InstallKind::AppImage {
            file: target.clone(),
        };
        let relaunch = update(kind, "new.AppImage", b"new").install().unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"new");
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o755
        );
        assert_eq!(relaunch.program, target);
        // Only the AppImage is left.
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    fn tarball(files: &[(&str, &[u8])]) -> Vec<u8> {
        let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        let mut tar = tar::Builder::new(gz);
        for (path, content) in files {
            let mut h = tar::Header::new_gnu();
            h.set_size(content.len() as u64);
            h.set_mode(0o755);
            h.set_cksum();
            tar.append_data(&mut h, path, *content).unwrap();
        }
        tar.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn replaces_a_tarball_install() {
        let prefix = tempfile::tempdir().unwrap();
        let p = prefix.path();
        for (path, content) in [
            ("bin/texoria", "old"),
            ("lib/texoria/libpdfium.so", "old"),
            ("lib/texoria/stale.so", "old"),
            (
                "share/applications/texoria.desktop",
                "Exec=/home/me/.local/bin/texoria",
            ),
        ] {
            fs::create_dir_all(p.join(path).parent().unwrap()).unwrap();
            fs::write(p.join(path), content).unwrap();
        }
        let archive = tarball(&[
            ("usr/bin/texoria", b"new"),
            ("usr/lib/texoria/libpdfium.so", b"new"),
            ("usr/lib/texoria/bin/tectonic", b"new"),
            ("usr/share/applications/texoria.desktop", b"Exec=texoria"),
            ("usr/share/icons/hicolor/64x64/apps/texoria.png", b"png"),
        ]);
        let kind = InstallKind::Tarball {
            prefix: p.to_path_buf(),
        };
        let relaunch = update(kind, "t.tar.gz", &archive).install().unwrap();
        let read = |path: &str| fs::read_to_string(p.join(path)).unwrap();
        assert_eq!(read("bin/texoria"), "new");
        assert_eq!(read("lib/texoria/libpdfium.so"), "new");
        assert_eq!(read("lib/texoria/bin/tectonic"), "new");
        assert!(!p.join("lib/texoria/stale.so").exists());
        assert_eq!(read("share/icons/hicolor/64x64/apps/texoria.png"), "png");
        assert_eq!(
            read("share/applications/texoria.desktop"),
            "Exec=/home/me/.local/bin/texoria"
        );
        assert_eq!(relaunch.program, p.join("bin/texoria"));
        let left: Vec<_> = fs::read_dir(p)
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(left.len(), 3, "{left:?}");
    }

    #[test]
    fn bad_tarball_leaves_the_install_alone() {
        let prefix = tempfile::tempdir().unwrap();
        let p = prefix.path();
        fs::create_dir_all(p.join("bin")).unwrap();
        fs::create_dir_all(p.join("lib/texoria")).unwrap();
        fs::write(p.join("bin/texoria"), "old").unwrap();
        let archive = tarball(&[("usr/bin/texoria", b"new")]);
        let kind = InstallKind::Tarball {
            prefix: p.to_path_buf(),
        };
        assert!(update(kind, "t.tar.gz", &archive).install().is_err());
        assert_eq!(fs::read_to_string(p.join("bin/texoria")).unwrap(), "old");
        assert!(p.join("lib/texoria").is_dir());
    }

    #[test]
    fn swap_puts_back_on_failure() {
        let dir = tempfile::tempdir().unwrap();
        let current = dir.path().join("TeXoria.app");
        fs::create_dir(&current).unwrap();
        let missing = dir.path().join("missing.app");
        assert!(swap(&current, &missing, &dir.path().join("old.app")).is_err());
        assert!(current.is_dir());
    }
}
