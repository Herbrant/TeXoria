//! TeX engines and their discovery on the host system.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Which TeX format/engine latexmk should drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Flavor {
    PdfLatex,
    XeLatex,
    LuaLatex,
}

impl Flavor {
    fn latexmk_flag(self) -> &'static str {
        match self {
            Flavor::PdfLatex => "-pdf",
            Flavor::XeLatex => "-xelatex",
            Flavor::LuaLatex => "-lualatex",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Flavor::PdfLatex => "pdfLaTeX",
            Flavor::XeLatex => "XeLaTeX",
            Flavor::LuaLatex => "LuaLaTeX",
        }
    }
}

/// A single compilation request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    /// Absolute path of the root `.tex` file.
    pub root_file: PathBuf,
    /// Directory for all build artifacts (created if missing).
    pub out_dir: PathBuf,
}

impl Job {
    /// Convention used by the app: artifacts go in `<root dir>/.texoria/build`.
    pub fn for_root(root_file: impl Into<PathBuf>) -> Self {
        let root_file = root_file.into();
        let out_dir = root_file
            .parent()
            .unwrap_or(Path::new("."))
            .join(".texoria")
            .join("build");
        Job { root_file, out_dir }
    }

    pub fn stem(&self) -> String {
        self.root_file
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    pub fn work_dir(&self) -> &Path {
        self.root_file.parent().unwrap_or(Path::new("."))
    }

    pub fn artifact(&self, ext: &str) -> PathBuf {
        self.out_dir.join(format!("{}.{ext}", self.stem()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineKind {
    /// System TeX distribution through latexmk.
    Latexmk { latexmk: PathBuf, flavor: Flavor },
    /// Self-contained Tectonic binary (bundled with the app or on PATH).
    Tectonic { binary: PathBuf },
    /// User-defined command, run in the root file's directory with the root
    /// file name appended as the last argument.
    Custom { program: PathBuf, args: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Engine {
    pub kind: EngineKind,
    /// Extra directories to prepend to `PATH` for the child process. GUI apps
    /// on macOS don't inherit the shell `PATH`, so latexmk would otherwise
    /// fail to find `pdflatex`, `bibtex`, `perl`...
    pub extra_path: Vec<PathBuf>,
}

impl Engine {
    pub fn label(&self) -> String {
        match &self.kind {
            EngineKind::Latexmk { flavor, .. } => format!("{} (latexmk)", flavor.label()),
            EngineKind::Tectonic { .. } => "Tectonic".to_string(),
            EngineKind::Custom { program, .. } => program
                .file_name()
                .map_or("custom".into(), |n| n.to_string_lossy().into_owned()),
        }
    }

    pub fn command(&self, job: &Job) -> Command {
        let file = job.root_file.file_name().unwrap_or_default();
        let mut cmd = match &self.kind {
            EngineKind::Latexmk { latexmk, flavor } => {
                let mut c = Command::new(latexmk);
                c.arg(flavor.latexmk_flag())
                    .args(["-interaction=nonstopmode", "-file-line-error", "-synctex=1"])
                    .arg(format!("-outdir={}", job.out_dir.display()))
                    .arg(file);
                c
            }
            EngineKind::Tectonic { binary } => {
                let mut c = Command::new(binary);
                c.args([
                    "-X",
                    "compile",
                    "--synctex",
                    "--keep-logs",
                    "--keep-intermediates",
                ])
                .arg("--outdir")
                .arg(&job.out_dir)
                .arg(file);
                c
            }
            EngineKind::Custom { program, args } => {
                let mut c = Command::new(program);
                c.args(args).arg(file);
                c
            }
        };
        cmd.current_dir(job.work_dir());
        if !self.extra_path.is_empty() {
            cmd.env("PATH", prepend_path(&self.extra_path));
        }
        cmd
    }
}

fn prepend_path(dirs: &[PathBuf]) -> OsString {
    let current = std::env::var_os("PATH").unwrap_or_default();
    let all = dirs.iter().cloned().chain(std::env::split_paths(&current));
    std::env::join_paths(all).unwrap_or(current)
}

/// Discover every usable engine, best first: system TeX (more complete and
/// faster for users who have it) followed by Tectonic.
pub fn detect(bundled_dir: Option<&Path>) -> Vec<Engine> {
    let mut engines = Vec::new();

    if let Some((latexmk, bin_dir)) = find_tool("latexmk", &tex_bin_dirs()) {
        let extra_path = bin_dir.into_iter().collect::<Vec<_>>();
        for flavor in [Flavor::PdfLatex, Flavor::XeLatex, Flavor::LuaLatex] {
            engines.push(Engine {
                kind: EngineKind::Latexmk {
                    latexmk: latexmk.clone(),
                    flavor,
                },
                extra_path: extra_path.clone(),
            });
        }
    }

    let mut tectonic_dirs: Vec<PathBuf> = bundled_dir.into_iter().map(Path::to_path_buf).collect();
    tectonic_dirs.extend(bundled_tectonic_dirs());
    if let Some((binary, _)) = find_tool("tectonic", &tectonic_dirs) {
        engines.push(Engine {
            kind: EngineKind::Tectonic { binary },
            extra_path: Vec::new(),
        });
    }

    engines
}

/// Look for `name` on `PATH` first, then in `fallback` dirs. Returns the
/// binary and, if found in a fallback dir, that dir (to add to `PATH`).
fn find_tool(name: &str, fallback: &[PathBuf]) -> Option<(PathBuf, Option<PathBuf>)> {
    if let Ok(p) = which::which(name) {
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

/// Well-known TeX binary locations not necessarily on a GUI app's `PATH`.
fn tex_bin_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from);

    if cfg!(target_os = "macos") {
        dirs.push(PathBuf::from("/Library/TeX/texbin"));
        dirs.push(PathBuf::from("/opt/homebrew/bin"));
    }
    if cfg!(unix) {
        dirs.push(PathBuf::from("/usr/local/bin"));
        dirs.push(PathBuf::from("/usr/bin"));
        // Vanilla TeX Live: /usr/local/texlive/<year>/bin/<arch>, newest first.
        dirs.extend(texlive_year_dirs(Path::new("/usr/local/texlive")));
        if let Some(h) = &home {
            dirs.extend(glob_one_level(&h.join(".TinyTeX/bin")));
            dirs.extend(glob_one_level(&h.join("Library/TinyTeX/bin")));
        }
    }
    if cfg!(windows) {
        if let Some(local) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
            dirs.push(local.join("Programs/MiKTeX/miktex/bin/x64"));
        }
        for pf in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(p) = std::env::var_os(pf).map(PathBuf::from) {
                dirs.push(p.join("MiKTeX/miktex/bin/x64"));
            }
        }
        dirs.extend(texlive_year_dirs(Path::new("C:/texlive")));
        if let Some(h) = &home {
            dirs.extend(glob_one_level(&h.join("AppData/Roaming/TinyTeX/bin")));
        }
    }
    dirs
}

fn texlive_year_dirs(base: &Path) -> Vec<PathBuf> {
    let mut years: Vec<PathBuf> = std::fs::read_dir(base)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("20"))
        })
        .collect();
    years.sort();
    years.reverse();
    years
        .iter()
        .flat_map(|y| glob_one_level(&y.join("bin")))
        .collect()
}

fn glob_one_level(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect()
}

/// Where a packaged app ships its own `tectonic` binary.
fn bundled_tectonic_dirs() -> Vec<PathBuf> {
    let Ok(exe) = std::env::current_exe() else {
        return Vec::new();
    };
    let Some(dir) = exe.parent() else {
        return Vec::new();
    };
    let mut dirs = vec![dir.to_path_buf(), dir.join("bin")];
    if cfg!(target_os = "macos") {
        // TeXoria.app/Contents/MacOS/texoria → Contents/Resources/bin
        dirs.push(dir.join("../Resources/bin"));
    }
    if cfg!(target_os = "linux") {
        // deb / AppImage: usr/bin/texoria → usr/lib/texoria/bin
        dirs.push(dir.join("../lib/texoria/bin"));
    }
    dirs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_paths() {
        let job = Job::for_root("/p/thesis.tex");
        assert_eq!(job.out_dir, PathBuf::from("/p/.texoria/build"));
        assert_eq!(
            job.artifact("pdf"),
            PathBuf::from("/p/.texoria/build/thesis.pdf")
        );
        assert_eq!(job.work_dir(), Path::new("/p"));
    }

    #[test]
    fn latexmk_command_line() {
        let engine = Engine {
            kind: EngineKind::Latexmk {
                latexmk: "latexmk".into(),
                flavor: Flavor::XeLatex,
            },
            extra_path: vec![],
        };
        let job = Job::for_root("/p/main.tex");
        let cmd = engine.command(&job);
        let args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args[0], "-xelatex");
        assert!(args.contains(&"-synctex=1".to_string()));
        // `job_paths` checks the directory itself; joining uses `\` on Windows.
        assert!(args.contains(&format!("-outdir={}", job.out_dir.display())));
        assert_eq!(args.last().unwrap(), "main.tex");
        assert_eq!(cmd.get_current_dir(), Some(Path::new("/p")));
    }
}
