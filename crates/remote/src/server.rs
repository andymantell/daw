// SPDX-License-Identifier: GPL-3.0-or-later
//! The desktop's end of the connection: a listener on the local network
//! and a thread per phone.
//!
//! Everything runs on plain threads with blocking sockets, so the UI
//! thread never waits on the network: it polls [`Server::events`] each
//! frame and hands over the latest `layout` and `status` with
//! [`Server::send_layout`] and [`Server::send_status`]. A phone that
//! reads slowly only ever misses the messages that newer ones have
//! replaced; it never holds anything up.

use std::io::{self, BufRead, BufReader, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

use crate::pairing::{Devices, Paired, new_token, same_secret};
use crate::protocol::{self, FromPhone, MAX_LINE, ToPhone, VERSION};

/// The port tried first, so a paired phone finds the desktop again after
/// a restart. Any free port is used if it is taken.
pub const DEFAULT_PORT: u16 = 7345;

/// How long a phone has to say hello.
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
/// A phone that sends nothing (not even a ping) for this long is gone.
const IDLE_TIMEOUT: Duration = Duration::from_secs(30);
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

/// What happened on the network, for the UI to act on.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// A phone paired with the code, which is now spent. The UI saves
    /// the devices list.
    Paired(Paired),
    Connected {
        id: u64,
        name: String,
    },
    Disconnected {
        id: u64,
    },
    /// A request from a connected phone (never `hello` or `ping`).
    Command {
        id: u64,
        cmd: FromPhone,
    },
}

struct Auth {
    code: Option<String>,
    devices: Devices,
    desktop: String,
}

#[derive(Default)]
struct Outbox {
    layout: Option<String>,
    status: Option<String>,
    closed: bool,
}

struct Client {
    id: u64,
    name: String,
    outbox: Mutex<Outbox>,
    wake: Condvar,
    /// The connection, to hang up from any thread.
    sock: TcpStream,
}

impl Client {
    fn close(&self) {
        self.outbox.lock().unwrap().closed = true;
        self.wake.notify_all();
        let _ = self.sock.shutdown(Shutdown::Both);
    }
}

struct Shared {
    auth: Mutex<Auth>,
    clients: Mutex<Vec<Arc<Client>>>,
    stop: AtomicBool,
    next_id: AtomicU64,
    events: Sender<Event>,
}

/// The listener and its threads. Dropping it closes every connection.
pub struct Server {
    port: u16,
    events: Receiver<Event>,
    shared: Arc<Shared>,
    accept: Option<thread::JoinHandle<()>>,
}

impl Server {
    /// Listens on `port` on every interface, or on any free port if
    /// that one is taken. `desktop` is this computer's name for the
    /// phone's screen; `devices` are the phones already paired.
    pub fn start(port: u16, desktop: &str, devices: Devices) -> io::Result<Server> {
        let listener = TcpListener::bind((Ipv4Addr::UNSPECIFIED, port))
            .or_else(|_| TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0)))?;
        let port = listener.local_addr()?.port();
        let (tx, rx) = mpsc::channel();
        let shared = Arc::new(Shared {
            auth: Mutex::new(Auth {
                code: None,
                devices,
                desktop: desktop.to_string(),
            }),
            clients: Mutex::new(Vec::new()),
            stop: AtomicBool::new(false),
            next_id: AtomicU64::new(1),
            events: tx,
        });
        let s = shared.clone();
        let accept = thread::Builder::new()
            .name("remote-accept".into())
            .spawn(move || accept_loop(listener, s))?;
        Ok(Server {
            port,
            events: rx,
            shared,
            accept: Some(accept),
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// The one-time code the next phone may pair with; `None` turns
    /// pairing off.
    pub fn set_code(&self, code: Option<String>) {
        self.shared.auth.lock().unwrap().code = code;
    }

    pub fn code(&self) -> Option<String> {
        self.shared.auth.lock().unwrap().code.clone()
    }

    pub fn devices(&self) -> Devices {
        self.shared.auth.lock().unwrap().devices.clone()
    }

    /// Forgets a paired phone, and drops it if it is connected (a
    /// phone of the same name, since its connection isn't tied to the
    /// token; another with that name simply reconnects).
    pub fn forget(&self, token: &str) -> bool {
        let name = {
            let mut auth = self.shared.auth.lock().unwrap();
            let name = auth.devices.find(token).map(|p| p.name.clone());
            auth.devices.forget(token);
            name
        };
        let Some(name) = name else {
            return false;
        };
        for c in self.shared.clients.lock().unwrap().iter() {
            if c.name == name {
                c.close();
            }
        }
        true
    }

    /// Drops a connected phone.
    pub fn disconnect(&self, id: u64) {
        for c in self.shared.clients.lock().unwrap().iter() {
            if c.id == id {
                c.close();
            }
        }
    }

    /// The phones connected now.
    pub fn connected(&self) -> Vec<(u64, String)> {
        self.shared
            .clients
            .lock()
            .unwrap()
            .iter()
            .map(|c| (c.id, c.name.clone()))
            .collect()
    }

    /// What has happened since the last call.
    pub fn events(&self) -> Vec<Event> {
        self.events.try_iter().collect()
    }

    /// Gives every phone the project's layout. A phone still reading
    /// an older layout gets this one instead of it.
    pub fn send_layout(&self, layout: &ToPhone) {
        self.send(layout, true);
    }

    /// Gives every phone the latest status; an unread older one is
    /// replaced.
    pub fn send_status(&self, status: &ToPhone) {
        self.send(status, false);
    }

    fn send(&self, msg: &ToPhone, layout: bool) {
        let clients = self.shared.clients.lock().unwrap();
        if clients.is_empty() {
            return;
        }
        let line = protocol::encode(msg);
        for c in clients.iter() {
            let mut o = c.outbox.lock().unwrap();
            if layout {
                o.layout = Some(line.clone());
            } else {
                o.status = Some(line.clone());
            }
            c.wake.notify_all();
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        for c in self.shared.clients.lock().unwrap().iter() {
            c.close();
        }
        // Wake the acceptor so it sees the flag.
        let _ = TcpStream::connect_timeout(
            &SocketAddr::from((Ipv4Addr::LOCALHOST, self.port)),
            Duration::from_millis(500),
        );
        if let Some(h) = self.accept.take() {
            let _ = h.join();
        }
    }
}

fn accept_loop(listener: TcpListener, shared: Arc<Shared>) {
    for conn in listener.incoming() {
        if shared.stop.load(Ordering::SeqCst) {
            break;
        }
        let Ok(stream) = conn else {
            continue;
        };
        let s = shared.clone();
        let _ = thread::Builder::new()
            .name("remote-phone".into())
            .spawn(move || serve(stream, s));
    }
}

/// One line, or `None` at the end of the stream. Fails if a line is
/// longer than [`MAX_LINE`].
fn read_line(r: &mut BufReader<TcpStream>) -> io::Result<Option<String>> {
    let mut line = Vec::new();
    loop {
        let buf = r.fill_buf()?;
        if buf.is_empty() {
            return Ok((!line.is_empty()).then(|| String::from_utf8_lossy(&line).into_owned()));
        }
        let (chunk, done) = match buf.iter().position(|&b| b == b'\n') {
            Some(i) => (&buf[..i], true),
            None => (buf, false),
        };
        if line.len() + chunk.len() > MAX_LINE {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "line too long"));
        }
        line.extend_from_slice(chunk);
        let used = chunk.len() + usize::from(done);
        r.consume(used);
        if done {
            return Ok(Some(String::from_utf8_lossy(&line).into_owned()));
        }
    }
}

fn write_line(stream: &mut TcpStream, msg: &ToPhone) -> io::Result<()> {
    stream.write_all(protocol::encode(msg).as_bytes())
}

/// Whether a hello may come in, and under what name. Spends the code if
/// that is what it used, and hands out a token.
fn admit(shared: &Shared, hello: &FromPhone) -> Result<(String, String), String> {
    let FromPhone::Hello {
        version,
        name,
        code,
        token,
    } = hello
    else {
        return Err("say hello first".into());
    };
    if *version != VERSION {
        return Err(format!(
            "the app speaks version {version} and the desktop version {VERSION}; update one of them"
        ));
    }
    let mut auth = shared.auth.lock().unwrap();
    if let Some(t) = token
        && let Some(p) = auth.devices.find(t)
    {
        return Ok((p.name.clone(), t.clone()));
    }
    if let (Some(c), Some(expected)) = (code, &auth.code)
        && same_secret(c, expected)
    {
        auth.code = None;
        let name = if name.trim().is_empty() {
            "Phone".to_string()
        } else {
            name.trim().chars().take(40).collect()
        };
        let paired = Paired {
            name: name.clone(),
            token: new_token(),
            since_ms: now_ms(),
        };
        auth.devices.phones.push(paired.clone());
        let token = paired.token.clone();
        let _ = shared.events.send(Event::Paired(paired));
        return Ok((name, token));
    }
    Err("not paired: scan the code on the desktop".into())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

fn serve(stream: TcpStream, shared: Arc<Shared>) {
    let _ = stream.set_nodelay(true);
    let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
    let _ = stream.set_read_timeout(Some(HELLO_TIMEOUT));
    let (Ok(mut writer), Ok(sock)) = (stream.try_clone(), stream.try_clone()) else {
        return;
    };
    let mut reader = BufReader::new(stream);
    let hello = match read_line(&mut reader) {
        Ok(Some(l)) => protocol::decode::<FromPhone>(&l),
        _ => return,
    };
    let (name, token) = match hello.and_then(|h| admit(&shared, &h)) {
        Ok(ok) => ok,
        Err(reason) => {
            let _ = write_line(&mut writer, &ToPhone::Denied { reason });
            let _ = writer.shutdown(Shutdown::Both);
            return;
        }
    };
    let desktop = shared.auth.lock().unwrap().desktop.clone();
    if write_line(
        &mut writer,
        &ToPhone::Welcome {
            version: VERSION,
            token,
            desktop,
        },
    )
    .is_err()
    {
        return;
    }
    let id = shared.next_id.fetch_add(1, Ordering::Relaxed);
    let client = Arc::new(Client {
        id,
        name: name.clone(),
        outbox: Mutex::new(Outbox::default()),
        wake: Condvar::new(),
        sock,
    });
    shared.clients.lock().unwrap().push(client.clone());
    let _ = shared.events.send(Event::Connected { id, name });

    let c = client.clone();
    let writer_thread = thread::spawn(move || {
        let mut guard = c.outbox.lock().unwrap();
        loop {
            while !guard.closed && guard.layout.is_none() && guard.status.is_none() {
                guard = c.wake.wait(guard).unwrap();
            }
            if guard.closed {
                break;
            }
            let pending: Vec<String> = guard
                .layout
                .take()
                .into_iter()
                .chain(guard.status.take())
                .collect();
            drop(guard);
            if pending
                .iter()
                .any(|l| writer.write_all(l.as_bytes()).is_err())
            {
                let _ = writer.shutdown(Shutdown::Both);
                break;
            }
            guard = c.outbox.lock().unwrap();
        }
    });

    let _ = reader.get_ref().set_read_timeout(Some(IDLE_TIMEOUT));
    while !shared.stop.load(Ordering::SeqCst) {
        if client.outbox.lock().unwrap().closed {
            break;
        }
        match read_line(&mut reader) {
            Ok(Some(l)) => match protocol::decode::<FromPhone>(&l) {
                Ok(FromPhone::Ping | FromPhone::Hello { .. }) => {}
                Ok(cmd) => {
                    let _ = shared.events.send(Event::Command { id, cmd });
                }
                // A message this desktop doesn't know is ignored.
                Err(_) => {}
            },
            _ => break,
        }
    }
    client.close();
    let _ = reader.get_ref().shutdown(Shutdown::Both);
    let _ = writer_thread.join();
    shared.clients.lock().unwrap().retain(|c| c.id != id);
    let _ = shared.events.send(Event::Disconnected { id });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;
    use std::time::Instant;

    struct Phone {
        reader: BufReader<TcpStream>,
        writer: TcpStream,
    }

    impl Phone {
        fn connect(port: u16) -> Phone {
            let s = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
            s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            Phone {
                writer: s.try_clone().unwrap(),
                reader: BufReader::new(s),
            }
        }

        fn send(&mut self, m: &FromPhone) {
            self.writer
                .write_all(protocol::encode(m).as_bytes())
                .unwrap();
        }

        fn read(&mut self) -> Option<ToPhone> {
            let mut l = String::new();
            match self.reader.read_line(&mut l) {
                Ok(0) | Err(_) => None,
                Ok(_) => Some(protocol::decode(&l).unwrap()),
            }
        }

        fn hello(port: u16, code: Option<&str>, token: Option<&str>) -> (Phone, ToPhone) {
            let mut p = Phone::connect(port);
            p.send(&FromPhone::Hello {
                version: VERSION,
                name: "Test phone".into(),
                code: code.map(String::from),
                token: token.map(String::from),
            });
            let reply = p.read().expect("a reply");
            (p, reply)
        }
    }

    /// Events polled so far and not yet asked for, since one poll can
    /// bring several.
    #[derive(Default)]
    struct Seen(Vec<Event>);

    impl Seen {
        fn wait_for(&mut self, server: &Server, mut f: impl FnMut(&Event) -> bool) -> Event {
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(5) {
                self.0.extend(server.events());
                if let Some(i) = self.0.iter().position(&mut f) {
                    return self.0.remove(i);
                }
                thread::sleep(Duration::from_millis(5));
            }
            panic!("event never came; seen {:?}", self.0);
        }
    }

    fn status() -> ToPhone {
        ToPhone::Status {
            playing: false,
            counting_in: false,
            recording: false,
            position: 0,
            bar: 1,
            beat: 1,
            seconds: 0.0,
            levels: vec![],
            master: [0.0, 0.0],
            can_undo_take: false,
        }
    }

    #[test]
    fn a_phone_pairs_with_the_code_and_comes_back_with_its_token() {
        let server = Server::start(0, "Desk", Devices::default()).unwrap();
        let mut seen = Seen::default();
        server.set_code(Some("CODE".into()));

        let (_, denied) = Phone::hello(server.port(), Some("WRONG"), None);
        assert!(matches!(denied, ToPhone::Denied { .. }));

        let (mut p, welcome) = Phone::hello(server.port(), Some("CODE"), None);
        let ToPhone::Welcome { token, desktop, .. } = welcome else {
            panic!("not welcome: {welcome:?}");
        };
        assert_eq!(desktop, "Desk");
        let paired = seen.wait_for(&server, |e| matches!(e, Event::Paired(_)));
        assert_eq!(
            paired,
            Event::Paired(server.devices().phones[0].clone()),
            "the app is told to save the new phone"
        );
        assert_eq!(server.devices().phones[0].token, token);
        assert_eq!(server.code(), None, "the code is spent");
        seen.wait_for(&server, |e| matches!(e, Event::Connected { .. }));
        assert_eq!(server.connected().len(), 1);

        // Commands come through, pings don't.
        p.send(&FromPhone::Ping);
        p.send(&FromPhone::Seek { frames: 10 });
        let e = seen.wait_for(&server, |e| matches!(e, Event::Command { .. }));
        assert!(matches!(
            e,
            Event::Command {
                cmd: FromPhone::Seek { frames: 10 },
                ..
            }
        ));

        // The status replaces an unread one; the layout gets through.
        server.send_status(&status());
        server.send_layout(&ToPhone::Layout {
            project: "Song".into(),
            sample_rate: 48_000,
            tempo_bpm: 120.0,
            beats_per_bar: 4,
            bar_frames: 96_000.0,
            end_frames: 0,
            count_in: false,
            metronome: false,
            looping: false,
            loop_bars: [1, 5],
            may_record: true,
            tracks: vec![],
            markers: vec![],
        });
        let mut got = [p.read().unwrap(), p.read().unwrap()];
        got.sort_by_key(|m| matches!(m, ToPhone::Status { .. }));
        assert!(matches!(got[0], ToPhone::Layout { .. }));
        assert!(matches!(got[1], ToPhone::Status { .. }));

        // The same code is no use twice; the token works.
        let (_, again) = Phone::hello(server.port(), Some("CODE"), None);
        assert!(matches!(again, ToPhone::Denied { .. }));
        let (p2, back) = Phone::hello(server.port(), None, Some(&token));
        assert!(matches!(back, ToPhone::Welcome { .. }));
        seen.wait_for(&server, |e| matches!(e, Event::Connected { id: 2, .. }));
        assert_eq!(server.connected().len(), 2);

        // Hanging up is noticed.
        drop(p2);
        seen.wait_for(&server, |e| matches!(e, Event::Disconnected { id: 2 }));
        assert_eq!(server.connected().len(), 1);

        // Forgetting the phone shuts its token out.
        assert!(server.forget(&token));
        let (_, out) = Phone::hello(server.port(), None, Some(&token));
        assert!(matches!(out, ToPhone::Denied { .. }));

        // Dropping the server hangs up on the phone.
        drop(server);
        assert!(p.read().is_none());
    }

    #[test]
    fn the_wrong_version_and_nonsense_are_turned_away() {
        let server = Server::start(0, "Desk", Devices::default()).unwrap();
        let mut p = Phone::connect(server.port());
        p.send(&FromPhone::Hello {
            version: VERSION + 1,
            name: "New".into(),
            code: None,
            token: None,
        });
        let ToPhone::Denied { reason } = p.read().unwrap() else {
            panic!("let in");
        };
        assert!(reason.contains("version"));
        assert!(p.read().is_none(), "and hung up on");

        let mut p = Phone::connect(server.port());
        p.writer.write_all(b"{\"type\":\"play\"}\n").unwrap();
        assert!(matches!(p.read(), Some(ToPhone::Denied { .. })));

        let mut p = Phone::connect(server.port());
        p.writer.write_all(b"hello?\n").unwrap();
        assert!(matches!(p.read(), Some(ToPhone::Denied { .. })));
        assert!(p.read().is_none());
        assert!(server.connected().is_empty());
    }

    #[test]
    fn the_preferred_port_is_used_when_free() {
        let a = Server::start(DEFAULT_PORT, "A", Devices::default()).unwrap();
        let b = Server::start(a.port(), "B", Devices::default()).unwrap();
        assert_ne!(a.port(), b.port(), "a taken port falls back to any");
    }
}
