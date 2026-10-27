// SPDX-License-Identifier: GPL-3.0-or-later
//! The one STUN exchange we need (RFC 8489, previously RFC 5389): a
//! binding request, and the XOR-MAPPED-ADDRESS from the response, which is
//! our address as the internet sees it.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

const MAGIC: u32 = 0x2112_A442;
const BINDING_REQUEST: u16 = 0x0001;
const BINDING_SUCCESS: u16 = 0x0101;
const MAPPED_ADDRESS: u16 = 0x0001;
const XOR_MAPPED_ADDRESS: u16 = 0x0020;

pub fn request(txid: [u8; 12]) -> [u8; 20] {
    let mut m = [0u8; 20];
    m[0..2].copy_from_slice(&BINDING_REQUEST.to_be_bytes());
    // Length 0: no attributes.
    m[4..8].copy_from_slice(&MAGIC.to_be_bytes());
    m[8..20].copy_from_slice(&txid);
    m
}

/// The mapped address in a binding success response for `txid`.
pub fn parse_response(msg: &[u8], txid: [u8; 12]) -> Option<SocketAddr> {
    if msg.len() < 20
        || u16::from_be_bytes([msg[0], msg[1]]) != BINDING_SUCCESS
        || u32::from_be_bytes([msg[4], msg[5], msg[6], msg[7]]) != MAGIC
        || msg[8..20] != txid
    {
        return None;
    }
    let len = u16::from_be_bytes([msg[2], msg[3]]) as usize;
    let body = msg.get(20..20 + len)?;
    let mut plain = None;
    let mut i = 0;
    while i + 4 <= body.len() {
        let kind = u16::from_be_bytes([body[i], body[i + 1]]);
        let alen = u16::from_be_bytes([body[i + 2], body[i + 3]]) as usize;
        let val = body.get(i + 4..i + 4 + alen)?;
        match kind {
            XOR_MAPPED_ADDRESS => return decode_addr(val, Some(&msg[4..20])),
            MAPPED_ADDRESS => plain = decode_addr(val, None),
            _ => {}
        }
        // Attributes are padded to four bytes.
        i += 4 + alen.div_ceil(4) * 4;
    }
    plain
}

/// `xor` is the magic cookie followed by the transaction id.
fn decode_addr(v: &[u8], xor: Option<&[u8]>) -> Option<SocketAddr> {
    let x = |i: usize| xor.map_or(0, |k| k[i]);
    let port = u16::from_be_bytes([v.get(2)? ^ x(0), v.get(3)? ^ x(1)]);
    let ip = match v.get(1)? {
        0x01 => {
            let b: Vec<u8> = (0..4).map(|i| v[4 + i] ^ x(i)).collect();
            IpAddr::V4(Ipv4Addr::new(b[0], b[1], b[2], b[3]))
        }
        0x02 if v.len() >= 20 => {
            let mut b = [0u8; 16];
            for (i, o) in b.iter_mut().enumerate() {
                *o = v[4 + i] ^ x(i);
            }
            IpAddr::V6(Ipv6Addr::from(b))
        }
        _ => return None,
    };
    Some(SocketAddr::new(ip, port))
}

fn txid() -> [u8; 12] {
    use std::hash::{BuildHasher, Hasher, RandomState};
    let mut t = [0u8; 12];
    for chunk in t.chunks_mut(8) {
        let mut h = RandomState::new().build_hasher();
        h.write_u128(Instant::now().elapsed().as_nanos());
        let v = h.finish().to_le_bytes();
        chunk.copy_from_slice(&v[..chunk.len()]);
    }
    t
}

/// Asks one STUN server for our mapped address, from `sock`, retrying a
/// few times as UDP may drop packets.
pub fn query(
    sock: &UdpSocket,
    server: SocketAddr,
    timeout: Duration,
) -> std::io::Result<SocketAddr> {
    let id = txid();
    let req = request(id);
    let deadline = Instant::now() + timeout;
    let mut buf = [0u8; 1500];
    let old = sock.read_timeout()?;
    let result = loop {
        let now = Instant::now();
        if now >= deadline {
            break Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "no STUN answer",
            ));
        }
        sock.send_to(&req, server)?;
        sock.set_read_timeout(Some((deadline - now).min(Duration::from_millis(500))))?;
        match sock.recv_from(&mut buf) {
            Ok((n, from)) if from == server => {
                if let Some(a) = parse_response(&buf[..n], id) {
                    break Ok(a);
                }
            }
            Ok(_) => {}
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(e) => break Err(e),
        }
    };
    sock.set_read_timeout(old)?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        let s: String = s.split_whitespace().collect();
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    const TX: [u8; 12] = [
        0xb7, 0xe7, 0xa7, 0x01, 0xbc, 0x34, 0xd6, 0x86, 0xfa, 0x87, 0xdf, 0xae,
    ];

    /// RFC 5769 section 2.2, sample IPv4 response.
    #[test]
    fn rfc5769_ipv4() {
        let m = hex(
            "0101003c 2112a442 b7e7a701 bc34d686 fa87dfae 8022000b 74657374 20766563 746f7220
             00200008 0001a147 e112a643 00080014 2b91f599 fd9e90c3 8c7489f9 2af9ba53 f06be7d7
             80280004 c07d4c96",
        );
        assert_eq!(
            parse_response(&m, TX),
            Some("192.0.2.1:32853".parse().unwrap())
        );
    }

    /// RFC 5769 section 2.3, sample IPv6 response.
    #[test]
    fn rfc5769_ipv6() {
        let m = hex(
            "01010048 2112a442 b7e7a701 bc34d686 fa87dfae 8022000b 74657374 20766563 746f7220
             00200014 0002a147 0113a9fa a5d3f179 bc25f4b5 bed2b9d9 00080014 a382954e 4be67bf1
             1784c97c 8292c275 bfe3ed41 80280004 c8fb0b4c",
        );
        assert_eq!(
            parse_response(&m, TX),
            Some(
                "[2001:db8:1234:5678:11:2233:4455:6677]:32853"
                    .parse()
                    .unwrap()
            )
        );
    }

    #[test]
    fn wrong_transaction_is_ignored() {
        let mut m = request(TX).to_vec();
        m[1] = 0x01;
        assert_eq!(parse_response(&m, [0; 12]), None);
    }

    #[test]
    fn query_against_a_local_fake_server() {
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        let addr = server.local_addr().unwrap();
        std::thread::spawn(move || {
            let mut buf = [0u8; 100];
            let (_, from) = server.recv_from(&mut buf).unwrap();
            // Answer with the sender's address, XOR-encoded.
            let mut r = vec![0x01, 0x01, 0x00, 0x0c];
            r.extend_from_slice(&buf[4..20]);
            r.extend_from_slice(&[0x00, 0x20, 0x00, 0x08, 0x00, 0x01]);
            let port = from.port() ^ 0x2112;
            r.extend_from_slice(&port.to_be_bytes());
            let IpAddr::V4(ip) = from.ip() else {
                unreachable!()
            };
            for (i, b) in ip.octets().iter().enumerate() {
                r.push(b ^ MAGIC.to_be_bytes()[i]);
            }
            server.send_to(&r, from).unwrap();
        });
        let client = UdpSocket::bind("127.0.0.1:0").unwrap();
        let me = client.local_addr().unwrap();
        assert_eq!(query(&client, addr, Duration::from_secs(2)).unwrap(), me);
    }
}
