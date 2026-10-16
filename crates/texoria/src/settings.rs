//! User settings, persisted as TOML in the OS configuration directory.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Label of the preferred engine (see `tex_compile::Engine::label`).
    pub engine: Option<String>,
    /// Recompile automatically after edits.
    pub live_preview: bool,
    /// Pause after the last keystroke before a live compile.
    pub live_delay_ms: u64,
    pub soft_wrap: bool,
    /// Stop a compile after this many seconds (0: never).
    pub compile_timeout_secs: u64,
    pub recent: Vec<PathBuf>,
    /// Project directories (and their subdirectories) whose Git
    /// configuration may run.
    pub trusted_projects: Vec<PathBuf>,
    /// Project directories whose `latexmkrc` may run. `None` in settings
    /// written before it was split from `trusted_projects`.
    #[serde(default)]
    pub rc_allowed: Option<Vec<PathBuf>>,
    /// Show diffs side by side instead of in one column.
    pub diff_split: bool,
    /// `git pull --rebase` instead of merging.
    pub pull_rebase: bool,
    /// Stop a fetch, pull or push after this many seconds (0: never).
    pub git_timeout_secs: u64,
    /// Overleaf Server Pro instances (`https://latex.example.org`).
    pub overleaf_servers: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            engine: None,
            live_preview: true,
            live_delay_ms: 700,
            soft_wrap: true,
            compile_timeout_secs: 300,
            recent: Vec::new(),
            trusted_projects: Vec::new(),
            rc_allowed: Some(Vec::new()),
            diff_split: false,
            pull_rebase: false,
            git_timeout_secs: 120,
            overleaf_servers: Vec::new(),
        }
    }
}

fn path() -> Option<PathBuf> {
    directories::ProjectDirs::from("app", "TeXoria", "TeXoria")
        .map(|d| d.config_dir().join("settings.toml"))
}

impl Settings {
    pub fn load() -> Self {
        path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|s| Self::parse(&s))
            .unwrap_or_default()
    }

    fn parse(text: &str) -> Self {
        let mut settings: Settings = toml::from_str(text).unwrap_or_default();
        if settings.rc_allowed.is_none() {
            // Trusting a project used to allow both.
            settings.rc_allowed = Some(settings.trusted_projects.clone());
        }
        settings
    }

    pub fn save(&self) {
        let Some(p) = path() else { return };
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match toml::to_string_pretty(self) {
            Ok(s) => {
                if let Err(e) = crate::atomic::write(&p, s.as_bytes()) {
                    log::warn!("saving settings: {e}");
                }
            }
            Err(e) => log::warn!("serializing settings: {e}"),
        }
    }

    pub fn compile_timeout(&self) -> Option<Duration> {
        (self.compile_timeout_secs > 0).then(|| Duration::from_secs(self.compile_timeout_secs))
    }

    pub fn git_timeout(&self) -> Option<Duration> {
        (self.git_timeout_secs > 0).then(|| Duration::from_secs(self.git_timeout_secs))
    }

    /// Whether Git may run in `dir`: trusting a repository (a clone) also
    /// covers the subdirectory its root file is in.
    pub fn is_trusted(&self, dir: &Path) -> bool {
        self.trusted_projects.iter().any(|p| dir.starts_with(p))
    }

    pub fn trust(&mut self, dir: PathBuf) {
        if !self.is_trusted(&dir) {
            self.trusted_projects.push(dir);
        }
    }

    /// Whether the `latexmkrc` in `dir` may run.
    pub fn allows_rc(&self, dir: &Path) -> bool {
        self.rc_allowed.iter().flatten().any(|p| p == dir)
    }

    pub fn allow_rc(&mut self, dir: PathBuf) {
        if !self.allows_rc(&dir) {
            self.rc_allowed.get_or_insert_default().push(dir);
        }
    }

    pub fn push_recent(&mut self, file: PathBuf) {
        self.recent.retain(|p| *p != file);
        self.recent.insert(0, file);
        self.recent.truncate(10);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_trust_covers_subdirectories() {
        let mut s = Settings::default();
        s.trust(PathBuf::from("/p/paper"));
        assert!(s.is_trusted(Path::new("/p/paper")));
        assert!(s.is_trusted(Path::new("/p/paper/chapters")));
        assert!(!s.is_trusted(Path::new("/p/paper2")));
        assert!(!s.is_trusted(Path::new("/p")));
        assert!(!s.allows_rc(Path::new("/p/paper")));
    }

    #[test]
    fn rc_permission_is_per_directory() {
        let mut s = Settings::default();
        s.allow_rc(PathBuf::from("/p/paper"));
        assert!(s.allows_rc(Path::new("/p/paper")));
        assert!(!s.allows_rc(Path::new("/p/paper/sub")));
        assert!(!s.is_trusted(Path::new("/p/paper")));
    }

    #[test]
    fn old_trust_still_allows_rc() {
        let s = Settings::parse("trusted_projects = [\"/p/paper\"]\n");
        assert!(s.is_trusted(Path::new("/p/paper")));
        assert!(s.allows_rc(Path::new("/p/paper")));

        let s = Settings::parse("trusted_projects = [\"/p/paper\"]\nrc_allowed = []\n");
        assert!(!s.allows_rc(Path::new("/p/paper")));
    }
}
