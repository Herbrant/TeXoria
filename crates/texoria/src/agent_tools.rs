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
    pub arguments: Value,
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
                input_schema: none.clone(),
            },
            Tool {
                name: "pdf_page",
                description: "A picture (PNG) of a page of the PDF the user sees, as last \
                    built: to check the layout (figures, tables, overfull lines, page \
                    breaks). Build first if the sources changed.",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "page": { "type": "integer", "minimum": 1, "description": "1-based" },
                        "scale": {
                            "type": "number",
                            "minimum": 0.5,
                            "maximum": 3,
                            "description": "Pixels per PDF point (default 1.5)"
                        }
                    },
                    "required": ["page"]
                }),
            },
            Tool {
                name: "pdf_text",
                description: "The text of the PDF as last built: one page, or every page \
                    (separated by form feeds) if no page is given.",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "page": { "type": "integer", "minimum": 1, "description": "1-based" }
                    }
                }),
            },
            Tool {
                name: "bib_search",
                description: "Entries of the project's bibliography whose key, title, \
                    authors or year contain every word of the query, with their key to \
                    \\cite. An empty query lists them all.",
                input_schema: json!({
                    "type": "object",
                    "properties": { "query": { "type": "string" } }
                }),
            },
            Tool {
                name: "word_count",
                description: "How many words the document has, in the text, headings and \
                    captions, with its formulas, in total and per file.",
                input_schema: none,
            },
        ]
    }

    fn call(&self, name: &str, arguments: Value) -> ToolResult {
        let (reply, answer) = mpsc::channel();
        let call = ToolCall {
            name: name.to_string(),
            arguments,
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
