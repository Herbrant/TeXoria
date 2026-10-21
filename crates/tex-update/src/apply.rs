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
            in_use,
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
        drop(in_use);
        drop(staging);
        Ok(relaunch)
    }
}

impl Relaunch {
    /// Whether [`Relaunch::spawn`] runs the installer (Windows): until then,
    /// nothing is installed.
    pub fn installs(&self) -> bool {
        !self.forwards_args
    }

    /// Start the new version (with `args`, where possible) once this process
    /// has exited. The caller should quit right after.
    pub fn spawn(&self, args: impl IntoIterator<Item = OsString>) -> Result<()> {
        let mut extra: Vec<OsString> = Vec::new();
        if self.forwards_args {
            extra.extend(args);
        }
        self.spawn_after(std::process::id(), extra)
    }

    /// The program's arguments, with `extra` (`open` takes the app's after
    /// `--args`).
    #[cfg(any(unix, test))]
    fn command_args(&self, extra: Vec<OsString>) -> Vec<OsString> {
        let mut args = self.args.clone();
        if !extra.is_empty() {
            if self.program == Path::new("/usr/bin/open") {
                args.push("--args".into());
            }
            args.extend(extra);
        }
        args
    }

    /// Start the program once the process `pid` has exited.
    #[cfg(unix)]
    fn spawn_after(&self, pid: u32, extra: Vec<OsString>) -> Result<()> {
        use std::os::unix::process::CommandExt;

        let args = self.command_args(extra);
        let script = r#"pid=$1; shift
while kill -0 "$pid" 2>/dev/null; do sleep 0.1; done
exec "$@""#;
        Command::new("/bin/sh")
            .arg("-c")
            .arg(script)
            .arg("sh")
            .arg(pid.to_string())
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

    /// The installer closes every `texoria.exe` without asking (the NSIS
    /// template's passive mode): PowerShell waits for this process to exit,
    /// asks to close the other windows, which would lose their unsaved edits,
    /// and only then starts it.
    #[cfg(windows)]
    fn spawn_after(&self, pid: u32, _extra: Vec<OsString>) -> Result<()> {
        use std::os::windows::process::CommandExt;

        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
        let powershell = Path::new(&root).join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
        let args = self.installer_args();
        let spawn = |flags: u32| {
            Command::new(&powershell)
                .args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-ExecutionPolicy",
                    "Bypass",
                ])
                .args(["-WindowStyle", "Hidden", "-EncodedCommand"])
                .arg(encode_command(WINDOWS_SCRIPT))
                .env("TEXORIA_UPDATE_PID", pid.to_string())
                .env("TEXORIA_UPDATE_INSTALLER", &self.program)
                .env("TEXORIA_UPDATE_ARGS", &args)
                .env("TEXORIA_UPDATE_MESSAGE", OTHER_WINDOWS)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .creation_flags(flags)
                .spawn()
        };
        // Out of the app's job, if it is in one that ends its processes with
        // it; not every job allows that.
        let flags = CREATE_NEW_PROCESS_GROUP | tex_sys::process::NO_WINDOW;
        spawn(flags | CREATE_BREAKAWAY_FROM_JOB)
            .or_else(|_| spawn(flags))
            .with_context(|| format!("starting {}", powershell.display()))?;
        Ok(())
    }

    /// `Start-Process` passes a single argument string as is: NSIS reads
    /// `/D=` raw, and quoting would become part of the path.
    #[cfg(any(windows, test))]
    fn installer_args(&self) -> OsString {
        let mut args = OsString::new();
        for (i, a) in self.args.iter().enumerate() {
            if i > 0 {
                args.push(" ");
            }
            args.push(a);
        }
        args
    }
}

/// Run by [`Relaunch::spawn`] on Windows, with its values in the environment
/// (no quoting). The agents' `texoria.exe --mcp-bridge` (`tex_agent::mcp`)
/// end with the app, so they are not waited for. Popup 0x35: Retry/Cancel,
/// warning icon; 4 is Retry.
#[cfg(any(windows, test))]
const WINDOWS_SCRIPT: &str = r#"$ErrorActionPreference = 'SilentlyContinue'
Wait-Process -Id ([int]$env:TEXORIA_UPDATE_PID)
while ($true) {
    $open = @(Get-CimInstance Win32_Process -Filter "Name = 'texoria.exe'" |
        Where-Object { $_.CommandLine -notlike '*--mcp-bridge*' })
    if ($open.Count -eq 0) { break }
    $answer = (New-Object -ComObject WScript.Shell).Popup($env:TEXORIA_UPDATE_MESSAGE, 0, 'TeXoria', 0x35)
    if ($answer -ne 4) { exit 1 }
}
Start-Process -FilePath $env:TEXORIA_UPDATE_INSTALLER -ArgumentList $env:TEXORIA_UPDATE_ARGS
"#;

#[cfg(windows)]
const OTHER_WINDOWS: &str = "TeXoria is still open in another window. Close it to install \
    the update: the installer would close it without saving.\n\nThe update is not installed \
    if you cancel; TeXoria offers it again.";

/// `-EncodedCommand`'s format: Base64 of the UTF-16LE text.
#[cfg(any(windows, test))]
fn encode_command(script: &str) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | (*b as u32) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Attempts after the first to mount the disk image.
const ATTACH_RETRIES: usize = 3;

/// Copy the app out of the disk image, check its signature, and swap it in.
fn install_mac(dmg: &Path, bundle: &Path, staging: &Path) -> Result<()> {
    let mnt = tempfile::tempdir()?;
    // Absolute, as `hdiutil info` names it.
    let image = std::path::absolute(dmg)?;
    // Not `-quiet`, which also silences the reason of a failure (stdout is not shown).
    let attach = || {
        run(Command::new("hdiutil")
            .args([
                "attach",
                "-nobrowse",
                "-readonly",
                "-noautoopen",
                "-mountpoint",
            ])
            .arg(mnt.path())
            .arg(&image))
    };
    // hdiutil fails now and then while the system is busy with another image ("Resource
    // temporarily unavailable"), and may leave the image attached but not mounted: until that
    // is detached, attaching the image again fails the same way.
    let mut attached = attach();
    for _ in 0..ATTACH_RETRIES {
        if attached.is_ok() {
            break;
        }
        detach_image(&image);
        std::thread::sleep(std::time::Duration::from_secs(1));
        attached = attach();
    }
    if attached.is_err() {
        detach_image(&image);
    }
    attached?;
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

/// Detach what a failed `hdiutil attach` left of `image` (attached, not mounted).
fn detach_image(image: &Path) {
    let Ok(out) = Command::new("hdiutil")
        .arg("info")
        .stdin(Stdio::null())
        .output()
    else {
        return;
    };
    let info = String::from_utf8_lossy(&out.stdout);
    // Named as it was given, or resolved (`/private/tmp` for `/tmp`) when given relative.
    let mut devices = image_devices(&info, image);
    if let Ok(resolved) = fs::canonicalize(image)
        && resolved != image
    {
        devices.extend(image_devices(&info, &resolved));
    }
    for device in devices {
        // Its slices and synthesized disks go with it.
        let _ = run(Command::new("hdiutil")
            .args(["detach", "-quiet", "-force"])
            .arg(device));
    }
}

/// The whole disks (`/dev/disk7`, not `/dev/disk7s1`) that `hdiutil info` lists for `image`.
fn image_devices(info: &str, image: &Path) -> Vec<String> {
    let mut devices = Vec::new();
    let mut ours = false;
    for line in info.lines() {
        if let Some(rest) = line.strip_prefix("image-path") {
            let path = rest.trim_start().strip_prefix(':').map(str::trim);
            ours = path.is_some_and(|p| Path::new(p) == image);
        } else if line.starts_with("====") {
            ours = false;
        } else if ours
            && let Some(device) = line.split_whitespace().next()
            && device
                .strip_prefix("/dev/disk")
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        {
            devices.push(device.to_string());
        }
    }
    devices
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
    // The new libraries out of the way, the old ones (if any) back.
    let restore = || {
        let _ = fs::rename(&target_lib, staging.join("new-lib"));
        if had_lib {
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
    // The new version is in place by now: failing here would report an update
    // that went through as failed.
    let share = usr.join("share");
    if share.is_dir()
        && let Err(e) = copy_tree(&share, &prefix.join("share"), &|p| {
            p.starts_with(share.join("applications"))
        })
    {
        log::warn!("copying the icons of the update: {e:#}");
    }
    Ok(())
}

/// Copy the files under `from` into `to`, replacing them, except `skip`ped
/// ones. Links are copied as links: `fs::copy` would copy what they point to,
/// wherever that is. A file or link in `to` is replaced, not written through.
fn copy_tree(from: &Path, to: &Path, skip: &dyn Fn(&Path) -> bool) -> Result<()> {
    fs::create_dir_all(to)?;
    for e in fs::read_dir(from)? {
        let e = e?;
        let path = e.path();
        if skip(&path) {
            continue;
        }
        let dest = to.join(e.file_name());
        let kind = e.file_type()?;
        if kind.is_dir() {
            copy_tree(&path, &dest, skip)?;
            continue;
        }
        if fs::symlink_metadata(&dest).is_ok_and(|m| !m.is_dir()) {
            fs::remove_file(&dest)?;
        }
        if kind.is_symlink() {
            copy_link(&path, &dest)?;
        } else {
            fs::copy(&path, &dest)?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn copy_link(link: &Path, dest: &Path) -> Result<()> {
    std::os::unix::fs::symlink(fs::read_link(link)?, dest)?;
    Ok(())
}

/// Only Linux has tarball installs.
#[cfg(not(unix))]
fn copy_link(link: &Path, _: &Path) -> Result<()> {
    bail!("{} is a link", link.display())
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
        // Some tools (hdiutil -quiet) say nothing on stderr: then what they printed, or at least
        // how they ended.
        let err = String::from_utf8_lossy(&out.stderr);
        let said = match err.trim() {
            "" => String::from_utf8_lossy(&out.stdout).trim().to_string(),
            err => err.to_string(),
        };
        let said = if said.is_empty() {
            out.status.to_string()
        } else {
            said
        };
        return Err(anyhow!("{program} failed: {said}"));
    }
    Ok(())
}

#[cfg(test)]
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
            in_use: None,
            staging,
            file,
        }
    }

    #[test]
    fn replaces_an_appimage() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("TeXoria.AppImage");
        fs::write(&target, b"old").unwrap();
        let kind = InstallKind::AppImage {
            file: target.clone(),
        };
        let relaunch = update(kind, "new.AppImage", b"new").install().unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"new");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&target).unwrap().permissions().mode() & 0o777,
                0o755
            );
        }
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
            test_support::write(p, path, content);
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

    fn tarball_kind(prefix: &Path) -> InstallKind {
        InstallKind::Tarball {
            prefix: prefix.to_path_buf(),
        }
    }

    fn release_tarball() -> Vec<u8> {
        tarball(&[
            ("usr/bin/texoria", b"new"),
            ("usr/lib/texoria/libpdfium.so", b"new"),
        ])
    }

    /// The program cannot be put in place (here: no `bin` folder): the
    /// libraries are what they were before.
    #[test]
    fn failed_program_rename_puts_the_libraries_back() {
        let prefix = tempfile::tempdir().unwrap();
        let p = prefix.path();
        fs::create_dir_all(p.join("lib/texoria")).unwrap();
        fs::write(p.join("lib/texoria/libpdfium.so"), "old").unwrap();
        let err = update(tarball_kind(p), "t.tar.gz", &release_tarball())
            .install()
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("replacing the program"),
            "{err:#}"
        );
        assert_eq!(
            fs::read_to_string(p.join("lib/texoria/libpdfium.so")).unwrap(),
            "old"
        );

        // Without libraries before, none after.
        let prefix = tempfile::tempdir().unwrap();
        let p = prefix.path();
        assert!(
            update(tarball_kind(p), "t.tar.gz", &release_tarball())
                .install()
                .is_err()
        );
        assert!(!p.join("lib/texoria").exists());
        let left: Vec<_> = fs::read_dir(p)
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(left, ["lib"]);
    }

    /// Links in the update are copied as links, never as the files they
    /// point to, and the prefix's own links are replaced, not written through.
    #[cfg(unix)]
    #[test]
    fn links_are_copied_as_links() {
        let prefix = tempfile::tempdir().unwrap();
        let p = prefix.path();
        let elsewhere = tempfile::tempdir().unwrap();
        let secret = elsewhere.path().join("secret");
        let mine = elsewhere.path().join("mine");
        fs::write(&secret, "private").unwrap();
        fs::write(&mine, "mine").unwrap();
        fs::create_dir_all(p.join("bin")).unwrap();
        fs::create_dir_all(p.join("share/icons")).unwrap();
        std::os::unix::fs::symlink(&mine, p.join("share/icons/b.png")).unwrap();

        let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        let mut tar = tar::Builder::new(gz);
        for (path, content) in [
            ("usr/bin/texoria", &b"new"[..]),
            ("usr/lib/texoria/libpdfium.so", b"new"),
            ("usr/share/icons/b.png", b"png"),
        ] {
            let mut h = tar::Header::new_gnu();
            h.set_size(content.len() as u64);
            h.set_mode(0o755);
            tar.append_data(&mut h, path, content).unwrap();
        }
        let mut h = tar::Header::new_gnu();
        h.set_entry_type(tar::EntryType::Symlink);
        h.set_size(0);
        h.set_mode(0o777);
        tar.append_link(&mut h, "usr/share/icons/a.png", &secret)
            .unwrap();
        let archive = tar.into_inner().unwrap().finish().unwrap();

        update(tarball_kind(p), "t.tar.gz", &archive)
            .install()
            .unwrap();
        let a = p.join("share/icons/a.png");
        assert!(fs::symlink_metadata(&a).unwrap().file_type().is_symlink());
        assert_eq!(fs::read_link(&a).unwrap(), secret);
        let b = p.join("share/icons/b.png");
        assert!(fs::symlink_metadata(&b).unwrap().is_file());
        assert_eq!(fs::read_to_string(&b).unwrap(), "png");
        assert_eq!(fs::read_to_string(&mine).unwrap(), "mine");
    }

    #[test]
    fn encodes_powershell_commands() {
        // [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes(...))
        assert_eq!(encode_command(""), "");
        assert_eq!(encode_command("ab"), "YQBiAA==");
        assert_eq!(encode_command("abc"), "YQBiAGMA");
        assert_eq!(encode_command("é"), "6QA=");
        assert!(WINDOWS_SCRIPT.contains("$env:TEXORIA_UPDATE_ARGS"));
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

    #[test]
    fn some_installations_cannot_be_updated() {
        let dir = tempfile::tempdir().unwrap();
        for kind in [
            InstallKind::Managed(crate::Manager::Homebrew),
            InstallKind::Manual("no"),
            InstallKind::Dev,
        ] {
            let staging = tempfile::tempdir_in(dir.path()).unwrap();
            let file = staging.path().join("x");
            let update = Update {
                version: Version::new(9, 0, 0),
                kind,
                in_use: None,
                staging,
                file,
            };
            let err = update.install().unwrap_err();
            assert!(err.to_string().contains("cannot update itself"), "{err}");
        }
        // Nothing left behind.
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    /// Windows: the installer runs once the app has quit, from a folder that
    /// outlives it, in the installation's folder.
    #[test]
    fn the_windows_installer_runs_later() {
        let dir = PathBuf::from(r"C:\Users\me\AppData\Local\TeXoria");
        let update = update(
            InstallKind::Windows { dir: dir.clone() },
            "setup.exe",
            b"MZ",
        );
        let setup = update.file.clone();
        let relaunch = update.install().unwrap();
        assert!(relaunch.installs());
        assert_eq!(relaunch.program, setup);
        let mut d = OsString::from("/D=");
        d.push(&dir);
        assert_eq!(
            relaunch.args,
            [OsString::from("/P"), "/R".into(), "/NS".into(), d]
        );
        assert_eq!(
            relaunch.installer_args(),
            OsString::from(r"/P /R /NS /D=C:\Users\me\AppData\Local\TeXoria")
        );
        // Kept for the installer.
        assert!(setup.is_file());
        fs::remove_dir_all(setup.parent().unwrap()).unwrap();
    }

    #[test]
    fn relaunch_arguments() {
        let open = Relaunch {
            program: "/usr/bin/open".into(),
            args: vec!["/Applications/TeXoria.app".into()],
            forwards_args: true,
        };
        assert!(!open.installs());
        assert_eq!(open.command_args(Vec::new()), ["/Applications/TeXoria.app"]);
        assert_eq!(
            open.command_args(vec!["a.tex".into()]),
            ["/Applications/TeXoria.app", "--args", "a.tex"]
        );
        let direct = Relaunch {
            program: "/home/me/.local/bin/texoria".into(),
            args: Vec::new(),
            forwards_args: true,
        };
        assert_eq!(direct.command_args(vec!["a.tex".into()]), ["a.tex"]);
        assert_eq!(direct.installer_args(), OsString::new());
    }

    /// The icons are not worth failing an update that went through.
    #[test]
    fn failing_to_copy_the_icons_is_not_a_failure() {
        let prefix = tempfile::tempdir().unwrap();
        let p = prefix.path();
        fs::create_dir_all(p.join("bin")).unwrap();
        fs::write(p.join("share"), "a file where the folder goes").unwrap();
        let archive = tarball(&[
            ("usr/bin/texoria", b"new"),
            ("usr/lib/texoria/libpdfium.so", b"new"),
            ("usr/share/icons/texoria.png", b"png"),
        ]);
        update(tarball_kind(p), "t.tar.gz", &archive)
            .install()
            .unwrap();
        assert_eq!(fs::read_to_string(p.join("bin/texoria")).unwrap(), "new");
        assert!(p.join("share").is_file());
    }

    #[test]
    fn archives_that_are_not_releases() {
        let prefix = tempfile::tempdir().unwrap();
        let p = prefix.path();
        fs::create_dir_all(p.join("bin")).unwrap();
        for archive in [
            b"not gzip".to_vec(),
            tarball(&[("usr/lib/texoria/libpdfium.so", b"new")]),
            tarball(&[("bin/texoria", b"new"), ("lib/texoria/x", b"new")]),
        ] {
            assert!(
                update(tarball_kind(p), "t.tar.gz", &archive)
                    .install()
                    .is_err()
            );
            let left: Vec<_> = fs::read_dir(p)
                .unwrap()
                .flatten()
                .map(|e| e.file_name())
                .collect();
            assert_eq!(left, ["bin"]);
        }
    }

    #[test]
    fn replacing_a_file_that_cannot_be_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("new");
        fs::write(&file, "x").unwrap();
        let err = replace_file(&file, &dir.path().join("gone/TeXoria.AppImage")).unwrap_err();
        assert!(format!("{err:#}").contains("replacing"), "{err:#}");
        assert!(replace_file(&dir.path().join("missing"), &file).is_err());
    }

    #[test]
    fn devices_of_an_image() {
        let info = "framework       : 671.0.2
driver          : 671.0.2
================================================
image-path      : /private/tmp/u/texoria.dmg
image-alias     : /private/tmp/u/texoria.dmg
image-type      : UDIF read-only [write once]
process ID      : 62463
/dev/disk7\tGUID_partition_scheme\t
/dev/disk7s1\t7C3457EF-0000-11AA-AA11-00306543ECAC\t
/dev/disk17\tEF57347C-0000-11AA-AA11-00306543ECAC\t
/dev/disk17s1\t41504653-0000-11AA-AA11-00306543ECAC\t/private/tmp/m
================================================
image-path      : /private/tmp/u/other.dmg
/dev/disk9\tGUID_partition_scheme\t
";
        let image = Path::new("/private/tmp/u/texoria.dmg");
        assert_eq!(image_devices(info, image), ["/dev/disk7", "/dev/disk17"]);
        let other = Path::new("/private/tmp/u/other.dmg");
        assert_eq!(image_devices(info, other), ["/dev/disk9"]);
        assert!(image_devices(info, Path::new("/private/tmp/u")).is_empty());
        assert!(image_devices("", image).is_empty());
    }

    #[test]
    fn commands_that_fail() {
        let missing = Path::new("/nonexistent/program");
        let err = run(&mut Command::new(missing)).unwrap_err();
        assert!(
            err.to_string().contains("running /nonexistent/program"),
            "{err}"
        );
        let exe = std::env::current_exe().unwrap();
        // The test program, with an option it does not know: its error is
        // in the message.
        let err = run(Command::new(&exe).arg("--no-such-option")).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains(" failed: ") && msg.contains("no-such-option"),
            "{msg}"
        );
    }

    /// The reference: Base64 of the UTF-16LE text, bit by bit.
    fn reference_encoding(text: &str) -> String {
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let bytes: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let bits: Vec<bool> = bytes
            .iter()
            .flat_map(|b| (0..8).rev().map(move |i| b >> i & 1 == 1))
            .collect();
        let mut out: String = bits
            .chunks(6)
            .map(|c| {
                let n = (0..6).fold(0, |n, i| n << 1 | usize::from(c.get(i) == Some(&true)));
                ALPHABET[n] as char
            })
            .collect();
        while !out.len().is_multiple_of(4) {
            out.push('=');
        }
        out
    }

    proptest::proptest! {
        #[test]
        fn powershell_encoding_is_base64_of_utf16(text in "\\PC{0,40}") {
            proptest::prop_assert_eq!(encode_command(&text), reference_encoding(&text));
        }
    }

    #[test]
    fn encodes_the_windows_script() {
        assert_eq!(
            encode_command(WINDOWS_SCRIPT),
            reference_encoding(WINDOWS_SCRIPT)
        );
        assert_eq!(reference_encoding("abc"), "YQBiAGMA");
    }

    /// The libraries' folder cannot be written to: nothing changes.
    #[cfg(unix)]
    #[test]
    fn failed_library_rename_changes_nothing() {
        use std::os::unix::fs::PermissionsExt;

        let prefix = tempfile::tempdir().unwrap();
        let p = prefix.path();
        fs::create_dir_all(p.join("bin")).unwrap();
        fs::write(p.join("bin/texoria"), "old").unwrap();
        fs::create_dir_all(p.join("lib")).unwrap();
        fs::set_permissions(p.join("lib"), fs::Permissions::from_mode(0o555)).unwrap();
        if tempfile::tempfile_in(p.join("lib")).is_ok() {
            // root writes anywhere.
            return;
        }
        let err = update(tarball_kind(p), "t.tar.gz", &release_tarball())
            .install()
            .unwrap_err();
        fs::set_permissions(p.join("lib"), fs::Permissions::from_mode(0o755)).unwrap();
        assert!(
            format!("{err:#}").contains("replacing the libraries"),
            "{err:#}"
        );
        assert_eq!(fs::read_to_string(p.join("bin/texoria")).unwrap(), "old");
        assert!(!p.join("lib/texoria").exists());
    }

    /// The new version starts once the old one has exited, with its arguments.
    #[cfg(unix)]
    #[test]
    fn relaunches_after_exit() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("started");
        let script = format!("#!/bin/sh\necho \"$@\" > '{}'\n", marker.display());
        let program = test_support::executable(dir.path(), "texoria", &script);
        let relaunch = Relaunch {
            program,
            args: vec!["--new".into()],
            forwards_args: true,
        };
        let mut old = Command::new("sleep").arg("1").spawn().unwrap();
        relaunch
            .spawn_after(old.id(), vec!["a b.tex".into()])
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(!marker.exists(), "started before the old version exited");
        old.wait().unwrap();
        let started = std::time::Instant::now();
        while !marker.exists() {
            assert!(started.elapsed().as_secs() < 30, "never started");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert_eq!(fs::read_to_string(&marker).unwrap(), "--new a b.tex\n");
    }

    /// PowerShell waits for the old process, then starts the installer with
    /// its arguments as they are.
    #[cfg(windows)]
    #[test]
    fn windows_installer_starts_after_exit() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("started.txt");
        let root = std::env::var_os("SystemRoot").unwrap();
        let cmd = Path::new(&root).join(r"System32\cmd.exe");
        let mut args: Vec<OsString> = ["/C", "echo", "installed>"].map(OsString::from).to_vec();
        args.push(marker.clone().into());
        let relaunch = Relaunch {
            program: cmd.clone(),
            args,
            forwards_args: false,
        };
        let mut old = Command::new(&cmd)
            .args(["/C", "ping -n 3 127.0.0.1 >NUL"])
            .spawn()
            .unwrap();
        relaunch.spawn_after(old.id(), Vec::new()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(500));
        assert!(!marker.exists(), "started before the old version exited");
        old.wait().unwrap();
        let started = std::time::Instant::now();
        while !marker.exists() {
            assert!(started.elapsed().as_secs() < 60, "never started");
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(fs::read_to_string(&marker).unwrap().contains("installed"));
    }

    /// The `.dmg` route, with a tiny app signed ad hoc like the releases.
    #[cfg(target_os = "macos")]
    mod mac {
        use super::*;

        fn tools() -> bool {
            ["/usr/bin/hdiutil", "/usr/bin/codesign", "/usr/bin/ditto"]
                .iter()
                .all(|t| Path::new(t).exists())
        }

        const PLIST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>texoria</string>
<key>CFBundleIdentifier</key><string>org.texoria.test</string>
<key>CFBundlePackageType</key><string>APPL</string>
</dict></plist>
"#;

        /// `TeXoria.app` in `dir`, whose `Resources/version` is `version`.
        fn app(dir: &Path, version: &str, sign: bool) -> PathBuf {
            let app = dir.join("TeXoria.app");
            let contents = app.join("Contents");
            fs::create_dir_all(contents.join("MacOS")).unwrap();
            fs::create_dir_all(contents.join("Resources")).unwrap();
            fs::write(contents.join("Info.plist"), PLIST).unwrap();
            fs::write(contents.join("Resources/version"), version).unwrap();
            fs::copy("/usr/bin/true", contents.join("MacOS/texoria")).unwrap();
            if sign {
                run(Command::new("codesign")
                    .args(["--force", "--deep", "--sign", "-"])
                    .arg(&app))
                .unwrap();
            }
            app
        }

        /// A disk image of `folder`, as the update's file.
        fn dmg(folder: &Path, kind: InstallKind) -> Update {
            let update = update(kind, "texoria.dmg", b"");
            fs::remove_file(&update.file).unwrap();
            run(Command::new("hdiutil")
                .args(["create", "-quiet", "-format", "UDZO", "-volname", "TeXoria"])
                .arg("-srcfolder")
                .arg(folder)
                .arg(&update.file))
            .unwrap();
            update
        }

        fn installed(apps: &Path) -> (InstallKind, PathBuf) {
            let bundle = app(apps, "old", false);
            (
                InstallKind::MacApp {
                    bundle: bundle.clone(),
                },
                bundle,
            )
        }

        fn version(bundle: &Path) -> String {
            fs::read_to_string(bundle.join("Contents/Resources/version")).unwrap()
        }

        fn left(apps: &Path) -> Vec<OsString> {
            fs::read_dir(apps)
                .unwrap()
                .flatten()
                .map(|e| e.file_name())
                .collect()
        }

        #[test]
        fn installs_from_a_disk_image() {
            if !tools() {
                test_support::skip("HDIUTIL", "hdiutil or codesign not found");
                return;
            }
            let tmp = tempfile::tempdir().unwrap();
            let apps = tmp.path().join("Applications");
            let (kind, bundle) = installed(&apps);
            let src = tmp.path().join("src");
            app(&src, "new", true);
            let relaunch = dmg(&src, kind).install().unwrap();
            assert_eq!(version(&bundle), "new");
            assert_eq!(relaunch.program, Path::new("/usr/bin/open"));
            assert_eq!(relaunch.args, [bundle.as_os_str()]);
            assert!(!relaunch.installs());
            run(Command::new("codesign")
                .args(["--verify", "--strict"])
                .arg(&bundle))
            .unwrap();
            assert_eq!(left(&apps), ["TeXoria.app"]);
        }

        /// An app whose signature does not hold is not installed.
        #[test]
        fn refuses_a_broken_signature() {
            if !tools() {
                test_support::skip("HDIUTIL", "hdiutil or codesign not found");
                return;
            }
            let tmp = tempfile::tempdir().unwrap();
            let apps = tmp.path().join("Applications");
            let (kind, bundle) = installed(&apps);
            let src = tmp.path().join("src");
            let new = app(&src, "new", true);
            fs::write(new.join("Contents/Resources/version"), "tampered").unwrap();
            let err = dmg(&src, kind).install().unwrap_err();
            assert!(err.to_string().contains("codesign failed"), "{err}");
            assert_eq!(version(&bundle), "old");
            assert_eq!(left(&apps), ["TeXoria.app"]);
        }

        #[test]
        fn refuses_an_image_without_the_app() {
            if !tools() {
                test_support::skip("HDIUTIL", "hdiutil or codesign not found");
                return;
            }
            let tmp = tempfile::tempdir().unwrap();
            let apps = tmp.path().join("Applications");
            let (kind, bundle) = installed(&apps);
            let src = tmp.path().join("src");
            fs::create_dir_all(&src).unwrap();
            fs::write(src.join("README"), "no app").unwrap();
            let err = dmg(&src, kind.clone()).install().unwrap_err();
            assert!(err.to_string().contains("ditto failed"), "{err}");
            assert_eq!(version(&bundle), "old");
            // Not a disk image at all.
            let update = update(kind, "texoria.dmg", b"not a dmg");
            let err = update.install().unwrap_err();
            assert!(err.to_string().contains("hdiutil failed"), "{err}");
            assert_eq!(left(&apps), ["TeXoria.app"]);
        }
    }
}
