// SPDX-License-Identifier: GPL-3.0-or-later
//! The built program run as `daw --headless`, driven over its assistant
//! server as an assistant would: connect, add a track, read it back,
//! undo it.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};

use serde_json::{Value, json};

const TOKEN: &str = "headless-test-token";

/// Stops the program when the test ends, passed or not.
struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start() -> (Running, u16) {
    let dir = std::env::temp_dir().join(format!("daw-headless-{}", std::process::id()));
    let mut child = Command::new(env!("CARGO_BIN_EXE_daw"))
        // A port of its own, so tests running at once don't meet.
        .args([
            "--headless",
            "--port",
            &(20_000 + std::process::id() % 20_000).to_string(),
        ])
        .env("DAW_AGENT_TOKEN", TOKEN)
        // Settings and caches go somewhere of the test's own.
        .env("HOME", &dir)
        .env("XDG_CONFIG_HOME", dir.join("config"))
        .env("XDG_CACHE_HOME", dir.join("cache"))
        .env("APPDATA", dir.join("config"))
        .env("LOCALAPPDATA", dir.join("cache"))
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("the program starts");
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let url = line
        .strip_prefix("listening ")
        .unwrap_or_else(|| panic!("first line names the address: {line:?}"))
        .trim();
    let port = url
        .trim_start_matches("http://127.0.0.1:")
        .trim_end_matches("/mcp")
        .parse()
        .unwrap();
    (Running(child), port)
}

/// A JSON-RPC request; gives the HTTP status, the session header and the body.
fn post(port: u16, session: Option<&str>, body: Value) -> (u16, Option<String>, Value) {
    let body = body.to_string();
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let mut req = format!(
        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {TOKEN}\r\n\
         Content-Type: application/json\r\nAccept: application/json, text/event-stream\r\n\
         MCP-Protocol-Version: 2025-06-18\r\nContent-Length: {}\r\n",
        body.len()
    );
    if let Some(id) = session {
        req.push_str(&format!("Mcp-Session-Id: {id}\r\n"));
    }
    req.push_str("\r\n");
    req.push_str(&body);
    s.write_all(req.as_bytes()).unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    let status = out[9..12].parse().unwrap();
    let (head, body) = out.split_once("\r\n\r\n").unwrap();
    let session = head.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.eq_ignore_ascii_case("mcp-session-id")
            .then(|| v.trim().to_string())
    });
    let body = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_str(body).unwrap()
    };
    (status, session, body)
}

fn call(port: u16, session: &str, id: u64, tool: &str, args: Value) -> Value {
    let (status, _, r) = post(
        port,
        Some(session),
        json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
               "params": {"name": tool, "arguments": args}}),
    );
    assert_eq!(status, 200, "{r}");
    assert_eq!(r["result"]["isError"], json!(false), "{tool}: {r}");
    r["result"]["structuredContent"].clone()
}

fn tracks(port: u16, session: &str, id: u64) -> usize {
    call(port, session, id, "get_project", json!({}))["tracks"]
        .as_array()
        .unwrap()
        .len()
}

#[test]
fn an_assistant_drives_the_headless_program() {
    let (_app, port) = start();
    let (status, session, r) = post(
        port,
        None,
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": "headless-test", "version": "1"}}}),
    );
    assert_eq!(status, 200, "{r}");
    let session = session.expect("a session is given");
    let (status, _, _) = post(
        port,
        Some(&session),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
    );
    assert_eq!(status, 202);

    let (_, _, list) = post(
        port,
        Some(&session),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
    );
    let names: Vec<&str> = (list["result"]["tools"].as_array().unwrap().iter())
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(names.contains(&"run_command"), "{names:?}");

    assert_eq!(tracks(port, &session, 3), 0);
    call(
        port,
        &session,
        4,
        "run_command",
        json!({"id": "add-synth-track"}),
    );
    assert_eq!(tracks(port, &session, 5), 1);
    call(port, &session, 6, "run_command", json!({"id": "undo"}));
    assert_eq!(tracks(port, &session, 7), 0);

    // There's no window, so no pictures; it says so rather than hanging.
    let (_, _, r) = post(
        port,
        Some(&session),
        json!({"jsonrpc": "2.0", "id": 8, "method": "tools/call",
               "params": {"name": "get_picture", "arguments": {}}}),
    );
    assert_eq!(r["result"]["isError"], json!(true), "{r}");
}
