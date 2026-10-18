//! A running agent, spoken to with ACP over its stdin/stdout.

use std::future::Future;
use std::io::BufReader;
use std::marker::PhantomData;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use agent_client_protocol_schema::ProtocolVersion;
use agent_client_protocol_schema::v1 as acp;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::catalog::AgentSpec;
use crate::process;
use crate::rpc::{Connection, Incoming, Responder, RpcError};

/// How much of the agent's stderr is kept, to explain a failure.
const STDERR_TAIL: usize = 4000;
/// How long the end of stderr is waited for once the agent closed stdout (a
/// helper it started may keep stderr open).
const STDERR_WAIT: Duration = Duration::from_secs(2);

/// What the agent asks or tells, in the order it did.
pub enum Event {
    Update(acp::SessionNotification),
    Permission(
        acp::RequestPermissionRequest,
        Reply<acp::RequestPermissionResponse>,
    ),
    ReadFile(acp::ReadTextFileRequest, Reply<acp::ReadTextFileResponse>),
    WriteFile(acp::WriteTextFileRequest, Reply<acp::WriteTextFileResponse>),
    /// The agent's process is gone; the end of what it wrote on stderr.
    Exited(String),
}

/// The answer to one of the agent's requests (an error if dropped).
pub struct Reply<T> {
    responder: Responder,
    _type: PhantomData<fn(T)>,
}

impl<T: Serialize> Reply<T> {
    pub fn ok(self, value: T) {
        let value = serde_json::to_value(value).map_err(|e| RpcError::internal(e.to_string()));
        self.responder.respond(value);
    }

    pub fn err(self, error: RpcError) {
        self.responder.respond(Err(error));
    }
}

pub struct Agent {
    pub spec: AgentSpec,
    conn: Connection,
    child: Mutex<Child>,
    stderr: Arc<Mutex<String>>,
}

impl Agent {
    /// Start `spec` in `cwd`. Events are delivered on a thread of the
    /// connection, one at a time: `on_event` must not block (answers to
    /// our requests are read on that thread too). The agent gets
    /// [`process::search_path`] as its `PATH`, unless `spec.env` sets one.
    pub fn start(
        spec: &AgentSpec,
        cwd: &Path,
        mut on_event: impl FnMut(Event) + Send + 'static,
    ) -> std::io::Result<Agent> {
        let mut cmd = Command::new(&spec.program);
        let own_path = spec.env.iter().any(|(k, _)| {
            if cfg!(windows) {
                k.eq_ignore_ascii_case("PATH")
            } else {
                k == "PATH"
            }
        });
        if !own_path {
            cmd.env("PATH", process::search_path());
        }
        cmd.args(&spec.args)
            .current_dir(cwd)
            // Windows: no program from the project before those on `PATH`.
            .env("NoDefaultCurrentDirectoryInExePath", "1")
            .envs(spec.env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        process::isolate(&mut cmd);
        log::info!(
            "starting agent {}: {} {:?}",
            spec.name,
            spec.program.display(),
            spec.args
        );
        let mut child = cmd.spawn()?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr_pipe = child.stderr.take().expect("piped stderr");

        let stderr = Arc::new(Mutex::new(String::new()));
        let (stderr_done, stderr_ended) = mpsc::channel::<()>();
        {
            let stderr = stderr.clone();
            let name = spec.name.clone();
            std::thread::spawn(move || {
                // Drained to the end whatever it holds: a full pipe would
                // block the agent.
                let mut pipe = BufReader::new(stderr_pipe);
                while let Ok(Some(line)) = crate::rpc::read_line(&mut pipe, STDERR_TAIL) {
                    log::debug!("{name}: {line}");
                    let mut tail = stderr.lock().unwrap();
                    tail.push_str(&line);
                    tail.push('\n');
                    if tail.len() > STDERR_TAIL {
                        let mut cut = tail.len() - STDERR_TAIL;
                        while !tail.is_char_boundary(cut) {
                            cut += 1;
                        }
                        tail.drain(..cut);
                    }
                }
                drop(stderr_done);
            });
        }

        let tail = stderr.clone();
        let conn = Connection::new(BufReader::new(stdout), stdin, move |msg| match msg {
            Incoming::Notification { method, params } => {
                if method == "session/update" {
                    match serde_json::from_value(params) {
                        Ok(n) => on_event(Event::Update(n)),
                        // A newer kind of update: nothing to show for it.
                        Err(e) => log::debug!("session/update not understood: {e}"),
                    }
                }
            }
            Incoming::Request {
                method,
                params,
                responder,
            } => match method.as_str() {
                "session/request_permission" => {
                    if let Some((req, reply)) = parse(params, responder) {
                        on_event(Event::Permission(req, reply));
                    }
                }
                "fs/read_text_file" => {
                    if let Some((req, reply)) = parse(params, responder) {
                        on_event(Event::ReadFile(req, reply));
                    }
                }
                "fs/write_text_file" => {
                    if let Some((req, reply)) = parse(params, responder) {
                        on_event(Event::WriteFile(req, reply));
                    }
                }
                _ => responder.respond(Err(RpcError::new(
                    RpcError::METHOD_NOT_FOUND,
                    format!("{method} is not supported"),
                ))),
            },
            Incoming::Closed => {
                // Let the stderr thread read the last lines.
                let _ = stderr_ended.recv_timeout(STDERR_WAIT);
                on_event(Event::Exited(tail.lock().unwrap().clone()));
            }
        });
        Ok(Agent {
            spec: spec.clone(),
            conn,
            child: Mutex::new(child),
            stderr,
        })
    }

    pub fn initialize(&self) -> impl Future<Output = Result<acp::InitializeResponse, RpcError>> {
        let fs = acp::FileSystemCapabilities::new()
            .read_text_file(true)
            .write_text_file(true);
        let request = acp::InitializeRequest::new(ProtocolVersion::V1)
            .client_capabilities(acp::ClientCapabilities::new().fs(fs).terminal(false))
            .client_info(
                acp::Implementation::new("texoria", env!("CARGO_PKG_VERSION")).title("TeXoria"),
            );
        call(self.conn.request("initialize", request))
    }

    pub fn authenticate(
        &self,
        method: acp::AuthMethodId,
    ) -> impl Future<Output = Result<acp::AuthenticateResponse, RpcError>> {
        call(
            self.conn
                .request("authenticate", acp::AuthenticateRequest::new(method)),
        )
    }

    pub fn new_session(
        &self,
        cwd: &Path,
        mcp_servers: Vec<acp::McpServer>,
    ) -> impl Future<Output = Result<acp::NewSessionResponse, RpcError>> {
        let request = acp::NewSessionRequest::new(cwd).mcp_servers(mcp_servers);
        call(self.conn.request("session/new", request))
    }

    pub fn prompt(
        &self,
        session: acp::SessionId,
        content: Vec<acp::ContentBlock>,
    ) -> impl Future<Output = Result<acp::PromptResponse, RpcError>> {
        call(
            self.conn
                .request("session/prompt", acp::PromptRequest::new(session, content)),
        )
    }

    /// Stop the current turn; the pending `prompt` then ends as cancelled.
    pub fn cancel(&self, session: &acp::SessionId) {
        let _ = self.conn.notify(
            "session/cancel",
            acp::CancelNotification::new(session.clone()),
        );
    }

    pub fn set_mode(
        &self,
        session: acp::SessionId,
        mode: acp::SessionModeId,
    ) -> impl Future<Output = Result<acp::SetSessionModeResponse, RpcError>> {
        call(self.conn.request(
            "session/set_mode",
            acp::SetSessionModeRequest::new(session, mode),
        ))
    }

    /// The end of what the agent wrote on stderr (to explain a failure).
    pub fn stderr_tail(&self) -> String {
        self.stderr.lock().unwrap().clone()
    }
}

impl Drop for Agent {
    fn drop(&mut self) {
        process::kill_tree(&mut self.child.lock().unwrap());
    }
}

fn parse<T: DeserializeOwned, R>(params: Value, responder: Responder) -> Option<(T, Reply<R>)> {
    match serde_json::from_value(params) {
        Ok(req) => Some((
            req,
            Reply {
                responder,
                _type: PhantomData,
            },
        )),
        Err(e) => {
            responder.respond(Err(RpcError::new(RpcError::INVALID_PARAMS, e.to_string())));
            None
        }
    }
}

async fn call<T: DeserializeOwned>(response: crate::rpc::Response) -> Result<T, RpcError> {
    let value = response.await?;
    serde_json::from_value(value).map_err(|e| RpcError::internal(e.to_string()))
}
