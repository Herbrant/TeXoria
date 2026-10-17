//! JSON-RPC 2.0 over newline-delimited JSON: the framing of ACP, and of MCP's
//! stdio transport.

use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::io::{BufRead, Write};
use std::pin::Pin;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use futures_channel::oneshot;
use serde::Serialize;
use serde_json::{Value, json};

#[derive(Debug, Clone, PartialEq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

impl RpcError {
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const INTERNAL: i64 = -32603;
    /// ACP: the agent wants the user to sign in first.
    pub const AUTH_REQUIRED: i64 = -32000;
    /// ACP: a file or another resource does not exist.
    pub const NOT_FOUND: i64 = -32002;

    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(Self::INTERNAL, message)
    }

    /// The other side went away before answering.
    pub fn closed() -> Self {
        Self::internal("The connection was closed")
    }

    pub fn is_closed(&self) -> bool {
        *self == Self::closed()
    }

    fn from_value(v: &Value) -> Self {
        Self {
            code: v
                .get("code")
                .and_then(Value::as_i64)
                .unwrap_or(Self::INTERNAL),
            message: v
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("Unknown error")
                .to_string(),
            data: v.get("data").cloned(),
        }
    }

    fn to_value(&self) -> Value {
        let mut v = json!({ "code": self.code, "message": self.message });
        if let Some(data) = &self.data {
            v["data"] = data.clone();
        }
        v
    }
}

impl fmt::Display for RpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)?;
        // Agents put the useful part (the underlying error) in `data`.
        match &self.data {
            Some(Value::String(s)) if !s.is_empty() && *s != self.message => write!(f, ": {s}"),
            Some(Value::Object(o)) => match o.get("message").or(o.get("details")) {
                Some(Value::String(s)) if *s != self.message => write!(f, ": {s}"),
                _ => Ok(()),
            },
            _ => Ok(()),
        }
    }
}

impl std::error::Error for RpcError {}

type Writer = Arc<Mutex<Box<dyn Write + Send>>>;
type Reply = oneshot::Sender<Result<Value, RpcError>>;
/// `None` once the connection is closed: later requests fail right away.
type Pending = Arc<Mutex<Option<HashMap<i64, Reply>>>>;

/// A message from the other side that is not an answer to ours.
pub enum Incoming {
    Request {
        method: String,
        params: Value,
        responder: Responder,
    },
    Notification {
        method: String,
        params: Value,
    },
    /// The other side closed the connection (the last message).
    Closed,
}

/// Answers one incoming request. Dropping it unanswered sends an error, so
/// the other side never waits forever.
pub struct Responder {
    id: Value,
    writer: Writer,
    sent: bool,
}

impl Responder {
    pub fn respond(mut self, result: Result<Value, RpcError>) {
        self.sent = true;
        let msg = match result {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": self.id, "result": result }),
            Err(e) => json!({ "jsonrpc": "2.0", "id": self.id, "error": e.to_value() }),
        };
        let _ = write_message(&self.writer, &msg);
    }
}

impl Drop for Responder {
    fn drop(&mut self) {
        if !self.sent {
            let msg = json!({
                "jsonrpc": "2.0",
                "id": self.id,
                "error": RpcError::internal("The request was not handled").to_value(),
            });
            let _ = write_message(&self.writer, &msg);
        }
    }
}

/// One end of a connection. Cheap to clone; the reading happens on a thread
/// of its own, which hands incoming messages to the handler in order.
#[derive(Clone)]
pub struct Connection {
    writer: Writer,
    pending: Pending,
    next_id: Arc<AtomicI64>,
}

impl Connection {
    pub fn new<R, W>(
        reader: R,
        writer: W,
        mut handler: impl FnMut(Incoming) + Send + 'static,
    ) -> Self
    where
        R: BufRead + Send + 'static,
        W: Write + Send + 'static,
    {
        let writer: Writer = Arc::new(Mutex::new(Box::new(writer)));
        let pending: Pending = Arc::new(Mutex::new(Some(HashMap::new())));
        let conn = Connection {
            writer: writer.clone(),
            pending: pending.clone(),
            next_id: Arc::new(AtomicI64::new(0)),
        };
        std::thread::Builder::new()
            .name("json-rpc".into())
            .spawn(move || {
                for line in reader.lines() {
                    let Ok(line) = line else { break };
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    let msg: Value = match serde_json::from_str(line) {
                        Ok(v) => v,
                        Err(_) => {
                            // Some programs print banners or logs on stdout.
                            log::debug!("not JSON-RPC: {line}");
                            continue;
                        }
                    };
                    dispatch(msg, &writer, &pending, &mut handler);
                }
                // Dropping the senders fails every pending request.
                pending.lock().unwrap().take();
                handler(Incoming::Closed);
            })
            .expect("spawning the JSON-RPC reader");
        conn
    }

    /// Send a request; the answer arrives through the returned future.
    pub fn request(&self, method: &str, params: impl Serialize) -> Response {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        match self.pending.lock().unwrap().as_mut() {
            Some(map) => {
                map.insert(id, tx);
            }
            None => return Response::ready(Err(RpcError::closed())),
        }
        let msg = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        if let Err(e) = write_message(&self.writer, &msg) {
            if let Some(map) = self.pending.lock().unwrap().as_mut() {
                map.remove(&id);
            }
            return Response::ready(Err(e));
        }
        Response {
            rx: Some(rx),
            ready: None,
        }
    }

    pub fn notify(&self, method: &str, params: impl Serialize) -> Result<(), RpcError> {
        let msg = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        write_message(&self.writer, &msg)
    }

    pub fn is_closed(&self) -> bool {
        self.pending.lock().unwrap().is_none()
    }
}

fn dispatch(msg: Value, writer: &Writer, pending: &Pending, handler: &mut impl FnMut(Incoming)) {
    let id = msg.get("id").cloned().filter(|id| !id.is_null());
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    match (msg.get("method").and_then(Value::as_str), id) {
        (Some(method), Some(id)) => handler(Incoming::Request {
            method: method.to_string(),
            params,
            responder: Responder {
                id,
                writer: writer.clone(),
                sent: false,
            },
        }),
        (Some(method), None) => handler(Incoming::Notification {
            method: method.to_string(),
            params,
        }),
        (None, Some(id)) => {
            let Some(id) = id.as_i64() else { return };
            let reply = pending.lock().unwrap().as_mut().and_then(|m| m.remove(&id));
            if let Some(reply) = reply {
                let result = match msg.get("error") {
                    Some(e) if !e.is_null() => Err(RpcError::from_value(e)),
                    _ => Ok(msg.get("result").cloned().unwrap_or(Value::Null)),
                };
                let _ = reply.send(result);
            }
        }
        (None, None) => log::debug!("ignoring a JSON-RPC message without id or method"),
    }
}

fn write_message(writer: &Writer, msg: &Value) -> Result<(), RpcError> {
    let mut line = serde_json::to_vec(msg).map_err(|e| RpcError::internal(e.to_string()))?;
    line.push(b'\n');
    let mut w = writer.lock().unwrap();
    w.write_all(&line)
        .and_then(|()| w.flush())
        .map_err(|_| RpcError::closed())
}

/// The answer to a request.
pub struct Response {
    rx: Option<oneshot::Receiver<Result<Value, RpcError>>>,
    ready: Option<Result<Value, RpcError>>,
}

impl Response {
    fn ready(result: Result<Value, RpcError>) -> Self {
        Self {
            rx: None,
            ready: Some(result),
        }
    }
}

impl Future for Response {
    type Output = Result<Value, RpcError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if let Some(result) = self.ready.take() {
            return Poll::Ready(result);
        }
        match self.rx.as_mut() {
            Some(rx) => Pin::new(rx)
                .poll(cx)
                .map(|r| r.unwrap_or_else(|_| Err(RpcError::closed()))),
            None => Poll::Ready(Err(RpcError::closed())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_executor::block_on;
    use std::io::BufReader;
    use std::sync::mpsc;

    /// Two connected ends.
    fn pair(
        a: impl FnMut(Incoming) + Send + 'static,
        b: impl FnMut(Incoming) + Send + 'static,
    ) -> (Connection, Connection) {
        let (ar, bw) = std::io::pipe().unwrap();
        let (br, aw) = std::io::pipe().unwrap();
        (
            Connection::new(BufReader::new(ar), aw, a),
            Connection::new(BufReader::new(br), bw, b),
        )
    }

    #[test]
    fn requests_get_their_answers() {
        let (a, _b) = pair(
            |_| {},
            |msg| {
                if let Incoming::Request {
                    method,
                    params,
                    responder,
                } = msg
                {
                    match method.as_str() {
                        "echo" => responder.respond(Ok(params)),
                        _ => responder.respond(Err(RpcError::new(
                            RpcError::METHOD_NOT_FOUND,
                            "no such method",
                        ))),
                    }
                }
            },
        );
        let one = a.request("echo", json!({ "n": 1 }));
        let two = a.request("echo", json!({ "n": 2 }));
        assert_eq!(block_on(two).unwrap(), json!({ "n": 2 }));
        assert_eq!(block_on(one).unwrap(), json!({ "n": 1 }));
        let err = block_on(a.request("nope", json!({}))).unwrap_err();
        assert_eq!(err.code, RpcError::METHOD_NOT_FOUND);
    }

    #[test]
    fn notifications_and_unanswered_requests() {
        let (tx, rx) = mpsc::channel();
        let (a, _b) = pair(
            |_| {},
            move |msg| match msg {
                Incoming::Notification { method, params } => tx.send((method, params)).unwrap(),
                // Dropped: the requester gets an error, not silence.
                Incoming::Request { .. } | Incoming::Closed => {}
            },
        );
        a.notify("hello", json!([1])).unwrap();
        assert_eq!(rx.recv().unwrap(), ("hello".to_string(), json!([1])));
        assert!(block_on(a.request("ignored", json!({}))).is_err());
    }

    #[test]
    fn closing_fails_pending_requests() {
        let (ar, bw) = std::io::pipe().unwrap();
        let (_br, aw) = std::io::pipe().unwrap();
        let a = Connection::new(BufReader::new(ar), aw, |_| {});
        let pending = a.request("never", json!({}));
        drop(bw);
        assert!(block_on(pending).unwrap_err().is_closed());
        assert!(a.is_closed());
        assert!(block_on(a.request("later", json!({}))).is_err());
    }

    #[test]
    fn skips_lines_that_are_not_json() {
        let (ar, mut bw) = std::io::pipe().unwrap();
        let (_br, aw) = std::io::pipe().unwrap();
        let (tx, rx) = mpsc::channel();
        let _a = Connection::new(BufReader::new(ar), aw, move |msg| {
            if let Incoming::Notification { method, .. } = msg {
                tx.send(method).unwrap();
            }
        });
        bw.write_all(b"Starting agent v1.0...\n\n{\"jsonrpc\":\"2.0\",\"method\":\"ok\"}\n")
            .unwrap();
        assert_eq!(rx.recv().unwrap(), "ok");
    }

    #[test]
    fn error_messages_include_their_data() {
        let mut e = RpcError::internal("Internal error");
        e.data = Some(json!({ "details": "model not available" }));
        assert_eq!(e.to_string(), "Internal error: model not available");
        e.data = Some(json!("Internal error"));
        assert_eq!(e.to_string(), "Internal error");
    }
}
