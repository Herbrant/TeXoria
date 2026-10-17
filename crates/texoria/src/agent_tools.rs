//! TeXoria's tools for coding agents, served over MCP by `tex-agent`: they
//! are called on the server's threads and answered by the workspace on the UI
//! thread (`Workspace::on_tool_call`).

use std::sync::mpsc;
use std::time::Duration;

use serde_json::{Value, json};
use tex_agent::mcp::{Tool, ToolResult, Tools};

/// Longer than any build (they have their own timeout).
const TIMEOUT: Duration = Duration::from_secs(30 * 60);

pub struct ToolCall {
    pub name: String,
    pub reply: mpsc::Sender<ToolResult>,
}

pub struct AppTools {
    tx: smol::channel::Sender<ToolCall>,
}

impl AppTools {
    pub fn new() -> (Self, smol::channel::Receiver<ToolCall>) {
        let (tx, rx) = smol::channel::unbounded();
        (AppTools { tx }, rx)
    }
}

impl Tools for AppTools {
    fn list(&self) -> Vec<Tool> {
        let none = json!({ "type": "object", "properties": {} });
        vec![
            Tool {
                name: "build",
                description: "Save the open files and build the document with TeXoria's \
                    engine (the user sees the PDF). Returns whether it succeeded, with its \
                    errors and warnings as `file:line: severity: message`.",
                input_schema: none.clone(),
            },
            Tool {
                name: "problems",
                description: "The errors and warnings of the last build, without building \
                    again.",
                input_schema: none.clone(),
            },
            Tool {
                name: "outline",
                description: "The document's structure: its parts, chapters, sections, \
                    figures and tables, with their file and line.",
                input_schema: none,
            },
        ]
    }

    fn call(&self, name: &str, _: Value) -> ToolResult {
        let (reply, answer) = mpsc::channel();
        let call = ToolCall {
            name: name.to_string(),
            reply,
        };
        if self.tx.send_blocking(call).is_err() {
            return ToolResult::error("TeXoria's window is closed.");
        }
        answer
            .recv_timeout(TIMEOUT)
            .unwrap_or_else(|_| ToolResult::error("TeXoria did not answer."))
    }
}
