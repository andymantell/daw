// SPDX-License-Identifier: GPL-3.0-or-later
//! Pairing: a one-time code shown on the desktop as a QR image, and the
//! tokens paired phones keep to come back with.
//!
//! The code only ever travels by the screen, so another device on the
//! same network can't learn it. A phone that presents it is given a
//! token of its own and the code is spent; the token works until the
//! phone is forgotten in the desktop's settings.

use serde::{Deserialize, Serialize};

/// The scheme in the QR image: `daw-remote://HOST:PORT/CODE`.
pub const SCHEME: &str = "daw-remote";

/// The text in the QR image, also shown beside it to type by hand.
pub fn qr_text(host: &str, port: u16, code: &str) -> String {
    let host = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_string()
    };
    format!("{SCHEME}://{host}:{port}/{code}")
}

/// Takes a QR text apart: (host, port, code).
pub fn parse_qr_text(text: &str) -> Option<(String, u16, String)> {
    let rest = text.trim().strip_prefix(SCHEME)?.strip_prefix("://")?;
    let (addr, code) = rest.split_once('/')?;
    let (host, port) = if let Some(r) = addr.strip_prefix('[') {
        let (h, p) = r.split_once("]:")?;
        (h.to_string(), p)
    } else {
        let (h, p) = addr.rsplit_once(':')?;
        (h.to_string(), p)
    };
    if host.is_empty() || code.is_empty() {
        return None;
    }
    Some((host, port.parse().ok()?, code.to_string()))
}

const ALPHABET: &[u8; 32] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

/// `n` random characters from an alphabet without look-alikes (no I, O,
/// 0 or 1), for codes read off a screen.
fn random_chars(n: usize) -> String {
    let mut bytes = vec![0u8; n];
    // Without random numbers the pairing code would be guessable, so
    // refusing is right; this only fails on a broken system.
    getrandom::fill(&mut bytes).expect("the system gives random numbers");
    bytes
        .iter()
        .map(|b| ALPHABET[(b % 32) as usize] as char)
        .collect()
}

/// A fresh one-time pairing code: 16 characters, 80 bits.
pub fn new_code() -> String {
    random_chars(16)
}

/// A fresh token for a paired phone: 48 characters, 240 bits.
pub fn new_token() -> String {
    random_chars(48)
}

/// Compares two secrets in time that doesn't depend on where they
/// differ.
pub fn same_secret(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

/// A phone that has paired.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Paired {
    pub name: String,
    pub token: String,
    /// When it paired, in milliseconds since 1970.
    #[serde(default)]
    pub since_ms: i64,
}

/// The paired phones, as kept in the settings folder.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct Devices {
    pub phones: Vec<Paired>,
}

impl Devices {
    pub fn from_text(text: &str) -> Devices {
        serde_json::from_str(text).unwrap_or_default()
    }

    pub fn to_text(&self) -> String {
        serde_json::to_string_pretty(self).expect("devices always serialise")
    }

    /// The phone a token belongs to.
    pub fn find(&self, token: &str) -> Option<&Paired> {
        self.phones.iter().find(|p| same_secret(&p.token, token))
    }

    pub fn forget(&mut self, token: &str) -> bool {
        let before = self.phones.len();
        self.phones.retain(|p| !same_secret(&p.token, token));
        self.phones.len() != before
    }
}

/// A QR image as a square of dark cells, for the UI to draw.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QrImage {
    pub size: usize,
    /// `size * size` cells, row by row; true is dark.
    pub dark: Vec<bool>,
}

/// The QR image for a text, or `None` if it is too long for one.
pub fn qr_image(text: &str) -> Option<QrImage> {
    let code = qrcode::QrCode::with_error_correction_level(text, qrcode::EcLevel::M).ok()?;
    let size = code.width();
    let dark = code
        .to_colors()
        .into_iter()
        .map(|c| c == qrcode::Color::Dark)
        .collect();
    Some(QrImage { size, dark })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qr_text_goes_there_and_back() {
        let t = qr_text("192.168.1.20", 7345, "ABCD2345EFGH6789");
        assert_eq!(t, "daw-remote://192.168.1.20:7345/ABCD2345EFGH6789");
        assert_eq!(
            parse_qr_text(&t),
            Some(("192.168.1.20".into(), 7345, "ABCD2345EFGH6789".into()))
        );
        let v6 = qr_text("fe80::1", 7345, "CODE");
        assert_eq!(
            parse_qr_text(&v6),
            Some(("fe80::1".into(), 7345, "CODE".into()))
        );
        assert_eq!(parse_qr_text("https://example.com/x"), None);
        assert_eq!(parse_qr_text("daw-remote://host:notaport/CODE"), None);
        assert_eq!(parse_qr_text("daw-remote://host:1/"), None);
    }

    #[test]
    fn codes_are_fresh_and_readable() {
        let a = new_code();
        let b = new_code();
        assert_eq!(a.len(), 16);
        assert_ne!(a, b);
        assert!(a.chars().all(|c| ALPHABET.contains(&(c as u8))));
        assert!(!a.contains(['I', 'O', '0', '1']));
        assert_eq!(new_token().len(), 48);
    }

    #[test]
    fn secrets_compare_whole() {
        assert!(same_secret("ABC", "ABC"));
        assert!(!same_secret("ABC", "ABD"));
        assert!(!same_secret("ABC", "AB"));
    }

    #[test]
    fn devices_are_kept_as_text() {
        let mut d = Devices::default();
        d.phones.push(Paired {
            name: "Phone".into(),
            token: "T1".into(),
            since_ms: 5,
        });
        let back = Devices::from_text(&d.to_text());
        assert_eq!(back, d);
        assert_eq!(back.find("T1").map(|p| p.name.as_str()), Some("Phone"));
        assert!(back.find("T2").is_none());
        assert_eq!(Devices::from_text("garbage"), Devices::default());
        assert!(d.forget("T1"));
        assert!(!d.forget("T1"));
        assert!(d.phones.is_empty());
    }

    #[test]
    fn qr_image_is_square() {
        let img = qr_image(&qr_text("192.168.1.20", 7345, &new_code())).unwrap();
        assert!(img.size >= 21);
        assert_eq!(img.dark.len(), img.size * img.size);
        // The finder pattern's corner cell is dark.
        assert!(img.dark[0]);
    }
}
