//! How the running copy was installed, which decides whether it can replace
//! itself and with which release file.

use std::path::{Path, PathBuf};

use semver::Version;

/// Homebrew's cask folders (Apple silicon, Intel).
const CASKROOMS: [&str; 2] = [
    "/opt/homebrew/Caskroom/texoria",
    "/usr/local/Caskroom/texoria",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallKind {
    /// A `TeXoria.app` the user may replace (copied from the `.dmg`, or by
    /// `install.sh`).
    MacApp { bundle: PathBuf },
    /// The NSIS installer's per-user installation.
    Windows { dir: PathBuf },
    /// The AppImage file.
    AppImage { file: PathBuf },
    /// `install.sh`'s tarball, unpacked under a prefix the user owns.
    Tarball { prefix: PathBuf },
    /// Updated by a package manager.
    Managed(Manager),
    /// Packaged, but it cannot replace itself: download the release by hand.
    Manual(&'static str),
    /// Built from source (`cargo run`).
    Dev,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Manager {
    Homebrew,
    /// deb, rpm, the AUR...
    System,
}

/// What [`InstallKind::classify`] needs to know about the file system.
pub trait Probe {
    fn exists(&self, path: &Path) -> bool;
    /// Whether this user can create files in the directory `path`.
    fn writable(&self, path: &Path) -> bool;
}

/// The real file system.
pub struct Disk;

impl Probe for Disk {
    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn writable(&self, path: &Path) -> bool {
        // Permissions alone do not tell (ACLs, read-only volumes, SIP).
        tempfile::tempfile_in(path).is_ok()
    }
}

impl InstallKind {
    /// The running copy.
    pub fn detect() -> Self {
        let Ok(exe) = std::env::current_exe().and_then(|e| e.canonicalize()) else {
            return Self::Manual("TeXoria could not find where it is installed.");
        };
        let exe = plain(exe);
        let var = |name| std::env::var_os(name).map(PathBuf::from);
        let appdir = var("APPDIR").and_then(|d| d.canonicalize().ok());
        let appimage = running_appimage(&exe, var("APPIMAGE"), appdir.as_deref());
        Self::classify(&exe, std::env::consts::OS, appimage.as_deref(), &Disk)
    }

    /// `exe` is the running executable on `os` (`std::env::consts::OS`),
    /// `appimage` the `$APPIMAGE` variable set by the AppImage runtime.
    pub fn classify(exe: &Path, os: &str, appimage: Option<&Path>, fs: &impl Probe) -> Self {
        let Some(dir) = exe.parent() else {
            return Self::Dev;
        };
        // `target/{debug,release}/texoria`
        let built = |d: &Path| {
            matches!(
                d.file_name().and_then(|n| n.to_str()),
                Some("debug" | "release")
            ) && d.parent().and_then(Path::file_name) == Some("target".as_ref())
        };
        if built(dir) {
            return Self::Dev;
        }
        match os {
            "macos" => {
                // TeXoria.app/Contents/MacOS/texoria
                let bundle = dir
                    .parent()
                    .filter(|_| dir.file_name() == Some("MacOS".as_ref()))
                    .and_then(Path::parent)
                    .filter(|b| b.extension() == Some("app".as_ref()));
                let Some(bundle) = bundle else {
                    return Self::Manual("TeXoria is not running from its application bundle.");
                };
                if CASKROOMS.iter().any(|c| fs.exists(Path::new(c))) {
                    return Self::Managed(Manager::Homebrew);
                }
                // Gatekeeper runs quarantined apps from a read-only copy.
                if bundle
                    .components()
                    .any(|c| c.as_os_str() == "AppTranslocation")
                {
                    return Self::Manual(
                        "Move TeXoria to the Applications folder to update it from the app.",
                    );
                }
                match bundle.parent() {
                    Some(parent) if fs.writable(parent) => Self::MacApp {
                        bundle: bundle.to_path_buf(),
                    },
                    _ => Self::Manual(
                        "TeXoria is in a folder you cannot write to (a disk image?): \
                         copy it to Applications first.",
                    ),
                }
            }
            "windows" => {
                if !fs.exists(&dir.join("uninstall.exe")) {
                    Self::Manual("TeXoria was not installed with its installer.")
                } else if fs.writable(dir) {
                    Self::Windows {
                        dir: dir.to_path_buf(),
                    }
                } else {
                    Self::Manual(
                        "TeXoria was installed for all users: update it as an administrator.",
                    )
                }
            }
            "linux" => {
                if let Some(file) = appimage {
                    return match file.parent() {
                        Some(parent) if fs.writable(parent) => Self::AppImage {
                            file: file.to_path_buf(),
                        },
                        _ => Self::Manual("The AppImage is in a folder you cannot write to."),
                    };
                }
                // The distributions' packages (deb, rpm, the AUR) install to
                // `/usr`, which only root could write to anyway.
                let system = |p: &Path| p.starts_with("/usr") && !p.starts_with("/usr/local");
                if system(exe) {
                    return Self::Managed(Manager::System);
                }
                // <prefix>/bin/texoria, <prefix>/lib/texoria: `install.sh`'s
                // layout, in any prefix the user owns (`/usr/local`, `/opt/x`...).
                let prefix = dir
                    .parent()
                    .filter(|_| dir.file_name() == Some("bin".as_ref()));
                // The update is unpacked in the prefix itself (see `staging_parent`).
                match prefix {
                    Some(prefix)
                        if fs.exists(&prefix.join("lib/texoria"))
                            && fs.writable(prefix)
                            && fs.writable(dir)
                            && fs.writable(&prefix.join("lib")) =>
                    {
                        Self::Tarball {
                            prefix: prefix.to_path_buf(),
                        }
                    }
                    _ if exe.starts_with("/usr/local") || exe.starts_with("/opt") => {
                        Self::Managed(Manager::System)
                    }
                    _ => Self::Manual("TeXoria is not installed where it can update itself."),
                }
            }
            _ => Self::Manual("Updates are not available on this system."),
        }
    }

    /// Whether the app can download and install a release by itself.
    pub fn self_updating(&self) -> bool {
        matches!(
            self,
            Self::MacApp { .. }
                | Self::Windows { .. }
                | Self::AppImage { .. }
                | Self::Tarball { .. }
        )
    }

    /// The release file this installation is updated from, for `arch`
    /// (`std::env::consts::ARCH`).
    pub fn asset(&self, version: &Version, arch: &str) -> Option<String> {
        let ext = match self {
            Self::MacApp { .. } => format!("macos-{arch}.dmg"),
            Self::Windows { .. } => format!("windows-{arch}-setup.exe"),
            Self::AppImage { .. } => format!("linux-{arch}.AppImage"),
            Self::Tarball { .. } => format!("linux-{arch}.tar.gz"),
            Self::Managed(_) | Self::Manual(_) | Self::Dev => return None,
        };
        Some(format!("texoria-{version}-{ext}"))
    }

    /// How to update an installation the app cannot update by itself.
    pub fn instructions(&self) -> Option<&'static str> {
        match self {
            Self::Managed(Manager::Homebrew) => Some("Update it with: brew upgrade --cask texoria"),
            Self::Managed(Manager::System) => Some("Update it with your package manager."),
            Self::Manual(why) => Some(why),
            _ => None,
        }
    }

    /// The folder new files are downloaded to, on the same file system as
    /// what they replace (a rename cannot cross file systems). The Windows
    /// installer goes to the temporary folder: it must outlive the app.
    pub(crate) fn staging_parent(&self) -> Option<PathBuf> {
        match self {
            Self::MacApp { bundle } => bundle.parent().map(Path::to_path_buf),
            Self::Windows { .. } => Some(std::env::temp_dir()),
            Self::AppImage { file } => file.parent().map(Path::to_path_buf),
            Self::Tarball { prefix } => Some(prefix.clone()),
            Self::Managed(_) | Self::Manual(_) | Self::Dev => None,
        }
    }
}

/// `$APPIMAGE`, if it is the AppImage this executable runs from. The
/// variable is inherited by every program an AppImage starts (a terminal, a
/// file manager...), so a copy installed otherwise may see another app's.
/// The runtime also sets `$APPDIR`, where the image is mounted.
fn running_appimage(
    exe: &Path,
    appimage: Option<PathBuf>,
    appdir: Option<&Path>,
) -> Option<PathBuf> {
    appimage.filter(|_| appdir.is_some_and(|d| exe.starts_with(d)))
}

/// `\\?\C:\dir` as `C:\dir`. `canonicalize` returns such verbatim paths on
/// Windows, which the installer would take as its folder (`/D=`).
fn plain(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    if let Some(rest) = path.to_str().and_then(|s| s.strip_prefix(r"\\?\"))
        && rest.as_bytes().get(1) == Some(&b':')
    {
        return PathBuf::from(rest);
    }
    path
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[derive(Default)]
    struct Fake {
        exists: HashSet<PathBuf>,
        writable: HashSet<PathBuf>,
    }

    impl Fake {
        fn with(mut self, path: &str, writable: bool) -> Self {
            self.exists.insert(path.into());
            if writable {
                self.writable.insert(path.into());
            }
            self
        }
    }

    impl Probe for Fake {
        fn exists(&self, path: &Path) -> bool {
            self.exists.contains(path)
        }
        fn writable(&self, path: &Path) -> bool {
            self.writable.contains(path)
        }
    }

    fn classify(exe: &str, os: &str, fs: &Fake) -> InstallKind {
        InstallKind::classify(Path::new(exe), os, None, fs)
    }

    const MAC_EXE: &str = "/Applications/TeXoria.app/Contents/MacOS/texoria";

    #[test]
    fn source_builds() {
        let fs = Fake::default();
        for os in ["macos", "linux", "windows"] {
            assert_eq!(
                classify("/src/TeXoria/target/debug/texoria", os, &fs),
                InstallKind::Dev
            );
            assert_eq!(
                classify("/src/TeXoria/target/release/texoria", os, &fs),
                InstallKind::Dev
            );
        }
    }

    #[test]
    fn mac_app() {
        let fs = Fake::default().with("/Applications", true);
        assert_eq!(
            classify(MAC_EXE, "macos", &fs),
            InstallKind::MacApp {
                bundle: "/Applications/TeXoria.app".into()
            }
        );
        let fs = Fake::default()
            .with("/Applications", true)
            .with(CASKROOMS[0], false);
        assert_eq!(
            classify(MAC_EXE, "macos", &fs),
            InstallKind::Managed(Manager::Homebrew)
        );
    }

    #[test]
    fn mac_app_that_cannot_be_replaced() {
        let fs = Fake::default().with("/Volumes/TeXoria", false);
        let kind = classify(
            "/Volumes/TeXoria/TeXoria.app/Contents/MacOS/texoria",
            "macos",
            &fs,
        );
        assert!(matches!(kind, InstallKind::Manual(_)));
        let translocated =
            "/private/var/folders/x/T/AppTranslocation/ABCD/d/TeXoria.app/Contents/MacOS/texoria";
        let fs = Fake::default().with("/private/var/folders/x/T/AppTranslocation/ABCD/d", true);
        assert!(matches!(
            classify(translocated, "macos", &fs),
            InstallKind::Manual(_)
        ));
        assert!(matches!(
            classify("/usr/local/bin/texoria", "macos", &fs),
            InstallKind::Manual(_)
        ));
    }

    #[test]
    fn windows_installer() {
        let dir = "C:/Users/me/AppData/Local/TeXoria";
        let exe = "C:/Users/me/AppData/Local/TeXoria/texoria.exe";
        let fs = Fake::default()
            .with(dir, true)
            .with("C:/Users/me/AppData/Local/TeXoria/uninstall.exe", false);
        assert_eq!(
            classify(exe, "windows", &fs),
            InstallKind::Windows { dir: dir.into() }
        );
        let fs = Fake::default().with(dir, true);
        assert!(matches!(
            classify(exe, "windows", &fs),
            InstallKind::Manual(_)
        ));
    }

    #[test]
    fn linux_layouts() {
        let fs = Fake::default()
            .with("/home/me/.local", true)
            .with("/home/me/.local/bin", true)
            .with("/home/me/.local/lib", true)
            .with("/home/me/.local/lib/texoria", true)
            .with("/home/me/Apps", true);
        assert_eq!(
            classify("/home/me/.local/bin/texoria", "linux", &fs),
            InstallKind::Tarball {
                prefix: "/home/me/.local".into()
            }
        );
        assert_eq!(
            classify("/usr/bin/texoria", "linux", &fs),
            InstallKind::Managed(Manager::System)
        );
        let appimage = Path::new("/home/me/Apps/TeXoria.AppImage");
        assert_eq!(
            InstallKind::classify(
                Path::new("/tmp/.mount_TeXori/usr/bin/texoria"),
                "linux",
                Some(appimage),
                &fs
            ),
            InstallKind::AppImage {
                file: appimage.into()
            }
        );
        // An unpacked tarball without its libraries.
        assert!(matches!(
            classify("/home/me/x/bin/texoria", "linux", &fs),
            InstallKind::Manual(_)
        ));
        // A prefix where the update cannot be unpacked.
        let fs = Fake::default()
            .with("/srv/me", false)
            .with("/srv/me/bin", true)
            .with("/srv/me/lib", true)
            .with("/srv/me/lib/texoria", true);
        assert!(matches!(
            classify("/srv/me/bin/texoria", "linux", &fs),
            InstallKind::Manual(_)
        ));
    }

    /// `install.sh` takes any prefix the user owns (`TEXORIA_PREFIX`), even
    /// under `/usr/local` or `/opt`; the distributions' packages are in `/usr`.
    #[test]
    fn tarball_installs_under_system_folders() {
        let prefix = |p: &str| {
            Fake::default()
                .with(p, true)
                .with(&format!("{p}/bin"), true)
                .with(&format!("{p}/lib"), true)
                .with(&format!("{p}/lib/texoria"), true)
        };
        for p in ["/usr/local", "/opt/texoria", "/opt"] {
            assert_eq!(
                classify(&format!("{p}/bin/texoria"), "linux", &prefix(p)),
                InstallKind::Tarball { prefix: p.into() },
                "{p}"
            );
        }
        // deb, rpm and the AUR, even for root, who can write there.
        assert_eq!(
            classify("/usr/bin/texoria", "linux", &prefix("/usr")),
            InstallKind::Managed(Manager::System)
        );
        // Someone else's package, or a copy the user cannot replace.
        let fs = Fake::default()
            .with("/opt/texoria", false)
            .with("/opt/texoria/bin", false)
            .with("/opt/texoria/lib", false)
            .with("/opt/texoria/lib/texoria", false);
        for exe in [
            "/opt/texoria/bin/texoria",
            "/opt/texoria/texoria",
            "/usr/local/bin/texoria",
            "/usr/lib/texoria/texoria",
        ] {
            assert_eq!(
                classify(exe, "linux", &fs),
                InstallKind::Managed(Manager::System),
                "{exe}"
            );
        }
    }

    #[test]
    fn appimage_of_another_app() {
        let exe = Path::new("/tmp/.mount_TeXori/usr/bin/texoria");
        let file = || Some(PathBuf::from("/home/me/Apps/TeXoria.AppImage"));
        let mount = Path::new("/tmp/.mount_TeXori");
        assert_eq!(running_appimage(exe, file(), Some(mount)), file());
        // Started from a terminal that an AppImage started.
        let deb = Path::new("/usr/bin/texoria");
        assert_eq!(running_appimage(deb, file(), Some(mount)), None);
        assert_eq!(running_appimage(exe, file(), None), None);
    }

    #[test]
    fn assets() {
        let v = Version::new(1, 2, 3);
        let mac = InstallKind::MacApp {
            bundle: "/Applications/TeXoria.app".into(),
        };
        assert_eq!(
            mac.asset(&v, "aarch64").unwrap(),
            "texoria-1.2.3-macos-aarch64.dmg"
        );
        let win = InstallKind::Windows { dir: "C:/x".into() };
        assert_eq!(
            win.asset(&v, "x86_64").unwrap(),
            "texoria-1.2.3-windows-x86_64-setup.exe"
        );
        let app = InstallKind::AppImage { file: "/a".into() };
        assert_eq!(
            app.asset(&v, "x86_64").unwrap(),
            "texoria-1.2.3-linux-x86_64.AppImage"
        );
        let tar = InstallKind::Tarball {
            prefix: "/p".into(),
        };
        assert_eq!(
            tar.asset(&v, "aarch64").unwrap(),
            "texoria-1.2.3-linux-aarch64.tar.gz"
        );
        assert_eq!(
            InstallKind::Managed(Manager::Homebrew).asset(&v, "x86_64"),
            None
        );
    }
}
