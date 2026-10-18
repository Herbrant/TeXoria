//! TeXoria's own tools for the agent (the build, the problems, the
//! outline, the PDF's pages…), served over MCP.
//!
//! Agents start MCP servers as programs speaking on stdin/stdout, so the app
//! listens on a loopback port and the agent runs `texoria --mcp-bridge`, which
//! relays its stdio to that port ([`bridge`]). The bridge must present a token
//! first: any local program could connect to the port.

use std::collections::HashMap;
use std::io::{self, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_client_protocol_schema::v1 as acp;
use serde_json::{Value, json};

/// The command-line flag that turns `texoria` into the bridge.
pub const BRIDGE_FLAG: &str = "--mcp-bridge";
const ADDR_VAR: &str = "TEXORIA_MCP_ADDR";
const TOKEN_VAR: &str = "TEXORIA_MCP_TOKEN";
/// The server's name: agents show tools as `texoria/build` or similar.
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolResult {
    pub text: String,
    /// Pictures sent after the text.
    pub images: Vec<ToolImage>,
    pub is_error: bool,
}

/// A picture in a tool's answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolImage {
    /// The file, base64-encoded.
    pub data: String,
    /// `image/png`…
    pub mime_type: String,
}

impl ToolResult {
    pub fn ok(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            images: Vec::new(),
            is_error: false,
        }
    }

    pub fn error(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            images: Vec::new(),
            is_error: true,
        }
    }

    pub fn with_image(mut self, image: ToolImage) -> Self {
        self.images.push(image);
        self
    }

    /// The MCP `content` of the answer.
    fn content(&self) -> Value {
        let mut content = vec![json!({ "type": "text", "text": self.text })];
        content.extend(
            self.images
                .iter()
                .map(|i| json!({ "type": "image", "data": i.data, "mimeType": i.mime_type })),
        );
        Value::Array(content)
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
    shared: Arc<Shared>,
}

/// What the server and its threads share.
#[derive(Default)]
struct Shared {
    stop: AtomicBool,
    /// The open connections (by number), closed when the server stops.
    open: Mutex<HashMap<u64, TcpStream>>,
}

impl Server {
    pub fn start(tools: Arc<dyn Tools>) -> io::Result<Server> {
        Self::start_with(tools, TOKEN_TIMEOUT)
    }

    fn start_with(tools: Arc<dyn Tools>, token_timeout: Duration) -> io::Result<Server> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let addr = listener.local_addr()?;
        let token = new_token()?;
        let shared = Arc::new(Shared::default());
        {
            let (token, shared) = (token.clone(), shared.clone());
            std::thread::Builder::new()
                .name("mcp-server".into())
                .spawn(move || {
                    let mut next = 0u64;
                    accept_loop(listener.incoming(), &shared.stop, |stream| {
                        next += 1;
                        let id = next;
                        {
                            let mut open = shared.open.lock().unwrap();
                            // Checked again here: `Drop` sets it with the lock held.
                            if shared.stop.load(Ordering::SeqCst) || open.len() >= MAX_CONNECTIONS {
                                return;
                            }
                            let Ok(handle) = stream.try_clone() else {
                                return;
                            };
                            open.insert(id, handle);
                        }
                        let (tools, token, shared) = (tools.clone(), token.clone(), shared.clone());
                        std::thread::spawn(move || {
                            if let Err(e) = serve(stream, &token, &*tools, token_timeout) {
                                log::debug!("MCP connection: {e}");
                            }
                            shared.open.lock().unwrap().remove(&id);
                        });
                    });
                })?;
        }
        Ok(Server {
            addr,
            token,
            shared,
        })
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
        {
            let mut open = self.shared.open.lock().unwrap();
            self.shared.stop.store(true, Ordering::SeqCst);
            // The tools are no longer the agent's: hang up on it.
            for (_, stream) in open.drain() {
                let _ = stream.shutdown(std::net::Shutdown::Both);
            }
        }
        // Wake the accept loop up so it sees `stop`.
        let _ = TcpStream::connect(self.addr);
    }
}

/// After a failed accept (out of file descriptors…), wait this long: the
/// error would come right back.
const ACCEPT_PAUSE: Duration = Duration::from_millis(100);

/// Hand the connections of `incoming` to `serve` until `stop`.
fn accept_loop(
    incoming: impl Iterator<Item = io::Result<TcpStream>>,
    stop: &AtomicBool,
    mut serve: impl FnMut(TcpStream),
) {
    for stream in incoming {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        match stream {
            Ok(stream) => serve(stream),
            Err(e) => {
                log::debug!("MCP server: {e}");
                std::thread::sleep(ACCEPT_PAUSE);
            }
        }
    }
}

/// 128 random bits from the operating system.
fn new_token() -> io::Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| io::Error::other(e.to_string()))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn serve(
    stream: TcpStream,
    token: &str,
    tools: &dyn Tools,
    token_timeout: Duration,
) -> io::Result<()> {
    stream.set_read_timeout(Some(token_timeout))?;
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
                Build with the `build` tool, not by running latexmk or another \
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
                    "content": r.content(),
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
    relay(
        std::env::var(ADDR_VAR).ok(),
        std::env::var(TOKEN_VAR).ok(),
        io::stdin(),
        io::stdout(),
    )
}

/// Relay `input` to the server at `addr` and its answers to `output`.
fn relay(
    addr: Option<String>,
    token: Option<String>,
    mut input: impl Read + Send + 'static,
    mut output: impl Write,
) -> io::Result<()> {
    let unset = |k: &str| io::Error::new(io::ErrorKind::NotFound, format!("{k} unset"));
    let addr: SocketAddr = addr
        .ok_or_else(|| unset(ADDR_VAR))?
        .parse()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    if !addr.ip().is_loopback() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a local address",
        ));
    }
    let token = token.ok_or_else(|| unset(TOKEN_VAR))?;
    let mut upstream = TcpStream::connect(addr)?;
    upstream.write_all(format!("{token}\n").as_bytes())?;
    let mut down = upstream.try_clone()?;
    let to_app = std::thread::spawn(move || {
        let _ = io::copy(&mut input, &mut upstream);
        let _ = upstream.shutdown(std::net::Shutdown::Write);
    });
    let mut buf = [0u8; 16 * 1024];
    loop {
        let n = down.read(&mut buf)?;
        if n == 0 {
            break;
        }
        output.write_all(&buf[..n])?;
        output.flush()?;
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
                "picture" => ToolResult::ok("a dot").with_image(ToolImage {
                    data: "iVBORw0KGgo=".into(),
                    mime_type: "image/png".into(),
                }),
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
                json!({"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"picture"}}),
            ],
        );
        assert_eq!(replies.len(), 6);
        let picture = &replies[5]["result"]["content"];
        assert_eq!(picture[0]["text"], "a dot");
        assert_eq!(picture[1]["type"], "image");
        assert_eq!(picture[1]["mimeType"], "image/png");
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

    /// A connection that has shown the token, and reads its answers.
    fn connect(server: &Server) -> (TcpStream, BufReader<TcpStream>) {
        let stream = TcpStream::connect(server.addr).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut w = stream.try_clone().unwrap();
        writeln!(w, "{}", server.token).unwrap();
        (w, BufReader::new(stream))
    }

    fn ask(w: &mut TcpStream, r: &mut BufReader<TcpStream>, msg: Value) -> Value {
        writeln!(w, "{msg}").unwrap();
        let mut line = String::new();
        r.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }

    /// Whether the server hangs up on `stream` (rather than leaving it open).
    fn hung_up(stream: &mut impl Read) -> bool {
        let mut buf = [0u8; 64];
        matches!(stream.read(&mut buf), Ok(0) | Err(_)) && {
            // A reset or the end: either way, not a timeout.
            !matches!(stream.read(&mut buf), Err(e) if e.kind() == io::ErrorKind::WouldBlock
                || e.kind() == io::ErrorKind::TimedOut)
        }
    }

    #[test]
    fn answers_pings_and_reports_bad_calls() {
        let server = Server::start(Arc::new(Echo)).unwrap();
        let replies = exchange(
            &server,
            &server.token,
            &[
                json!({"jsonrpc":"2.0","id":"p","method":"ping"}),
                json!({"jsonrpc":"2.0","id":null,"method":"ping"}),
                json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"arguments":{}}}),
                json!({"jsonrpc":"2.0","id":3,"method":"tools/call"}),
                json!({"jsonrpc":"2.0","id":4,"method":"initialize"}),
                json!({"jsonrpc":"2.0","id":5}),
            ],
        );
        assert_eq!(replies.len(), 5, "{replies:?}");
        assert_eq!(replies[0], json!({"jsonrpc":"2.0","id":"p","result":{}}));
        assert_eq!(replies[1]["error"]["code"], -32602);
        assert_eq!(replies[2]["error"]["message"], "Missing tool name");
        assert_eq!(replies[3]["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(replies[3]["result"]["serverInfo"]["name"], SERVER_NAME);
        assert_eq!(replies[4]["error"]["code"], -32601);
    }

    #[test]
    fn skips_blank_and_broken_lines() {
        let server = Server::start(Arc::new(Echo)).unwrap();
        let (mut w, mut r) = connect(&server);
        w.write_all(b"\n  \nnot json\n").unwrap();
        let reply = ask(
            &mut w,
            &mut r,
            json!({"jsonrpc":"2.0","id":1,"method":"ping"}),
        );
        assert_eq!(reply["id"], 1);
    }

    #[test]
    fn tokens_are_compared_whole() {
        assert!(same(b"abc", b"abc"));
        assert!(!same(b"abc", b"abd"));
        assert!(!same(b"abc", b"abcd"));
        assert!(!same(b"", b"a"));
        assert!(same(b"", b""));
    }

    #[test]
    fn refuses_empty_and_oversized_tokens() {
        let server = Server::start(Arc::new(Echo)).unwrap();
        let ping = [json!({"jsonrpc":"2.0","id":1,"method":"ping"})];
        assert!(exchange(&server, "", &ping).is_empty());
        assert!(
            exchange(
                &server,
                &format!("{}{}", server.token, " ".repeat(300)),
                &ping
            )
            .is_empty()
        );
        // The right token with a line break of its own is fine.
        assert_eq!(
            exchange(&server, &format!("{}\r", server.token), &ping).len(),
            1
        );
    }

    #[test]
    fn the_token_must_come_in_time() {
        let server = Server::start_with(Arc::new(Echo), Duration::from_millis(200)).unwrap();
        let mut stream = TcpStream::connect(server.addr).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        std::thread::sleep(Duration::from_millis(600));
        let _ = writeln!(stream, "{}", server.token);
        let _ = writeln!(
            stream,
            "{}",
            json!({"jsonrpc":"2.0","id":1,"method":"ping"})
        );
        assert!(hung_up(&mut stream));
        // Once it is shown, there is no hurry.
        let (mut w, mut r) = connect(&server);
        std::thread::sleep(Duration::from_millis(400));
        assert_eq!(
            ask(
                &mut w,
                &mut r,
                json!({"jsonrpc":"2.0","id":2,"method":"ping"})
            )["id"],
            2
        );
    }

    #[test]
    fn caps_the_connections_waiting_for_a_token() {
        let server = Server::start_with(Arc::new(Echo), Duration::from_secs(3)).unwrap();
        let idle: Vec<TcpStream> = (0..MAX_CONNECTIONS)
            .map(|_| TcpStream::connect(server.addr).unwrap())
            .collect();
        // Wait until all are being served.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while server.shared.open.lock().unwrap().len() < MAX_CONNECTIONS {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        let mut extra = TcpStream::connect(server.addr).unwrap();
        extra
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let _ = writeln!(extra, "{}", server.token);
        assert!(hung_up(&mut extra), "one too many");
        drop(idle);
        // Room again once they are gone.
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !server.shared.open.lock().unwrap().is_empty() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        let (mut w, mut r) = connect(&server);
        assert_eq!(
            ask(
                &mut w,
                &mut r,
                json!({"jsonrpc":"2.0","id":1,"method":"ping"})
            )["id"],
            1
        );
    }

    #[test]
    fn stopping_the_server_hangs_up_on_its_clients() {
        let server = Server::start(Arc::new(Echo)).unwrap();
        let addr = server.addr;
        let (mut w, mut r) = connect(&server);
        assert_eq!(
            ask(
                &mut w,
                &mut r,
                json!({"jsonrpc":"2.0","id":1,"method":"ping"})
            )["id"],
            1
        );
        drop(server);
        let _ = writeln!(w, "{}", json!({"jsonrpc":"2.0","id":2,"method":"ping"}));
        assert!(hung_up(r.get_mut()));
        // Nobody listens any more.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while TcpStream::connect(addr).is_ok() {
            assert!(std::time::Instant::now() < deadline, "still accepting");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn waits_after_a_failed_accept() {
        let stop = AtomicBool::new(false);
        let errors = (0..3).map(|_| Err(io::Error::other("Too many open files")));
        let start = std::time::Instant::now();
        accept_loop(errors, &stop, |_| unreachable!());
        assert!(start.elapsed() >= ACCEPT_PAUSE * 3, "{:?}", start.elapsed());
        // A stopped server takes nothing more.
        stop.store(true, Ordering::SeqCst);
        let start = std::time::Instant::now();
        accept_loop(
            std::iter::repeat_with(|| Err(io::Error::other("x"))),
            &stop,
            |_| {},
        );
        assert!(start.elapsed() < ACCEPT_PAUSE);
    }

    #[test]
    fn the_bridge_relays_both_ways() {
        let server = Server::start(Arc::new(Echo)).unwrap();
        let input = [
            json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
            json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"echo","arguments":[1]}}),
        ]
        .map(|m| format!("{m}\n"))
        .concat();
        let mut output = Vec::new();
        relay(
            Some(server.addr.to_string()),
            Some(server.token.clone()),
            io::Cursor::new(input.into_bytes()),
            &mut output,
        )
        .unwrap();
        let replies: Vec<Value> = output
            .lines()
            .map(|l| serde_json::from_str(&l.unwrap()).unwrap())
            .collect();
        assert_eq!(replies.len(), 2);
        assert_eq!(replies[0]["result"]["tools"][0]["name"], "echo");
        assert_eq!(replies[1]["result"]["content"][0]["text"], "[1]");

        // A wrong token: the server hangs up, the bridge ends.
        let mut output = Vec::new();
        let input = io::Cursor::new(b"{}\n".to_vec());
        let _ = relay(
            Some(server.addr.to_string()),
            Some("x".into()),
            input,
            &mut output,
        );
        assert!(output.is_empty());
    }

    #[test]
    fn the_bridge_needs_a_local_address_and_a_token() {
        let none = || io::Cursor::new(Vec::new());
        let addr = Some("127.0.0.1:9".to_string());
        let e = relay(None, Some("t".into()), none(), Vec::new()).unwrap_err();
        assert_eq!(
            (e.kind(), e.to_string()),
            (io::ErrorKind::NotFound, format!("{ADDR_VAR} unset"))
        );
        let e = relay(addr, None, none(), Vec::new()).unwrap_err();
        assert_eq!(
            (e.kind(), e.to_string()),
            (io::ErrorKind::NotFound, format!("{TOKEN_VAR} unset"))
        );
        for bad in ["localhost:1", "", "127.0.0.1"] {
            let e = relay(Some(bad.into()), Some("t".into()), none(), Vec::new()).unwrap_err();
            assert_eq!(e.kind(), io::ErrorKind::InvalidInput, "{bad}");
        }
        // Never connects to another machine, even with the token.
        for remote in ["10.0.0.1:80", "[2001:db8::1]:80", "0.0.0.0:80"] {
            let e = relay(Some(remote.into()), Some("t".into()), none(), Vec::new()).unwrap_err();
            assert_eq!(e.to_string(), "not a local address", "{remote}");
        }
    }

    #[test]
    fn bridge_config_carries_the_address() {
        let server = Server::start(Arc::new(Echo)).unwrap();
        let config = serde_json::to_value(server.stdio_config(Path::new("/app/texoria"))).unwrap();
        assert_eq!(config["name"], SERVER_NAME);
        assert_eq!(config["command"], "/app/texoria");
        assert_eq!(config["args"][0], BRIDGE_FLAG);
        assert_eq!(config["env"][0]["value"], server.addr.to_string());
        assert_eq!(config["env"][1]["name"], TOKEN_VAR);
        assert_eq!(config["env"][1]["value"], server.token);
    }
}
