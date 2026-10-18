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

/// Longer lines are cut (and then dropped as invalid JSON): a misbehaving
/// program must not exhaust the memory.
const MAX_LINE: usize = 256 << 20;

/// One line of `reader`, without its line break, decoded lossily (a stray
/// byte that is not UTF-8 must not end the connection). At most `max` bytes
/// are kept; the rest of a longer line is skipped. `None` at the end.
pub(crate) fn read_line(reader: &mut impl BufRead, max: usize) -> std::io::Result<Option<String>> {
    let mut line = Vec::new();
    let mut any = false;
    loop {
        let buf = match reader.fill_buf() {
            Ok(buf) => buf,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if buf.is_empty() {
            return Ok(any.then(|| String::from_utf8_lossy(&line).into_owned()));
        }
        any = true;
        let end = buf.iter().position(|&b| b == b'\n');
        let chunk = &buf[..end.unwrap_or(buf.len())];
        let room = max.saturating_sub(line.len());
        line.extend_from_slice(&chunk[..chunk.len().min(room)]);
        let used = end.map_or(buf.len(), |i| i + 1);
        reader.consume(used);
        if end.is_some() {
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            return Ok(Some(String::from_utf8_lossy(&line).into_owned()));
        }
    }
}

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
    /// Start reading `reader`. `handler` runs on the reading thread, so it
    /// must not block: until it returns, no answer to this side's requests
    /// is read either.
    pub fn new<R, W>(
        mut reader: R,
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
                while let Ok(Some(line)) = read_line(&mut reader, MAX_LINE) {
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
    use proptest::strategy::Just;
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
    fn lines_survive_bad_bytes_and_huge_lengths() {
        let mut input: &[u8] = b"caf\xe9\r\nabcdefgh\nlast";
        assert_eq!(read_line(&mut input, 100).unwrap().unwrap(), "caf\u{fffd}");
        assert_eq!(read_line(&mut input, 3).unwrap().unwrap(), "abc");
        assert_eq!(read_line(&mut input, 100).unwrap().unwrap(), "last");
        assert_eq!(read_line(&mut input, 100).unwrap(), None);

        let (ar, mut bw) = std::io::pipe().unwrap();
        let (_br, aw) = std::io::pipe().unwrap();
        let (tx, rx) = mpsc::channel();
        let _a = Connection::new(BufReader::new(ar), aw, move |msg| {
            if let Incoming::Notification { method, .. } = msg {
                tx.send(method).unwrap();
            }
        });
        bw.write_all(b"\xff\xfe log\n{\"jsonrpc\":\"2.0\",\"method\":\"still\"}\n")
            .unwrap();
        assert_eq!(rx.recv().unwrap(), "still");
    }

    /// One end whose peer is driven by hand: what it writes, and what it reads.
    fn manual(
        handler: impl FnMut(Incoming) + Send + 'static,
    ) -> (
        Connection,
        std::io::PipeWriter,
        BufReader<std::io::PipeReader>,
    ) {
        let (ar, bw) = std::io::pipe().unwrap();
        let (br, aw) = std::io::pipe().unwrap();
        (
            Connection::new(BufReader::new(ar), aw, handler),
            bw,
            BufReader::new(br),
        )
    }

    fn read_json(r: &mut impl BufRead) -> Value {
        serde_json::from_str(&read_line(r, MAX_LINE).unwrap().unwrap()).unwrap()
    }

    #[test]
    fn answers_are_matched_by_integer_id_only() {
        let (a, mut peer, mut from_a) = manual(|_| {});
        let pending = a.request("work", json!({}));
        let sent = read_json(&mut from_a);
        assert_eq!(sent["method"], "work");
        let id = sent["id"].as_i64().unwrap();
        // Ids that are not ours, or not integers, answer nothing...
        for wrong in [
            json!(id.to_string()),
            json!(id as f64 + 0.5),
            json!(id + 100),
        ] {
            writeln!(
                peer,
                "{}",
                json!({ "jsonrpc": "2.0", "id": wrong, "result": 1 })
            )
            .unwrap();
        }
        // ...nor does a message with neither id nor method.
        writeln!(peer, r#"{{"jsonrpc":"2.0","result":2}}"#).unwrap();
        // `error: null` is not an error.
        writeln!(
            peer,
            "{}",
            json!({ "jsonrpc": "2.0", "id": id, "result": 3, "error": null })
        )
        .unwrap();
        assert_eq!(block_on(pending).unwrap(), json!(3));

        // A missing result is null; an error without fields still fails.
        let pending = a.request("more", json!({}));
        let id = read_json(&mut from_a)["id"].clone();
        writeln!(peer, "{}", json!({ "jsonrpc": "2.0", "id": id })).unwrap();
        assert_eq!(block_on(pending).unwrap(), Value::Null);
        let pending = a.request("fail", json!({}));
        let id = read_json(&mut from_a)["id"].clone();
        writeln!(
            peer,
            "{}",
            json!({ "jsonrpc": "2.0", "id": id, "error": {} })
        )
        .unwrap();
        let e = block_on(pending).unwrap_err();
        assert_eq!(
            (e.code, e.message.as_str()),
            (RpcError::INTERNAL, "Unknown error")
        );

        // A request answered with a wrong id waits until the connection closes.
        let pending = a.request("lost", json!({}));
        drop(peer);
        assert!(block_on(pending).unwrap_err().is_closed());
    }

    #[test]
    fn requests_from_the_other_side_are_answered_once() {
        let (_a, mut peer, mut from_a) = manual(|msg| match msg {
            Incoming::Request {
                method, responder, ..
            } if method == "fail" => {
                let mut e = RpcError::new(RpcError::NOT_FOUND, "gone");
                e.data = Some(json!({ "path": "x" }));
                responder.respond(Err(e));
            }
            Incoming::Request { responder, .. } => responder.respond(Ok(json!("done"))),
            _ => {}
        });
        writeln!(peer, r#"{{"jsonrpc":"2.0","id":"a","method":"fail"}}"#).unwrap();
        let reply = read_json(&mut from_a);
        assert_eq!(reply["id"], "a");
        assert_eq!(reply["error"]["code"], RpcError::NOT_FOUND);
        assert_eq!(reply["error"]["data"]["path"], "x");
        // A null id is a notification: nothing comes back for it.
        writeln!(peer, r#"{{"jsonrpc":"2.0","id":null,"method":"note"}}"#).unwrap();
        writeln!(peer, r#"{{"jsonrpc":"2.0","id":7,"method":"ok"}}"#).unwrap();
        let reply = read_json(&mut from_a);
        assert_eq!(
            (reply["id"].clone(), reply["result"].clone()),
            (json!(7), json!("done"))
        );
    }

    /// A writer whose other side is gone.
    struct Broken;

    impl Write for Broken {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::ErrorKind::BrokenPipe.into())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_failed_write_forgets_the_request() {
        let (ar, _bw) = std::io::pipe().unwrap();
        let a = Connection::new(BufReader::new(ar), Broken, |_| {});
        assert!(block_on(a.request("x", json!({}))).unwrap_err().is_closed());
        assert!(a.pending.lock().unwrap().as_ref().unwrap().is_empty());
        assert!(a.notify("y", json!({})).unwrap_err().is_closed());
        assert!(!a.is_closed(), "still reading");
    }

    /// Hands out `data` a few bytes at a time, interrupted before each piece.
    struct Choppy {
        data: Vec<u8>,
        at: usize,
        interrupt: bool,
    }

    impl std::io::Read for Choppy {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.interrupt = !self.interrupt;
            if self.interrupt {
                return Err(std::io::ErrorKind::Interrupted.into());
            }
            let n = buf.len().min(3).min(self.data.len() - self.at);
            buf[..n].copy_from_slice(&self.data[self.at..self.at + n]);
            self.at += n;
            Ok(n)
        }
    }

    #[test]
    fn reading_resumes_after_an_interruption() {
        let data = b"first line\nsecond\n".to_vec();
        let mut r = BufReader::with_capacity(
            4,
            Choppy {
                data,
                at: 0,
                interrupt: false,
            },
        );
        assert_eq!(
            read_line(&mut r, 100).unwrap().as_deref(),
            Some("first line")
        );
        assert_eq!(read_line(&mut r, 4).unwrap().as_deref(), Some("seco"));
        assert_eq!(read_line(&mut r, 100).unwrap(), None);

        struct Failing;
        impl std::io::Read for Failing {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::ConnectionReset.into())
            }
        }
        assert!(read_line(&mut BufReader::new(Failing), 10).is_err());
    }

    proptest::proptest! {
        #[test]
        fn lines_are_split_like_bufread(
            data in proptest::collection::vec(
                proptest::prop_oneof![Just(b'\n'), Just(b'\r'), Just(0xff), proptest::num::u8::ANY],
                0..200,
            ),
            max in 0usize..40,
            capacity in 1usize..16,
        ) {
            let mut r = BufReader::with_capacity(capacity, &data[..]);
            let mut ours = Vec::new();
            while let Some(line) = read_line(&mut r, max).unwrap() {
                ours.push(line);
            }
            let pieces: Vec<&[u8]> = data.split(|&b| b == b'\n').collect();
            let last = pieces.len() - 1;
            let expected: Vec<String> = pieces
                .iter()
                .enumerate()
                .map(|(i, l)| {
                    let l = &l[..l.len().min(max)];
                    // Like `BufRead::lines`, a `\r` goes only with a line break.
                    let l = if i < last { l.strip_suffix(b"\r").unwrap_or(l) } else { l };
                    String::from_utf8_lossy(l).into_owned()
                })
                .collect();
            // `split` yields an empty last piece after a final line break.
            let expected = match data.last() {
                Some(b'\n') | None => &expected[..expected.len() - 1],
                _ => &expected[..],
            };
            proptest::prop_assert_eq!(&ours, expected);
            for line in &ours {
                // Every kept byte becomes at most one replacement character.
                proptest::prop_assert!(line.chars().count() <= max);
            }
        }
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
