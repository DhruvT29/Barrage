//! When the session moves to another UDP cable. librqbit binds its DHT,
//! UDP tracker and uTP sockets once, when the session starts, so moving
//! them means a new session: rebuilt when the UDP cable `choose::udp_cable`
//! wants, or its address, is not the one the session is on, and never twice within
//! `REBUILD_GAP`, however often a cable flaps (Review Focus 4).

use std::net::IpAddr;
use std::time::Duration;

use tokio::time::Instant;

/// The least time between two rebuilds.
pub const REBUILD_GAP: Duration = Duration::from_secs(30);

/// Whether to rebuild now: the wanted UDP cable, or its address, is another
/// than the session's (`current`; `None`: started with no cable, on this
/// machine only), and the last rebuild was at least `REBUILD_GAP` ago. With
/// no cable wanted, there is nowhere better to go, so the session stays.
pub fn due(
    now: Instant,
    current: Option<(&str, IpAddr)>,
    wanted: Option<(&str, IpAddr)>,
    last: Option<Instant>,
) -> bool {
    wanted.is_some()
        && wanted != current
        && last.is_none_or(|last| now.duration_since(last) >= REBUILD_GAP)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on(cable: &str, ip: [u8; 4]) -> Option<(&str, IpAddr)> {
        Some((cable, IpAddr::from(ip)))
    }

    const ETH2: [u8; 4] = [192, 0, 2, 2];
    const ETH3: [u8; 4] = [192, 0, 2, 3];

    #[test]
    fn a_new_udp_cable_rebuilds() {
        let now = Instant::now();
        assert!(due(
            now,
            on("Ethernet 3", ETH3),
            on("Ethernet 2", ETH2),
            None
        ));
        assert!(
            due(now, None, on("Ethernet 2", ETH2), None),
            "started with no cable: the first one moves it"
        );
    }

    #[test]
    fn the_same_cable_does_not() {
        let now = Instant::now();
        assert!(!due(
            now,
            on("Ethernet 2", ETH2),
            on("Ethernet 2", ETH2),
            None
        ));
    }

    /// A tether back with a new address: the sockets are bound to the old.
    #[test]
    fn the_same_cable_on_a_new_address_rebuilds() {
        let now = Instant::now();
        assert!(due(
            now,
            on("Ethernet 2", ETH2),
            on("Ethernet 2", [192, 0, 2, 9]),
            None
        ));
    }

    #[test]
    fn no_cable_left_keeps_the_session() {
        let now = Instant::now();
        assert!(!due(now, on("Ethernet 2", ETH2), None, None));
        assert!(!due(now, None, None, None));
    }

    /// Review Focus 4: a flapping cable moves the session at most once
    /// every 30 s.
    #[test]
    fn no_two_rebuilds_within_30_s() {
        let last = Instant::now();
        let at = |secs| last + Duration::from_secs(secs);
        let (two, three) = (on("Ethernet 2", ETH2), on("Ethernet 3", ETH3));
        assert!(!due(at(0), two, three, Some(last)));
        assert!(!due(at(29), two, three, Some(last)));
        assert!(due(at(30), two, three, Some(last)));
    }
}
