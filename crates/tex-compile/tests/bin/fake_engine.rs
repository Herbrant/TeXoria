//! A stand-in for a TeX engine in the scheduler's tests, the same on every
//! platform. It is run as an `EngineKind::Custom` program, so its last
//! argument is the root file (`./main.tex`); the others are steps, run in
//! order:
//!
//! - `out=DIR`: the build folder (default `.texoria/build`)
//! - `sleep=MS`
//! - `log=TEXT`: write `<stem>.log` (`\n` in TEXT is a new line)
//! - `big-log=N:TEXT`: a log of N bytes of filler, then TEXT
//! - `pdf`, `synctex`, `synctex-gz`: write that artifact
//! - `print=TEXT`: write TEXT on the standard output
//! - `cwd=FILE`: write the current directory to FILE
//! - `grandchild=FILE`: start `fake-engine heartbeat=FILE` and wait until it
//!   has written FILE (a process the engine itself started, like latexmk's
//!   pdflatex)
//! - `heartbeat=FILE`: append a byte to FILE every 10 ms, forever
//! - `exit=CODE`

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let stem = args
        .iter()
        .rfind(|a| a.ends_with(".tex"))
        .and_then(|a| Path::new(a).file_stem())
        .map_or("main".to_string(), |s| s.to_string_lossy().into_owned());
    let mut out = PathBuf::from(".texoria/build");
    let artifact = |out: &Path, ext: &str| {
        std::fs::create_dir_all(out).unwrap();
        out.join(format!("{stem}.{ext}"))
    };

    for arg in &args {
        let (step, value) = arg.split_once('=').unwrap_or((arg, ""));
        match step {
            "out" => out = PathBuf::from(value),
            "sleep" => std::thread::sleep(Duration::from_millis(value.parse().unwrap())),
            "log" => std::fs::write(artifact(&out, "log"), value.replace("\\n", "\n")).unwrap(),
            "big-log" => {
                let (size, text) = value.split_once(':').unwrap();
                let mut log = vec![b'x'; size.parse().unwrap()];
                log.extend_from_slice(format!("\n{}", text.replace("\\n", "\n")).as_bytes());
                std::fs::write(artifact(&out, "log"), log).unwrap();
            }
            "pdf" => std::fs::write(artifact(&out, "pdf"), "%PDF-1.5\n").unwrap(),
            "synctex" => std::fs::write(artifact(&out, "synctex"), "SyncTeX Version:1\n").unwrap(),
            "synctex-gz" => std::fs::write(artifact(&out, "synctex.gz"), [0x1f, 0x8b]).unwrap(),
            "print" => {
                println!("{value}");
                std::io::stdout().flush().unwrap();
            }
            "cwd" => {
                let cwd = std::env::current_dir().unwrap();
                std::fs::write(value, cwd.to_string_lossy().as_bytes()).unwrap();
            }
            "grandchild" => {
                #[allow(clippy::zombie_processes, reason = "it runs until the tree is killed")]
                std::process::Command::new(std::env::current_exe().unwrap())
                    .arg(format!("heartbeat={value}"))
                    .spawn()
                    .unwrap();
                while std::fs::metadata(value).map_or(true, |m| m.len() == 0) {
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
            "heartbeat" => loop {
                let mut f = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(value)
                    .unwrap();
                f.write_all(b".").unwrap();
                drop(f);
                std::thread::sleep(Duration::from_millis(10));
            },
            "exit" => std::process::exit(value.parse().unwrap()),
            _ => {}
        }
    }
}
