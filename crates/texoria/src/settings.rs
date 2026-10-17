//! User settings, persisted as TOML in the OS configuration directory.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tex_agent::CustomAgent;

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
    /// Format a file with tex-fmt when it is saved with ⌘S.
    pub format_on_save: bool,
    /// Size of the editor's text, in pixels.
    pub editor_font_size: f32,
    /// Stop a compile after this many seconds (0: never).
    pub compile_timeout_secs: u64,
    pub recent: Vec<PathBuf>,
    /// The tabs left open in each recent project, most recent first.
    pub sessions: Vec<Session>,
    /// Repositories (their working tree) whose Git configuration may run.
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
    /// Id of the coding agent to use (see `tex_agent::AgentSpec::id`).
    pub agent: Option<String>,
    /// Agents added by hand, for those TeXoria does not know.
    pub custom_agents: Vec<CustomAgent>,
    /// Project directories coding agents may work in.
    pub agent_projects: Vec<PathBuf>,
    /// Look for a new release at startup, at most once a day.
    pub check_updates: bool,
    /// When a check last found no new release (Unix seconds).
    pub last_update_check: Option<u64>,
    /// Why the file could not be read in full, to tell the user once.
    #[serde(skip)]
    pub load_error: Option<String>,
}

/// A project's open tabs, restored when it is opened again.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    /// The project's root file.
    pub root: PathBuf,
    pub files: Vec<PathBuf>,
    /// The file of the active tab.
    pub active: Option<PathBuf>,
    /// Where the PDF was last exported.
    pub export: Option<PathBuf>,
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
            format_on_save: false,
            editor_font_size: Settings::DEFAULT_FONT_SIZE,
            compile_timeout_secs: 300,
            recent: Vec::new(),
            sessions: Vec::new(),
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
            agent: None,
            custom_agents: Vec::new(),
            agent_projects: Vec::new(),
            check_updates: true,
            last_update_check: None,
            load_error: None,
        }
    }
}

fn path() -> Option<PathBuf> {
    directories::ProjectDirs::from("app", "TeXoria", "TeXoria")
        .map(|d| d.config_dir().join("settings.toml"))
}

impl Settings {
    pub fn load() -> Self {
        let Some(p) = path() else {
            return Self::default();
        };
        let Ok(text) = std::fs::read_to_string(&p) else {
            return Self::default();
        };
        let mut settings = Self::parse(&text);
        if let Some(e) = &settings.load_error {
            // The next save drops what could not be read: keep the file.
            let backup = p.with_extension("toml.bak");
            let kept = match std::fs::copy(&p, &backup) {
                Ok(_) => format!("The file as it was is kept as {}.", backup.display()),
                Err(err) => {
                    log::warn!("backing up {}: {err}", p.display());
                    String::new()
                }
            };
            log::warn!("reading {}: {e}", p.display());
            settings.load_error = Some(
                format!("Some settings could not be read and were reset. {kept}\n{e}")
                    .trim()
                    .to_string(),
            );
        }
        settings
    }

    /// The settings in `text`. A value that cannot be read (a typo, or a
    /// newer version's) only resets its own setting, never the others: the
    /// trusted folders and the dictionary would be lost on the next save.
    fn parse(text: &str) -> Self {
        let (mut settings, error) = match toml::from_str::<Settings>(text) {
            Ok(s) => (s, None),
            Err(e) => (Self::salvage(text), Some(e.message().to_string())),
        };
        if error.is_some() {
            // Never extend trust from what was salvaged.
            settings.rc_allowed.get_or_insert_default();
        } else if settings.rc_allowed.is_none() {
            // Trusting a project used to allow both.
            settings.rc_allowed = Some(settings.trusted_projects.clone());
        }
        settings.load_error = error;
        settings
    }

    /// The settings of `text` that can be read, each on its own.
    fn salvage(text: &str) -> Self {
        let Ok(table) = text.parse::<toml::Table>() else {
            return Self::default();
        };
        let readable = |key: &String, value: &toml::Value| {
            let one = toml::Table::from_iter([(key.clone(), value.clone())]);
            toml::Value::Table(one).try_into::<Settings>().is_ok()
        };
        let kept: toml::Table = table.into_iter().filter(|(k, v)| readable(k, v)).collect();
        toml::Value::Table(kept).try_into().unwrap_or_default()
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

    pub const DEFAULT_FONT_SIZE: f32 = 14.5;
    pub const MIN_FONT_SIZE: f32 = 8.;
    pub const MAX_FONT_SIZE: f32 = 40.;

    /// The editor's text size, within the allowed range whatever the file says.
    pub fn font_size(&self) -> f32 {
        let size = self.editor_font_size;
        if size.is_finite() {
            size.clamp(Self::MIN_FONT_SIZE, Self::MAX_FONT_SIZE)
        } else {
            Self::DEFAULT_FONT_SIZE
        }
    }

    pub fn compile_timeout(&self) -> Option<Duration> {
        (self.compile_timeout_secs > 0).then(|| Duration::from_secs(self.compile_timeout_secs))
    }

    pub fn git_timeout(&self) -> Option<Duration> {
        (self.git_timeout_secs > 0).then(|| Duration::from_secs(self.git_timeout_secs))
    }

    /// Whether Git may run in the repository whose working tree is `dir`.
    /// Only `dir` itself: trusting a folder must not extend to every
    /// repository below it (a home directory).
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

    /// Whether coding agents may work in `dir` (exact path, like trust).
    pub fn allows_agents(&self, dir: &Path) -> bool {
        self.agent_projects.iter().any(|p| p == dir)
    }

    pub fn allow_agents(&mut self, dir: PathBuf) {
        if !self.allows_agents(&dir) {
            self.agent_projects.push(dir);
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

    pub fn session(&self, root: &Path) -> Option<&Session> {
        self.sessions.iter().find(|s| s.root == root)
    }

    /// Record a project's session; returns whether it changed.
    pub fn set_session(&mut self, session: Session) -> bool {
        if self.sessions.first() == Some(&session) {
            return false;
        }
        self.sessions.retain(|s| s.root != session.root);
        self.sessions.insert(0, session);
        self.sessions.truncate(Self::MAX_SESSIONS);
        true
    }

    const MAX_SESSIONS: usize = 20;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_are_per_project_and_capped() {
        let mut s = Settings::default();
        let session = |root: &str| Session {
            root: PathBuf::from(root),
            files: vec![PathBuf::from(root)],
            active: None,
            export: None,
        };
        assert!(s.set_session(session("/a/main.tex")));
        assert!(!s.set_session(session("/a/main.tex")));
        for i in 0..30 {
            s.set_session(session(&format!("/p{i}/main.tex")));
        }
        assert_eq!(s.sessions.len(), Settings::MAX_SESSIONS);
        assert!(s.session(Path::new("/a/main.tex")).is_none());
        let text = toml::to_string(&s).unwrap();
        let back = Settings::parse(&text);
        assert_eq!(back.sessions, s.sessions);
    }

    #[test]
    fn font_size_stays_in_range() {
        assert_eq!(Settings::parse("").font_size(), Settings::DEFAULT_FONT_SIZE);
        assert_eq!(Settings::parse("editor_font_size = 18").font_size(), 18.);
        assert_eq!(
            Settings::parse("editor_font_size = 2.5").font_size(),
            Settings::MIN_FONT_SIZE
        );
        assert_eq!(
            Settings::parse("editor_font_size = 300").font_size(),
            Settings::MAX_FONT_SIZE
        );
        assert_eq!(
            Settings::parse("editor_font_size = nan").font_size(),
            Settings::DEFAULT_FONT_SIZE
        );
    }

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
    fn agent_permission_is_per_directory() {
        let mut s = Settings::default();
        s.allow_agents(PathBuf::from("/p/paper"));
        s.allow_agents(PathBuf::from("/p/paper"));
        assert_eq!(s.agent_projects.len(), 1);
        assert!(s.allows_agents(Path::new("/p/paper")));
        assert!(!s.allows_agents(Path::new("/p/paper/sub")));
        assert!(!s.is_trusted(Path::new("/p/paper")));
        let back = Settings::parse(&toml::to_string_pretty(&s).unwrap());
        assert!(back.allows_agents(Path::new("/p/paper")));
    }

    #[test]
    fn a_bad_value_resets_only_its_setting() {
        let text = "theme = \"sepia\"\nlive_delay_ms = 300\n\
                    trusted_projects = [\"/p/paper\"]\nspell_words = [\"TeXoria\"]\n";
        let s = Settings::parse(text);
        assert!(s.load_error.is_some());
        assert_eq!(s.theme, ThemeChoice::System);
        assert_eq!(s.live_delay_ms, 300);
        assert!(s.is_trusted(Path::new("/p/paper")));
        assert_eq!(s.spell_words, ["TeXoria"]);
        // Salvaged trust does not extend to latexmkrc.
        assert!(!s.allows_rc(Path::new("/p/paper")));

        let s = Settings::parse("not toml at all [");
        assert!(s.load_error.is_some());
        assert!(
            Settings::parse("live_preview = false\n")
                .load_error
                .is_none()
        );
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
