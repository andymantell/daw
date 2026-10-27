// SPDX-License-Identifier: GPL-3.0-or-later
//! Invite and reply codes, swapped by hand (chat, email, read aloud), so
//! no server is needed to introduce the two machines.
//!
//! The teacher's invite carries where to reach them, the fingerprint of
//! their session certificate (so the student knows they reached the right
//! machine) and a session secret (so the teacher knows the student was
//! invited). The student's reply carries only where to reach them, so the
//! teacher can open a path through their own router towards the student.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use sha2::{Digest, Sha256};

const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ"; // Crockford base32

#[derive(Clone, Debug, PartialEq)]
pub struct Invite {
    /// First 16 bytes of the SHA-256 of the teacher's session certificate.
    pub fingerprint: [u8; 16],
    pub secret: [u8; 16],
    pub addrs: Vec<SocketAddr>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Reply {
    pub addrs: Vec<SocketAddr>,
}

fn put_addrs(out: &mut Vec<u8>, addrs: &[SocketAddr]) {
    out.push(addrs.len().min(255) as u8);
    for a in addrs.iter().take(255) {
        match a.ip() {
            IpAddr::V4(ip) => {
                out.push(4);
                out.extend_from_slice(&ip.octets());
            }
            IpAddr::V6(ip) => {
                out.push(6);
                out.extend_from_slice(&ip.octets());
            }
        }
        out.extend_from_slice(&a.port().to_be_bytes());
    }
}

fn take_addrs(b: &[u8]) -> Option<(Vec<SocketAddr>, &[u8])> {
    let (&n, mut b) = b.split_first()?;
    let mut v = Vec::new();
    for _ in 0..n {
        let (&fam, rest) = b.split_first()?;
        let (ip, rest) = match fam {
            4 => {
                let o: [u8; 4] = rest.get(..4)?.try_into().ok()?;
                (IpAddr::V4(Ipv4Addr::from(o)), &rest[4..])
            }
            6 => {
                let o: [u8; 16] = rest.get(..16)?.try_into().ok()?;
                (IpAddr::V6(Ipv6Addr::from(o)), &rest[16..])
            }
            _ => return None,
        };
        let port = u16::from_be_bytes(rest.get(..2)?.try_into().ok()?);
        v.push(SocketAddr::new(ip, port));
        b = &rest[2..];
    }
    Some((v, b))
}

/// Base32 with a 4-byte checksum, grouped in fives for reading aloud.
fn wrap(prefix: &str, payload: &[u8]) -> String {
    let mut data = payload.to_vec();
    data.extend_from_slice(&Sha256::digest(payload)[..4]);
    let mut bits = 0u32;
    let mut nbits = 0;
    let mut chars = Vec::new();
    for &byte in &data {
        bits = (bits << 8) | byte as u32;
        nbits += 8;
        while nbits >= 5 {
            nbits -= 5;
            chars.push(ALPHABET[((bits >> nbits) & 31) as usize]);
        }
    }
    if nbits > 0 {
        chars.push(ALPHABET[((bits << (5 - nbits)) & 31) as usize]);
    }
    let groups: Vec<String> = chars
        .chunks(5)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect();
    format!("{prefix}-{}", groups.join("-"))
}

fn unwrap(prefix: &str, code: &str) -> Result<Vec<u8>, String> {
    let code = code.trim().to_ascii_uppercase();
    let body = code
        .strip_prefix(prefix)
        .and_then(|c| c.strip_prefix('-'))
        .ok_or(format!("expected a code starting {prefix}-"))?;
    let mut bits = 0u32;
    let mut nbits = 0;
    let mut out = Vec::new();
    for c in body.chars().filter(|c| !matches!(c, '-' | ' ')) {
        // Crockford: O reads as 0, I and L as 1.
        let c = match c {
            'O' => '0',
            'I' | 'L' => '1',
            c => c,
        };
        let v = ALPHABET
            .iter()
            .position(|&a| a as char == c)
            .ok_or(format!("'{c}' is not part of a code"))?;
        bits = (bits << 5) | v as u32;
        nbits += 5;
        if nbits >= 8 {
            nbits -= 8;
            out.push((bits >> nbits) as u8);
        }
    }
    if out.len() < 4 {
        return Err("code too short".into());
    }
    let (payload, sum) = out.split_at(out.len() - 4);
    if Sha256::digest(payload)[..4] != *sum {
        return Err("code has a typo (checksum does not match)".into());
    }
    Ok(payload.to_vec())
}

impl Invite {
    pub fn encode(&self) -> String {
        let mut b = Vec::new();
        b.extend_from_slice(&self.fingerprint);
        b.extend_from_slice(&self.secret);
        put_addrs(&mut b, &self.addrs);
        wrap("DAWI", &b)
    }

    pub fn decode(code: &str) -> Result<Invite, String> {
        let b = unwrap("DAWI", code)?;
        let bad = || "invite code is damaged".to_string();
        let fingerprint = b.get(..16).ok_or_else(bad)?.try_into().unwrap();
        let secret = b.get(16..32).ok_or_else(bad)?.try_into().unwrap();
        let (addrs, rest) = take_addrs(&b[32..]).ok_or_else(bad)?;
        if !rest.is_empty() {
            return Err(bad());
        }
        Ok(Invite {
            fingerprint,
            secret,
            addrs,
        })
    }
}

impl Reply {
    pub fn encode(&self) -> String {
        let mut b = Vec::new();
        put_addrs(&mut b, &self.addrs);
        wrap("DAWR", &b)
    }

    pub fn decode(code: &str) -> Result<Reply, String> {
        let b = unwrap("DAWR", code)?;
        let (addrs, rest) = take_addrs(&b).ok_or("reply code is damaged")?;
        if !rest.is_empty() {
            return Err("reply code is damaged".into());
        }
        Ok(Reply { addrs })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invite() -> Invite {
        Invite {
            fingerprint: [7; 16],
            secret: [9; 16],
            addrs: vec![
                "192.168.1.20:50123".parse().unwrap(),
                "203.0.113.5:61000".parse().unwrap(),
                "[2001:db8::1]:50123".parse().unwrap(),
            ],
        }
    }

    #[test]
    fn round_trip_and_tolerant_reading() {
        let i = invite();
        let code = i.encode();
        assert_eq!(Invite::decode(&code).unwrap(), i);
        // Lower case, spaces instead of dashes, O for 0.
        let sloppy = code
            .to_lowercase()
            .replace('-', " ")
            .replacen("dawi ", "DAWI-", 1);
        assert_eq!(Invite::decode(&sloppy.replace('0', "o")).unwrap(), i);
        let r = Reply {
            addrs: vec!["10.0.0.2:4000".parse().unwrap()],
        };
        assert_eq!(Reply::decode(&r.encode()).unwrap(), r);
    }

    #[test]
    fn typos_are_caught() {
        let code = invite().encode();
        let mut chars: Vec<char> = code.chars().collect();
        let i = chars.len() - 8;
        chars[i] = if chars[i] == 'A' { 'B' } else { 'A' };
        let bad: String = chars.into_iter().collect();
        assert!(Invite::decode(&bad).is_err());
        assert!(Reply::decode(&code).is_err(), "an invite is not a reply");
    }

    #[test]
    fn invite_is_short_enough_to_paste() {
        let n = invite().encode().len();
        assert!(n < 160, "{n} characters");
    }
}
