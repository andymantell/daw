// SPDX-License-Identifier: GPL-3.0-or-later
//! A direct, encrypted link between teacher and student over QUIC, and
//! live sync of the project document across it.
//!
//! The teacher's machine listens; the student's machine connects. Both use
//! one UDP socket for everything (address discovery, punching through
//! routers, the connection itself), because routers map each socket to its
//! own public port.
//!
//! Trust: the teacher's certificate is a throwaway made for the session;
//! the student checks it against the fingerprint in the invite. The
//! student proves they were invited by sending the invite's secret as the
//! first message. A product version would use each member's long-term key.

use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;
use std::time::{Duration, Instant};

use quinn::{Connection, Endpoint, EndpointConfig, RecvStream, SendStream, TokioRuntime};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use sha2::{Digest, Sha256};
use spike_collab_doc::ProjectDoc;
use spike_collab_doc::loro_doc::LoroProject;

use crate::code::{Invite, Reply};

const ALPN: &[u8] = b"daw-sync/0";
const SERVER_NAME: &str = "daw-session";

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

fn random<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    rustls::crypto::ring::default_provider()
        .secure_random
        .fill(&mut b)
        .expect("system randomness");
    b
}

/// Accepts exactly one server certificate: the one in the invite.
#[derive(Debug)]
struct Pinned {
    fingerprint: [u8; 16],
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for Pinned {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        if Sha256::digest(end_entity.as_ref())[..16] == self.fingerprint {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General("not the invited teacher".into()))
        }
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

fn transport() -> Arc<quinn::TransportConfig> {
    let mut t = quinn::TransportConfig::default();
    t.keep_alive_interval(Some(Duration::from_secs(5)));
    t.max_idle_timeout(Some(Duration::from_secs(30).try_into().unwrap()));
    Arc::new(t)
}

pub type Result<T> = std::result::Result<T, String>;

/// The teacher's side before anyone has joined.
pub struct Host {
    endpoint: Endpoint,
    invite: Invite,
}

impl Host {
    /// `sock` is already bound (and may have been used for discovery);
    /// `addrs` are the addresses to put in the invite.
    pub fn new(sock: UdpSocket, addrs: Vec<SocketAddr>) -> Result<Host> {
        let ck = rcgen::generate_simple_self_signed(vec![SERVER_NAME.to_string()])
            .map_err(|e| e.to_string())?;
        let cert = ck.cert.der().clone();
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(ck.signing_key.serialize_der()));
        let fingerprint: [u8; 16] = Sha256::digest(cert.as_ref())[..16].try_into().unwrap();
        let mut tls = rustls::ServerConfig::builder_with_provider(provider())
            .with_protocol_versions(&[&rustls::version::TLS13])
            .map_err(|e| e.to_string())?
            .with_no_client_auth()
            .with_single_cert(vec![cert], key)
            .map_err(|e| e.to_string())?;
        tls.alpn_protocols = vec![ALPN.to_vec()];
        let crypto =
            quinn::crypto::rustls::QuicServerConfig::try_from(tls).map_err(|e| e.to_string())?;
        let mut server = quinn::ServerConfig::with_crypto(Arc::new(crypto));
        server.transport_config(transport());
        sock.set_nonblocking(true).map_err(|e| e.to_string())?;
        let endpoint = Endpoint::new(
            EndpointConfig::default(),
            Some(server),
            sock,
            Arc::new(TokioRuntime),
        )
        .map_err(|e| e.to_string())?;
        Ok(Host {
            endpoint,
            invite: Invite {
                fingerprint,
                secret: random(),
                addrs,
            },
        })
    }

    pub fn invite(&self) -> &Invite {
        &self.invite
    }

    /// Opens a path through the teacher's router towards the student. Can
    /// be called while [`Host::accept`] is already waiting.
    pub fn punch(&self, reply: &Reply) {
        punch(&self.endpoint, &reply.addrs);
    }

    /// Waits for the invited student. If their reply code is known, also
    /// sends packets towards them so the teacher's router lets them in.
    pub async fn accept(&self, reply: Option<&Reply>, timeout: Duration) -> Result<Link> {
        if let Some(r) = reply {
            self.punch(r);
        }
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let incoming = tokio::time::timeout(left, self.endpoint.accept())
                .await
                .map_err(|_| "nobody joined in time".to_string())?
                .ok_or("endpoint closed")?;
            let Ok(conn) = incoming.await else { continue };
            let Ok((send, mut recv)) = conn.accept_bi().await else {
                continue;
            };
            let mut secret = [0u8; 16];
            if recv.read_exact(&mut secret).await.is_err() || secret != self.invite.secret {
                // Not our student: drop them and keep waiting.
                conn.close(1u32.into(), b"not invited");
                continue;
            }
            return Ok(Link::new(conn, send, recv));
        }
    }
}

/// Sends a few connection attempts at the other side's addresses. They
/// are expected to fail; their job is to make our router expect replies
/// from those addresses.
fn punch(endpoint: &Endpoint, addrs: &[SocketAddr]) {
    let mut client = endpoint.clone();
    let tls = rustls::ClientConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .expect("TLS 1.3")
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(Pinned {
            fingerprint: [0; 16],
            provider: provider(),
        }))
        .with_no_client_auth();
    if let Ok(c) = quinn::crypto::rustls::QuicClientConfig::try_from(tls) {
        client.set_default_client_config(quinn::ClientConfig::new(Arc::new(c)));
    }
    for &a in addrs {
        if let Ok(connecting) = client.connect(a, SERVER_NAME) {
            tokio::spawn(async move {
                let _ = tokio::time::timeout(Duration::from_secs(5), connecting).await;
            });
        }
    }
}

/// The student's side: connects to the first teacher address that answers.
pub async fn join(sock: UdpSocket, invite: &Invite, timeout: Duration) -> Result<Link> {
    let tls = rustls::ClientConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| e.to_string())?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(Pinned {
            fingerprint: invite.fingerprint,
            provider: provider(),
        }))
        .with_no_client_auth();
    let mut tls = tls;
    tls.alpn_protocols = vec![ALPN.to_vec()];
    let crypto =
        quinn::crypto::rustls::QuicClientConfig::try_from(tls).map_err(|e| e.to_string())?;
    let mut client = quinn::ClientConfig::new(Arc::new(crypto));
    client.transport_config(transport());
    sock.set_nonblocking(true).map_err(|e| e.to_string())?;
    let mut endpoint = Endpoint::new(
        EndpointConfig::default(),
        None,
        sock,
        Arc::new(TokioRuntime),
    )
    .map_err(|e| e.to_string())?;
    endpoint.set_default_client_config(client);

    // Try every address at once, retrying until the deadline (the teacher
    // may open their side late); keep the first that completes.
    let deadline = Instant::now() + timeout;
    let (tx, mut rx) = tokio::sync::mpsc::channel(invite.addrs.len().max(1));
    for &a in &invite.addrs {
        let tx = tx.clone();
        let ep = endpoint.clone();
        tokio::spawn(async move {
            let r = loop {
                let r = match ep.connect(a, SERVER_NAME) {
                    Ok(c) => c.await.map_err(|e| format!("{a}: {e}")),
                    Err(e) => Err(format!("{a}: {e}")),
                };
                let retry = matches!(&r, Err(e) if e.contains("timed out"));
                if !retry || Instant::now() + Duration::from_secs(1) >= deadline {
                    break r;
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            };
            let _ = tx.send(r).await;
        });
    }
    drop(tx);
    let mut errors = Vec::new();
    let conn = loop {
        match tokio::time::timeout(timeout, rx.recv()).await {
            Ok(Some(Ok(c))) => break c,
            Ok(Some(Err(e))) => errors.push(e),
            Ok(None) => {
                return Err(format!(
                    "could not reach the teacher: {}",
                    errors.join("; ")
                ));
            }
            Err(_) => return Err("timed out reaching the teacher".into()),
        }
    };
    let (mut send, recv) = conn.open_bi().await.map_err(|e| e.to_string())?;
    send.write_all(&invite.secret)
        .await
        .map_err(|e| e.to_string())?;
    Ok(Link::new(conn, send, recv))
}

// ---------------------------------------------------------------------------
// Sync over the link.

const MSG_VERSION: u8 = 1;
const MSG_UPDATES: u8 = 2;
const MSG_PING: u8 = 3;
const MSG_PONG: u8 = 4;
const MSG_BYE: u8 = 5;

pub struct Link {
    pub conn: Connection,
    send: SendStream,
    recv: RecvStream,
    /// The peer's document version as last told to us.
    peer_version: Option<Vec<u8>>,
}

impl Link {
    fn new(conn: Connection, send: SendStream, recv: RecvStream) -> Link {
        Link {
            conn,
            send,
            recv,
            peer_version: None,
        }
    }

    pub fn remote(&self) -> SocketAddr {
        self.conn.remote_address()
    }

    async fn write(&mut self, kind: u8, body: &[u8]) -> Result<()> {
        let mut head = [0u8; 5];
        head[..4].copy_from_slice(&(body.len() as u32 + 1).to_be_bytes());
        head[4] = kind;
        self.send
            .write_all(&head)
            .await
            .map_err(|e| e.to_string())?;
        self.send.write_all(body).await.map_err(|e| e.to_string())
    }

    async fn read(&mut self) -> Result<(u8, Vec<u8>)> {
        let mut len = [0u8; 4];
        self.recv
            .read_exact(&mut len)
            .await
            .map_err(|e| e.to_string())?;
        let len = u32::from_be_bytes(len) as usize;
        if len == 0 || len > 64 << 20 {
            return Err("bad message length".into());
        }
        let mut b = vec![0u8; len];
        self.recv
            .read_exact(&mut b)
            .await
            .map_err(|e| e.to_string())?;
        let kind = b.remove(0);
        Ok((kind, b))
    }

    /// First exchange after connecting (or reconnecting): each side says
    /// what it has, then sends what the other is missing. Brings two
    /// documents that were edited offline back together.
    pub async fn catch_up(&mut self, doc: &mut LoroProject) -> Result<()> {
        self.write(MSG_VERSION, &doc.version()).await?;
        let (k, v) = self.read().await?;
        if k != MSG_VERSION {
            return Err("expected the peer's version first".into());
        }
        let missing = doc.updates_since(&v);
        self.write(MSG_UPDATES, &missing).await?;
        let (k, u) = self.read().await?;
        if k != MSG_UPDATES {
            return Err("expected updates".into());
        }
        doc.apply(&u);
        self.peer_version = Some(doc.version());
        Ok(())
    }

    /// Sends local edits made since the last send.
    pub async fn push(&mut self, doc: &LoroProject) -> Result<usize> {
        let since = self.peer_version.clone().unwrap_or_default();
        let u = doc.updates_since(&since);
        self.write(MSG_UPDATES, &u).await?;
        self.peer_version = Some(doc.version());
        Ok(u.len())
    }

    /// Receives one batch of the peer's edits, answering pings meanwhile.
    pub async fn pull(&mut self, doc: &mut LoroProject) -> Result<()> {
        loop {
            match self.read().await? {
                (MSG_UPDATES, u) => {
                    doc.apply(&u);
                    return Ok(());
                }
                (MSG_PING, b) => self.write(MSG_PONG, &b).await?,
                (k, _) => return Err(format!("unexpected message {k}")),
            }
        }
    }

    /// Round-trip time of a small message through the whole stack.
    pub async fn ping(&mut self) -> Result<Duration> {
        let t = Instant::now();
        self.write(MSG_PING, b"p").await?;
        loop {
            match self.read().await? {
                (MSG_PONG, _) => return Ok(t.elapsed()),
                (MSG_PING, b) => self.write(MSG_PONG, &b).await?,
                (k, _) => return Err(format!("unexpected message {k}")),
            }
        }
    }

    /// Answers the peer's pings until they send something else.
    pub async fn answer_pings(&mut self, n: usize) -> Result<()> {
        for _ in 0..n {
            match self.read().await? {
                (MSG_PING, b) => self.write(MSG_PONG, &b).await?,
                (k, _) => return Err(format!("unexpected message {k}")),
            }
        }
        Ok(())
    }

    /// Says goodbye and waits briefly for the peer's goodbye, so nothing
    /// still in flight is lost when the connection closes.
    pub async fn close(mut self) {
        if self.write(MSG_BYE, &[]).await.is_ok() {
            let _ = tokio::time::timeout(Duration::from_secs(3), async {
                while let Ok((k, _)) = self.read().await {
                    if k == MSG_BYE {
                        break;
                    }
                }
            })
            .await;
        }
        let _ = self.send.finish();
        self.conn.close(0u32.into(), b"bye");
    }
}
