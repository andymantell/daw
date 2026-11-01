// SPDX-License-Identifier: GPL-3.0-or-later
//! Settings → AI assistants… (roadmap 2.11): turns on the local server
//! that assistants connect to, shows its address and token, and asks
//! the person at the computer what each assistant may do the first time
//! it calls: look only, edit, or edit and export. "Stop assistants"
//! (Ctrl+Shift+Backspace) turns the server off and forgets every
//! permission at once.
//!
//! The server and its protocol live in `daw_agent`; the tools in
//! [`crate::agent`]. Off unless turned on, and only this computer can
//! reach it.

use std::collections::HashMap;
use std::path::PathBuf;

use daw_agent::{Client, DEFAULT_PORT, Event, Server, ToolInfo};
use eframe::egui::{self, RichText};

use crate::App;
use crate::agent::{Access, Request, all_tools};
use crate::commands::Cmd;

/// Told to assistants when they connect.
const INSTRUCTIONS: &str = "This is a music app (a digital audio workstation). get_project \
    gives the tracks, clips and effects with their ids; list_commands gives every command; \
    list_parameters gives each track setting with its range; set_parameter sets one for the \
    whole song, and edit_automation makes one change over time (volume rides, fades, filter \
    sweeps, pan moves). Each request that changes the \
    project is one undo step, which the person can take back. Ask before big changes.";

/// What has been chosen for one assistant.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Choice {
    Allowed(Access),
    Refused,
}

#[derive(Default)]
pub(crate) struct Assistants {
    pub(crate) open: bool,
    /// Turned on by the person; kept between runs.
    on: bool,
    token: String,
    show_token: bool,
    server: Option<Server>,
    /// Each assistant's permission, by its id, for this run.
    chosen: HashMap<String, (Client, Choice)>,
    /// Assistants waiting for the person to choose, oldest first.
    asking: Vec<Client>,
    error: Option<String>,
    path: Option<PathBuf>,
    /// The port tried first.
    port: Option<u16>,
    /// Every assistant may do this without asking: headless mode, where
    /// no one is there to ask.
    allow_all: Option<Access>,
}

fn settings_path() -> PathBuf {
    crate::config_dir().join("assistants.txt")
}

impl Assistants {
    /// The saved setting, and the server started if it was on. Tests
    /// never touch the network or the user's settings.
    pub(crate) fn load() -> Assistants {
        let mut a = Assistants::default();
        if cfg!(test) {
            return a;
        }
        a.path = Some(settings_path());
        let text = std::fs::read_to_string(settings_path()).unwrap_or_default();
        for line in text.lines() {
            match line.split_once('=') {
                Some(("on", v)) => a.on = v.trim() == "1",
                Some(("token", v)) => a.token = v.trim().into(),
                _ => {}
            }
        }
        if a.on {
            a.start();
        }
        a
    }

    fn save(&mut self) {
        let Some(path) = &self.path else {
            return;
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let text = format!("on={}\ntoken={}\n", self.on as u8, self.token);
        let tmp = path.with_extension("tmp");
        if std::fs::write(&tmp, text)
            .and_then(|()| std::fs::rename(&tmp, path))
            .is_err()
        {
            self.error = Some("The setting couldn't be saved.".into());
        }
    }

    fn start(&mut self) {
        if self.server.is_some() {
            return;
        }
        if self.token.is_empty() {
            self.token = daw_remote::new_token();
        }
        let tools = all_tools()
            .map(|t| ToolInfo {
                name: t.name.into(),
                description: t.description.into(),
                input_schema: crate::agent::schema_of(t),
                read_only: t.access == Access::Read,
            })
            .collect();
        let started = Server::start(self.port.unwrap_or(DEFAULT_PORT), &self.token, tools, |m| {
            m.name = crate::APP_NAME.into();
            m.version = env!("CARGO_PKG_VERSION").into();
            m.instructions = INSTRUCTIONS.into();
        });
        match started {
            Ok(s) => {
                self.server = Some(s);
                self.error = None;
            }
            Err(e) => self.error = Some(format!("Can't start the server: {e}")),
        }
    }

    /// Serves without asking anyone, for headless mode: on `port` (or
    /// the next free one) with `token`, every assistant allowed
    /// `access`. The address, or why it can't start.
    pub(crate) fn serve(
        &mut self,
        port: u16,
        token: &str,
        access: Access,
    ) -> Result<String, String> {
        // Not the window's settings: a server they started is stopped,
        // and nothing here is saved over them.
        self.server = None;
        self.path = None;
        self.port = Some(port);
        self.token = token.into();
        self.allow_all = Some(access);
        self.start();
        match &self.server {
            Some(s) => Ok(s.url()),
            None => Err(self.error.clone().unwrap_or_default()),
        }
    }

    /// Turns the server off and forgets every permission.
    pub(crate) fn stop(&mut self) {
        self.server = None;
        self.chosen.clear();
        self.asking.clear();
        if self.on {
            self.on = false;
            self.save();
        }
    }

    fn set_on(&mut self, on: bool) {
        if on {
            self.on = true;
            self.start();
            self.save();
        } else {
            self.stop();
        }
    }

    fn new_token(&mut self) {
        self.token = daw_remote::new_token();
        self.chosen.clear();
        self.asking.clear();
        if let Some(s) = &self.server {
            s.set_token(&self.token);
        }
        self.save();
    }

    /// How an assistant's settings file names the server.
    fn config_text(&self, url: &str) -> String {
        format!(
            "{{\n  \"mcpServers\": {{\n    \"daw\": {{\n      \"type\": \"http\",\n      \
             \"url\": \"{url}\",\n      \"headers\": {{ \"Authorization\": \"Bearer {}\" }}\n    \
             }}\n  }}\n}}",
            self.token
        )
    }

    /// Asks the person about an assistant not yet allowed, once.
    fn ask(&mut self, client: Client) {
        if !self.asking.iter().any(|c| c.id == client.id) {
            self.asking.push(client);
            self.open = true;
        }
    }
}

impl App {
    /// Settings → AI assistants…, and the calls that have come in.
    pub(crate) fn assistants_tick(&mut self, ctx: &egui::Context) {
        let Some(server) = &self.assistants.server else {
            return;
        };
        let events = server.events();
        if events.is_empty() {
            // Calls wait for the UI thread, which may be idle.
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
            return;
        }
        for e in events {
            match e {
                Event::Call {
                    client,
                    tool,
                    args,
                    reply,
                } => {
                    let who = client.name.clone();
                    let a = &self.assistants;
                    let choice = (a.chosen.get(&client.id).map(|c| c.1))
                        .or(a.allow_all.map(Choice::Allowed));
                    match choice {
                        Some(Choice::Allowed(access)) => {
                            let req = Request {
                                tool,
                                args,
                                access,
                                reply,
                            };
                            let _ = self.agent.inbox.send(req);
                        }
                        Some(Choice::Refused) => {
                            let _ = reply.send(Err(format!(
                                "The person at the computer didn't allow {who} to use the app."
                            )));
                        }
                        None => {
                            self.assistants.ask(client);
                            let _ = reply.send(Err(format!(
                                "Waiting for the person at the computer to allow {who}: the \
                                 app is asking them now. Try again once they have chosen."
                            )));
                        }
                    }
                }
                Event::Gone { client } => {
                    self.assistants.chosen.remove(&client.id);
                    self.assistants.asking.retain(|c| c.id != client.id);
                }
            }
        }
        ctx.request_repaint();
    }

    /// Settings → AI assistants.
    pub(crate) fn assistants_page(&mut self, ui: &mut egui::Ui) {
        let a = &mut self.assistants;
        let mut turn = None;
        let mut new_token = false;
        let mut stop = false;
        let stop_keys = self.commands.keymap.text(Cmd::StopAssistants);
        {
            ui.label(
                "Let an AI assistant on this computer read the project and use the app's \
                     commands, through the Model Context Protocol. Only this computer can \
                     connect, and only with the token below.",
            );
            ui.add_space(6.0);
            if let Some(e) = &a.error {
                ui.colored_label(ui.visuals().error_fg_color, e);
            }
            let mut on = a.on;
            if ui.checkbox(&mut on, "Let assistants connect").changed() {
                turn = Some(on);
            }
            if let Some(s) = &a.server {
                let url = s.url();
                ui.add_space(6.0);
                egui::Grid::new("assistant-server")
                    .num_columns(2)
                    .show(ui, |ui| {
                        ui.label("Address");
                        ui.horizontal(|ui| {
                            ui.monospace(&url);
                            if ui.small_button("Copy").clicked() {
                                ui.ctx().copy_text(url.clone());
                            }
                        });
                        ui.end_row();
                        ui.label("Token");
                        ui.horizontal(|ui| {
                            if a.show_token {
                                ui.monospace(&a.token);
                            } else {
                                ui.monospace("•".repeat(12));
                            }
                            ui.checkbox(&mut a.show_token, "Show");
                            if ui.small_button("Copy").clicked() {
                                ui.ctx().copy_text(a.token.clone());
                            }
                            if ui
                                .small_button("New token")
                                .on_hover_text("The old token stops working")
                                .clicked()
                            {
                                new_token = true;
                            }
                        });
                        ui.end_row();
                    });
                ui.collapsing("Settings to paste into an assistant", |ui| {
                    let text = a.config_text(&url);
                    ui.label(
                        "Many assistants take a server in this shape. It holds the token, \
                             so keep it private.",
                    );
                    if ui.button("Copy").clicked() {
                        ui.ctx().copy_text(text.clone());
                    }
                });
                ui.separator();
                for c in a.asking.clone() {
                    ui.label(RichText::new(format!("{} wants to use the app.", c.name)).strong());
                    ui.horizontal(|ui| {
                        let mut pick = None;
                        if ui.button("Look only").clicked() {
                            pick = Some(Choice::Allowed(Access::Read));
                        }
                        if ui.button("Edit").clicked() {
                            pick = Some(Choice::Allowed(Access::Edit));
                        }
                        if ui
                            .button("Edit and export")
                            .on_hover_text("Also save, and write files")
                            .clicked()
                        {
                            pick = Some(Choice::Allowed(Access::EditExport));
                        }
                        if ui.button("Refuse").clicked() {
                            pick = Some(Choice::Refused);
                        }
                        if let Some(p) = pick {
                            a.asking.retain(|x| x.id != c.id);
                            a.chosen.insert(c.id.clone(), (c.clone(), p));
                        }
                    });
                }
                let mut chosen: Vec<_> = a.chosen.values().cloned().collect();
                chosen.sort_by(|x, y| x.0.name.cmp(&y.0.name));
                if chosen.is_empty() && a.asking.is_empty() {
                    ui.weak("No assistant has connected yet.");
                }
                for (c, choice) in chosen {
                    ui.horizontal(|ui| {
                        let what = match choice {
                            Choice::Allowed(x) => x.name(),
                            Choice::Refused => "refused",
                        };
                        ui.label(format!("{}: {what}", c.name));
                        if ui.small_button("Forget").clicked() {
                            a.chosen.remove(&c.id);
                        }
                    });
                }
                ui.separator();
                if ui
                    .button(format!("Stop assistants ({stop_keys})"))
                    .on_hover_text("Turns the server off and forgets every permission")
                    .clicked()
                {
                    stop = true;
                }
            }
        }
        if let Some(on) = turn {
            a.set_on(on);
        }
        if new_token {
            a.new_token();
        }
        if stop {
            a.stop();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::thread;

    use serde_json::{Value, json};

    use super::*;
    use crate::agent::driver::Driver;

    /// Posts JSON-RPC to the server; the status and the answer.
    fn post(port: u16, token: &str, session: Option<&str>, body: Value) -> (u16, String, Value) {
        let body = body.to_string();
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let mut req = format!(
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {token}\r\n\
             Content-Type: application/json\r\nAccept: application/json, text/event-stream\r\n\
             Content-Length: {}\r\n",
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
        let (head, body) = out.split_once("\r\n\r\n").unwrap();
        let session = (head.lines())
            .find_map(|l| l.strip_prefix("Mcp-Session-Id: "))
            .unwrap_or("")
            .to_string();
        let v = serde_json::from_str(body).unwrap_or(Value::Null);
        (out[9..12].parse().unwrap(), session, v)
    }

    /// Makes a tool call from another thread while the app runs frames.
    fn call(
        d: &mut Driver,
        port: u16,
        token: &str,
        session: &str,
        tool: &str,
        args: Value,
    ) -> Value {
        let (token, session, tool) = (token.to_string(), session.to_string(), tool.to_string());
        let t = thread::spawn(move || {
            let body = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
                "params": {"name": tool, "arguments": args}});
            post(port, &token, Some(&session), body).2
        });
        while !t.is_finished() {
            d.h.step();
            thread::sleep(std::time::Duration::from_millis(2));
        }
        t.join().unwrap()["result"].clone()
    }

    #[test]
    fn an_assistant_is_asked_about_then_drives_the_app_until_stopped() {
        let mut d = Driver::new();
        d.app().assistants.set_on(true);
        let (port, token) = {
            let a = &d.app().assistants;
            (a.server.as_ref().unwrap().port(), a.token.clone())
        };
        let init = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-11-25", "capabilities": {},
            "clientInfo": {"name": "Test assistant", "version": "1"}}});
        let (status, session, _) = post(port, &token, None, init);
        assert_eq!(status, 200);
        // Not yet allowed: the person is asked.
        let r = call(&mut d, port, &token, &session, "get_transport", json!({}));
        assert_eq!(r["isError"], true);
        let asking = d.app().assistants.asking.clone();
        assert_eq!(asking.len(), 1);
        assert_eq!(asking[0].name, "Test assistant");
        assert!(d.app().assistants.open);
        // Allowed to edit, it runs a command, but may not save.
        let a = &mut d.app().assistants;
        a.asking.clear();
        let c = asking[0].clone();
        a.chosen
            .insert(c.id.clone(), (c, Choice::Allowed(Access::Edit)));
        let r = call(
            &mut d,
            port,
            &token,
            &session,
            "run_command",
            json!({"id": "loop"}),
        );
        assert_eq!(r["isError"], false, "{r}");
        assert!(d.app().looping);
        let r = call(
            &mut d,
            port,
            &token,
            &session,
            "run_command",
            json!({"id": "save"}),
        );
        assert_eq!(r["isError"], true);
        // The stop key turns the server off.
        let ctx = d.h.ctx.clone();
        d.app().run(&ctx, Cmd::StopAssistants);
        assert!(d.app().assistants.server.is_none());
        assert!(d.app().assistants.chosen.is_empty());
    }
}
