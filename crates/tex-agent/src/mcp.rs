//! TeXoria's own tools for the agent (typesetting, the problems, the
//! outline), served over MCP.
//!
//! Agents start MCP servers as programs speaking on stdin/stdout, so the app
//! listens on a loopback port and the agent runs `texoria --mcp-bridge`, which
//! relays its stdio to that port ([`bridge`]). The bridge must present a token
//! first: any local program could connect to the port.

use std::io::{self, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use agent_client_protocol_schema::v1 as acp;
use serde_json::{Value, json};

/// The command-line flag that turns `texoria` into the bridge.
pub const BRIDGE_FLAG: &str = "--mcp-bridge";
const ADDR_VAR: &str = "TEXORIA_MCP_ADDR";
const TOKEN_VAR: &str = "TEXORIA_MCP_TOKEN";
/// The server's name: agents show tools as `texoria/typeset` or similar.
pub const SERVER_NAME: &str = "texoria";
const PROTOCOL_VERSION: &str = "2025-06-18";
/// Any local program can connect: before it shows the token, it gets little
/// time, little memory and few connections.
const MAX_CONNECTIONS: usize = 32;
const TOKEN_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_TOKEN_LINE: u64 = 256;

pub struct Tool {
    pub name: &'static str,
    pub description: &'static str,
    /// JSON Schema of the arguments.
    pub input_schema: Value,
}

pub struct ToolResult {
    pub text: String,
    pub is_error: bool,
}

impl ToolResult {
    pub fn ok(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: false,
        }
    }

    pub fn error(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: true,
        }
    }
}

/// What the tools do. Called on a connection's thread, so it may block (for
/// a compile to finish).
pub trait Tools: Send + Sync + 'static {
    fn list(&self) -> Vec<Tool>;
    fn call(&self, name: &str, arguments: Value) -> ToolResult;
}

/// Serves [`Tools`] until dropped.
pub struct Server {
    addr: SocketAddr,
    token: String,
    stop: Arc<AtomicBool>,
}

impl Server {
    pub fn start(tools: Arc<dyn Tools>) -> io::Result<Server> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let addr = listener.local_addr()?;
        let token = new_token()?;
        let open = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        {
            let (token, stop) = (token.clone(), stop.clone());
            std::thread::Builder::new()
                .name("mcp-server".into())
                .spawn(move || {
                    for stream in listener.incoming() {
                        if stop.load(Ordering::SeqCst) {
                            break;
                        }
                        let Ok(stream) = stream else { continue };
                        if open.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
                            open.fetch_sub(1, Ordering::SeqCst);
                            continue;
                        }
                        let (tools, token, open) = (tools.clone(), token.clone(), open.clone());
                        std::thread::spawn(move || {
                            if let Err(e) = serve(stream, &token, &*tools) {
                                log::debug!("MCP connection: {e}");
                            }
                            open.fetch_sub(1, Ordering::SeqCst);
                        });
                    }
                })?;
        }
        Ok(Server { addr, token, stop })
    }

    /// How an agent starts the bridge to this server (`exe` is TeXoria's).
    pub fn stdio_config(&self, exe: &Path) -> acp::McpServer {
        acp::McpServer::Stdio(
            acp::McpServerStdio::new(SERVER_NAME, exe)
                .args(vec![BRIDGE_FLAG.to_string()])
                .env(vec![
                    acp::EnvVariable::new(ADDR_VAR, self.addr.to_string()),
                    acp::EnvVariable::new(TOKEN_VAR, self.token.clone()),
                ]),
        )
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wake the accept loop up so it sees `stop`.
        let _ = TcpStream::connect(self.addr);
    }
}

/// 128 random bits from the operating system.
fn new_token() -> io::Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| io::Error::other(e.to_string()))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn serve(stream: TcpStream, token: &str, tools: &dyn Tools) -> io::Result<()> {
    stream.set_read_timeout(Some(TOKEN_TIMEOUT))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = stream;
    let first = crate::rpc::read_line(&mut (&mut reader).take(MAX_TOKEN_LINE), 256)?;
    if !first.is_some_and(|t| same(t.as_bytes(), token.as_bytes())) {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "bad token"));
    }
    writer.set_read_timeout(None)?;
    while let Some(line) = crate::rpc::read_line(&mut reader, 64 << 20)? {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if let Some(reply) = handle(&msg, tools) {
            let mut out = serde_json::to_vec(&reply)?;
            out.push(b'\n');
            writer.write_all(&out)?;
            writer.flush()?;
        }
    }
    Ok(())
}

/// Constant-time comparison.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// The answer to one MCP message (none for notifications).
fn handle(msg: &Value, tools: &dyn Tools) -> Option<Value> {
    let id = msg.get("id").filter(|id| !id.is_null())?.clone();
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or(PROTOCOL_VERSION),
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": { "name": SERVER_NAME, "version": env!("CARGO_PKG_VERSION") },
            "instructions": "Tools of TeXoria, the LaTeX editor the user is working in. \
                Typeset with the `typeset` tool, not by running latexmk or another \
                engine yourself.",
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({
            "tools": tools.list().into_iter().map(|t| json!({
                "name": t.name,
                "description": t.description,
                "inputSchema": t.input_schema,
            })).collect::<Vec<_>>(),
        })),
        "tools/call" => match params.get("name").and_then(Value::as_str) {
            Some(name) => {
                let args = params.get("arguments").cloned().unwrap_or(json!({}));
                let r = tools.call(name, args);
                Ok(json!({
                    "content": [{ "type": "text", "text": r.text }],
                    "isError": r.is_error,
                }))
            }
            None => Err((-32602, "Missing tool name".to_string())),
        },
        _ => Err((-32601, format!("Method not found: {method}"))),
    };
    Some(match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err((code, message)) => {
            json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
        }
    })
}

/// `texoria --mcp-bridge`: relay stdin/stdout to the app's server.
pub fn bridge() -> io::Result<()> {
    let var = |k: &str| {
        std::env::var(k).map_err(|_| io::Error::new(io::ErrorKind::NotFound, format!("{k} unset")))
    };
    let addr: SocketAddr = var(ADDR_VAR)?
        .parse()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    if !addr.ip().is_loopback() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a local address",
        ));
    }
    let token = var(TOKEN_VAR)?;
    let mut upstream = TcpStream::connect(addr)?;
    upstream.write_all(format!("{token}\n").as_bytes())?;
    let mut down = upstream.try_clone()?;
    let to_app = std::thread::spawn(move || {
        let _ = io::copy(&mut io::stdin().lock(), &mut upstream);
        let _ = upstream.shutdown(std::net::Shutdown::Write);
    });
    let mut stdout = io::stdout().lock();
    let mut buf = [0u8; 16 * 1024];
    loop {
        let n = down.read(&mut buf)?;
        if n == 0 {
            break;
        }
        stdout.write_all(&buf[..n])?;
        stdout.flush()?;
    }
    drop(to_app);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufRead;

    struct Echo;

    impl Tools for Echo {
        fn list(&self) -> Vec<Tool> {
            vec![Tool {
                name: "echo",
                description: "Says it back",
                input_schema: json!({ "type": "object" }),
            }]
        }

        fn call(&self, name: &str, arguments: Value) -> ToolResult {
            match name {
                "echo" => ToolResult::ok(arguments.to_string()),
                _ => ToolResult::error("no such tool"),
            }
        }
    }

    fn exchange(server: &Server, token: &str, messages: &[Value]) -> Vec<Value> {
        let stream = TcpStream::connect(server.addr).unwrap();
        let mut w = stream.try_clone().unwrap();
        // A refused connection is closed with these still unread: writing
        // and reading may then fail (a reset), which ends the exchange.
        let _ = w.write_all(format!("{token}\n").as_bytes());
        for m in messages {
            let _ = w.write_all(format!("{m}\n").as_bytes());
        }
        let _ = w.shutdown(std::net::Shutdown::Write);
        BufReader::new(stream)
            .lines()
            .map_while(Result::ok)
            .map(|l| serde_json::from_str(&l).unwrap())
            .collect()
    }

    #[test]
    fn lists_and_calls_tools() {
        let server = Server::start(Arc::new(Echo)).unwrap();
        let token = server.token.clone();
        let replies = exchange(
            &server,
            &token,
            &[
                json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}),
                json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
                json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
                json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"echo","arguments":{"a":1}}}),
                json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"nope"}}),
                json!({"jsonrpc":"2.0","id":5,"method":"resources/list"}),
            ],
        );
        assert_eq!(replies.len(), 5);
        assert_eq!(replies[0]["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(replies[1]["result"]["tools"][0]["name"], "echo");
        assert_eq!(replies[2]["result"]["content"][0]["text"], "{\"a\":1}");
        assert_eq!(replies[3]["result"]["isError"], true);
        assert_eq!(replies[4]["error"]["code"], -32601);
    }

    #[test]
    fn refuses_a_wrong_token() {
        let server = Server::start(Arc::new(Echo)).unwrap();
        let replies = exchange(
            &server,
            "guess",
            &[json!({"jsonrpc":"2.0","id":1,"method":"tools/list"})],
        );
        assert!(replies.is_empty());
        let token = new_token().unwrap();
        assert_eq!(token.len(), 32);
        assert_ne!(token, new_token().unwrap());
    }

    #[test]
    fn hangs_up_on_endless_garbage() {
        let server = Server::start(Arc::new(Echo)).unwrap();
        let mut stream = TcpStream::connect(server.addr).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        // No line break: the server stops reading after a few hundred bytes.
        let _ = stream.write_all(&[b'x'; 4096]);
        let mut buf = [0u8; 16];
        let n = stream.read(&mut buf).unwrap_or(0);
        assert_eq!(n, 0, "the connection is closed without an answer");
    }

    #[test]
    fn bridge_config_carries_the_address() {
        let server = Server::start(Arc::new(Echo)).unwrap();
        let config = serde_json::to_value(server.stdio_config(Path::new("/app/texoria"))).unwrap();
        assert_eq!(config["name"], SERVER_NAME);
        assert_eq!(config["command"], "/app/texoria");
        assert_eq!(config["args"][0], BRIDGE_FLAG);
        assert_eq!(config["env"][0]["value"], server.addr.to_string());
    }
}
