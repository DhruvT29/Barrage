//! Task 2: which candidates deserve a probe.
//!
//! Every address here comes from a range reserved for documentation, RFC 5737
//! for IPv4 and RFC 3849 for IPv6, so no test value resembles a network anyone
//! actually runs. That matters beyond tidiness: phase 0 measured a USB tether
//! on a prefix nobody predicted, and a test fixture that looks like a real
//! network invites exactly the hardcoding the task 1 guard exists to forbid.

use std::net::IpAddr;

use barrage::links::filter::usable;
use barrage::links::model::{Addr, Candidate};

fn cand(name: &str, ip: &str, up: bool) -> Candidate {
    Candidate {
        os_index: 1,
        name: name.to_string(),
        addr: Addr::from(ip.parse::<IpAddr>().expect("test address should parse")),
        up,
    }
}

// ---------------------------------------------------------------- rejections

#[test]
fn rejects_link_local() {
    // Windows invents one of these when DHCP fails. Phase 0 found four on the
    // test machine, every one of them without internet access.
    let got = usable(&[cand("Ethernet", "169.254.123.14", true)]);
    assert!(got.is_empty());
}

#[test]
fn rejects_loopback() {
    let got = usable(&[cand("Loopback", "127.0.0.1", true)]);
    assert!(got.is_empty());
}

#[test]
fn rejects_disconnected() {
    let got = usable(&[cand("Ethernet", "192.0.2.10", false)]);
    assert!(got.is_empty());
}

#[test]
fn rejects_unspecified() {
    // An adapter mid-configuration can report this. It is an address in form
    // only and nothing can be bound to it.
    let got = usable(&[cand("Ethernet", "0.0.0.0", true)]);
    assert!(got.is_empty());
}

// ---------------------------------------------------------------- acceptance

#[test]
fn keeps_a_routable_connected_address() {
    let got = usable(&[cand("Wi-Fi", "192.0.2.10", true)]);
    assert_eq!(got.len(), 1);
}

/// Review Focus 2. One adapter can hold several addresses, and each one is
/// independently bindable, so each must survive as its own candidate. Merging
/// them would silently lose a usable path; counting the adapter once would
/// lose the same thing more quietly.
#[test]
fn one_interface_with_two_addresses_yields_two_links() {
    let got = usable(&[
        cand("Wi-Fi", "192.0.2.10", true),
        cand("Wi-Fi", "192.0.2.11", true),
    ]);
    assert_eq!(got.len(), 2);
}

// ---------------------------------------------------------------- behaviour

#[test]
fn preserves_input_order() {
    // Task 10 renders a table that refreshes every second. Stable ordering is
    // what stops rows swapping places between polls for no reason.
    let got = usable(&[
        cand("Wi-Fi", "192.0.2.10", true),
        cand("Loopback", "127.0.0.1", true),
        cand("Tether", "198.51.100.5", true),
        cand("Ethernet", "169.254.1.1", true),
        cand("Second", "198.51.100.6", true),
    ]);
    let names: Vec<&str> = got.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, vec!["Wi-Fi", "Tether", "Second"]);
}

#[test]
fn does_not_deduplicate() {
    // A filter removes, it does not merge. If the OS ever reports the same
    // address twice that is a fact worth surfacing, not one to paper over here.
    let got = usable(&[
        cand("Wi-Fi", "192.0.2.10", true),
        cand("Wi-Fi", "192.0.2.10", true),
    ]);
    assert_eq!(got.len(), 2);
}

#[test]
fn empty_input_is_empty_output() {
    // Airplane mode. Must be an ordinary empty result, never a panic.
    let got = usable(&[]);
    assert!(got.is_empty());
}

// ---------------------------------------------------------------- IPv6
// The same rules, applied family-agnostically. Deciding what the prober can
// actually bind is task 5's problem, not the filter's.

#[test]
fn rejects_ipv6_link_local() {
    let got = usable(&[cand("Wi-Fi", "fe80::1", true)]);
    assert!(got.is_empty());
}

#[test]
fn rejects_ipv6_loopback() {
    let got = usable(&[cand("Loopback", "::1", true)]);
    assert!(got.is_empty());
}

#[test]
fn keeps_a_routable_ipv6_address() {
    let got = usable(&[cand("Wi-Fi", "2001:db8::1", true)]);
    assert_eq!(got.len(), 1);
}
