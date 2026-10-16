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
    /// Project directories whose `latexmkrc` may run.
    pub trusted_projects: Vec<PathBuf>,
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
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default()
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

    pub fn is_trusted(&self, dir: &Path) -> bool {
        self.trusted_projects.iter().any(|p| p == dir)
    }

    pub fn trust(&mut self, dir: PathBuf) {
        if !self.is_trusted(&dir) {
            self.trusted_projects.push(dir);
        }
    }

    pub fn push_recent(&mut self, file: PathBuf) {
        self.recent.retain(|p| *p != file);
        self.recent.insert(0, file);
        self.recent.truncate(10);
    }
}
