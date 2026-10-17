//! Talk to an installed agent from the command line, to check the protocol:
//!
//! ```sh
//! cargo run -p tex-agent --example probe -- [agent id] [prompt]
//! ```
//!
//! Without arguments it lists the installed agents. The agent gets a dummy
//! `typeset` tool over MCP (this program is its bridge too). Permissions are
//! granted once, files are read from disk and writes are refused.

use std::sync::Arc;

use futures_executor::block_on;
use serde_json::{Value, json};
use tex_agent::mcp::{self, Tool, ToolResult, Tools};
use tex_agent::{Agent, Event, RpcError, acp, catalog};

struct Dummy;

impl Tools for Dummy {
    fn list(&self) -> Vec<Tool> {
        vec![Tool {
            name: "typeset",
            description: "Typeset the document; returns its errors and warnings.",
            input_schema: json!({ "type": "object", "properties": {} }),
        }]
    }

    fn call(&self, name: &str, _: Value) -> ToolResult {
        println!("tool called: {name}");
        ToolResult::ok("Typeset in 1.2 s. No errors or warnings.")
    }
}

fn main() {
    if std::env::args().nth(1).as_deref() == Some(mcp::BRIDGE_FLAG) {
        mcp::bridge().expect("bridge");
        return;
    }
    let mut args = std::env::args().skip(1);
    let agents = catalog::detect(&[]);
    let Some(id) = args.next() else {
        for a in &agents {
            println!(
                "{:10} {} {} {:?}",
                a.id,
                a.name,
                a.program.display(),
                a.args
            );
        }
        return;
    };
    let prompt = args
        .next()
        .unwrap_or_else(|| "Reply with one short sentence.".into());
    let spec = agents.iter().find(|a| a.id == id).expect("not installed");
    let cwd = std::env::current_dir().unwrap();
    let agent = Agent::start(spec, &cwd, |event| match event {
        Event::Update(n) => println!("update: {:?}", n.update),
        Event::Permission(req, reply) => {
            println!("permission: {:?}", req.tool_call.fields.title);
            let once = req
                .options
                .iter()
                .find(|o| o.kind == acp::PermissionOptionKind::AllowOnce);
            let outcome = match once {
                Some(o) => acp::RequestPermissionOutcome::Selected(
                    acp::SelectedPermissionOutcome::new(o.option_id.clone()),
                ),
                None => acp::RequestPermissionOutcome::Cancelled,
            };
            reply.ok(acp::RequestPermissionResponse::new(outcome));
        }
        Event::ReadFile(req, reply) => match std::fs::read_to_string(&req.path) {
            Ok(text) => reply.ok(acp::ReadTextFileResponse::new(text)),
            Err(e) => reply.err(RpcError::new(RpcError::NOT_FOUND, e.to_string())),
        },
        Event::WriteFile(req, reply) => {
            println!("write refused: {}", req.path.display());
            reply.err(RpcError::internal("read-only probe"));
        }
        Event::Exited(stderr) => println!("exited: {stderr}"),
    })
    .expect("start");
    let init = block_on(agent.initialize()).expect("initialize");
    println!(
        "agent: {:?}\nauth: {:?}",
        init.agent_info, init.auth_methods
    );
    let server = mcp::Server::start(Arc::new(Dummy)).expect("MCP server");
    let exe = std::env::current_exe().unwrap();
    let session = match block_on(agent.new_session(&cwd, vec![server.stdio_config(&exe)])) {
        Ok(s) => s,
        Err(e) => {
            println!("session/new failed ({}): {e}", e.code);
            return;
        }
    };
    println!("modes: {:?}", session.modes);
    let content = vec![acp::ContentBlock::Text(acp::TextContent::new(prompt))];
    match block_on(agent.prompt(session.session_id, content)) {
        Ok(r) => println!("stop: {:?}", r.stop_reason),
        Err(e) => println!("prompt failed: {e}\n{}", agent.stderr_tail()),
    }
}
