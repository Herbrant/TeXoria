//! TeX engines and their discovery on the host system.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

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
    /// Build even if the outputs look up to date (latexmk `-g`): they may
    /// not be the engine's.
    pub force: bool,
    /// Let the document run programs (`\write18`: minted, svg, pythontex),
    /// for projects the user trusts.
    pub shell_escape: bool,
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
            shell_escape: false,
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
        // The parent of a bare `main.tex` is the empty path: no folder to
        // run in.
        self.root_file
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
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
                    for rc in latexmk_rc_files(&process_env) {
                        c.arg("-r").arg(rc);
                    }
                }
                if job.force {
                    c.arg("-g");
                }
                if job.shell_escape {
                    c.arg("-shell-escape");
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
                ]);
                if job.shell_escape {
                    c.args(["-Z", "shell-escape"]);
                }
                c.arg("--outdir").arg(&job.out_dir).arg(file);
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
        cmd.env(
            "PATH",
            prepend_path(&self.extra_path, std::env::var_os("PATH")),
        );
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

/// Reads an environment variable: the process's ([`process_env`]), or
/// another one in tests.
type Env<'a> = &'a dyn Fn(&str) -> Option<OsString>;

fn process_env(key: &str) -> Option<OsString> {
    std::env::var_os(key)
}

/// The system and user rc files latexmk would read on its own, in its order.
fn latexmk_rc_files(env: Env) -> Vec<PathBuf> {
    let env_path = |k: &str| env(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    let mut files = Vec::new();
    files.extend(system_rc_candidates(env).into_iter().find(|p| p.is_file()));

    let home = env_path("HOME").or_else(|| env_path("USERPROFILE"));
    let xdg = env_path("XDG_CONFIG_HOME")
        .or_else(|| home.as_ref().map(|h| h.join(".config")))
        .map(|d| d.join("latexmk").join("latexmkrc"));
    let mut user = xdg.into_iter().chain(home.map(|h| h.join(".latexmkrc")));
    files.extend(user.find(|p| p.is_file()));
    files
}

/// Where latexmk looks for its system rc file.
fn system_rc_candidates(env: Env) -> Vec<PathBuf> {
    match env("LATEXMKRCSYS").filter(|v| !v.is_empty()) {
        Some(p) => vec![PathBuf::from(p)],
        None if cfg!(windows) => vec![PathBuf::from("C:/latexmk/LatexMk")],
        None => [
            "/opt/local/share/latexmk/LatexMk",
            "/usr/local/share/latexmk/LatexMk",
            "/usr/local/lib/latexmk/LatexMk",
        ]
        .map(PathBuf::from)
        .to_vec(),
    }
}

/// `dirs`, then `path` (the process's `PATH`), without empty or relative
/// entries: those name the current directory (the project) on Unix. A dir
/// that cannot be in a `PATH` (its name has the separator) is left out
/// rather than failing the whole.
fn prepend_path(dirs: &[PathBuf], path: Option<OsString>) -> OsString {
    let current = path.unwrap_or_default();
    let all: Vec<PathBuf> = dirs
        .iter()
        .cloned()
        .chain(std::env::split_paths(&current))
        .filter(|d| d.is_absolute() && std::env::join_paths([d]).is_ok())
        .collect();
    if all.is_empty() && cfg!(unix) {
        // An empty `PATH` is one empty entry, which `execvp` reads as the
        // current directory.
        return OsString::from("/usr/bin:/bin");
    }
    std::env::join_paths(all).unwrap_or_default()
}

/// Discover every usable engine, best first: system TeX (more complete and
/// faster for users who have it) followed by Tectonic.
pub fn detect(bundled_dir: Option<&Path>) -> Vec<Engine> {
    let exe = std::env::current_exe().ok();
    detect_in(bundled_dir, &process_env, exe.as_deref())
}

/// [`detect`] with the environment `env` for a program at `exe`.
fn detect_in(bundled_dir: Option<&Path>, env: Env, exe: Option<&Path>) -> Vec<Engine> {
    let mut engines = Vec::new();

    if let Some((latexmk, bin_dir)) = find_tool("latexmk", env("PATH"), &tex_bin_dirs(env)) {
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
    let mut tectonic_dirs = exe.map(bundled_tectonic_dirs).unwrap_or_default();
    tectonic_dirs.extend(bundled_dir.map(Path::to_path_buf));
    if let Some((binary, _)) = find_tool("tectonic", env("PATH"), &tectonic_dirs) {
        engines.push(Engine {
            kind: EngineKind::Tectonic { binary },
            extra_path: Vec::new(),
        });
    }

    engines
}

/// A TeX distribution's program (`texdoc`, `kpsewhich`…), looked for like
/// latexmk, with the `PATH` to run it with (its folder first).
pub fn find_tex_tool(name: &str) -> Option<(PathBuf, OsString)> {
    find_tex_tool_in(name, &process_env)
}

fn find_tex_tool_in(name: &str, env: Env) -> Option<(PathBuf, OsString)> {
    let (tool, dir) = find_tool(name, env("PATH"), &tex_bin_dirs(env))?;
    let extra: Vec<PathBuf> = dir.into_iter().collect();
    Some((tool, prepend_path(&extra, env("PATH"))))
}

/// The local documentation of `package` (a PDF or HTML file), from `texdoc`.
pub fn texdoc(package: &str) -> Option<PathBuf> {
    if !is_package_name(package) {
        return None;
    }
    let (texdoc, path) = find_tex_tool("texdoc")?;
    let mut cmd = Command::new(texdoc);
    cmd.args(["-l", "-M", "-I", package]).env("PATH", path);
    parse_texdoc(&capture(cmd, Duration::from_secs(15))?)
}

/// Run `cmd` without a console and return its standard output, unless it
/// fails to start or runs longer than `timeout`.
fn capture(mut cmd: Command, timeout: Duration) -> Option<String> {
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd.spawn().ok()?;
    // Read while it runs: once the pipe is full, the child would wait for
    // the reader until the timeout.
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut out = Vec::new();
        std::io::Read::read_to_end(&mut stdout, &mut out).ok()?;
        Some(String::from_utf8_lossy(&out).into_owned())
    });
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    reader.join().ok()?
}

/// A name, never an option.
fn is_package_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// The first result of `texdoc -M`: `name \t score \t path \t lang \t description`.
fn parse_texdoc(out: &str) -> Option<PathBuf> {
    out.lines()
        .filter_map(|l| l.split('\t').nth(2))
        .map(PathBuf::from)
        .find(|p| p.is_absolute())
}

/// Look for `name` on `path` (`PATH`) first, then in `fallback` dirs.
/// Returns the binary and, if found in a fallback dir, that dir (to add to
/// `PATH`).
fn find_tool(
    name: &str,
    path: Option<OsString>,
    fallback: &[PathBuf],
) -> Option<(PathBuf, Option<PathBuf>)> {
    let cwd = std::env::current_dir().unwrap_or_default();
    if let Ok(p) = which::which_in(name, path, cwd) {
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
fn tex_bin_dirs(env: Env) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let home = env("HOME")
        .or_else(|| env("USERPROFILE"))
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
        if let Some(local) = env("LOCALAPPDATA").map(PathBuf::from) {
            dirs.push(local.join("Programs/MiKTeX/miktex/bin/x64"));
        }
        for pf in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(p) = env(pf).map(PathBuf::from) {
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

/// Where a packaged app (whose executable is `exe`) ships its own
/// `tectonic` binary.
fn bundled_tectonic_dirs(exe: &Path) -> Vec<PathBuf> {
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
    use std::ffi::OsStr;

    #[test]
    fn parses_texdoc_output() {
        // Absolute on every platform.
        let doc = std::env::temp_dir().join("amsldoc.pdf");
        let out = format!(
            "amsmath\t10\t{}\t\tUser's guide\namsmath\t1\trelative/amsmath.tex\t\t\n",
            doc.display()
        );
        assert_eq!(parse_texdoc(&out), Some(doc));
        assert_eq!(parse_texdoc("amsmath\t1\trelative.pdf\t\t\n"), None);
        assert_eq!(parse_texdoc(""), None);
    }

    #[test]
    fn texdoc_refuses_options() {
        assert_eq!(texdoc("-h"), None);
        assert_eq!(texdoc("a b"), None);
    }

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
    fn shell_escape_only_when_asked() {
        let tectonic = Engine {
            kind: EngineKind::Tectonic {
                binary: "tectonic".into(),
            },
            extra_path: vec![],
        };
        let mut job = Job::for_root("/p/main.tex");
        for engine in [latexmk(Flavor::LuaLatex), tectonic.clone()] {
            let args = args_of(&engine.command(&job));
            assert!(!args.iter().any(|a| a.contains("shell-escape")), "{args:?}");
        }
        job.shell_escape = true;
        assert!(
            args_of(&latexmk(Flavor::LuaLatex).command(&job)).contains(&"-shell-escape".into())
        );
        let args = args_of(&tectonic.command(&job));
        assert!(
            args.windows(2).any(|w| w == ["-Z", "shell-escape"]),
            "{args:?}"
        );
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
        let joined = prepend_path(&dirs, std::env::var_os("PATH"));
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

    #[test]
    fn bare_file_name_runs_in_the_current_folder() {
        let job = Job::for_root("main.tex");
        assert_eq!(job.work_dir(), Path::new("."));
        let cmd = latexmk(Flavor::PdfLatex).command(&job);
        assert_eq!(cmd.get_current_dir(), Some(Path::new(".")));
    }

    #[test]
    fn path_survives_a_dir_that_cannot_be_joined() {
        let bad = if cfg!(windows) { "C:\\a\"b" } else { "/a:b" };
        let good = std::env::temp_dir();
        let joined = prepend_path(&[PathBuf::from(bad), good.clone()], None);
        let dirs: Vec<PathBuf> = std::env::split_paths(&joined).collect();
        assert_eq!(dirs, vec![good], "{joined:?}");
        // Also from `PATH` itself (on Unix, `:` splits it first).
        let path = std::env::join_paths([std::env::temp_dir()]).unwrap();
        assert_eq!(prepend_path(&[], Some(path.clone())), path);
        // Never empty on Unix: an empty `PATH` is one empty entry, which
        // `execvp` (latexmk's `system`) reads as the current directory.
        for empty in [None, Some(OsString::new()), Some(".".into())] {
            let path = prepend_path(&[], empty);
            assert_eq!(path.is_empty(), cfg!(windows), "{path:?}");
            if !path.is_empty() {
                assert!(std::env::split_paths(&path).all(|d| d.is_absolute()));
            }
        }
    }

    /// Not a test: what the `capture` tests run in a child process (this
    /// test binary again), told what to do by `TEXORIA_TEST_CHILD`.
    #[test]
    #[ignore = "run by other tests"]
    fn child_helper() {
        match std::env::var("TEXORIA_TEST_CHILD").as_deref() {
            Ok("print") => {
                // Well over a pipe's buffer.
                let line = "x".repeat(99);
                for _ in 0..4000 {
                    println!("{line}");
                }
                println!(
                    "pkg\t1\t{}\t\t",
                    std::env::temp_dir().join("doc.pdf").display()
                );
            }
            Ok("sleep") => std::thread::sleep(Duration::from_secs(30)),
            _ => {}
        }
    }

    fn child(mode: &str) -> Command {
        let mut cmd = Command::new(std::env::current_exe().unwrap());
        cmd.args([
            "--exact",
            "engine::tests::child_helper",
            "--ignored",
            "--nocapture",
        ])
        .env("TEXORIA_TEST_CHILD", mode);
        cmd
    }

    #[test]
    fn capture_reads_long_output() {
        let start = Instant::now();
        let out = capture(child("print"), Duration::from_secs(10)).expect("output");
        assert!(start.elapsed() < Duration::from_secs(8));
        assert!(out.len() > 400_000);
        assert_eq!(
            parse_texdoc(&out),
            Some(std::env::temp_dir().join("doc.pdf"))
        );
    }

    #[test]
    fn capture_stops_at_the_timeout() {
        let start = Instant::now();
        assert_eq!(capture(child("sleep"), Duration::from_millis(300)), None);
        assert!(start.elapsed() < Duration::from_secs(10));
        let missing = Command::new(std::env::temp_dir().join("no-such-program"));
        assert_eq!(capture(missing, Duration::from_secs(1)), None);
    }

    /// An environment with only `vars`.
    fn env_of(vars: &[(&str, &Path)]) -> impl Fn(&str) -> Option<OsString> + use<> {
        let vars: Vec<(String, OsString)> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.as_os_str().to_owned()))
            .collect();
        move |key| vars.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
    }

    /// An empty file that `which` takes for a program.
    fn program(dir: &Path, name: &str) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&path, "").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }

    #[test]
    fn labels() {
        let labels: Vec<_> = [Flavor::PdfLatex, Flavor::XeLatex, Flavor::LuaLatex]
            .map(|f| latexmk(f).label())
            .into();
        assert_eq!(
            labels,
            [
                "pdfLaTeX (latexmk)",
                "XeLaTeX (latexmk)",
                "LuaLaTeX (latexmk)"
            ]
        );
        let engine = |kind| Engine {
            kind,
            extra_path: vec![],
        };
        assert_eq!(engine(tectonic_kind()).label(), "Tectonic");
        let custom = |program: &str| {
            engine(EngineKind::Custom {
                program: program.into(),
                args: vec![],
            })
            .label()
        };
        assert_eq!(custom("/opt/bin/mytex"), "mytex");
        assert_eq!(custom("/"), "custom");
    }

    fn tectonic_kind() -> EngineKind {
        EngineKind::Tectonic {
            binary: "tectonic".into(),
        }
    }

    #[test]
    fn tectonic_command_line() {
        let engine = Engine {
            kind: tectonic_kind(),
            extra_path: vec![],
        };
        let job = Job::for_root("/p/main.tex");
        let cmd = engine.command(&job);
        assert_eq!(cmd.get_program(), "tectonic");
        let mut expected: Vec<String> = [
            "-X",
            "compile",
            "--synctex",
            "--keep-logs",
            "--keep-intermediates",
            "--outdir",
        ]
        .map(String::from)
        .into();
        expected.push(job.out_dir.display().to_string());
        expected.push(Path::new(".").join("main.tex").display().to_string());
        assert_eq!(args_of(&cmd), expected);
    }

    #[test]
    fn custom_command_line() {
        let dir = std::env::temp_dir();
        let engine = Engine {
            kind: EngineKind::Custom {
                program: "/opt/bin/mytex".into(),
                args: vec!["--fast".into(), "-x".into()],
            },
            extra_path: vec![dir.clone()],
        };
        let job = Job::for_root("/p/main.tex");
        let cmd = engine.command(&job);
        assert_eq!(cmd.get_program(), "/opt/bin/mytex");
        let args = args_of(&cmd);
        assert_eq!(args[..2], ["--fast", "-x"]);
        assert_eq!(Path::new(&args[2]), Path::new(".").join("main.tex"));
        assert_eq!(cmd.get_current_dir(), Some(Path::new("/p")));
        let env = |key: &str| {
            cmd.get_envs()
                .find(|(k, _)| *k == key)
                .and_then(|(_, v)| v.map(OsStr::to_owned))
        };
        let path = env("PATH").expect("PATH is set");
        assert_eq!(std::env::split_paths(&path).next(), Some(dir));
        // cmd.exe must not look for programs in the project first.
        assert_eq!(
            env("NoDefaultCurrentDirectoryInExePath").is_some(),
            cfg!(windows)
        );
    }

    #[test]
    fn stems_and_artifacts() {
        let job = Job::for_root("/p/my.thesis.v2.tex");
        assert_eq!(job.stem(), "my.thesis.v2");
        assert_eq!(
            job.artifact("synctex.gz"),
            PathBuf::from("/p/.texoria/build/my.thesis.v2.synctex.gz")
        );
        let job = Job::for_root("/");
        assert_eq!(job.stem(), "");
        assert_eq!(job.work_dir(), Path::new("."));
    }

    #[test]
    fn rc_files_follow_latexmk() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let sys = d.join("LatexMk");
        let home = d.join("home");
        let xdg = d.join("xdg");
        let none: &[(&str, &Path)] = &[("LATEXMKRCSYS", &d.join("missing"))];
        assert_eq!(latexmk_rc_files(&env_of(none)), Vec::<PathBuf>::new());

        std::fs::write(&sys, "").unwrap();
        std::fs::create_dir_all(home.join(".config/latexmk")).unwrap();
        std::fs::create_dir_all(xdg.join("latexmk")).unwrap();
        let home_rc = home.join(".latexmkrc");
        std::fs::write(&home_rc, "").unwrap();
        let env = [("LATEXMKRCSYS", sys.as_path()), ("HOME", &home)];
        assert_eq!(
            latexmk_rc_files(&env_of(&env)),
            [sys.clone(), home_rc.clone()]
        );

        // `~/.config/latexmk/latexmkrc` comes before `~/.latexmkrc`...
        let config_rc = home.join(".config/latexmk/latexmkrc");
        std::fs::write(&config_rc, "").unwrap();
        assert_eq!(latexmk_rc_files(&env_of(&env)), [sys.clone(), config_rc]);
        // ...unless `XDG_CONFIG_HOME` points elsewhere.
        let xdg_rc = xdg.join("latexmk/latexmkrc");
        std::fs::write(&xdg_rc, "").unwrap();
        let env = [
            ("LATEXMKRCSYS", sys.as_path()),
            ("HOME", &home),
            ("XDG_CONFIG_HOME", &xdg),
        ];
        assert_eq!(latexmk_rc_files(&env_of(&env)), [sys, xdg_rc]);
        // Windows has `USERPROFILE`; empty values are unset.
        let env = [
            ("LATEXMKRCSYS", Path::new("")),
            ("HOME", Path::new("")),
            ("USERPROFILE", &home),
        ];
        let files = latexmk_rc_files(&env_of(&env));
        assert_eq!(files.last(), Some(&home.join(".config/latexmk/latexmkrc")));
    }

    #[test]
    fn system_rc_locations() {
        let mine = Path::new("/etc/mine");
        assert_eq!(
            system_rc_candidates(&env_of(&[("LATEXMKRCSYS", mine)])),
            [mine]
        );
        let defaults = system_rc_candidates(&env_of(&[]));
        if cfg!(windows) {
            assert_eq!(defaults, [PathBuf::from("C:/latexmk/LatexMk")]);
        } else {
            assert_eq!(defaults.len(), 3);
            assert!(defaults.iter().all(|p| p.ends_with("LatexMk")));
        }
    }

    #[test]
    fn finds_tools_on_path_then_in_fallbacks() {
        let dir = tempfile::tempdir().unwrap();
        let on_path = dir.path().join("path");
        let fallback = dir.path().join("fallback");
        let a = program(&on_path, "tool");
        let b = program(&fallback, "tool");
        let path = std::env::join_paths([&on_path]).unwrap();
        let dirs = [dir.path().join("nothing"), fallback.clone()];
        assert_eq!(
            find_tool("tool", Some(path.clone()), &dirs),
            Some((a, None))
        );
        assert_eq!(find_tool("tool", None, &dirs), Some((b, Some(fallback))));
        assert_eq!(find_tool("other", Some(path), &dirs), None);
    }

    #[test]
    fn finds_tex_tools_with_their_path() {
        let dir = tempfile::tempdir().unwrap();
        let tool = program(dir.path(), "texdoc");
        let path = std::env::join_paths([dir.path()]).unwrap();
        let (found, run_path) =
            find_tex_tool_in("texdoc", &env_of(&[("PATH", Path::new(&path))])).unwrap();
        assert_eq!(found, tool);
        assert_eq!(run_path, path);
    }

    #[test]
    fn detects_engines_best_first() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        let latexmk_bin = program(&bin, "latexmk");
        let tectonic_bin = program(&bin, "tectonic");
        let path = std::env::join_paths([&bin]).unwrap();
        let env = env_of(&[("PATH", Path::new(&path)), ("HOME", dir.path())]);
        let engines = detect_in(None, &env, None);
        let kinds: Vec<_> = engines.iter().map(|e| e.kind.clone()).collect();
        let flavors = [Flavor::PdfLatex, Flavor::XeLatex, Flavor::LuaLatex];
        let mut expected: Vec<_> = flavors
            .map(|flavor| EngineKind::Latexmk {
                latexmk: latexmk_bin.clone(),
                flavor,
            })
            .into();
        expected.push(EngineKind::Tectonic {
            binary: tectonic_bin,
        });
        assert_eq!(kinds, expected);
        // Found on `PATH`: nothing to add to it.
        assert!(engines.iter().all(|e| e.extra_path.is_empty()));
    }

    #[test]
    fn bundled_tectonic_wins_over_the_callers() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("app");
        let exe = app.join("texoria");
        let extra = dir.path().join("extra");
        let theirs = program(&extra, "tectonic");
        // `PATH` names a folder without programs.
        let env = env_of(&[("PATH", dir.path())]);
        let tectonic = |engines: Vec<Engine>| {
            engines.into_iter().find_map(|e| match e.kind {
                EngineKind::Tectonic { binary } => Some(binary),
                _ => None,
            })
        };
        assert_eq!(
            tectonic(detect_in(Some(&extra), &env, Some(&exe))),
            Some(theirs)
        );
        let ours = program(&app.join("bin"), "tectonic");
        assert_eq!(
            tectonic(detect_in(Some(&extra), &env, Some(&exe))),
            Some(ours)
        );
        // `detect` itself looks at the real system: it only must not fail.
        let _ = detect(None);
    }

    #[test]
    fn bundled_tectonic_locations() {
        let exe = Path::new("/app/bin/texoria");
        let dirs = bundled_tectonic_dirs(exe);
        assert_eq!(
            dirs[..2],
            [PathBuf::from("/app/bin"), PathBuf::from("/app/bin/bin")]
        );
        assert_eq!(
            dirs.contains(&PathBuf::from("/app/bin/../Resources/bin")),
            cfg!(target_os = "macos")
        );
        assert_eq!(
            dirs.contains(&PathBuf::from("/app/bin/../lib/texoria/bin")),
            cfg!(target_os = "linux")
        );
        let root = if cfg!(windows) { "C:\\" } else { "/" };
        assert_eq!(
            bundled_tectonic_dirs(Path::new(root)),
            Vec::<PathBuf>::new()
        );
    }

    #[test]
    fn texlive_years_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        for d in [
            "2023/bin/x86_64-linux",
            "2025/bin/universal-darwin",
            "2024basic/bin/a",
            "2022",
            "other/bin/x",
        ] {
            std::fs::create_dir_all(dir.path().join(d)).unwrap();
        }
        std::fs::write(dir.path().join("2025/bin/file"), "").unwrap();
        let rel: Vec<PathBuf> = texlive_year_dirs(dir.path())
            .iter()
            .map(|p| p.strip_prefix(dir.path()).unwrap().to_path_buf())
            .collect();
        assert_eq!(
            rel,
            [
                Path::new("2025/bin/universal-darwin"),
                Path::new("2024basic/bin/a"),
                Path::new("2023/bin/x86_64-linux"),
            ]
        );
        assert_eq!(
            texlive_year_dirs(&dir.path().join("none")),
            Vec::<PathBuf>::new()
        );
        assert_eq!(
            glob_one_level(&dir.path().join("none")),
            Vec::<PathBuf>::new()
        );
    }

    #[test]
    fn tex_bin_dirs_include_user_installs() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let unix_tinytex = home.join(".TinyTeX/bin/x86_64-linux");
        let mac_tinytex = home.join("Library/TinyTeX/bin/universal-darwin");
        let win_tinytex = home.join("AppData/Roaming/TinyTeX/bin/windows");
        for d in [&unix_tinytex, &mac_tinytex, &win_tinytex] {
            std::fs::create_dir_all(d).unwrap();
        }
        let local = Path::new("C:/Users/u/AppData/Local");
        let pf = Path::new("C:/Program Files");
        for key in ["HOME", "USERPROFILE"] {
            let env = env_of(&[(key, home), ("LOCALAPPDATA", local), ("ProgramFiles", pf)]);
            let dirs = tex_bin_dirs(&env);
            assert_eq!(dirs.contains(&unix_tinytex), cfg!(unix), "{key}");
            assert_eq!(dirs.contains(&mac_tinytex), cfg!(unix), "{key}");
            assert_eq!(dirs.contains(&win_tinytex), cfg!(windows), "{key}");
            assert_eq!(
                dirs.contains(&local.join("Programs/MiKTeX/miktex/bin/x64")),
                cfg!(windows)
            );
            assert_eq!(
                dirs.contains(&pf.join("MiKTeX/miktex/bin/x64")),
                cfg!(windows)
            );
            assert_eq!(
                dirs.contains(&PathBuf::from("/Library/TeX/texbin")),
                cfg!(target_os = "macos")
            );
        }
        assert!(tex_bin_dirs(&env_of(&[])).iter().all(|d| d.is_absolute()));
    }

    #[test]
    fn package_names() {
        for ok in ["amsmath", "l3kernel", "tikz-cd", "a_b.c"] {
            assert!(is_package_name(ok), "{ok}");
        }
        for bad in ["", "-h", "--help", "a b", "à", "a/b", "a\\b", "a;b"] {
            assert!(!is_package_name(bad), "{bad}");
            assert_eq!(texdoc(bad), None);
        }
    }

    mod props {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            #[test]
            fn texdoc_output_gives_only_absolute_paths(out in "\\PC*") {
                if let Some(p) = parse_texdoc(&out) {
                    prop_assert!(p.is_absolute());
                }
            }

            #[test]
            fn texdoc_lines_give_their_path(name in "[a-z]{1,8}", rest in "[^\t\r\n]{0,20}") {
                let doc = std::env::temp_dir().join(format!("{name}.pdf"));
                let out = format!("{name}\t1\trelative/{name}\t\t\n{name}\t2\t{}\t{rest}\n", doc.display());
                prop_assert_eq!(parse_texdoc(&out), Some(doc));
            }

            #[test]
            fn path_has_no_relative_entry(
                dirs in prop::collection::vec("[a-z:;./\\\\\"]{0,12}", 0..6),
                current in prop::collection::vec("[a-z./]{0,12}", 0..6),
            ) {
                let dirs: Vec<PathBuf> = dirs.into_iter().map(PathBuf::from).collect();
                let current = std::env::join_paths(current).ok();
                let joined = prepend_path(&dirs, current);
                // Empty only on Windows, where it is no entry at all.
                prop_assert!(!joined.is_empty() || cfg!(windows));
                if !joined.is_empty() {
                    prop_assert!(std::env::split_paths(&joined).all(|d| d.is_absolute()));
                }
            }

            #[test]
            fn package_names_are_never_options(name in "\\PC{0,12}") {
                if is_package_name(&name) {
                    prop_assert!(!name.starts_with('-'));
                    prop_assert!(name.is_ascii());
                }
            }

            #[test]
            fn artifacts_stay_in_the_build_folder(
                // No `:`: on Windows `C:` is a drive.
                name in "[^/\\\\\0:]{1,20}",
                ext in prop_oneof![Just("pdf"), Just("log"), Just("synctex.gz"), Just("synctex")],
            ) {
                let root = std::env::temp_dir().join("p").join(&name);
                let job = Job::for_root(root);
                let artifact = job.artifact(ext);
                prop_assert_eq!(artifact.parent(), Some(job.out_dir.as_path()));
            }
        }
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
