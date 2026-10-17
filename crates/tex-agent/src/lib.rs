//! Coding agents (Claude Code, Codex, Gemini CLI, …) installed on the user's
//! system, driven through the Agent Client Protocol
//! (<https://agentclientprotocol.com>).
//!
//! - [`catalog`]: which agents are installed and how to start them.
//! - [`client`]: a running agent: sessions, prompts, and its requests
//!   (permissions, reading and writing files).
//! - [`mcp`]: TeXoria's tools for the agent (build, problems, outline).
//! - [`checkpoint`]: the project before a turn, to review and undo it.
//! - [`prompt`]: what TeXoria asks the agent.

pub mod catalog;
pub mod checkpoint;
pub mod client;
pub mod mcp;
mod process;
pub mod prompt;
pub mod rpc;

/// The ACP wire types.
pub use agent_client_protocol_schema::v1 as acp;
pub use catalog::{AgentSpec, CustomAgent};
pub use checkpoint::{Change, ChangeKind, Checkpoint};
pub use client::{Agent, Event, Reply};
pub use process::{find as find_program, search_path};
pub use rpc::RpcError;
