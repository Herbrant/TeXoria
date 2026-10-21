//! A tiny HTTP/1.1 server on a loopback port, for clients that are otherwise only tested online.
//! One thread per connection, `Connection: close` on every response.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A request as the server read it.
#[derive(Debug, Clone)]
pub struct Request {
    pub method: String,
    /// The request target: path and query.
    pub path: String,
    /// Header names in lowercase.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    /// The fields of a form-encoded body (`a=1&b=x+y`), decoded.
    pub fn form(&self) -> Vec<(String, String)> {
        let decode = |s: &str| percent_decode(&s.replace('+', " "));
        std::str::from_utf8(&self.body)
            .unwrap()
            .split('&')
            .filter(|kv| !kv.is_empty())
            .map(|kv| {
                let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
                (decode(k), decode(v))
            })
            .collect()
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        let name = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| v.as_str())
    }
}

/// `s` with its `%XX` escapes decoded (`+` stays). Panics on a malformed escape or a result
/// that is not UTF-8: in a test, that is a failure.
#[track_caller]
pub fn percent_decode(s: &str) -> String {
    let (mut out, b) = (Vec::new(), s.as_bytes());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            out.push(u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap());
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap()
}

/// The URL of a loopback port where nothing listens (it was bound, then let go).
pub fn closed_url() -> String {
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    format!("http://127.0.0.1:{port}")
}

/// What to answer.
#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// Wait this long before answering.
    pub delay: Duration,
    /// Announce this `Content-Length` instead of the body's (to test size limits without
    /// sending it all).
    pub content_length: Option<u64>,
}

impl Response {
    pub fn ok(body: impl Into<Vec<u8>>) -> Self {
        Self::status(200).body(body)
    }

    pub fn status(status: u16) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: Vec::new(),
            delay: Duration::ZERO,
            content_length: None,
        }
    }

    pub fn not_found() -> Self {
        Self::status(404)
    }

    /// A `302 Found` to `location`.
    pub fn redirect(location: &str) -> Self {
        Self::status(302).header("Location", location)
    }

    pub fn body(mut self, body: impl Into<Vec<u8>>) -> Self {
        self.body = body.into();
        self
    }

    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    pub fn delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    pub fn content_length(mut self, len: u64) -> Self {
        self.content_length = Some(len);
        self
    }
}

type Handler = dyn Fn(&Request) -> Response + Send + Sync;

/// A running server. It keeps serving until the test process ends.
pub struct Server {
    base: String,
    requests: Arc<Mutex<Vec<Request>>>,
}

impl Server {
    /// `http://127.0.0.1:<port>`, without a trailing slash.
    pub fn url(&self) -> &str {
        &self.base
    }

    /// The requests received so far, in order.
    pub fn requests(&self) -> Vec<Request> {
        self.requests.lock().unwrap().clone()
    }
}

/// Serve `handler`'s answers until the test process ends.
pub fn serve(handler: impl Fn(&Request) -> Response + Send + Sync + 'static) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let handler: Arc<Handler> = Arc::new(handler);
    let log = requests.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let (handler, log) = (handler.clone(), log.clone());
            std::thread::spawn(move || {
                let _ = answer(stream, &*handler, &log);
            });
        }
    });
    Server { base, requests }
}

/// Serve fixed answers by exact path (query included); anything else is a 404.
pub fn serve_paths(routes: Vec<(&str, Response)>) -> Server {
    let routes: Vec<(String, Response)> = routes
        .into_iter()
        .map(|(p, r)| (p.to_string(), r))
        .collect();
    serve(move |req| {
        routes
            .iter()
            .find(|(p, _)| *p == req.path)
            .map_or_else(Response::not_found, |(_, r)| r.clone())
    })
}

fn answer(stream: TcpStream, handler: &Handler, log: &Mutex<Vec<Request>>) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();
    let mut headers = Vec::new();
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h)? == 0 || h.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = h.split_once(':') {
            headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
        }
    }
    let len = headers
        .iter()
        .find(|(n, _)| n == "content-length")
        .and_then(|(_, v)| v.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = vec![0; len];
    reader.read_exact(&mut body)?;
    let request = Request {
        method,
        path,
        headers,
        body,
    };
    let response = handler(&request);
    log.lock().unwrap().push(request);

    std::thread::sleep(response.delay);
    let mut out = format!(
        "HTTP/1.1 {} {}\r\n",
        response.status,
        reason(response.status)
    );
    for (name, value) in &response.headers {
        out.push_str(&format!("{name}: {value}\r\n"));
    }
    let length = response
        .content_length
        .unwrap_or(response.body.len() as u64);
    out.push_str(&format!(
        "Content-Length: {length}\r\nConnection: close\r\n\r\n"
    ));
    let mut stream = stream;
    stream.write_all(out.as_bytes())?;
    // A client that gives up early (size limits) closes the socket: not an error here.
    let _ = stream.write_all(&response.body);
    stream.flush()
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Status",
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn decodes_forms() {
        let req = super::Request {
            method: "POST".into(),
            path: "/".into(),
            headers: Vec::new(),
            body: b"text=a+b%26c&language=fr%C3%A9".to_vec(),
        };
        assert_eq!(
            req.form(),
            [
                ("text".to_string(), "a b&c".to_string()),
                ("language".to_string(), "fré".to_string())
            ]
        );
        assert!(super::closed_url().starts_with("http://127.0.0.1:"));
    }

    use super::*;

    fn get(url: &str, path: &str, body: &str) -> String {
        let addr = url.trim_start_matches("http://");
        let mut s = TcpStream::connect(addr).unwrap();
        write!(
            s,
            "POST {path} HTTP/1.1\r\nHost: x\r\nX-Test: yes\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        out
    }

    #[test]
    fn answers_by_path_and_logs_requests() {
        let server = serve_paths(vec![
            ("/a", Response::ok("hello")),
            ("/r", Response::redirect("http://example.com/x")),
        ]);
        let a = get(server.url(), "/a", "payload");
        assert!(a.starts_with("HTTP/1.1 200 OK\r\n"), "{a}");
        assert!(a.ends_with("\r\n\r\nhello"), "{a}");
        let r = get(server.url(), "/r", "");
        assert!(r.contains("Location: http://example.com/x\r\n"), "{r}");
        assert!(get(server.url(), "/missing", "").starts_with("HTTP/1.1 404"));

        let reqs = server.requests();
        assert_eq!(reqs.len(), 3);
        assert_eq!(reqs[0].method, "POST");
        assert_eq!(reqs[0].body, b"payload");
        assert_eq!(reqs[0].header("x-TEST"), Some("yes"));
    }
}
