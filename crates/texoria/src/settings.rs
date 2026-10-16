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
    /// Project directories whose Git configuration may run.
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
    pub theme: ThemeChoice,
    /// Underline misspelled words.
    pub spell_check: bool,
    /// Words never reported as misspelled.
    pub spell_words: Vec<String>,
    /// Languages chosen by hand, per file.
    pub spell_languages: Vec<SpellLanguage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpellLanguage {
    pub file: PathBuf,
    /// A `tex_spell::catalog` code, or [`SpellLanguage::OFF`].
    pub language: String,
}

impl SpellLanguage {
    /// Not checked at all.
    pub const OFF: &str = "off";
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeChoice {
    /// Follow the system appearance.
    #[default]
    System,
    Light,
    Dark,
}

impl ThemeChoice {
    pub const ALL: [ThemeChoice; 3] = [ThemeChoice::System, ThemeChoice::Light, ThemeChoice::Dark];

    pub fn key(self) -> &'static str {
        match self {
            ThemeChoice::System => "system",
            ThemeChoice::Light => "light",
            ThemeChoice::Dark => "dark",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ThemeChoice::System => "System",
            ThemeChoice::Light => "Paper (light)",
            ThemeChoice::Dark => "Ink (dark)",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.key() == key)
    }
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
            theme: ThemeChoice::System,
            spell_check: true,
            spell_words: Vec::new(),
            spell_languages: Vec::new(),
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

    /// Whether Git may run in `dir`. Only `dir` itself: trusting a folder
    /// must not extend to every repository below it (a home directory).
    pub fn is_trusted(&self, dir: &Path) -> bool {
        self.trusted_projects.iter().any(|p| p == dir)
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

    /// The language chosen for `file`, if any (exact path, like trust).
    pub fn spell_language(&self, file: &Path) -> Option<&str> {
        self.spell_languages
            .iter()
            .find(|l| l.file == file)
            .map(|l| l.language.as_str())
    }

    /// Choose the language of `file`; `None` goes back to detecting it.
    pub fn set_spell_language(&mut self, file: &Path, language: Option<String>) {
        self.spell_languages.retain(|l| l.file != file);
        if let Some(language) = language {
            self.spell_languages.push(SpellLanguage {
                file: file.to_path_buf(),
                language,
            });
        }
    }

    /// Follow a rename of `from` (a file or a directory) to `to`.
    pub fn move_spell_languages(&mut self, from: &Path, to: &Path) {
        for l in &mut self.spell_languages {
            if let Ok(rest) = l.file.strip_prefix(from) {
                l.file = if rest.as_os_str().is_empty() {
                    to.to_path_buf()
                } else {
                    to.join(rest)
                };
            }
        }
    }

    pub fn add_spell_word(&mut self, word: &str) {
        if !self.spell_words.iter().any(|w| w == word) {
            self.spell_words.push(word.to_string());
            self.spell_words.sort();
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
    fn git_trust_is_per_directory() {
        let mut s = Settings::default();
        s.trust(PathBuf::from("/p/paper"));
        assert!(s.is_trusted(Path::new("/p/paper")));
        assert!(!s.is_trusted(Path::new("/p/paper/chapters")));
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

    #[test]
    fn spell_languages_are_per_file_and_round_trip() {
        let mut s = Settings::default();
        assert!(s.spell_check);
        s.set_spell_language(Path::new("/p/main.tex"), Some("it".into()));
        s.set_spell_language(Path::new("/p/ch/a.tex"), Some(SpellLanguage::OFF.into()));
        s.set_spell_language(Path::new("/p/main.tex"), Some("de".into()));
        assert_eq!(s.spell_language(Path::new("/p/main.tex")), Some("de"));
        assert_eq!(s.spell_language(Path::new("/p/other.tex")), None);
        s.move_spell_languages(Path::new("/p/ch"), Path::new("/p/chapters"));
        assert_eq!(
            s.spell_language(Path::new("/p/chapters/a.tex")),
            Some("off")
        );
        s.add_spell_word("TeXoria");
        s.add_spell_word("TeXoria");
        assert_eq!(s.spell_words, vec!["TeXoria"]);

        let text = toml::to_string_pretty(&s).unwrap();
        let back = Settings::parse(&text);
        assert_eq!(back.spell_languages, s.spell_languages);
        assert_eq!(back.spell_words, s.spell_words);
        assert_eq!(back.theme, s.theme);

        s.set_spell_language(Path::new("/p/main.tex"), None);
        assert_eq!(s.spell_language(Path::new("/p/main.tex")), None);
    }

    #[test]
    fn theme_defaults_to_system_and_round_trips() {
        assert_eq!(Settings::parse("").theme, ThemeChoice::System);
        let s = Settings {
            theme: ThemeChoice::Dark,
            ..Settings::default()
        };
        let text = toml::to_string_pretty(&s).unwrap();
        assert!(text.contains("theme = \"dark\""));
        assert_eq!(Settings::parse(&text).theme, ThemeChoice::Dark);
        for c in ThemeChoice::ALL {
            assert_eq!(ThemeChoice::from_key(c.key()), Some(c));
        }
    }
}
