//! Ask the OS which interfaces exist and what addresses they hold.
//!
//! The I/O edge, kept as thin as it will go: one OS call, then a mapping into
//! the engine's own vocabulary. This module judges nothing. Deciding what an
//! entry means is `filter`'s job, because a decision made at the I/O edge is a
//! decision that can only be tested with a real adapter.
//!
//! On Windows the list comes from `GetAdaptersAddresses` through `if-addrs`.
//! The name is the adapter's friendly name, the one shown in Settings and by
//! `Get-NetIPAddress` as `InterfaceAlias`.
//!
//! WHAT THE CRATE DROPS BEFORE WE SEE IT
//!
//! `if-addrs` is not a raw view. Measured against `Get-NetIPAddress` on the
//! development machine, it returned 3 of 12 addresses, and every missing one is
//! explained by two filters it applies on Windows:
//!
//! 1. **Only addresses Windows marks `Preferred` are kept.** A `Tentative`
//!    address is still being checked for duplicates and cannot be bound yet; a
//!    `Deprecated` one should not start new connections. Both are right to
//!    drop. The consequence is timing: a freshly plugged phone appears only
//!    once Windows finishes that check, and that time comes out of the
//!    two-second budget in the acceptance criteria. Task 10 measures it.
//! 2. **Link-local addresses are dropped**, IPv4 and IPv6 alike, unless the
//!    crate's `link-local` feature is on. `filter` rejects link-local anyway
//!    and must keep doing so: Cargo merges features across the whole
//!    dependency tree, so if any other crate ever switches that feature on,
//!    this filter disappears with no change to our code.

use anyhow::Context;
use if_addrs::Interface;

use super::model::{Addr, Candidate};

/// Every address on every interface, one `Candidate` per address.
///
/// Per address rather than per interface, because an address is the thing a
/// socket binds to. An interface holding two addresses offers two separately
/// bindable paths (Review Focus 2), and folding them into one entry here would
/// lose one before anything had the chance to judge it.
pub fn enumerate() -> anyhow::Result<Vec<Candidate>> {
    let interfaces = if_addrs::get_if_addrs().context("listing the OS network interfaces")?;
    Ok(interfaces.into_iter().filter_map(to_candidate).collect())
}

fn to_candidate(iface: Interface) -> Option<Candidate> {
    // `if-addrs` reports no index only when the address family is switched off
    // on the adapter, which cannot coincide with the adapter holding an address
    // of that family. Should it happen anyway, the entry is dropped loudly:
    // quietly losing a link is the failure this project fears most.
    let Some(os_index) = iface.index else {
        tracing::warn!(
            name = %iface.name,
            addr = %iface.ip(),
            "interface reported without an OS index, skipped"
        );
        return None;
    };

    Some(Candidate {
        os_index,
        addr: Addr::from(iface.ip()),
        // Only `Up` counts. Dormant, Testing, Unknown and the three flavours
        // of down all describe an interface that is not passing traffic or
        // whose state the OS cannot vouch for. That is the strict reading, and
        // it is checked against the real tether rather than assumed: if the
        // phone ever reported anything but Up, this line would hide it.
        up: iface.is_oper_up(),
        name: iface.name,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use if_addrs::{IfAddr, IfOperStatus, Ifv4Addr, Ifv6Addr};
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    fn iface(name: &str, index: Option<u32>, status: IfOperStatus, ip: &str) -> Interface {
        let addr = match ip.parse::<IpAddr>().unwrap() {
            IpAddr::V4(ip) => IfAddr::V4(Ifv4Addr {
                ip,
                netmask: Ipv4Addr::UNSPECIFIED,
                prefixlen: 24,
                broadcast: None,
            }),
            IpAddr::V6(ip) => IfAddr::V6(Ifv6Addr {
                ip,
                netmask: Ipv6Addr::UNSPECIFIED,
                prefixlen: 64,
                broadcast: None,
            }),
        };
        Interface {
            name: name.to_string(),
            addr,
            index,
            oper_status: status,
            is_p2p: false,
            #[cfg(windows)]
            adapter_name: String::new(),
        }
    }

    #[test]
    fn maps_name_index_address_and_status() {
        let c = to_candidate(iface("Wi-Fi", Some(16), IfOperStatus::Up, "192.0.2.10")).unwrap();
        assert_eq!(c.name, "Wi-Fi");
        assert_eq!(c.os_index, 16);
        assert_eq!(c.addr.ip(), "192.0.2.10".parse::<IpAddr>().unwrap());
        assert!(c.up);
    }

    #[test]
    fn only_an_up_status_counts_as_up() {
        for status in [
            IfOperStatus::Down,
            IfOperStatus::Testing,
            IfOperStatus::Unknown,
            IfOperStatus::Dormant,
            IfOperStatus::NotPresent,
            IfOperStatus::LowerLayerDown,
        ] {
            let c = to_candidate(iface("Ethernet", Some(14), status.clone(), "192.0.2.20"));
            assert!(!c.unwrap().up, "{status:?} was treated as up");
        }
    }

    #[test]
    fn an_ipv6_address_stays_ipv6() {
        let c = to_candidate(iface("Wi-Fi", Some(16), IfOperStatus::Up, "2001:db8::10")).unwrap();
        assert!(matches!(c.addr, Addr::V6(_)));
    }

    #[test]
    fn an_entry_without_an_index_is_skipped() {
        assert!(to_candidate(iface("Ghost", None, IfOperStatus::Up, "192.0.2.30")).is_none());
    }
}
