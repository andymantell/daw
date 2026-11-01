// SPDX-License-Identifier: GPL-3.0-or-later
//! The listener: HTTP on the loopback address, a thread per request,
//! the answer worked out by [`Mcp`]. Each connection carries one
//! request and is closed after its answer.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use daw_remote::pairing::same_secret;

use crate::mcp::{Event, Headers, Mcp, Response, ToolInfo};

/// The port tried first, so an assistant set up once finds the app
/// again. Any free port is used if it is taken.
pub const DEFAULT_PORT: u16 = 7346;
/// Where on the server the protocol is.
pub const PATH: &str = "/mcp";

/// The largest request body taken.
const MAX_BODY: usize = 1 << 20;
const MAX_HEADERS: usize = 64;
const READ_TIMEOUT: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(50);

struct Shared {
    mcp: Mcp,
    token: Mutex<String>,
    stop: AtomicBool,
}

/// The server and its threads. Dropping it stops it.
pub struct Server {
    port: u16,
    shared: Arc<Shared>,
    events: Receiver<Event>,
}

impl Server {
    /// Listens on `port` of 127.0.0.1, or any free port if that one is
    /// taken, for requests carrying `token`.
    pub fn start(
        port: u16,
        token: &str,
        tools: Vec<ToolInfo>,
        setup: impl FnOnce(&mut Mcp),
    ) -> io::Result<Server> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))
            .or_else(|_| TcpListener::bind((Ipv4Addr::LOCALHOST, 0)))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let (tx, events) = mpsc::channel();
        let mut mcp = Mcp::new(tools, tx);
        setup(&mut mcp);
        let shared = Arc::new(Shared {
            mcp,
            token: Mutex::new(token.into()),
            stop: AtomicBool::new(false),
        });
        let s = shared.clone();
        thread::Builder::new()
            .name("agent-accept".into())
            .spawn(move || accept_loop(listener, s))?;
        Ok(Server {
            port,
            shared,
            events,
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// The address to give an assistant.
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}{PATH}", self.port)
    }

    /// From now on only `token` is let in, and sessions opened with the
    /// old one are ended.
    pub fn set_token(&self, token: &str) {
        *self.shared.token.lock().unwrap() = token.into();
        self.shared.mcp.end_sessions();
    }

    /// Ends every session; assistants must connect again.
    pub fn end_sessions(&self) {
        self.shared.mcp.end_sessions();
    }

    /// What has come in since the last call.
    pub fn events(&self) -> Vec<Event> {
        self.events.try_iter().collect()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
    }
}

fn accept_loop(listener: TcpListener, shared: Arc<Shared>) {
    while !shared.stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((sock, _)) => {
                let s = shared.clone();
                let _ = thread::Builder::new()
                    .name("agent-request".into())
                    .spawn(move || serve(sock, &s));
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => thread::sleep(POLL),
            Err(_) => thread::sleep(POLL),
        }
    }
}

/// A request as read off the socket.
struct HttpRequest {
    method: String,
    path: String,
    headers: Headers,
    body: Vec<u8>,
}

fn read_request(sock: &TcpStream) -> io::Result<HttpRequest> {
    let bad = |what: &str| io::Error::new(io::ErrorKind::InvalidData, what.to_string());
    let mut r = BufReader::new(sock.take(MAX_BODY as u64 + 64 * 1024));
    let mut line = String::new();
    r.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let (Some(method), Some(path)) = (parts.next(), parts.next()) else {
        return Err(bad("no request line"));
    };
    let (method, path) = (method.to_string(), path.to_string());
    let mut headers = Headers::default();
    loop {
        line.clear();
        if r.read_line(&mut line)? == 0 {
            return Err(bad("cut short"));
        }
        let l = line.trim_end();
        if l.is_empty() {
            break;
        }
        if headers.0.len() == MAX_HEADERS {
            return Err(bad("too many headers"));
        }
        let (k, v) = l.split_once(':').ok_or_else(|| bad("bad header"))?;
        headers.0.push((k.trim().into(), v.trim().into()));
    }
    let len: usize = match headers.get("content-length") {
        Some(n) => n.parse().map_err(|_| bad("bad length"))?,
        None => 0,
    };
    if len > MAX_BODY {
        return Err(bad("too long"));
    }
    let mut body = vec![0; len];
    r.read_exact(&mut body)?;
    Ok(HttpRequest {
        method,
        path,
        headers,
        body,
    })
}

/// Whether an `Origin` or `Host` names this computer.
fn is_local(value: &str) -> bool {
    let host = value
        .strip_prefix("http://")
        .or_else(|| value.strip_prefix("https://"))
        .unwrap_or(value);
    let host = match host.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or(""),
        None => host.split(':').next().unwrap_or(""),
    };
    matches!(host, "127.0.0.1" | "localhost" | "::1")
}

fn serve(mut sock: TcpStream, shared: &Shared) {
    let _ = sock.set_read_timeout(Some(READ_TIMEOUT));
    let _ = sock.set_nodelay(true);
    let reply = match read_request(&sock) {
        Ok(req) => answer(&req, shared),
        Err(_) => plain(400, "Bad request"),
    };
    let _ = sock.write_all(&reply);
    let _ = sock.flush();
}

fn answer(req: &HttpRequest, shared: &Shared) -> Vec<u8> {
    let h = &req.headers;
    if h.get("origin").is_some_and(|o| !is_local(o)) || h.get("host").is_some_and(|o| !is_local(o))
    {
        return plain(403, "Only this computer may connect.");
    }
    if req.path.split('?').next() != Some(PATH) {
        return plain(404, "Not here: the server is at /mcp.");
    }
    let given = h
        .get("authorization")
        .and_then(|a| a.strip_prefix("Bearer "))
        .unwrap_or("");
    if !same_secret(given.trim(), &shared.token.lock().unwrap()) {
        return http(
            401,
            &[("WWW-Authenticate", "Bearer")],
            "text/plain",
            b"Give the token shown in the app's AI assistants window.",
        );
    }
    let r = match req.method.as_str() {
        "POST" => shared.mcp.handle(h, &req.body),
        "DELETE" => shared.mcp.delete(h),
        _ => return http(405, &[("Allow", "POST, DELETE")], "text/plain", b""),
    };
    respond(&r)
}

fn respond(r: &Response) -> Vec<u8> {
    let mut extra = Vec::new();
    if let Some(s) = &r.session {
        extra.push(("Mcp-Session-Id", s.as_str()));
    }
    match &r.body {
        Some(b) => http(
            r.status,
            &extra,
            "application/json",
            b.to_string().as_bytes(),
        ),
        None => http(r.status, &extra, "text/plain", b""),
    }
}

fn plain(status: u16, text: &str) -> Vec<u8> {
    http(status, &[], "text/plain", text.as_bytes())
}

fn http(status: u16, headers: &[(&str, &str)], kind: &str, body: &[u8]) -> Vec<u8> {
    let reason = match status {
        200 => "OK",
        202 => "Accepted",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Error",
    };
    let mut out = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    for (k, v) in headers {
        out.push_str(&format!("{k}: {v}\r\n"));
    }
    out.push_str("\r\n");
    let mut out = out.into_bytes();
    out.extend_from_slice(body);
    out
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn post(port: u16, headers: &[(&str, &str)], body: &str) -> (u16, String, String) {
        let mut s = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        let mut req = format!(
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n",
            body.len()
        );
        for (k, v) in headers {
            req.push_str(&format!("{k}: {v}\r\n"));
        }
        req.push_str("\r\n");
        req.push_str(body);
        s.write_all(req.as_bytes()).unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        let status = out[9..12].parse().unwrap();
        let (head, body) = out.split_once("\r\n\r\n").unwrap();
        (status, head.to_string(), body.to_string())
    }

    fn server() -> Server {
        let tools = vec![ToolInfo {
            name: "get_transport".into(),
            description: "Where playback is.".into(),
            input_schema: json!({"type": "object"}),
            read_only: true,
        }];
        Server::start(0, "secret", tools, |_| {}).unwrap()
    }

    const INIT: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25"}}"#;

    #[test]
    fn the_token_is_needed() {
        let s = server();
        assert_eq!(post(s.port(), &[], INIT).0, 401);
        assert_eq!(
            post(s.port(), &[("Authorization", "Bearer wrong")], INIT).0,
            401
        );
        let (status, head, body) = post(s.port(), &[("Authorization", "Bearer secret")], INIT);
        assert_eq!(status, 200, "{body}");
        assert!(head.contains("Mcp-Session-Id: "), "{head}");
        s.set_token("new");
        assert_eq!(
            post(s.port(), &[("Authorization", "Bearer secret")], INIT).0,
            401
        );
    }

    #[test]
    fn web_pages_are_kept_out() {
        let s = server();
        let auth = ("Authorization", "Bearer secret");
        let (status, ..) = post(s.port(), &[auth, ("Origin", "https://example.com")], INIT);
        assert_eq!(status, 403);
        let (status, ..) = post(s.port(), &[auth, ("Origin", "http://localhost:3000")], INIT);
        assert_eq!(status, 200);
    }

    #[test]
    fn calls_reach_the_app_and_its_answer_comes_back() {
        let s = server();
        let (_, head, _) = post(s.port(), &[("Authorization", "Bearer secret")], INIT);
        let session = head
            .lines()
            .find_map(|l| l.strip_prefix("Mcp-Session-Id: "))
            .unwrap()
            .to_string();
        let port = s.port();
        let t = thread::spawn(move || {
            post(
                port,
                &[
                    ("Authorization", "Bearer secret"),
                    ("Mcp-Session-Id", &session),
                ],
                r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"get_transport"}}"#,
            )
        });
        let event = loop {
            if let Some(e) = s.events().pop() {
                break e;
            }
            thread::sleep(Duration::from_millis(5));
        };
        let Event::Call { tool, reply, .. } = event else {
            panic!("a call");
        };
        assert_eq!(tool, "get_transport");
        reply.send(Ok(json!({"playing": false}))).unwrap();
        let (status, _, body) = t.join().unwrap();
        assert_eq!(status, 200);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["result"]["structuredContent"]["playing"], false);
    }

    #[test]
    fn local_names_are_known() {
        assert!(is_local("127.0.0.1:7346"));
        assert!(is_local("http://localhost"));
        assert!(is_local("[::1]:7346"));
        assert!(!is_local("http://127.0.0.1.example.com"));
        assert!(!is_local("evil.test"));
    }
}
