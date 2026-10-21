//! git with the user's own configuration out of the way.

use std::path::{Path, PathBuf};

/// Environment variables that make git ignore the user's and the system's configuration: a
/// global config in `home` with an identity and `main` as the default branch.
pub fn isolated_env(home: &Path) -> Vec<(&'static str, PathBuf)> {
    let config = home.join("gitconfig");
    std::fs::write(
        &config,
        "[user]\n\tname = Test\n\temail = test@example.com\n[init]\n\tdefaultBranch = main\n",
    )
    .unwrap();
    vec![
        ("GIT_CONFIG_GLOBAL", config),
        ("GIT_CONFIG_NOSYSTEM", "1".into()),
        ("HOME", home.to_path_buf()),
    ]
}

/// A git program run directly, with [`isolated_env`], for the states a test sets up.
pub struct Isolated {
    program: PathBuf,
    env: Vec<(&'static str, PathBuf)>,
}

impl Isolated {
    /// `program`, with its configuration in `home`.
    pub fn new(program: impl Into<PathBuf>, home: &Path) -> Self {
        Self {
            program: program.into(),
            env: isolated_env(home),
        }
    }

    /// The environment it runs with.
    pub fn env(&self) -> &[(&'static str, PathBuf)] {
        &self.env
    }

    /// git `args` in `dir`, with English messages.
    pub fn raw(&self, dir: &Path, args: &[&str]) -> std::process::Output {
        std::process::Command::new(&self.program)
            .current_dir(dir)
            .args(args)
            .env("LC_ALL", "C")
            .envs(self.env.iter().map(|(k, v)| (k, v)))
            .output()
            .unwrap()
    }

    /// [`Isolated::raw`], which must succeed: its standard output, trimmed.
    #[track_caller]
    pub fn ok(&self, dir: &Path, args: &[&str]) -> String {
        let out = self.raw(dir, args);
        let text = |b: &[u8]| String::from_utf8_lossy(b).trim().to_string();
        assert!(out.status.success(), "git {args:?}: {}", text(&out.stderr));
        text(&out.stdout)
    }
}
