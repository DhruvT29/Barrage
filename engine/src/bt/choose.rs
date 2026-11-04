//! Which cable takes a torrent's next peer connection, and which cable
//! carries its UDP (the DHT, UDP trackers and uTP) and its listening port.
//!
//! A torrent's peers are shared between cables as a download's connections
//! are (phase 4): by what each delivers, cables on one internet connection
//! as one path carried by one of them, a degraded cable held to one. Here
//! the connections are peer connections, each opened when librqbit asks, so
//! the question is only ever "which cable for this one": the one furthest
//! below its share. A peer of a family no cable has (an IPv6 peer, only
//! IPv4 cables) is refused: binding it to an address of the other family
//! would leave it to Windows to pick the link.
//!
//! The UDP cable is chosen by measurement, never by name or type: usable,
//! not on a metered connection, the fastest. When every cable is metered,
//! the fastest. Pure: the torrents pass the link table.

use std::collections::HashMap;
use std::fmt;
use std::net::{IpAddr, SocketAddr};

use crate::fetch::schedule::{Want, allocate, paths};
use crate::links::render::{cable_speed, cables_in_order};
use crate::links::{Link, LinkState};

/// Why no cable can take a peer connection, in the engine's words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoCable {
    NoUsableCable,
    /// No usable cable has an address of the peer's family ("IPv4", "IPv6").
    NoCableForFamily(&'static str),
    /// Every cable for this peer holds all it may (a degraded cable, one).
    AllHeld,
}

impl fmt::Display for NoCable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoUsableCable => f.write_str("no cable is working"),
            Self::NoCableForFamily(family) => {
                write!(f, "no cable has an {family} address for this peer")
            }
            Self::AllHeld => {
                f.write_str("every cable for this peer holds all the connections it may for now")
            }
        }
    }
}

impl std::error::Error for NoCable {}

/// The cable for a new connection to `peer`, and the local address to bind
/// it to, given each cable's open peer connections by name in `open`.
pub fn next_cable(
    links: &[Link],
    open: &HashMap<String, u32>,
    peer: SocketAddr,
) -> Result<(String, IpAddr), NoCable> {
    let all = paths(links, |_| true);
    if all.is_empty() {
        return Err(NoCable::NoUsableCable);
    }
    let v6 = peer.is_ipv6();
    // Each path's carrier that has an address of the peer's family, with
    // its claim: its speed test, and one connection while it is degraded.
    let mut takers: Vec<(&str, IpAddr, Want)> = Vec::new();
    for path in &all {
        let on_path: Vec<&Link> = links
            .iter()
            .filter(|l| path.links.contains(&l.id))
            .collect();
        let Some(ip) = on_path
            .iter()
            .map(|l| l.addr.ip())
            .find(|ip| ip.is_ipv6() == v6)
        else {
            continue;
        };
        let degraded = !on_path.iter().any(|l| l.state == LinkState::Up);
        let cable: Vec<&Link> = links.iter().filter(|l| l.name == path.carrier).collect();
        let want = Want {
            weight: cable_speed(&cable).unwrap_or(0),
            most: if degraded { 1 } else { u32::MAX },
        };
        takers.push((&path.carrier, ip, want));
    }
    if takers.is_empty() {
        return Err(NoCable::NoCableForFamily(if v6 { "IPv6" } else { "IPv4" }));
    }

    let held = |name: &str| open.get(name).copied().unwrap_or(0);
    let total = takers.iter().map(|(name, ..)| held(name)).sum::<u32>() + 1;
    let wants: Vec<Want> = takers.iter().map(|(.., want)| *want).collect();
    let shares = allocate(total, 0, &wants);
    // The one furthest below its share among those that may take one more,
    // the first of equals.
    let mut best: Option<(usize, i64)> = None;
    for (i, ((name, _, want), share)) in takers.iter().zip(&shares).enumerate() {
        if held(name) >= want.most {
            continue;
        }
        let below = i64::from(*share) - i64::from(held(name));
        if best.is_none_or(|(_, b)| below > b) {
            best = Some((i, below));
        }
    }
    let (i, _) = best.ok_or(NoCable::AllHeld)?;
    Ok((takers[i].0.to_string(), takers[i].1))
}

/// The cable for UDP and the listening port, and its address (an IPv4 one
/// when it has one); `None` when no cable works.
pub fn udp_cable(links: &[Link]) -> Option<(String, IpAddr)> {
    let mut best: Option<(&str, (bool, u64))> = None;
    for name in cables_in_order(links) {
        let cable: Vec<&Link> = links.iter().filter(|l| l.name == name).collect();
        if !cable.iter().any(|l| l.state.is_usable()) {
            continue;
        }
        let rank = (
            !cable.iter().any(|l| l.metered),
            cable_speed(&cable).unwrap_or(0),
        );
        if best.is_none_or(|(_, b)| rank > b) {
            best = Some((name, rank));
        }
    }
    let (name, _) = best?;
    let usable = || {
        links
            .iter()
            .filter(|l| l.name == name && l.state.is_usable())
            .map(|l| l.addr.ip())
    };
    let ip = usable().find(IpAddr::is_ipv4).or_else(|| usable().next())?;
    Some((name.to_string(), ip))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::links::{Addr, LinkId};

    fn link(id: u64, name: &str, addr: &str, mbps: u64) -> Link {
        Link {
            id: LinkId::new(id),
            name: name.to_string(),
            addr: Addr::from(addr.parse::<IpAddr>().unwrap()),
            state: LinkState::Up,
            rtt: None,
            down_bps: Some(mbps * 1_000_000),
            public_ip: None,
            metered: false,
            tested: 0,
        }
    }

    fn peer(text: &str) -> SocketAddr {
        text.parse().unwrap()
    }

    /// Ask `n` times in a row, each connection opened before the next ask.
    fn take(links: &[Link], n: usize) -> HashMap<String, u32> {
        let mut open = HashMap::new();
        for _ in 0..n {
            let (cable, _) = next_cable(links, &open, peer("203.0.113.50:6881")).unwrap();
            *open.entry(cable).or_insert(0) += 1;
        }
        open
    }

    #[test]
    fn connections_follow_the_measured_speeds() {
        let links = [
            link(1, "Wi-Fi", "192.0.2.10", 10),
            link(2, "Ethernet", "198.51.100.7", 30),
        ];
        let open = take(&links, 8);
        assert_eq!((open["Ethernet"], open["Wi-Fi"]), (6, 2));
    }

    #[test]
    fn the_address_to_bind_is_the_chosen_cables() {
        let links = [
            link(1, "Wi-Fi", "192.0.2.10", 30),
            link(2, "Wi-Fi", "2001:db8::10", 30),
        ];
        let got = next_cable(&links, &HashMap::new(), peer("[2001:db8::99]:6881")).unwrap();
        assert_eq!(got, ("Wi-Fi".to_string(), "2001:db8::10".parse().unwrap()));
        let got = next_cable(&links, &HashMap::new(), peer("203.0.113.50:6881")).unwrap();
        assert_eq!(got.1, "192.0.2.10".parse::<IpAddr>().unwrap());
    }

    /// Review Focus 2: an IPv6 peer with only IPv4 cables is refused, never
    /// sent out through an IPv4 address and Windows' choice of link.
    #[test]
    fn an_ipv6_peer_goes_only_to_a_cable_with_ipv6() {
        let links = [
            link(1, "Wi-Fi", "192.0.2.10", 30),
            link(2, "Ethernet", "198.51.100.7", 30),
        ];
        let got = next_cable(&links, &HashMap::new(), peer("[2001:db8::99]:6881"));
        assert_eq!(got, Err(NoCable::NoCableForFamily("IPv6")));
        assert_eq!(
            got.unwrap_err().to_string(),
            "no cable has an IPv6 address for this peer"
        );

        let mut links = links.to_vec();
        links.push(link(3, "Ethernet 2", "2001:db8::7", 5));
        let (cable, _) = next_cable(&links, &HashMap::new(), peer("[2001:db8::99]:6881")).unwrap();
        assert_eq!(cable, "Ethernet 2");
    }

    #[test]
    fn a_degraded_cable_takes_one_connection() {
        let mut links = [
            link(1, "Wi-Fi", "192.0.2.10", 30),
            link(2, "Ethernet 2", "198.51.100.7", 30),
        ];
        links[1].state = LinkState::Degraded;
        let open = take(&links, 6);
        assert_eq!((open["Wi-Fi"], open["Ethernet 2"]), (5, 1));
    }

    #[test]
    fn with_only_a_degraded_cable_holding_its_one_there_is_no_cable() {
        let mut links = [link(1, "Ethernet 2", "198.51.100.7", 30)];
        links[0].state = LinkState::Degraded;
        let open = HashMap::from([("Ethernet 2".to_string(), 1)]);
        assert_eq!(
            next_cable(&links, &open, peer("203.0.113.50:6881")),
            Err(NoCable::AllHeld)
        );
    }

    /// The phone joined to the laptop's own Wi-Fi: one internet connection,
    /// carried by the cable that is not metered; the other takes no peers.
    #[test]
    fn a_cable_sitting_out_a_shared_path_takes_none() {
        let mut links = [
            link(1, "Wi-Fi", "192.0.2.10", 20),
            link(2, "Ethernet 2", "198.51.100.7", 80),
        ];
        for l in links.iter_mut() {
            l.public_ip = Some("203.0.113.7".parse().unwrap());
        }
        links[1].metered = true;
        let open = take(&links, 4);
        assert_eq!(open.get("Ethernet 2"), None);
        assert_eq!(open["Wi-Fi"], 4);
    }

    #[test]
    fn no_usable_cable_means_no_cable() {
        let mut links = [link(1, "Wi-Fi", "192.0.2.10", 30)];
        links[0].state = LinkState::Down;
        assert_eq!(
            next_cable(&links, &HashMap::new(), peer("203.0.113.50:6881")),
            Err(NoCable::NoUsableCable)
        );
        assert_eq!(udp_cable(&links), None);
    }

    #[test]
    fn the_udp_cable_is_the_fastest_not_metered() {
        let mut links = [
            link(1, "Ethernet 2", "198.51.100.7", 80),
            link(2, "Wi-Fi", "192.0.2.10", 20),
        ];
        links[0].metered = true;
        assert_eq!(
            udp_cable(&links),
            Some(("Wi-Fi".to_string(), "192.0.2.10".parse().unwrap()))
        );
    }

    #[test]
    fn with_every_cable_metered_the_fastest_carries_udp() {
        let mut links = [
            link(1, "Ethernet 2", "198.51.100.7", 20),
            link(2, "Ethernet 3", "203.0.113.9", 80),
        ];
        links[0].metered = true;
        links[1].metered = true;
        assert_eq!(
            udp_cable(&links).map(|(name, _)| name),
            Some("Ethernet 3".to_string())
        );
    }

    #[test]
    fn the_udp_cable_binds_an_ipv4_address_when_it_has_one() {
        let links = [
            link(1, "Wi-Fi", "2001:db8::10", 30),
            link(2, "Wi-Fi", "192.0.2.10", 30),
        ];
        assert_eq!(
            udp_cable(&links),
            Some(("Wi-Fi".to_string(), "192.0.2.10".parse().unwrap()))
        );
    }
}
