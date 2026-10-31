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
//! Whether a link is metered comes from WinRT's `NetworkInformation`: the
//! cost of the connection profile its adapter is on, matched by the
//! adapter's GUID, never its name, since a profile is named after its
//! network.
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

use std::collections::HashSet;
use std::sync::Mutex;

use anyhow::Context;
use if_addrs::Interface;

use super::model::{Addr, Candidate};

/// The metered adapters as Windows last said, and whether it refused the
/// last time it was asked.
static COSTS: Mutex<Costs> = Mutex::new(Costs {
    known: None,
    refused: false,
});

/// What is known of the connections' costs across polls.
#[derive(Debug, Default)]
struct Costs {
    known: Option<HashSet<String>>,
    refused: bool,
}

impl Costs {
    /// The metered adapters now, from Windows' answer: what it says, or when
    /// it refuses, what it said last (nothing, if it never has). A refusal
    /// says nothing about the links themselves, and forgetting them would
    /// let a metered tether carry a shared connection. The log says when it
    /// starts refusing and when it stops.
    fn now(&mut self, answer: anyhow::Result<Vec<Profile>>) -> HashSet<String> {
        match answer {
            Ok(profiles) => {
                if std::mem::take(&mut self.refused) {
                    tracing::info!("Windows says which connections are metered again");
                }
                let metered = metered_adapters(&profiles);
                self.known = Some(metered.clone());
                metered
            }
            Err(e) => {
                if !std::mem::replace(&mut self.refused, true) {
                    tracing::warn!(
                        error = %format!("{e:#}"),
                        "Windows would not say which connections are metered; keeping what it said last"
                    );
                }
                self.known.clone().unwrap_or_default()
            }
        }
    }
}

/// Every address on every interface, one `Candidate` per address.
///
/// Per address rather than per interface, because an address is the thing a
/// socket binds to. An interface holding two addresses offers two separately
/// bindable paths (Review Focus 2), and folding them into one entry here would
/// lose one before anything had the chance to judge it.
pub fn enumerate() -> anyhow::Result<Vec<Candidate>> {
    let interfaces = if_addrs::get_if_addrs().context("listing the OS network interfaces")?;
    let metered = COSTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .now(profiles());
    Ok(interfaces
        .into_iter()
        .filter_map(|i| to_candidate(i, &metered))
        .collect())
}

/// What Windows says a connection profile costs, its `NetworkCostType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cost {
    Unrestricted,
    /// A data limit: what "Set as metered connection" gives.
    Fixed,
    /// Paid by the byte.
    Variable,
    Unknown,
}

/// The connection profile an adapter is on now, as Windows reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    /// The adapter's GUID, the same one `if-addrs` gives as `adapter_name`.
    pub adapter: String,
    /// The network's name, as Settings shows it.
    pub name: String,
    pub cost: Cost,
    pub roaming: bool,
    pub over_limit: bool,
}

/// Whether a connection with this cost is metered: a data limit or paid by
/// the byte, or roaming, or over its limit.
pub(crate) fn is_metered(cost: Cost, roaming: bool, over_limit: bool) -> bool {
    matches!(cost, Cost::Fixed | Cost::Variable) || roaming || over_limit
}

/// The adapters on a metered connection, each GUID in one spelling.
fn metered_adapters(profiles: &[Profile]) -> HashSet<String> {
    profiles
        .iter()
        .filter(|p| is_metered(p.cost, p.roaming, p.over_limit))
        .map(|p| adapter_key(&p.adapter))
        .collect()
}

/// A GUID in one spelling: WinRT writes it bare and `if-addrs` in braces,
/// either in any case.
fn adapter_key(guid: &str) -> String {
    guid.trim_matches(['{', '}']).to_ascii_lowercase()
}

/// The adapter's GUID, which `if-addrs` gives only on Windows.
#[cfg(windows)]
fn adapter_of(iface: &Interface) -> &str {
    &iface.adapter_name
}

#[cfg(not(windows))]
fn adapter_of(_iface: &Interface) -> &str {
    ""
}

/// The connection profile each adapter is on now, with its cost.
///
/// Windows lists every network it has saved, each on the adapter that last
/// joined it: this machine's Wi-Fi has 21, one of them set as metered. Only
/// a profile the adapter is connected to says anything about the link, so
/// the rest are skipped before their cost is read, which also halves the
/// time this takes (about 30 ms a poll here, against 65 reading all).
#[cfg(windows)]
pub fn profiles() -> anyhow::Result<Vec<Profile>> {
    use windows::Networking::Connectivity::{
        NetworkConnectivityLevel, NetworkCostType, NetworkInformation,
    };

    let all = NetworkInformation::GetConnectionProfiles().context("listing connection profiles")?;
    let mut profiles = Vec::new();
    for profile in &all {
        // One profile Windows will not describe, say a tether's as it goes,
        // is skipped, not the whole list: the others' costs still hold.
        let read = || -> windows::core::Result<Option<Profile>> {
            if profile.GetNetworkConnectivityLevel()? == NetworkConnectivityLevel::None {
                return Ok(None);
            }
            // A profile with no adapter, such as a VPN's, is no cable's.
            let Ok(adapter) = profile.NetworkAdapter() else {
                return Ok(None);
            };
            let cost = profile.GetConnectionCost()?;
            Ok(Some(Profile {
                adapter: format!("{:?}", adapter.NetworkAdapterId()?),
                name: profile.ProfileName()?.to_string(),
                cost: match cost.NetworkCostType()? {
                    NetworkCostType::Unrestricted => Cost::Unrestricted,
                    NetworkCostType::Fixed => Cost::Fixed,
                    NetworkCostType::Variable => Cost::Variable,
                    _ => Cost::Unknown,
                },
                roaming: cost.Roaming()?,
                over_limit: cost.OverDataLimit()?,
            }))
        };
        match read() {
            Ok(Some(found)) => profiles.push(found),
            Ok(None) => {}
            Err(e) => {
                tracing::debug!(error = %e, "a connection profile could not be read, skipped")
            }
        }
    }
    Ok(profiles)
}

/// No other OS says what a link costs, so nothing is metered there.
#[cfg(not(windows))]
pub fn profiles() -> anyhow::Result<Vec<Profile>> {
    Ok(Vec::new())
}

fn to_candidate(iface: Interface, metered: &HashSet<String>) -> Option<Candidate> {
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
        metered: metered.contains(&adapter_key(adapter_of(&iface))),
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
        let c = to_candidate(
            iface("Wi-Fi", Some(16), IfOperStatus::Up, "192.0.2.10"),
            &none(),
        )
        .unwrap();
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
            let c = to_candidate(
                iface("Ethernet", Some(14), status.clone(), "192.0.2.20"),
                &none(),
            );
            assert!(!c.unwrap().up, "{status:?} was treated as up");
        }
    }

    #[test]
    fn an_ipv6_address_stays_ipv6() {
        let c = to_candidate(
            iface("Wi-Fi", Some(16), IfOperStatus::Up, "2001:db8::10"),
            &none(),
        )
        .unwrap();
        assert!(matches!(c.addr, Addr::V6(_)));
    }

    #[test]
    fn an_entry_without_an_index_is_skipped() {
        let ghost = iface("Ghost", None, IfOperStatus::Up, "192.0.2.30");
        assert!(to_candidate(ghost, &none()).is_none());
    }

    fn none() -> HashSet<String> {
        HashSet::new()
    }

    /// `iface` on the adapter with this GUID, as `if-addrs` writes it.
    fn on_adapter(mut iface: Interface, guid: &str) -> Interface {
        #[cfg(windows)]
        {
            iface.adapter_name = guid.to_string();
        }
        let _ = guid;
        iface
    }

    fn profile(adapter: &str, name: &str, cost: Cost) -> Profile {
        Profile {
            adapter: adapter.to_string(),
            name: name.to_string(),
            cost,
            roaming: false,
            over_limit: false,
        }
    }

    const WIFI_ADAPTER: &str = "{6B29FC40-CA47-1067-B31D-00DD010662DA}";
    const PHONE_ADAPTER: &str = "{3F2504E0-4F89-11D3-9A0C-0305E82C3301}";

    #[test]
    fn fixed_and_variable_costs_are_metered_unrestricted_and_unknown_are_not() {
        assert!(is_metered(Cost::Fixed, false, false));
        assert!(is_metered(Cost::Variable, false, false));
        assert!(!is_metered(Cost::Unrestricted, false, false));
        assert!(!is_metered(Cost::Unknown, false, false));
    }

    #[test]
    fn roaming_or_over_the_limit_is_metered() {
        for cost in [Cost::Unrestricted, Cost::Unknown] {
            assert!(is_metered(cost, true, false), "{cost:?} roaming");
            assert!(is_metered(cost, false, true), "{cost:?} over its limit");
        }
    }

    /// A profile's name is the network's, which can be anything, another
    /// adapter's name included. And WinRT writes a GUID bare, in any case,
    /// where `if-addrs` puts it in braces.
    #[cfg(windows)]
    #[test]
    fn a_cost_is_matched_by_adapter_not_by_name() {
        let metered = metered_adapters(&[
            profile("3f2504e0-4f89-11d3-9a0c-0305e82c3301", "Wi-Fi", Cost::Fixed),
            profile(
                "6B29FC40-CA47-1067-B31D-00DD010662DA",
                "Ethernet 3",
                Cost::Unrestricted,
            ),
        ]);
        let up = IfOperStatus::Up;
        let phone = on_adapter(
            iface("Ethernet 3", Some(21), up.clone(), "192.0.2.40"),
            PHONE_ADAPTER,
        );
        let wifi = on_adapter(iface("Wi-Fi", Some(16), up, "192.0.2.10"), WIFI_ADAPTER);
        assert!(to_candidate(phone, &metered).unwrap().metered);
        assert!(!to_candidate(wifi, &metered).unwrap().metered);
    }

    /// Review Focus 5: an adapter Windows has no profile for, or every
    /// adapter when Windows would not say, is not metered.
    #[cfg(windows)]
    #[test]
    fn an_adapter_with_no_profile_is_not_metered() {
        let metered = metered_adapters(&[profile(PHONE_ADAPTER, "Phone", Cost::Fixed)]);
        let wifi = || {
            on_adapter(
                iface("Wi-Fi", Some(16), IfOperStatus::Up, "192.0.2.10"),
                WIFI_ADAPTER,
            )
        };
        assert!(!to_candidate(wifi(), &metered).unwrap().metered);
        assert!(!to_candidate(wifi(), &none()).unwrap().metered);
    }

    /// Review: Windows refusing once must not make a metered tether look
    /// unmetered: what it said last stands until it answers again.
    #[test]
    fn a_refusal_keeps_what_windows_said_last() {
        let mut costs = Costs::default();
        let refused = || Err(anyhow::anyhow!("refused"));
        assert!(
            costs.now(refused()).is_empty(),
            "never answered: nothing known"
        );
        let tether = profile(WIFI_ADAPTER, "Phone", Cost::Fixed);
        let first = costs.now(Ok(vec![tether]));
        assert_eq!(first, HashSet::from([adapter_key(WIFI_ADAPTER)]));
        assert_eq!(costs.now(refused()), first, "kept through a refusal");
        assert!(costs.refused);
        assert!(
            costs.now(Ok(Vec::new())).is_empty(),
            "a new answer replaces it"
        );
        assert!(!costs.refused);
    }
}
