// SPDX-License-Identifier: GPL-3.0-or-later
//! Teacher and student on one machine, over real UDP sockets.

use std::net::UdpSocket;
use std::time::Duration;

use collab_net::code::{Invite, Reply};
use collab_net::peer::{Host, join};
use spike_collab_doc::ProjectDoc;
use spike_collab_doc::loro_doc::LoroProject;

const T: Duration = Duration::from_secs(10);

fn sock() -> UdpSocket {
    UdpSocket::bind("127.0.0.1:0").unwrap()
}

fn host() -> Host {
    let s = sock();
    let a = s.local_addr().unwrap();
    Host::new(s, vec![a]).unwrap()
}

/// The invite as the student would receive it: through its text code.
fn pasted(h: &Host) -> Invite {
    Invite::decode(&h.invite().encode()).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn live_edits_flow_both_ways() {
    let h = host();
    let invite = pasted(&h);
    let s = sock();
    let reply = Reply::decode(
        &Reply {
            addrs: vec![s.local_addr().unwrap()],
        }
        .encode(),
    )
    .unwrap();
    let (teacher, student) = tokio::join!(h.accept(Some(&reply), T), join(s, &invite, T));
    let (mut teacher, mut student) = (teacher.unwrap(), student.unwrap());

    let mut tdoc = LoroProject::new(1);
    let mut sdoc = LoroProject::new(2);
    tdoc.add_track("gtr", "Guitar");
    let (a, b) = tokio::join!(teacher.catch_up(&mut tdoc), student.catch_up(&mut sdoc));
    a.unwrap();
    b.unwrap();
    assert_eq!(tdoc.project(), sdoc.project());

    // Teacher renames, student adds a clip, at the same time.
    tdoc.rename_track("gtr", "Lead guitar");
    sdoc.add_clip("gtr", "take1", 0, 48_000);
    let (a, b) = tokio::join!(
        async {
            teacher.push(&tdoc).await.unwrap();
            teacher.pull(&mut tdoc).await.unwrap();
        },
        async {
            student.push(&sdoc).await.unwrap();
            student.pull(&mut sdoc).await.unwrap();
        }
    );
    let _ = (a, b);
    assert_eq!(tdoc.project(), sdoc.project());
    let p = tdoc.project();
    assert_eq!(p.tracks["gtr"].name, "Lead guitar");
    assert_eq!(p.tracks["gtr"].clips.len(), 1);

    // Round trips through the whole stack.
    let (rtt, ok) = tokio::join!(student.ping(), teacher.answer_pings(1));
    ok.unwrap();
    assert!(rtt.unwrap() < Duration::from_secs(1));
}

#[tokio::test(flavor = "multi_thread")]
async fn offline_edits_merge_on_reconnect() {
    let mut tdoc = LoroProject::new(1);
    let mut sdoc = LoroProject::new(2);
    tdoc.add_track("bass", "Bass");

    // First lesson.
    {
        let h = host();
        let inv = pasted(&h);
        let (t, s) = tokio::join!(h.accept(None, T), join(sock(), &inv, T));
        let (mut t, mut s) = (t.unwrap(), s.unwrap());
        let (a, b) = tokio::join!(t.catch_up(&mut tdoc), s.catch_up(&mut sdoc));
        a.unwrap();
        b.unwrap();
        t.close().await;
        s.close().await;
    }
    // A week apart, offline.
    tdoc.set_gain("bass", 0.5);
    sdoc.add_clip("bass", "practice", 96_000, 48_000);
    sdoc.add_track("click", "Click");
    assert_ne!(tdoc.project(), sdoc.project());

    // Next lesson: a fresh invite, and the documents catch up.
    let h = host();
    let inv = pasted(&h);
    let (t, s) = tokio::join!(h.accept(None, T), join(sock(), &inv, T));
    let (mut t, mut s) = (t.unwrap(), s.unwrap());
    let (a, b) = tokio::join!(t.catch_up(&mut tdoc), s.catch_up(&mut sdoc));
    a.unwrap();
    b.unwrap();
    assert_eq!(tdoc.project(), sdoc.project());
    assert_eq!(tdoc.project().tracks.len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn wrong_teacher_is_refused() {
    let h = host();
    let mut inv = pasted(&h);
    inv.fingerprint[0] ^= 1;
    let (_t, s) = tokio::join!(
        h.accept(None, Duration::from_secs(2)),
        join(sock(), &inv, T)
    );
    assert!(s.is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn uninvited_student_is_refused() {
    let h = host();
    let mut inv = pasted(&h);
    inv.secret[0] ^= 1;
    let (t, s) = tokio::join!(h.accept(None, Duration::from_secs(2)), async {
        let mut link = join(sock(), &inv, T).await?;
        let mut doc = LoroProject::new(9);
        link.catch_up(&mut doc).await
    });
    assert!(t.is_err(), "teacher must not accept a wrong secret");
    assert!(s.is_err(), "the student's link must be closed");
}

#[tokio::test(flavor = "multi_thread")]
async fn dead_addresses_in_the_invite_are_skipped() {
    let s = sock();
    let good = s.local_addr().unwrap();
    // A port nobody listens on, listed first.
    let dead = sock().local_addr().unwrap();
    let h = Host::new(s, vec![dead, good]).unwrap();
    let inv = pasted(&h);
    let (t, st) = tokio::join!(h.accept(None, T), join(sock(), &inv, T));
    t.unwrap();
    assert_eq!(st.unwrap().remote(), good);
}
