//! Which links share one internet connection.
//!
//! Two links are only worth combining if they reach the internet by different
//! routes. On 2026-10-13 and again on 2026-10-14 the phone was joined to the
//! laptop's own Wi-Fi: the tether was a working link, but its traffic left
//! through the same router as the Wi-Fi's, so using both bought nothing. The
//! public address gives it away: through the tether it was exactly the
//! Wi-Fi's own.
//!
//! Pure, like `filter`: the watcher looks the public addresses up, this
//! decides what they mean.

use std::collections::BTreeMap;
use std::net::IpAddr;

use super::model::{Link, LinkId};
use super::render::cables_in_order;

/// Groups of working links, on at least two different interfaces, that show
/// the internet the same public address. Each group is in table order.
///
/// Addresses on one interface are one cable whatever their public addresses,
/// so a group needs more than one interface. Only links that may be given
/// work count ([`is_usable`](super::model::LinkState::is_usable)): a portal or a dead link is not
/// being used anyway, and a link whose public address is not known yet is
/// left out rather than guessed.
pub fn shared_upstreams(links: &[Link]) -> Vec<Vec<LinkId>> {
    let mut by_public: BTreeMap<IpAddr, Vec<&Link>> = BTreeMap::new();
    for link in links.iter().filter(|l| l.state.is_usable()) {
        if let Some(public) = link.public_ip {
            by_public.entry(public).or_default().push(link);
        }
    }
    by_public
        .into_values()
        .filter(|group| group.iter().any(|l| l.name != group[0].name))
        .map(|group| group.iter().map(|l| l.id).collect())
        .collect()
}

/// The cables that reach the internet through one connection: two cables
/// [`shared_upstreams`] puts in a group are on one, and so is a cable joined to
/// either through another. A cable joins whether or not it has been measured,
/// since sharing comes from public addresses, not speeds. Each connection
/// lists its cables in table order, and connections come in the order their
/// first cable appeared. A cable that shares with nobody is in none.
///
/// The Links tab's cards, the combined speed and a download's paths all read
/// this one list, so they cannot disagree on what is one connection.
pub fn shared_connections(links: &[Link]) -> Vec<Vec<String>> {
    let names = cables_in_order(links);
    let groups: Vec<Vec<usize>> = shared_upstreams(links)
        .into_iter()
        .map(|group| {
            group
                .into_iter()
                .filter_map(|id| links.iter().find(|l| l.id == id))
                .filter_map(|l| names.iter().position(|&n| n == l.name))
                .collect()
        })
        .collect();

    // Which connection each cable is on: start apart, then join every pair a
    // shared group links, until nothing moves.
    let mut connection: Vec<usize> = (0..names.len()).collect();
    let mut moved = true;
    while moved {
        moved = false;
        for group in &groups {
            let Some(lowest) = group.iter().map(|&i| connection[i]).min() else {
                continue;
            };
            for &i in group {
                if connection[i] != lowest {
                    connection[i] = lowest;
                    moved = true;
                }
            }
        }
    }

    // A connection is numbered by its first cable.
    (0..names.len())
        .filter(|&first| connection[first] == first)
        .map(|first| {
            (0..names.len())
                .filter(|&i| connection[i] == first)
                .map(|i| names[i].to_string())
                .collect::<Vec<String>>()
        })
        .filter(|cables| cables.len() > 1)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::links::model::{Addr, LinkState};

    fn link(id: u64, name: &str, local: &str, public: Option<&str>, state: LinkState) -> Link {
        Link {
            id: LinkId::new(id),
            name: name.to_string(),
            addr: Addr::from(local.parse::<IpAddr>().unwrap()),
            state,
            rtt: None,
            down_bps: None,
            public_ip: public.map(|p| p.parse().unwrap()),
            metered: false,
        }
    }

    fn up(id: u64, name: &str, local: &str, public: Option<&str>) -> Link {
        link(id, name, local, public, LinkState::Up)
    }

    #[test]
    fn two_cables_behind_one_public_address_share() {
        let links = [
            up(1, "Wi-Fi", "192.0.2.10", Some("198.51.100.99")),
            up(2, "Ethernet 2", "192.0.2.20", Some("198.51.100.99")),
        ];
        assert_eq!(
            shared_upstreams(&links),
            [vec![LinkId::new(1), LinkId::new(2)]]
        );
    }

    #[test]
    fn different_public_addresses_share_nothing() {
        let links = [
            up(1, "Wi-Fi", "192.0.2.10", Some("198.51.100.99")),
            up(2, "Ethernet 2", "192.0.2.20", Some("203.0.113.5")),
        ];
        assert!(shared_upstreams(&links).is_empty());
    }

    /// Review Focus 2's two addresses on one interface: one cable, already
    /// known to be one, so not reported as sharing.
    #[test]
    fn addresses_on_one_interface_are_not_reported() {
        let links = [
            up(1, "Ethernet", "192.0.2.10", Some("198.51.100.99")),
            up(2, "Ethernet", "192.0.2.11", Some("198.51.100.99")),
        ];
        assert!(shared_upstreams(&links).is_empty());
    }

    #[test]
    fn an_unknown_public_address_is_not_guessed() {
        let links = [
            up(1, "Wi-Fi", "192.0.2.10", Some("198.51.100.99")),
            up(2, "Ethernet 2", "192.0.2.20", None),
        ];
        assert!(shared_upstreams(&links).is_empty());
    }

    #[test]
    fn only_working_links_count() {
        let links = [
            up(1, "Wi-Fi", "192.0.2.10", Some("198.51.100.99")),
            link(
                2,
                "Hotel",
                "192.0.2.30",
                Some("198.51.100.99"),
                LinkState::Portal,
            ),
            link(
                3,
                "Dead",
                "192.0.2.40",
                Some("198.51.100.99"),
                LinkState::Down,
            ),
        ];
        assert!(shared_upstreams(&links).is_empty());
    }

    /// A degraded link still carries work, so it still counts (code review
    /// of PR #2).
    #[test]
    fn a_degraded_link_counts() {
        let links = [
            up(1, "Wi-Fi", "192.0.2.10", Some("198.51.100.99")),
            link(
                2,
                "Ethernet 2",
                "192.0.2.20",
                Some("198.51.100.99"),
                LinkState::Degraded,
            ),
        ];
        assert_eq!(
            shared_upstreams(&links),
            [vec![LinkId::new(1), LinkId::new(2)]]
        );
    }

    /// The phone on the Wi-Fi: its IPv4 shares with the Wi-Fi, its IPv6
    /// address shows itself and joins no group.
    #[test]
    fn a_group_holds_every_sharing_link_and_nothing_else() {
        let links = [
            up(1, "Wi-Fi", "192.0.2.10", Some("198.51.100.99")),
            up(2, "Ethernet 2", "192.0.2.20", Some("198.51.100.99")),
            up(3, "Ethernet 2", "2001:db8::20", Some("2001:db8::20")),
            up(4, "Ethernet 3", "192.0.2.30", Some("198.51.100.99")),
        ];
        assert_eq!(
            shared_upstreams(&links),
            [vec![LinkId::new(1), LinkId::new(2), LinkId::new(4)]]
        );
    }

    fn measured(id: u64, name: &str, local: &str, bps: u64, public: &str) -> Link {
        Link {
            down_bps: Some(bps),
            ..up(id, name, local, Some(public))
        }
    }

    fn names(connections: &[&[&str]]) -> Vec<Vec<String>> {
        connections
            .iter()
            .map(|cables| cables.iter().map(|c| c.to_string()).collect())
            .collect()
    }

    /// The cards' "shared with", the combined speed and the scheduler's paths
    /// join cables the same way: directly or through a third, measured or not.
    #[test]
    fn connections_join_cables_directly_or_through_a_third() {
        let links = [
            measured(1, "Wi-Fi", "2001:db8::10", 30_000_000, "2001:db8::1"),
            measured(2, "Ethernet 2", "198.51.100.20", 50_000_000, "203.0.113.7"),
            measured(3, "Ethernet 2", "2001:db8::20", 50_000_000, "2001:db8::1"),
            measured(4, "Ethernet 3", "198.51.100.30", 20_000_000, "203.0.113.7"),
            measured(5, "Ethernet 4", "198.51.100.40", 9_000_000, "203.0.113.9"),
            up(6, "Ethernet 5", "198.51.100.50", Some("203.0.113.5")),
            up(7, "Ethernet 6", "198.51.100.60", Some("203.0.113.5")),
        ];
        assert_eq!(
            shared_connections(&links),
            names(&[
                &["Wi-Fi", "Ethernet 2", "Ethernet 3"],
                &["Ethernet 5", "Ethernet 6"]
            ])
        );

        // The group joining the later two comes first (its public address
        // sorts lower), so one pass over the groups is not enough.
        let late = [
            up(1, "Wi-Fi", "192.0.2.10", Some("203.0.113.9")),
            up(2, "Ethernet 2", "198.51.100.20", Some("203.0.113.9")),
            up(3, "Ethernet 2", "2001:db8::20", Some("198.51.100.1")),
            up(4, "Ethernet 3", "198.51.100.30", Some("198.51.100.1")),
        ];
        assert_eq!(
            shared_connections(&late),
            names(&[&["Wi-Fi", "Ethernet 2", "Ethernet 3"]])
        );
    }

    #[test]
    fn connections_come_in_the_order_their_first_cable_appeared() {
        let links = [
            up(1, "Wi-Fi", "192.0.2.10", Some("203.0.113.7")),
            up(2, "Ethernet 2", "198.51.100.20", Some("203.0.113.9")),
            up(3, "Ethernet 3", "198.51.100.30", Some("203.0.113.9")),
            up(4, "Ethernet 4", "198.51.100.40", Some("203.0.113.7")),
        ];
        assert_eq!(
            shared_connections(&links),
            names(&[&["Wi-Fi", "Ethernet 4"], &["Ethernet 2", "Ethernet 3"]])
        );
    }
}
