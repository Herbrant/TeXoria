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
    /// Let latexmk read the project's own `latexmkrc` / `.latexmkrc`. These
    /// are Perl scripts, so they are only honored for trusted projects.
    pub allow_project_rc: bool,
    /// Typeset even if the outputs look up to date (latexmk `-g`): they may
    /// not be the engine's.
    pub force: bool,
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
        Job {
            root_file,
            out_dir,
            allow_project_rc: false,
            force: false,
        }
    }

    /// Whether the project ships a latexmk rc file that latexmk would run.
    pub fn has_project_rc(&self) -> bool {
        PROJECT_RC_FILES
            .iter()
            .any(|name| self.work_dir().join(name).is_file())
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
        // `./name`, so a file called `-pdflatex=...tex` is not read as an option.
        let file = Path::new(".").join(job.root_file.file_name().unwrap_or_default());
        let mut cmd = match &self.kind {
            EngineKind::Latexmk { latexmk, flavor } => {
                let mut c = Command::new(latexmk);
                if !job.allow_project_rc {
                    // `-norc` also skips the system and user rc files, which
                    // are the user's own configuration: read those explicitly.
                    c.arg("-norc");
                    for rc in latexmk_rc_files() {
                        c.arg("-r").arg(rc);
                    }
                }
                if job.force {
                    c.arg("-g");
                }
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
        // The engine runs in the project, which may come from anywhere: a
        // `pdflatex` or `bibtex` planted there must never be what runs.
        cmd.env("PATH", prepend_path(&self.extra_path));
        if cfg!(windows) {
            // cmd.exe (latexmk's `system`) looks in the current directory
            // before `PATH` unless this is set.
            cmd.env("NoDefaultCurrentDirectoryInExePath", "1");
        }
        cmd
    }
}

/// Rc files latexmk reads from the directory it runs in.
const PROJECT_RC_FILES: [&str; 2] = ["latexmkrc", ".latexmkrc"];

/// The system and user rc files latexmk would read on its own, in its order.
fn latexmk_rc_files() -> Vec<PathBuf> {
    let env_path = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    let mut files = Vec::new();

    let system = match env_path("LATEXMKRCSYS") {
        Some(p) => vec![p],
        None if cfg!(windows) => vec![PathBuf::from("C:/latexmk/LatexMk")],
        None => [
            "/opt/local/share/latexmk/LatexMk",
            "/usr/local/share/latexmk/LatexMk",
            "/usr/local/lib/latexmk/LatexMk",
        ]
        .map(PathBuf::from)
        .to_vec(),
    };
    files.extend(system.into_iter().find(|p| p.is_file()));

    let home = env_path("HOME").or_else(|| env_path("USERPROFILE"));
    let xdg = env_path("XDG_CONFIG_HOME")
        .or_else(|| home.as_ref().map(|h| h.join(".config")))
        .map(|d| d.join("latexmk").join("latexmkrc"));
    let mut user = xdg.into_iter().chain(home.map(|h| h.join(".latexmkrc")));
    files.extend(user.find(|p| p.is_file()));
    files
}

/// `dirs`, then the process's `PATH`, without empty or relative entries:
/// those name the current directory (the project) on Unix.
fn prepend_path(dirs: &[PathBuf]) -> OsString {
    let current = std::env::var_os("PATH").unwrap_or_default();
    let all = dirs
        .iter()
        .cloned()
        .chain(std::env::split_paths(&current))
        .filter(|d| d.is_absolute());
    std::env::join_paths(all).unwrap_or_default()
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

    // The copy shipped with the app wins over the caller's extra dir.
    let mut tectonic_dirs = bundled_tectonic_dirs();
    tectonic_dirs.extend(bundled_dir.map(Path::to_path_buf));
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
        let engine = latexmk(Flavor::XeLatex);
        let job = Job::for_root("/p/main.tex");
        let cmd = engine.command(&job);
        let args = args_of(&cmd);
        assert!(args.contains(&"-xelatex".to_string()));
        assert!(args.contains(&"-synctex=1".to_string()));
        // `job_paths` checks the directory itself; joining uses `\` on Windows.
        assert!(args.contains(&format!("-outdir={}", job.out_dir.display())));
        assert_eq!(
            Path::new(args.last().unwrap()),
            Path::new(".").join("main.tex")
        );
        assert_eq!(cmd.get_current_dir(), Some(Path::new("/p")));
        // Untrusted by default: project rc files are not read.
        assert_eq!(args[0], "-norc");
        let rc_args = args
            .iter()
            .position(|a| a == "-xelatex")
            .expect("flavor flag");
        assert!(
            args[1..rc_args]
                .chunks(2)
                .all(|w| w[0] == "-r" && Path::new(&w[1]).is_file()),
            "{args:?}"
        );
    }

    #[test]
    fn trusted_job_reads_project_rc() {
        let engine = latexmk(Flavor::PdfLatex);
        let mut job = Job::for_root("/p/main.tex");
        job.allow_project_rc = true;
        let args = args_of(&engine.command(&job));
        assert_eq!(args[0], "-pdf");
        assert!(!args.iter().any(|a| a == "-norc" || a == "-r" || a == "-g"));
        job.force = true;
        assert!(args_of(&engine.command(&job)).iter().any(|a| a == "-g"));
    }

    #[test]
    fn file_name_cannot_inject_options() {
        let job = Job::for_root("/p/-pdflatex=touch pwned;.tex");
        let tectonic = Engine {
            kind: EngineKind::Tectonic {
                binary: "tectonic".into(),
            },
            extra_path: vec![],
        };
        for engine in [latexmk(Flavor::PdfLatex), tectonic] {
            let args = args_of(&engine.command(&job));
            assert!(args.iter().all(|a| !a.starts_with("-pdflatex")), "{args:?}");
        }
    }

    #[test]
    fn path_never_names_the_project() {
        let job = Job::for_root("/p/main.tex");
        let cmd = latexmk(Flavor::PdfLatex).command(&job);
        let path = cmd
            .get_envs()
            .find(|(k, _)| *k == "PATH")
            .and_then(|(_, v)| v)
            .expect("PATH is set");
        assert!(std::env::split_paths(path).all(|d| d.is_absolute()));
        let dirs = [PathBuf::from("."), PathBuf::from("bin")];
        let joined = prepend_path(&dirs);
        assert!(std::env::split_paths(&joined).all(|d| d.is_absolute()));
    }

    #[test]
    fn detects_project_rc() {
        let dir = tempfile::tempdir().unwrap();
        let job = Job::for_root(dir.path().join("main.tex"));
        assert!(!job.has_project_rc());
        std::fs::write(dir.path().join(".latexmkrc"), "").unwrap();
        assert!(job.has_project_rc());
    }

    fn latexmk(flavor: Flavor) -> Engine {
        Engine {
            kind: EngineKind::Latexmk {
                latexmk: "latexmk".into(),
                flavor,
            },
            extra_path: vec![],
        }
    }

    fn args_of(cmd: &Command) -> Vec<String> {
        cmd.get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }
}
