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
    let mut found: Vec<AgentSpec> = KNOWN.iter().filter_map(installed).collect();
    found.extend(custom.iter().filter_map(custom_spec));
    found
}

fn installed(known: &Known) -> Option<AgentSpec> {
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
            let program = process::find(program)?;
            Some(spec(program, args.iter().map(|a| a.to_string()).collect()))
        }
        Npx { cli, package } => {
            process::find(cli)?;
            let npx = process::find("npx")?;
            Some(spec(npx, vec!["-y".into(), package.to_string()]))
        }
    })
}

fn custom_spec(agent: &CustomAgent) -> Option<AgentSpec> {
    let command = agent.command.trim();
    if command.is_empty() {
        return None;
    }
    let as_path = PathBuf::from(command);
    let program = if as_path.is_absolute() {
        as_path.is_file().then_some(as_path)
    } else {
        process::find(command)
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

    #[test]
    fn ids_are_unique() {
        let mut ids: Vec<_> = KNOWN.iter().map(|k| k.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), KNOWN.len());
    }

    #[test]
    fn custom_agents_need_an_existing_program() {
        let missing = CustomAgent {
            name: "Mine".into(),
            command: "surely-not-an-installed-agent-xyz".into(),
            args: vec![],
        };
        assert!(custom_spec(&missing).is_none());
        assert!(custom_spec(&CustomAgent::default()).is_none());

        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("agent");
        std::fs::write(&exe, "").unwrap();
        let spec = custom_spec(&CustomAgent {
            name: String::new(),
            command: exe.to_string_lossy().into_owned(),
            args: vec!["--acp".into()],
        })
        .unwrap();
        assert_eq!(spec.program, exe);
        assert_eq!(spec.args, ["--acp"]);
        assert!(spec.id.starts_with("custom:"));
    }
}
