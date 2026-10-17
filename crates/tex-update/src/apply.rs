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

    /// The installer closes every `texoria.exe` without asking (the NSIS
    /// template's passive mode): PowerShell waits for this process to exit,
    /// asks to close the other windows, which would lose their unsaved edits,
    /// and only then starts it.
    #[cfg(windows)]
    fn spawn_with(&self, _extra: Vec<OsString>) -> Result<()> {
        use std::os::windows::process::CommandExt;

        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
        let powershell = Path::new(&root).join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
        // `Start-Process` passes a single argument string as is: NSIS reads
        // `/D=` raw, and quoting would become part of the path.
        let mut args = OsString::new();
        for (i, a) in self.args.iter().enumerate() {
            if i > 0 {
                args.push(" ");
            }
            args.push(a);
        }
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
                .env("TEXORIA_UPDATE_PID", std::process::id().to_string())
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
        let flags = CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW;
        spawn(flags | CREATE_BREAKAWAY_FROM_JOB)
            .or_else(|_| spawn(flags))
            .with_context(|| format!("starting {}", powershell.display()))?;
        Ok(())
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
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(anyhow!("{program} failed: {}", err.trim()));
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
}
