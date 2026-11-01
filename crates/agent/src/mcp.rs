// SPDX-License-Identifier: GPL-3.0-or-later
//! The protocol: JSON-RPC requests in, responses out, with no sockets.
//! [`Mcp::handle`] takes one HTTP request's headers and body and says
//! what to answer; tool calls go to the app as [`Event`]s.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::mpsc::{self, Sender};
use std::time::Duration;

use serde_json::{Value, json};

/// The stateless revision, where each request says who it is from.
pub const MODERN: &str = "2026-07-28";
/// The revisions that open a session with `initialize`, newest first.
pub const LEGACY: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26"];

const META_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
const META_CLIENT: &str = "io.modelcontextprotocol/clientInfo";

// JSON-RPC's own error codes, and the protocol's.
const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const HEADER_MISMATCH: i64 = -32020;
const UNSUPPORTED_VERSION: i64 = -32022;

/// A tool as assistants are told of it.
#[derive(Clone, Debug)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    /// A JSON schema for its arguments.
    pub input_schema: Value,
    /// It only looks; nothing changes.
    pub read_only: bool,
}

/// An assistant, as it names itself. Nothing checks the name: anything
/// with the token may call itself anything.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Client {
    /// What its calls are kept apart by: the session for the older
    /// revisions, else its name and version.
    pub id: String,
    pub name: String,
    pub version: String,
}

/// What the app is asked to act on.
#[derive(Debug)]
pub enum Event {
    /// A tool call. The answer is JSON, or why it failed. An object
    /// whose `$content` is a list of content blocks (an image, say)
    /// has them sent after the rest.
    Call {
        client: Client,
        tool: String,
        args: Value,
        reply: Sender<Result<Value, String>>,
    },
    /// An assistant ended its session.
    Gone { client: Client },
}

/// One HTTP request's headers, looked up without regard to case.
#[derive(Default, Debug)]
pub struct Headers(pub Vec<(String, String)>);

impl Headers {
    pub fn get(&self, name: &str) -> Option<&str> {
        (self.0.iter())
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.trim())
    }
}

/// What to answer: the status, the JSON body if any, and a session id
/// to give the client.
#[derive(Debug, PartialEq)]
pub struct Response {
    pub status: u16,
    pub body: Option<Value>,
    pub session: Option<String>,
}

impl Response {
    fn ok(id: &Value, result: Value) -> Response {
        Response {
            status: 200,
            body: Some(json!({"jsonrpc": "2.0", "id": id, "result": result})),
            session: None,
        }
    }

    fn error(
        status: u16,
        id: Option<&Value>,
        code: i64,
        message: &str,
        data: Option<Value>,
    ) -> Response {
        let mut e = json!({"code": code, "message": message});
        if let Some(d) = data {
            e["data"] = d;
        }
        let mut body = json!({"jsonrpc": "2.0", "error": e});
        if let Some(id) = id {
            body["id"] = id.clone();
        }
        Response {
            status,
            body: Some(body),
            session: None,
        }
    }

    fn accepted() -> Response {
        Response {
            status: 202,
            body: None,
            session: None,
        }
    }
}

/// The protocol's state: the tools, and the sessions open.
pub struct Mcp {
    tools: Vec<ToolInfo>,
    sessions: Mutex<HashMap<String, Client>>,
    events: Mutex<Sender<Event>>,
    /// How long a tool call may take before the assistant is told so.
    pub timeout: Duration,
    /// What the server calls itself.
    pub name: String,
    pub version: String,
    /// Told to assistants on connecting: how to use the tools.
    pub instructions: String,
}

impl Mcp {
    pub fn new(tools: Vec<ToolInfo>, events: Sender<Event>) -> Mcp {
        Mcp {
            tools,
            sessions: Mutex::new(HashMap::new()),
            events: Mutex::new(events),
            timeout: Duration::from_secs(120),
            name: "daw".into(),
            version: String::new(),
            instructions: String::new(),
        }
    }

    /// Ends every session: their next requests are told so.
    pub fn end_sessions(&self) {
        self.sessions.lock().unwrap().clear();
    }

    /// The answer to a DELETE, which ends a session.
    pub fn delete(&self, headers: &Headers) -> Response {
        let gone =
            (headers.get("mcp-session-id")).and_then(|s| self.sessions.lock().unwrap().remove(s));
        match gone {
            Some(client) => {
                let _ = self.events.lock().unwrap().send(Event::Gone { client });
                Response {
                    status: 200,
                    body: None,
                    session: None,
                }
            }
            None => Response::error(404, None, INVALID_REQUEST, "No such session.", None),
        }
    }

    /// The answer to a POST of `body`.
    pub fn handle(&self, headers: &Headers, body: &[u8]) -> Response {
        let Ok(msg) = serde_json::from_slice::<Value>(body) else {
            return Response::error(400, None, PARSE_ERROR, "The body isn't JSON.", None);
        };
        let Some(method) = msg["method"].as_str() else {
            return Response::error(
                400,
                None,
                INVALID_REQUEST,
                "Send one JSON-RPC request or notification.",
                None,
            );
        };
        let id = msg.get("id");
        let params = &msg["params"];
        let Some(id) = id.filter(|i| i.is_string() || i.is_number()) else {
            // A notification: nothing to answer.
            return Response::accepted();
        };
        if let Some(version) = params["_meta"][META_VERSION].as_str() {
            return self.modern(headers, id, method, params, version);
        }
        if method == "initialize" {
            return self.initialize(id, params);
        }
        let Some(session) = headers.get("mcp-session-id") else {
            return Response::error(
                400,
                Some(id),
                INVALID_REQUEST,
                "Start with initialize, or send the protocol version in _meta.",
                None,
            );
        };
        let client = self.sessions.lock().unwrap().get(session).cloned();
        let Some(client) = client else {
            return Response::error(404, Some(id), INVALID_REQUEST, "No such session.", None);
        };
        match method {
            "ping" => Response::ok(id, json!({})),
            "tools/list" => Response::ok(id, json!({"tools": self.tools_value()})),
            "tools/call" => self.call(id, params, client, false),
            _ => Response::error(404, Some(id), METHOD_NOT_FOUND, "Method not found", None),
        }
    }

    fn initialize(&self, id: &Value, params: &Value) -> Response {
        let asked = params["protocolVersion"].as_str().unwrap_or("");
        let version = LEGACY.iter().find(|v| **v == asked).unwrap_or(&LEGACY[0]);
        let session = daw_remote::new_token();
        let info = &params["clientInfo"];
        let client = Client {
            id: session.clone(),
            name: info["name"].as_str().unwrap_or("An assistant").into(),
            version: info["version"].as_str().unwrap_or("").into(),
        };
        self.sessions
            .lock()
            .unwrap()
            .insert(session.clone(), client);
        let mut r = Response::ok(
            id,
            json!({
                "protocolVersion": version,
                "capabilities": {"tools": {}},
                "serverInfo": {"name": self.name, "version": self.version},
                "instructions": self.instructions,
            }),
        );
        r.session = Some(session);
        r
    }

    fn modern(
        &self,
        headers: &Headers,
        id: &Value,
        method: &str,
        params: &Value,
        version: &str,
    ) -> Response {
        if headers.get("mcp-protocol-version") != Some(version)
            || headers.get("mcp-method") != Some(method)
        {
            return Response::error(
                400,
                Some(id),
                HEADER_MISMATCH,
                "The MCP-Protocol-Version and Mcp-Method headers must match the body.",
                None,
            );
        }
        if version != MODERN {
            return Response::error(
                400,
                Some(id),
                UNSUPPORTED_VERSION,
                "Unsupported protocol version",
                Some(json!({"supported": [MODERN], "requested": version})),
            );
        }
        let complete = |mut v: Value| {
            v["resultType"] = "complete".into();
            Response::ok(id, v)
        };
        match method {
            "server/discover" => complete(json!({
                "supportedVersions": [MODERN],
                "capabilities": {"tools": {}},
                "_meta": {"io.modelcontextprotocol/serverInfo":
                    {"name": self.name, "version": self.version}},
                "instructions": self.instructions,
            })),
            "ping" => complete(json!({})),
            "tools/list" => complete(json!({"tools": self.tools_value()})),
            "tools/call" => {
                if params.get("name").and_then(Value::as_str) != headers.get("mcp-name") {
                    return Response::error(
                        400,
                        Some(id),
                        HEADER_MISMATCH,
                        "The Mcp-Name header must match the tool's name.",
                        None,
                    );
                }
                let info = &params["_meta"][META_CLIENT];
                let name = info["name"].as_str().unwrap_or("An assistant").to_string();
                let version = info["version"].as_str().unwrap_or("").to_string();
                let client = Client {
                    id: format!("{name} {version}"),
                    name,
                    version,
                };
                self.call(id, params, client, true)
            }
            _ => Response::error(404, Some(id), METHOD_NOT_FOUND, "Method not found", None),
        }
    }

    fn tools_value(&self) -> Vec<Value> {
        (self.tools.iter())
            .map(|t| {
                json!({
                    "name": t.name,
                    "description": t.description,
                    "inputSchema": t.input_schema,
                    "annotations": {"readOnlyHint": t.read_only},
                })
            })
            .collect()
    }

    /// Hands a tool call to the app and waits for its answer.
    fn call(&self, id: &Value, params: &Value, client: Client, modern: bool) -> Response {
        let Some(tool) = params["name"].as_str() else {
            return Response::error(400, Some(id), INVALID_PARAMS, "Name the tool.", None);
        };
        if !self.tools.iter().any(|t| t.name == tool) {
            return Response::error(
                200,
                Some(id),
                INVALID_PARAMS,
                &format!("There is no tool {tool}."),
                None,
            );
        }
        let args = match &params["arguments"] {
            Value::Null => json!({}),
            a => a.clone(),
        };
        let (reply, answer) = mpsc::channel();
        let sent = self.events.lock().unwrap().send(Event::Call {
            client,
            tool: tool.into(),
            args,
            reply,
        });
        let result = match sent {
            Err(_) => Err("The app has closed.".to_string()),
            Ok(()) => answer.recv_timeout(self.timeout).unwrap_or_else(|_| {
                Err("The app didn't answer in time; it may be busy. Try again.".into())
            }),
        };
        let mut r = tool_result(result);
        if modern {
            r["resultType"] = "complete".into();
        }
        Response::ok(id, r)
    }
}

/// A tool's answer as a `tools/call` result.
fn tool_result(result: Result<Value, String>) -> Value {
    match result {
        Err(e) => json!({"content": [{"type": "text", "text": e}], "isError": true}),
        Ok(mut v) => {
            let extra = v
                .as_object_mut()
                .and_then(|o| o.remove("$content"))
                .and_then(|c| c.as_array().cloned())
                .unwrap_or_default();
            let mut content = vec![json!({"type": "text", "text": v.to_string()})];
            content.extend(extra);
            let mut r = json!({"content": content, "isError": false});
            if v.is_object() {
                r["structuredContent"] = v;
            }
            r
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tools() -> Vec<ToolInfo> {
        vec![ToolInfo {
            name: "get_transport".into(),
            description: "Where playback is.".into(),
            input_schema: json!({"type": "object", "properties": {}}),
            read_only: true,
        }]
    }

    fn headers(list: &[(&str, &str)]) -> Headers {
        Headers(
            (list.iter())
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        )
    }

    /// An app that answers each call with the tool's name and the
    /// client's.
    fn answering() -> Mcp {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for e in rx {
                if let Event::Call {
                    client,
                    tool,
                    reply,
                    ..
                } = e
                {
                    let _ = reply.send(Ok(json!({"tool": tool, "client": client.name})));
                }
            }
        });
        Mcp::new(tools(), tx)
    }

    #[test]
    fn an_older_client_opens_a_session_and_calls_in_it() {
        let m = answering();
        let r = m.handle(
            &headers(&[]),
            br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{
                "protocolVersion":"2025-06-18","clientInfo":{"name":"Desk","version":"2"}}}"#,
        );
        assert_eq!(r.status, 200);
        assert_eq!(
            r.body.as_ref().unwrap()["result"]["protocolVersion"],
            "2025-06-18"
        );
        let s = r.session.unwrap();
        let note = m.handle(
            &headers(&[("Mcp-Session-Id", &s)]),
            br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        );
        assert_eq!(note.status, 202);
        let list = m.handle(
            &headers(&[("Mcp-Session-Id", &s)]),
            br#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
        );
        let tools = &list.body.unwrap()["result"]["tools"];
        assert_eq!(tools[0]["name"], "get_transport");
        assert_eq!(tools[0]["annotations"]["readOnlyHint"], true);
        let call = m.handle(
            &headers(&[("mcp-session-id", &s)]),
            br#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"get_transport"}}"#,
        );
        let res = &call.body.unwrap()["result"];
        assert_eq!(res["isError"], false);
        assert_eq!(res["structuredContent"]["client"], "Desk");
        // Once ended, the session is gone.
        assert_eq!(m.delete(&headers(&[("Mcp-Session-Id", &s)])).status, 200);
        let after = m.handle(
            &headers(&[("Mcp-Session-Id", &s)]),
            br#"{"jsonrpc":"2.0","id":4,"method":"tools/list"}"#,
        );
        assert_eq!(after.status, 404);
    }

    #[test]
    fn an_unknown_version_is_answered_with_the_newest_older_one() {
        let m = answering();
        let r = m.handle(
            &headers(&[]),
            br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-01-01"}}"#,
        );
        assert_eq!(r.body.unwrap()["result"]["protocolVersion"], LEGACY[0]);
    }

    fn modern(method: &str, name: Option<&str>, version: &str) -> (Headers, Vec<u8>) {
        let mut h = vec![("MCP-Protocol-Version", version), ("Mcp-Method", method)];
        if let Some(n) = name {
            h.push(("Mcp-Name", n));
        }
        let body = json!({"jsonrpc": "2.0", "id": "a", "method": method, "params": {
            "name": name,
            "_meta": {META_VERSION: version, META_CLIENT: {"name": "Helper", "version": "1"}}
        }});
        (headers(&h), body.to_string().into_bytes())
    }

    #[test]
    fn a_stateless_client_calls_with_no_session() {
        let m = answering();
        let (h, b) = modern("server/discover", None, MODERN);
        let r = m.handle(&h, &b);
        let res = &r.body.unwrap()["result"];
        assert_eq!(res["resultType"], "complete");
        assert_eq!(res["supportedVersions"][0], MODERN);
        let (h, b) = modern("tools/call", Some("get_transport"), MODERN);
        let res = &m.handle(&h, &b).body.unwrap()["result"];
        assert_eq!(res["structuredContent"]["client"], "Helper");
        assert_eq!(res["resultType"], "complete");
    }

    #[test]
    fn headers_that_disagree_with_the_body_are_refused() {
        let m = answering();
        let (mut h, b) = modern("tools/call", Some("get_transport"), MODERN);
        h.0.retain(|(k, _)| k != "Mcp-Name");
        h.0.push(("Mcp-Name".into(), "other".into()));
        let r = m.handle(&h, &b);
        assert_eq!(r.status, 400);
        assert_eq!(r.body.unwrap()["error"]["code"], HEADER_MISMATCH);
        let (h, b) = modern("tools/list", None, "2099-01-01");
        let r = m.handle(&h, &b);
        assert_eq!(r.body.unwrap()["error"]["code"], UNSUPPORTED_VERSION);
    }

    #[test]
    fn a_call_without_a_session_or_version_is_refused() {
        let m = answering();
        let r = m.handle(
            &headers(&[]),
            br#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
        );
        assert_eq!(r.status, 400);
        let r = m.handle(&headers(&[]), b"not json");
        assert_eq!(r.status, 400);
    }

    #[test]
    fn extra_content_follows_the_text_and_errors_are_marked() {
        let r = tool_result(Ok(json!({"pane": "mixer", "$content": [
            {"type": "image", "data": "AAAA", "mimeType": "image/png"}
        ]})));
        assert_eq!(r["content"][1]["type"], "image");
        assert!(r["structuredContent"].get("$content").is_none());
        let e = tool_result(Err("No.".into()));
        assert_eq!(e["isError"], true);
        assert_eq!(e["content"][0]["text"], "No.");
    }
}
