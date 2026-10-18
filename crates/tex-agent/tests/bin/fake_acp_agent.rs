//! A fake ACP agent for tests: it speaks JSON-RPC over stdin/stdout (one
//! message per line, like the real adapters) and does what a script says.
//! It depends on nothing but std and serde_json, so any crate's tests can
//! run it (`env!("CARGO_BIN_EXE_fake-acp-agent")` in tex-agent's own).
//!
//! ```text
//! fake-acp-agent <script.json>      (or FAKE_ACP_SCRIPT=<script.json>)
//! fake-acp-agent --sleep            a child that sleeps for a minute
//! fake-acp-agent --late-stderr <text> <ms>
//!                                   a child that writes <text> on stderr after <ms>
//! ```
//!
//! The script is a JSON object:
//!
//! ```json
//! {
//!   "log": "/tmp/x/log.jsonl",
//!   "start": [ ...actions ],
//!   "on": {
//!     "session/prompt": [ ...actions ],
//!     "session/new": [ [ ...first call ], [ ...second and later calls ] ]
//!   }
//! }
//! ```
//!
//! - `log`: every line read from the client is appended there as it comes
//!   (requests, notifications, and its answers to the agent's requests).
//! - `start`: actions done before reading anything.
//! - `on`: actions done when the client sends `method` (a request or a
//!   notification). A list of lists gives the actions of each call in turn,
//!   the last one repeating.
//!
//! A request whose actions do not answer it gets a default answer:
//! `initialize` → `{"protocolVersion": 1}`, `session/new` →
//! `{"sessionId": "s1"}`, `session/prompt` → `{"stopReason": "end_turn"}`,
//! `authenticate`, `session/set_mode` → `{}`, anything else → error -32601.
//!
//! Actions (an object with one key):
//!
//! | action | does |
//! |---|---|
//! | `{"respond": value}` | answer the current request with `value` |
//! | `{"respond_error": {"code": -1, "message": "m"}}` | answer it with an error |
//! | `{"notify": {"method": "m", "params": p}}` | send a notification |
//! | `{"request": {"method": "m", "params": p}}` | send a request and wait for its answer (logged) |
//! | `{"raw": "text"}` | write `text` and a line break on stdout as is |
//! | `{"stderr": "text"}` | write `text` on stderr |
//! | `{"stderr_repeat": {"text": "t", "times": n}}` | write `t` n times on stderr |
//! | `{"sleep_ms": n}` | wait |
//! | `{"spawn": {"args": [...], "pid_file": "path"}}` | start this program again with `args` (stdin and stdout closed, stderr shared), its pid written to `pid_file` |
//! | `{"exit": code}` | exit at once |
//! | `{"abort": true}` | crash |

use std::collections::{HashMap, VecDeque};
use std::fs::OpenOptions;
use std::io::{BufRead, Write};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde_json::{Value, json};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--sleep") => {
            std::thread::sleep(Duration::from_secs(60));
            return;
        }
        Some("--late-stderr") => {
            let ms = args.get(2).and_then(|m| m.parse().ok()).unwrap_or(300);
            std::thread::sleep(Duration::from_millis(ms));
            eprint!("{}", args.get(1).map_or("", String::as_str));
            return;
        }
        _ => {}
    }
    let path = args
        .first()
        .cloned()
        .or_else(|| std::env::var("FAKE_ACP_SCRIPT").ok())
        .expect("usage: fake-acp-agent <script.json>");
    let script: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    Agent::new(script).run();
}

struct Agent {
    script: Value,
    /// Calls received so far, by method.
    calls: HashMap<String, usize>,
    next_id: i64,
    /// Messages read while waiting for an answer, handled after.
    queue: VecDeque<Value>,
    stdin: std::io::StdinLock<'static>,
}

impl Agent {
    fn new(script: Value) -> Self {
        Self {
            script,
            calls: HashMap::new(),
            next_id: 1000,
            queue: VecDeque::new(),
            stdin: std::io::stdin().lock(),
        }
    }

    fn run(mut self) {
        let start = self.script["start"].as_array().cloned().unwrap_or_default();
        self.act(&start, None);
        loop {
            let msg = match self.queue.pop_front() {
                Some(msg) => msg,
                None => match self.read() {
                    Some(msg) => msg,
                    None => return,
                },
            };
            let Some(method) = msg["method"].as_str().map(str::to_string) else {
                // An answer nobody waits for.
                continue;
            };
            let call = self.calls.entry(method.clone()).or_default();
            let n = *call;
            *call += 1;
            let actions = match &self.script["on"][&method] {
                Value::Array(list) if list.first().is_some_and(Value::is_array) => list
                    [n.min(list.len() - 1)]
                .as_array()
                .cloned()
                .unwrap_or_default(),
                Value::Array(list) => list.clone(),
                _ => Vec::new(),
            };
            let id = msg.get("id").filter(|id| !id.is_null()).cloned();
            let answered = self.act(&actions, id.as_ref());
            if let (Some(id), false) = (id, answered) {
                match default_result(&method) {
                    Some(result) => send(&json!({ "jsonrpc": "2.0", "id": id, "result": result })),
                    None => send(&json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": { "code": -32601, "message": format!("{method} unknown") },
                    })),
                }
            }
        }
    }

    /// One message from the client (logged), `None` at the end.
    fn read(&mut self) -> Option<Value> {
        loop {
            let mut line = String::new();
            if self.stdin.read_line(&mut line).ok()? == 0 {
                return None;
            }
            if let Some(log) = self.script["log"].as_str() {
                let mut f = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(log)
                    .unwrap();
                f.write_all(line.trim_end().as_bytes()).unwrap();
                f.write_all(b"\n").unwrap();
            }
            if let Ok(msg) = serde_json::from_str(line.trim()) {
                return Some(msg);
            }
        }
    }

    /// Do `actions` for the request `id`; whether they answered it.
    fn act(&mut self, actions: &[Value], id: Option<&Value>) -> bool {
        let mut answered = false;
        for action in actions {
            let Some((name, arg)) = action.as_object().and_then(|o| o.iter().next()) else {
                continue;
            };
            match name.as_str() {
                "respond" => {
                    answered = true;
                    send(&json!({ "jsonrpc": "2.0", "id": id, "result": arg }));
                }
                "respond_error" => {
                    answered = true;
                    send(&json!({ "jsonrpc": "2.0", "id": id, "error": arg }));
                }
                "notify" => send(&json!({
                    "jsonrpc": "2.0",
                    "method": arg["method"],
                    "params": arg["params"],
                })),
                "request" => {
                    self.next_id += 1;
                    let id = self.next_id;
                    send(&json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "method": arg["method"],
                        "params": arg["params"],
                    }));
                    // Wait for the answer; anything else is for later.
                    while let Some(msg) = self.read() {
                        if msg.get("method").is_none() && msg["id"] == json!(id) {
                            break;
                        }
                        self.queue.push_back(msg);
                    }
                }
                "raw" => {
                    let mut out = std::io::stdout().lock();
                    let _ = writeln!(out, "{}", arg.as_str().unwrap_or(""));
                    let _ = out.flush();
                }
                "stderr" => {
                    let _ = std::io::stderr().write_all(arg.as_str().unwrap_or("").as_bytes());
                }
                "stderr_repeat" => {
                    let text = arg["text"].as_str().unwrap_or("");
                    let mut err = std::io::stderr().lock();
                    for _ in 0..arg["times"].as_u64().unwrap_or(1) {
                        let _ = err.write_all(text.as_bytes());
                    }
                }
                "sleep_ms" => std::thread::sleep(Duration::from_millis(arg.as_u64().unwrap_or(0))),
                "spawn" => {
                    let args: Vec<&str> = arg["args"]
                        .as_array()
                        .map(|a| a.iter().filter_map(Value::as_str).collect())
                        .unwrap_or_default();
                    keep_pipes_to_ourselves();
                    let child = Command::new(std::env::current_exe().unwrap())
                        .args(args)
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .spawn()
                        .unwrap();
                    if let Some(file) = arg["pid_file"].as_str() {
                        std::fs::write(file, child.id().to_string()).unwrap();
                    }
                    // Not waited for: it outlives this process if nobody kills it.
                    std::mem::forget(child);
                }
                "exit" => {
                    let _ = std::io::stdout().flush();
                    std::process::exit(arg.as_i64().unwrap_or(0) as i32);
                }
                "abort" => std::process::abort(),
                other => panic!("unknown action {other}"),
            }
        }
        answered
    }
}

fn default_result(method: &str) -> Option<Value> {
    Some(match method {
        "initialize" => json!({ "protocolVersion": 1 }),
        "session/new" => json!({ "sessionId": "s1" }),
        "session/prompt" => json!({ "stopReason": "end_turn" }),
        "authenticate" | "session/set_mode" => json!({}),
        _ => return None,
    })
}

fn send(msg: &Value) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{msg}");
    let _ = out.flush();
}

/// On Windows a child inherits every inheritable handle of its parent, not only the ones given
/// as its stdio: a helper would keep this agent's stdin and stdout open too, and the client
/// would see the agent's stdout end only when the helper does. The helpers here must hold
/// stderr only, as the scripts say.
#[cfg(windows)]
fn keep_pipes_to_ourselves() {
    use std::ffi::c_void;
    unsafe extern "system" {
        fn GetStdHandle(which: u32) -> *mut c_void;
        fn SetHandleInformation(handle: *mut c_void, mask: u32, flags: u32) -> i32;
    }
    const STD_INPUT_HANDLE: u32 = -10i32 as u32;
    const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    const HANDLE_FLAG_INHERIT: u32 = 1;
    for which in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE] {
        // SAFETY: plain Win32 calls on this process's own standard handles.
        unsafe {
            SetHandleInformation(GetStdHandle(which), HANDLE_FLAG_INHERIT, 0);
        }
    }
}

#[cfg(not(windows))]
fn keep_pipes_to_ourselves() {}
