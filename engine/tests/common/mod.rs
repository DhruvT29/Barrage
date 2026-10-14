//! Helpers shared by the test files that go through a real link.

use std::net::IpAddr;

use barrage::links::{enumerate, usable};

/// The first usable IPv4 link on this machine, whichever that is. Tests that
/// call this assume a working connection and nothing about which one.
pub fn first_usable_local_addr() -> IpAddr {
    usable(&enumerate().expect("enumeration failed"))
        .into_iter()
        .map(|c| c.addr.ip())
        .find(IpAddr::is_ipv4)
        .expect("these tests need at least one working IPv4 network link")
}
