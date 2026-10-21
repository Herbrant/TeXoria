//! A running agent, against the scripted fake agent (`tests/bin/fake_acp_agent.rs`).

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use futures_executor::block_on;
use serde_json::{Value, json};
use tex_agent::{Agent, AgentSpec, Event, RpcError, acp};

const FAKE: &str = env!("CARGO_BIN_EXE_fake-acp-agent");

/// The fake agent playing `script`, and the file it logs what it reads to.
struct Fake {
    dir: tempfile::TempDir,
    spec: AgentSpec,
}

impl Fake {
    fn new(mut script: Value) -> Self {
        let dir = tempfile::tempdir().unwrap();
        script["log"] = json!(dir.path().join("log.jsonl"));
        let path = dir.path().join("script.json");
        std::fs::write(&path, script.to_string()).unwrap();
        let spec = AgentSpec {
            id: "fake".into(),
            name: "Fake".into(),
            program: PathBuf::from(FAKE),
            args: vec![path.to_string_lossy().into_owned()],
            // Not the login shell's: the fake needs none.
            env: vec![("PATH".into(), std::env::var("PATH").unwrap_or_default())],
            login_hint: None,
        };
        Fake { dir, spec }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    /// Start it; events go to the returned channel, after `answer` saw them.
    fn start(
        &self,
        mut answer: impl FnMut(&mut Event) + Send + 'static,
    ) -> (Agent, mpsc::Receiver<Event>) {
        let (tx, rx) = mpsc::channel();
        let agent = Agent::start(&self.spec, self.dir.path(), move |mut e| {
            answer(&mut e);
            let _ = tx.send(e);
        })
        .unwrap();
        (agent, rx)
    }

    /// What the fake read from the client so far.
    fn log(&self) -> Vec<Value> {
        std::fs::read_to_string(self.path("log.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    /// The client's answer to the fake's request number `id` (they start at 1001).
    fn answer(&self, id: i64) -> Value {
        self.log()
            .into_iter()
            .find(|m| m["id"] == id && m.get("method").is_none())
            .unwrap_or_else(|| panic!("no answer to {id} in {:?}", self.log()))
    }
}

fn recv(rx: &mpsc::Receiver<Event>) -> Event {
    rx.recv_timeout(Duration::from_secs(20)).expect("an event")
}

fn exited(rx: &mpsc::Receiver<Event>) -> String {
    loop {
        if let Event::Exited(tail) = recv(rx) {
            return tail;
        }
    }
}

fn text_update(text: &str) -> Value {
    json!({
        "method": "session/update",
        "params": {
            "sessionId": "s1",
            "update": {
                "sessionUpdate": "agent_message_chunk",
                "content": { "type": "text", "text": text },
            },
        },
    })
}

#[test]
fn runs_a_session() {
    let fake = Fake::new(json!({
        "on": {
            "initialize": [{ "respond": {
                "protocolVersion": 1,
                "agentCapabilities": { "loadSession": true },
                "authMethods": [{ "id": "login", "name": "Log in" }],
            }}],
            "session/new": [{ "respond": {
                "sessionId": "abc",
                "modes": {
                    "currentModeId": "ask",
                    "availableModes": [{ "id": "ask", "name": "Ask" }],
                },
            }}],
            "session/prompt": [
                { "notify": text_update("Hello") },
                { "request": { "method": "fs/read_text_file", "params": {
                    "sessionId": "abc", "path": "/p/main.tex", "line": 2, "limit": 3,
                }}},
                { "request": { "method": "fs/write_text_file", "params": {
                    "sessionId": "abc", "path": "/p/main.tex", "content": "new",
                }}},
                { "request": { "method": "session/request_permission", "params": {
                    "sessionId": "abc",
                    "toolCall": { "toolCallId": "t1", "title": "Run latexmk" },
                    "options": [
                        { "optionId": "no", "name": "Reject", "kind": "reject_once" },
                        { "optionId": "yes", "name": "Allow", "kind": "allow_once" },
                    ],
                }}},
                { "notify": text_update("Done") },
                { "respond": { "stopReason": "end_turn" } },
            ],
        },
    }));
    let (agent, rx) = fake.start(
        |e| match std::mem::replace(e, Event::Exited(String::new())) {
            Event::ReadFile(req, reply) => {
                assert_eq!((req.line, req.limit), (Some(2), Some(3)));
                reply.ok(acp::ReadTextFileResponse::new("old"));
                *e = Event::Exited("read".into());
            }
            Event::WriteFile(req, reply) => {
                assert_eq!(req.content, "new");
                reply.ok(acp::WriteTextFileResponse::new());
                *e = Event::Exited("write".into());
            }
            Event::Permission(req, reply) => {
                let allow = req
                    .options
                    .iter()
                    .find(|o| o.kind == acp::PermissionOptionKind::AllowOnce)
                    .unwrap();
                reply.ok(acp::RequestPermissionResponse::new(
                    acp::RequestPermissionOutcome::Selected(acp::SelectedPermissionOutcome::new(
                        allow.option_id.clone(),
                    )),
                ));
                *e = Event::Exited("permission".into());
            }
            other => *e = other,
        },
    );
    assert_eq!(agent.spec.id, "fake");

    let init = block_on(agent.initialize()).unwrap();
    assert!(init.agent_capabilities.load_session);
    assert_eq!(init.auth_methods.len(), 1);
    let session = block_on(agent.new_session(Path::new("/p"), Vec::new())).unwrap();
    assert_eq!(session.session_id.to_string(), "abc");
    let content = vec![acp::ContentBlock::from("Fix it".to_string())];
    let done = block_on(agent.prompt(session.session_id.clone(), content)).unwrap();
    assert_eq!(done.stop_reason, acp::StopReason::EndTurn);

    // In the order the agent sent them.
    let mut seen = Vec::new();
    while let Ok(e) = rx.recv_timeout(Duration::from_millis(200)) {
        seen.push(match e {
            Event::Update(n) => match n.update {
                acp::SessionUpdate::AgentMessageChunk(c) => match c.content {
                    acp::ContentBlock::Text(t) => t.text,
                    other => panic!("{other:?}"),
                },
                other => panic!("{other:?}"),
            },
            Event::Exited(what) => what,
            _ => unreachable!(),
        });
    }
    assert_eq!(seen, ["Hello", "read", "write", "permission", "Done"]);

    // What the client sent.
    let log = fake.log();
    let init = &log[0];
    assert_eq!(init["method"], "initialize");
    assert_eq!(
        init["params"]["clientCapabilities"]["fs"]["readTextFile"],
        true
    );
    assert_eq!(
        init["params"]["clientCapabilities"]["fs"]["writeTextFile"],
        true
    );
    assert_eq!(init["params"]["clientCapabilities"]["terminal"], false);
    assert_eq!(init["params"]["clientInfo"]["name"], "texoria");
    assert_eq!(log[1]["params"]["cwd"], "/p");
    assert_eq!(log[2]["params"]["prompt"][0]["text"], "Fix it");
    assert_eq!(fake.answer(1001)["result"]["content"], "old");
    assert_eq!(fake.answer(1002)["result"], json!({}));
    assert_eq!(
        fake.answer(1003)["result"]["outcome"],
        json!({ "outcome": "selected", "optionId": "yes" })
    );
}

#[test]
fn modes_sign_in_and_cancel() {
    let fake = Fake::new(json!({
        "on": {
            "authenticate": [{ "respond_error": { "code": -32000, "message": "Sign in first",
                "data": { "details": "token expired" } } }],
        },
    }));
    let (agent, _rx) = fake.start(|_| {});
    let e = block_on(agent.authenticate(acp::AuthMethodId::new("login"))).unwrap_err();
    assert_eq!(e.code, RpcError::AUTH_REQUIRED);
    assert_eq!(e.to_string(), "Sign in first: token expired");
    let session = acp::SessionId::new("s1");
    block_on(agent.set_mode(session.clone(), acp::SessionModeId::new("code"))).unwrap();
    agent.cancel(&session);
    let cancel = test_support::eventually(Duration::from_secs(10), || {
        (fake.log().into_iter()).find(|m| m["method"] == "session/cancel")
    })
    .unwrap_or_else(|| panic!("{:?}", fake.log()));
    assert_eq!(cancel["params"]["sessionId"], "s1");
    assert!(cancel.get("id").is_none(), "a notification");
    let set_mode = fake
        .log()
        .into_iter()
        .find(|m| m["method"] == "session/set_mode")
        .unwrap();
    assert_eq!(set_mode["params"]["modeId"], "code");
}

#[test]
fn refuses_what_it_does_not_understand() {
    let fake = Fake::new(json!({
        "on": {
            "session/prompt": [
                // Not a request the client knows.
                { "request": { "method": "terminal/create", "params": { "command": "sh" } } },
                // Known, but malformed.
                { "request": { "method": "fs/read_text_file", "params": { "path": 3 } } },
                { "request": { "method": "fs/write_text_file", "params": {} } },
                { "request": { "method": "session/request_permission", "params": [] } },
                // An update of a newer kind, and an unknown notification.
                { "notify": { "method": "session/update", "params": {
                    "sessionId": "s1", "update": { "sessionUpdate": "hologram" } } } },
                { "notify": { "method": "agent/telemetry", "params": {} } },
                // Lines that are no JSON-RPC at all.
                { "raw": "Debugger listening on ws://127.0.0.1:9229" },
                { "raw": "{\"jsonrpc\":\"2.0\",\"id\":[1],\"result\":{}}" },
                { "notify": text_update("still here") },
            ],
        },
    }));
    let (agent, rx) = fake.start(|_| {});
    let done = block_on(agent.prompt(acp::SessionId::new("s1"), Vec::new())).unwrap();
    assert_eq!(done.stop_reason, acp::StopReason::EndTurn);
    let code = |id| fake.answer(id)["error"]["code"].as_i64().unwrap();
    assert_eq!(code(1001), RpcError::METHOD_NOT_FOUND);
    assert!(
        fake.answer(1001)["error"]["message"]
            .as_str()
            .unwrap()
            .contains("terminal/create")
    );
    for id in [1002, 1003, 1004] {
        assert_eq!(code(id), RpcError::INVALID_PARAMS, "{id}");
    }
    // Only the update it understood became an event.
    match recv(&rx) {
        Event::Update(n) => assert!(matches!(n.update, acp::SessionUpdate::AgentMessageChunk(_))),
        _ => panic!("expected the text update"),
    }
    assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
}

#[test]
fn answers_it_cannot_read_are_errors() {
    let fake = Fake::new(json!({
        "on": { "initialize": [{ "respond": { "protocolVersion": "one" } }] },
    }));
    let (agent, _rx) = fake.start(|_| {});
    let e = block_on(agent.initialize()).unwrap_err();
    assert_eq!(e.code, RpcError::INTERNAL);
}

#[test]
fn an_agent_that_exits_ends_its_requests() {
    let fake = Fake::new(json!({
        "on": {
            "session/prompt": [
                { "stderr": "Error: out of credits\n" },
                { "exit": 3 },
            ],
        },
    }));
    let (agent, rx) = fake.start(|_| {});
    block_on(agent.initialize()).unwrap();
    let e = block_on(agent.prompt(acp::SessionId::new("s1"), Vec::new())).unwrap_err();
    assert!(e.is_closed(), "{e:?}");
    assert_eq!(exited(&rx), "Error: out of credits\n");
    assert_eq!(agent.stderr_tail(), "Error: out of credits\n");
    // Later requests fail at once.
    assert!(block_on(agent.initialize()).unwrap_err().is_closed());
}

#[test]
fn a_crash_ends_the_turn() {
    let fake = Fake::new(json!({ "on": { "session/prompt": [{ "abort": true }] } }));
    let (agent, rx) = fake.start(|_| {});
    let e = block_on(agent.prompt(acp::SessionId::new("s1"), Vec::new())).unwrap_err();
    assert!(e.is_closed());
    exited(&rx);
}

#[test]
fn keeps_the_end_of_stderr() {
    let fake = Fake::new(json!({
        "start": [
            // Long lines of two-byte characters, a line longer than the tail,
            // and a last line.
            { "stderr_repeat": { "text": format!("{}\n", "é".repeat(99)), "times": 100 } },
            { "stderr_repeat": { "text": "ü", "times": 5000 } },
            { "stderr": "\nthe end\n" },
            { "exit": 1 },
        ],
    }));
    let (agent, rx) = fake.start(|_| {});
    let tail = exited(&rx);
    assert!(tail.len() <= 4000, "{}", tail.len());
    assert!(tail.len() > 3000, "{}", tail.len());
    assert!(tail.ends_with("ü\nthe end\n"), "{tail:?}");
    assert!(!tail.contains('é'), "older lines are gone");
    assert_eq!(agent.stderr_tail(), tail);
}

#[test]
fn reports_stderr_written_after_stdout_closed() {
    // A helper of the agent (sharing its stderr) writes after the agent is gone.
    let fake = Fake::new(json!({
        "start": [
            { "spawn": { "args": ["--late-stderr", "late words\n", "400"] } },
            { "exit": 2 },
        ],
    }));
    let (_agent, rx) = fake.start(|_| {});
    assert_eq!(exited(&rx), "late words\n");
}

#[test]
fn does_not_wait_forever_for_stderr() {
    // A helper keeps stderr open long after the agent is gone.
    let fake = Fake::new(json!({
        "start": [
            { "stderr": "bye\n" },
            { "spawn": { "args": ["--sleep"] } },
            { "exit": 2 },
        ],
    }));
    let start = Instant::now();
    let (agent, rx) = fake.start(|_| {});
    assert_eq!(exited(&rx), "bye\n");
    assert!(start.elapsed() < Duration::from_secs(10));
    // Dropping it stops the helper.
    drop(agent);
}

#[test]
fn a_program_that_cannot_start() {
    let mut fake = Fake::new(json!({}));
    fake.spec.program = fake.path("missing");
    assert!(Agent::start(&fake.spec, fake.dir.path(), |_| {}).is_err());
}

/// Whether process `pid` is gone within a few seconds.
fn gone(pid: u32) -> bool {
    test_support::within(Duration::from_secs(10), || !alive(pid))
}

#[cfg(unix)]
fn alive(pid: u32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

#[cfg(windows)]
fn alive(pid: u32) -> bool {
    let out = std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).contains(&format!("\"{pid}\""))
}

#[test]
fn dropping_the_agent_stops_its_helpers() {
    let fake = Fake::new(json!({}));
    let pid_file = fake.path("child.pid");
    let script = json!({
        "start": [{ "spawn": { "args": ["--sleep"], "pid_file": pid_file } }],
        "log": fake.path("log.jsonl"),
    });
    std::fs::write(&fake.spec.args[0], script.to_string()).unwrap();
    let (agent, _rx) = fake.start(|_| {});
    block_on(agent.initialize()).unwrap();
    let pid: u32 = std::fs::read_to_string(&pid_file).unwrap().parse().unwrap();
    assert!(alive(pid));
    drop(agent);
    assert!(gone(pid), "the helper survived its agent");
}
