// SPDX-License-Identifier: GPL-3.0-or-later
//! The desktop's address on the local network, for the pairing code.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};

/// The address this computer uses to reach the local network, found by
/// asking the system which address it would send from (nothing is
/// sent). `None` when there is no network.
pub fn local_ip() -> Option<IpAddr> {
    let s = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    // A documentation-only address: never answered, never routed to.
    s.connect(SocketAddr::from(([192, 0, 2, 1], 9))).ok()?;
    let ip = s.local_addr().ok()?.ip();
    (!ip.is_unspecified() && !ip.is_loopback()).then_some(ip)
}
