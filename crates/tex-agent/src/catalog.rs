//! The coding agents TeXoria knows how to start, and which of them are
//! installed. Commands follow the ACP registry
//! (<https://github.com/agentclientprotocol/registry>).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::process;

/// An agent ready to start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSpec {
    /// A catalog id, or `custom:<name>`: what the settings remember.
    pub id: String,
    pub name: String,
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    /// How to sign in from a terminal, when the agent asks for it.
    pub login_hint: Option<String>,
}

/// An agent added by the user in the settings.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CustomAgent {
    pub name: String,
    /// A program name (looked up like the known agents) or a path.
    pub command: String,
    pub args: Vec<String>,
}

pub struct Known {
    pub id: &'static str,
    pub name: &'static str,
    launch: &'static [Launch],
    pub login: &'static str,
}

enum Launch {
    /// A program that speaks ACP itself, with the arguments that select it.
    Native {
        program: &'static str,
        args: &'static [&'static str],
    },
    /// An ACP adapter from npm, run with `npx` when the agent's own CLI is
    /// installed (it shares its sign-in). Pinned, like the registry.
    Npx {
        cli: &'static str,
        package: &'static str,
    },
}

use Launch::{Native, Npx};

pub const KNOWN: &[Known] = &[
    Known {
        id: "claude",
        name: "Claude Code",
        launch: &[
            Native {
                program: "claude-agent-acp",
                args: &[],
            },
            Npx {
                cli: "claude",
                package: "@agentclientprotocol/claude-agent-acp@0.93.0",
            },
        ],
        login: "Run `claude` in a terminal and sign in with /login.",
    },
    Known {
        id: "codex",
        name: "Codex",
        launch: &[
            Native {
                program: "codex-acp",
                args: &[],
            },
            Npx {
                cli: "codex",
                package: "@agentclientprotocol/codex-acp@2.2.2",
            },
        ],
        login: "Run `codex login` in a terminal.",
    },
    Known {
        id: "gemini",
        name: "Gemini CLI",
        launch: &[Native {
            program: "gemini",
            args: &["--acp"],
        }],
        login: "Run `gemini` in a terminal and sign in.",
    },
    Known {
        id: "copilot",
        name: "GitHub Copilot",
        launch: &[Native {
            program: "copilot",
            args: &["--acp"],
        }],
        login: "Run `copilot` in a terminal and sign in with /login.",
    },
    Known {
        id: "cursor",
        name: "Cursor",
        launch: &[Native {
            program: "cursor-agent",
            args: &["acp"],
        }],
        login: "Run `cursor-agent login` in a terminal.",
    },
    Known {
        id: "opencode",
        name: "OpenCode",
        launch: &[Native {
            program: "opencode",
            args: &["acp"],
        }],
        login: "Run `opencode auth login` in a terminal.",
    },
    Known {
        id: "goose",
        name: "goose",
        launch: &[Native {
            program: "goose",
            args: &["acp"],
        }],
        login: "Run `goose configure` in a terminal.",
    },
    Known {
        id: "qwen",
        name: "Qwen Code",
        launch: &[Native {
            program: "qwen",
            args: &["--acp"],
        }],
        login: "Run `qwen` in a terminal and sign in.",
    },
    Known {
        id: "kimi",
        name: "Kimi CLI",
        launch: &[Native {
            program: "kimi",
            args: &["acp"],
        }],
        login: "Run `kimi` in a terminal and sign in with /login.",
    },
    Known {
        id: "vibe",
        name: "Mistral Vibe",
        launch: &[Native {
            program: "vibe-acp",
            args: &[],
        }],
        login: "Run `vibe` in a terminal to set up your API key.",
    },
    Known {
        id: "auggie",
        name: "Auggie",
        launch: &[Native {
            program: "auggie",
            args: &["--acp"],
        }],
        login: "Run `auggie login` in a terminal.",
    },
];

/// The installed agents, known ones first. Blocking: it may ask the login
/// shell for its `PATH`.
pub fn detect(custom: &[CustomAgent]) -> Vec<AgentSpec> {
    detect_with(custom, &process::find)
}

/// [`detect`], with programs looked up by `find`.
fn detect_with(custom: &[CustomAgent], find: &dyn Fn(&str) -> Option<PathBuf>) -> Vec<AgentSpec> {
    let mut found: Vec<AgentSpec> = KNOWN.iter().filter_map(|k| installed(k, find)).collect();
    found.extend(custom.iter().filter_map(|c| custom_spec(c, find)));
    found
}

fn installed(known: &Known, find: &dyn Fn(&str) -> Option<PathBuf>) -> Option<AgentSpec> {
    let spec = |program: PathBuf, args: Vec<String>| AgentSpec {
        id: known.id.to_string(),
        name: known.name.to_string(),
        program,
        args,
        env: Vec::new(),
        login_hint: Some(known.login.to_string()),
    };
    known.launch.iter().find_map(|launch| match launch {
        Native { program, args } => {
            let program = find(program)?;
            Some(spec(program, args.iter().map(|a| a.to_string()).collect()))
        }
        Npx { cli, package } => {
            find(cli)?;
            let npx = find("npx")?;
            Some(spec(npx, vec!["-y".into(), package.to_string()]))
        }
    })
}

fn custom_spec(agent: &CustomAgent, find: &dyn Fn(&str) -> Option<PathBuf>) -> Option<AgentSpec> {
    let command = agent.command.trim();
    if command.is_empty() {
        return None;
    }
    let as_path = PathBuf::from(command);
    let program = if as_path.is_absolute() {
        as_path.is_file().then_some(as_path)
    } else {
        find(command)
    }?;
    let name = match agent.name.trim() {
        "" => command.to_string(),
        n => n.to_string(),
    };
    Some(AgentSpec {
        id: format!("custom:{name}"),
        name,
        program,
        args: agent.args.clone(),
        env: Vec::new(),
        login_hint: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::tests::executable;
    use std::path::Path;

    /// Programs looked up in `dir` only.
    fn finder(dir: &Path) -> impl Fn(&str) -> Option<PathBuf> {
        let path = dir.as_os_str().to_owned();
        move |name| process::find_in(name, path.clone())
    }

    #[test]
    fn ids_are_unique() {
        let mut ids: Vec<_> = KNOWN.iter().map(|k| k.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), KNOWN.len());
    }

    #[test]
    fn finds_installed_agents_known_ones_first() {
        let dir = tempfile::tempdir().unwrap();
        let find = finder(dir.path());
        assert!(detect_with(&[], &find).is_empty());

        let gemini = executable(dir.path(), "gemini");
        let custom = CustomAgent {
            name: " Mine ".into(),
            command: " gemini ".into(),
            args: vec!["--x".into()],
        };
        let found = detect_with(std::slice::from_ref(&custom), &find);
        let ids: Vec<&str> = found.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["gemini", "custom:Mine"]);
        assert_eq!(found[0].program, gemini);
        assert_eq!(found[0].args, ["--acp"]);
        assert!(found[0].login_hint.as_deref().unwrap().contains("gemini"));
        assert_eq!(
            (found[1].name.as_str(), &found[1].args),
            ("Mine", &custom.args)
        );
        assert_eq!(found[1].login_hint, None);
    }

    #[test]
    fn adapters_need_both_the_cli_and_npx() {
        let claude = KNOWN.iter().find(|k| k.id == "claude").unwrap();
        let dir = tempfile::tempdir().unwrap();
        let find = finder(dir.path());
        executable(dir.path(), "claude");
        assert_eq!(installed(claude, &find), None, "no npx");
        let npx = executable(dir.path(), "npx");
        let spec = installed(claude, &find).unwrap();
        assert_eq!(spec.program, npx);
        assert_eq!(spec.args[0], "-y");
        assert!(spec.args[1].starts_with("@agentclientprotocol/claude-agent-acp@"));

        let dir = tempfile::tempdir().unwrap();
        executable(dir.path(), "npx");
        assert_eq!(installed(claude, &finder(dir.path())), None, "no cli");
        // The native adapter comes first.
        let native = executable(dir.path(), "claude-agent-acp");
        executable(dir.path(), "claude");
        let spec = installed(claude, &finder(dir.path())).unwrap();
        assert_eq!((spec.program, spec.args), (native, Vec::<String>::new()));
    }

    #[test]
    fn custom_agents_need_an_existing_program() {
        let empty = tempfile::tempdir().unwrap();
        let find = finder(empty.path());
        let missing = CustomAgent {
            name: "Mine".into(),
            command: "surely-not-an-installed-agent-xyz".into(),
            args: vec![],
        };
        assert!(custom_spec(&missing, &find).is_none());
        assert!(custom_spec(&CustomAgent::default(), &find).is_none());
        let blank = CustomAgent {
            command: "  ".into(),
            ..CustomAgent::default()
        };
        assert!(custom_spec(&blank, &find).is_none());

        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("agent");
        std::fs::write(&exe, "").unwrap();
        let spec = custom_spec(
            &CustomAgent {
                name: String::new(),
                command: exe.to_string_lossy().into_owned(),
                args: vec!["--acp".into()],
            },
            &find,
        )
        .unwrap();
        assert_eq!(spec.program, exe);
        assert_eq!(spec.args, ["--acp"]);
        assert_eq!(spec.id, format!("custom:{}", exe.display()));
        // An absolute path that is not a file.
        let folder = CustomAgent {
            command: dir.path().to_string_lossy().into_owned(),
            ..CustomAgent::default()
        };
        assert!(custom_spec(&folder, &find).is_none());
    }

    #[test]
    fn custom_agents_round_trip_through_settings() {
        let agent = CustomAgent {
            name: "Mine".into(),
            command: "my-agent".into(),
            args: vec!["acp".into()],
        };
        let json = serde_json::to_string(&agent).unwrap();
        assert_eq!(serde_json::from_str::<CustomAgent>(&json).unwrap(), agent);
        let partial: CustomAgent = serde_json::from_str(r#"{"command":"x"}"#).unwrap();
        assert_eq!((partial.name.as_str(), partial.args.len()), ("", 0));
    }
}
