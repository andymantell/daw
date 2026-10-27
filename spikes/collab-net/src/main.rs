// SPDX-License-Identifier: GPL-3.0-or-later
//! Spike 0.5, second half: can a teacher and a student connect directly,
//! with no server we operate?
//!
//!   collab-net probe          what this network allows; run it at home,
//!                             on a phone hotspot and at school
//!   collab-net host           teacher: prints an invite code
//!   collab-net join <code>    student: prints a reply code, then connects
//!
//! Address discovery asks public STUN servers (run by others, free to
//! use) what our address looks like from outside. No project data goes
//! to them.

use std::fmt::Write as _;
use std::io::{BufRead, Write as _};
use std::net::{IpAddr, SocketAddr, ToSocketAddrs, UdpSocket};
use std::time::{Duration, Instant};

use collab_net::code::{Invite, Reply};
use collab_net::peer::{Host, Link, join};
use collab_net::stun;
use spike_collab_doc::ProjectDoc;
use spike_collab_doc::loro_doc::LoroProject;

const STUN_SERVERS: [&str; 3] = [
    "stun.l.google.com:19302",
    "stun1.l.google.com:19302",
    "stun.cloudflare.com:3478",
];

/// The address this machine uses to reach the internet. No packet is sent.
fn primary_ip() -> Option<IpAddr> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    // Connecting a UDP socket only picks a route; nothing is sent.
    s.connect("8.8.8.8:53").ok()?;
    s.local_addr().ok().map(|a| a.ip())
}

fn resolve(server: &str) -> Option<SocketAddr> {
    server.to_socket_addrs().ok()?.find(|a| a.is_ipv4())
}

/// Mapped addresses from each STUN server, asked from the same socket.
fn discover(sock: &UdpSocket) -> Vec<(String, Result<SocketAddr, String>)> {
    STUN_SERVERS
        .iter()
        .map(|s| {
            let r = match resolve(s) {
                None => Err("could not resolve".to_string()),
                Some(a) => stun::query(sock, a, Duration::from_secs(3)).map_err(|e| e.to_string()),
            };
            (s.to_string(), r)
        })
        .collect()
}

fn candidates(sock: &UdpSocket) -> (Vec<SocketAddr>, String) {
    let port = sock.local_addr().map(|a| a.port()).unwrap_or(0);
    let mut addrs = Vec::new();
    if let Some(ip) = primary_ip() {
        addrs.push(SocketAddr::new(ip, port));
    }
    let found = discover(sock);
    let mut note = String::new();
    for (server, r) in &found {
        let _ = writeln!(
            note,
            "  {server}: {}",
            match r {
                Ok(a) => a.to_string(),
                Err(e) => format!("no answer ({e})"),
            }
        );
    }
    if let Some(Ok(a)) = found.iter().map(|(_, r)| r.clone()).find(|r| r.is_ok())
        && !addrs.contains(&a)
    {
        addrs.push(a);
    }
    (addrs, note)
}

fn probe() -> String {
    let mut r = String::new();
    let _ = writeln!(r, "Network probe (spike 0.5)");
    let sock = match UdpSocket::bind("0.0.0.0:0") {
        Ok(s) => s,
        Err(e) => return format!("could not open a UDP socket: {e}\n"),
    };
    let local = primary_ip();
    let port = sock.local_addr().map(|a| a.port()).unwrap_or(0);
    let _ = writeln!(
        r,
        "  this machine: {}:{port}",
        local.map_or("unknown".into(), |i| i.to_string())
    );
    let found = discover(&sock);
    let mapped: Vec<SocketAddr> = found.iter().filter_map(|(_, x)| x.clone().ok()).collect();
    for (server, x) in &found {
        let _ = writeln!(
            r,
            "  seen by {server}: {}",
            match x {
                Ok(a) => a.to_string(),
                Err(e) => format!("no answer ({e})"),
            }
        );
    }
    let verdict = if mapped.is_empty() {
        "UDP BLOCKED: no STUN server answered. Direct connections will not work on this network; use project bundles or a shared folder."
    } else if mapped
        .iter()
        .any(|a| Some(a.ip()) == local && a.port() == port)
    {
        "NO ROUTER IN THE WAY: this machine has a public address. Others can usually connect directly, unless a firewall blocks them."
    } else if mapped.len() >= 2 && mapped.iter().all(|a| *a == mapped[0]) {
        "FRIENDLY ROUTER: the router keeps the same outside address and port for every destination. Direct connections should work, if the other side's network is similar or friendlier."
    } else if mapped.len() >= 2 {
        "STRICT ROUTER: the outside port changes per destination. Direct connections will probably fail unless the other side has a public address; bundles, a shared folder or a relay are the fallbacks."
    } else {
        "ONE ANSWER ONLY: not enough answers to judge the router."
    };
    let _ = writeln!(r, "  verdict: {verdict}");
    r
}

fn ask(prompt: &str) -> String {
    print!("{prompt}");
    let _ = std::io::stdout().flush();
    let mut s = String::new();
    let _ = std::io::stdin().lock().read_line(&mut s);
    s.trim().to_string()
}

/// The same short session on both sides: catch up, measure round trips,
/// each make an edit, and compare.
async fn session(link: &mut Link, doc: &mut LoroProject, teacher: bool) -> Result<String, String> {
    let mut r = String::new();
    let t = Instant::now();
    link.catch_up(doc).await?;
    let _ = writeln!(
        r,
        "  caught up in {:.0} ms",
        t.elapsed().as_secs_f64() * 1e3
    );
    if teacher {
        link.answer_pings(20).await?;
    } else {
        let mut rtts = Vec::new();
        for _ in 0..20 {
            rtts.push(link.ping().await?.as_secs_f64() * 1e3);
        }
        rtts.sort_by(f64::total_cmp);
        let _ = writeln!(
            r,
            "  round trip: median {:.1} ms, worst {:.1} ms (20 pings)",
            rtts[10], rtts[19]
        );
    }
    let me = if teacher { "teacher" } else { "student" };
    doc.add_track(me, &format!("Added by the {me}"));
    if teacher {
        link.push(doc).await?;
        link.pull(doc).await?;
    } else {
        link.pull(doc).await?;
        link.push(doc).await?;
    }
    let p = doc.project();
    let _ = writeln!(r, "  tracks now: {:?}", p.order);
    let ok = p.tracks.contains_key("teacher") && p.tracks.contains_key("student");
    let _ = writeln!(
        r,
        "  verdict: {}",
        if ok {
            "PASS: both edits arrived on both sides"
        } else {
            "FAIL: an edit is missing"
        }
    );
    Ok(r)
}

async fn host() -> String {
    let mut r = String::from("Teacher side (spike 0.5)\n");
    let sock = match UdpSocket::bind("0.0.0.0:0") {
        Ok(s) => s,
        Err(e) => return format!("could not open a UDP socket: {e}\n"),
    };
    let (addrs, note) = candidates(&sock);
    r.push_str(&note);
    let h = match Host::new(sock, addrs.clone()) {
        Ok(h) => h,
        Err(e) => return format!("{r}  failed: {e}\n"),
    };
    println!(
        "{r}\nSend this invite code to the student:\n\n  {}\n",
        h.invite().encode()
    );
    println!("Waiting up to 2 minutes for the student. When their reply code arrives,");
    println!("paste it here and press Enter (it helps get through your router).");
    // Read reply codes on a plain thread while waiting for the student.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let t = Instant::now();
    let accepted = {
        let accept = h.accept(None, Duration::from_secs(120));
        tokio::pin!(accept);
        loop {
            tokio::select! {
                res = &mut accept => break res,
                Some(line) = rx.recv() => {
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    match Reply::decode(line) {
                        Ok(reply) => {
                            println!("  got the reply code; opening a path to {:?}", reply.addrs);
                            h.punch(&reply);
                        }
                        Err(e) => println!("  that reply code did not read: {e}"),
                    }
                }
            }
        }
    };
    match accepted {
        Err(e) => {
            let _ = writeln!(r, "  no connection: {e}");
        }
        Ok(mut link) => {
            let _ = writeln!(
                r,
                "  connected to {} after {:.1} s",
                link.remote(),
                t.elapsed().as_secs_f64()
            );
            let mut doc = LoroProject::new(1);
            match session(&mut link, &mut doc, true).await {
                Ok(s) => r.push_str(&s),
                Err(e) => {
                    let _ = writeln!(r, "  session failed: {e}");
                }
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
            link.close().await;
        }
    }
    r
}

async fn join_cmd(code: &str) -> String {
    let mut r = String::from("Student side (spike 0.5)\n");
    let invite = match Invite::decode(code) {
        Ok(i) => i,
        Err(e) => return format!("{r}  {e}\n"),
    };
    let sock = match UdpSocket::bind("0.0.0.0:0") {
        Ok(s) => s,
        Err(e) => return format!("could not open a UDP socket: {e}\n"),
    };
    let (addrs, note) = candidates(&sock);
    r.push_str(&note);
    let _ = writeln!(r, "  teacher's addresses: {:?}", invite.addrs);
    println!(
        "{r}\nSend this reply code back to the teacher:\n\n  {}\n\nConnecting (up to 2 minutes)...",
        Reply { addrs }.encode()
    );
    let t = Instant::now();
    match join(sock, &invite, Duration::from_secs(120)).await {
        Err(e) => {
            let _ = writeln!(r, "  no connection: {e}");
        }
        Ok(mut link) => {
            let _ = writeln!(
                r,
                "  connected to {} after {:.1} s",
                link.remote(),
                t.elapsed().as_secs_f64()
            );
            let mut doc = LoroProject::new(2);
            match session(&mut link, &mut doc, false).await {
                Ok(s) => r.push_str(&s),
                Err(e) => {
                    let _ = writeln!(r, "  session failed: {e}");
                }
            }
            link.close().await;
        }
    }
    r
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (report, file) = match args.first().map(String::as_str) {
        Some("probe") => (probe(), "net-probe-report.txt"),
        Some("host") => (host().await, "net-host-report.txt"),
        Some("join") => {
            let code = match args.get(1) {
                Some(c) => c.clone(),
                None => ask("Paste the teacher's invite code: "),
            };
            (join_cmd(&code).await, "net-join-report.txt")
        }
        _ => {
            eprintln!("usage: collab-net probe | host | join [invite code]");
            std::process::exit(2);
        }
    };
    print!("{report}");
    if std::fs::write(file, &report).is_ok() {
        println!("Saved to {file}");
    }
    if cfg!(windows) {
        ask("Press Enter to close.");
    }
}
