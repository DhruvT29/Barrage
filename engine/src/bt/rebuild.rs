//! When the session moves. librqbit binds its DHT, UDP tracker and uTP
//! sockets and its listener once, when the session starts, so changing them
//! means a new session: rebuilt when the place `choose::udp_cable` and
//! `listen::next` want is not the one the session is on, and never twice
//! within `REBUILD_GAP`, however often a cable flaps (Review Focus 4).

use std::net::IpAddr;
use std::time::Duration;

use tokio::time::Instant;

/// The least time between two rebuilds.
pub const REBUILD_GAP: Duration = Duration::from_secs(30);

/// Where a session sits: its UDP cable, and whether it listens there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    pub cable: String,
    pub ip: IpAddr,
    pub listening: bool,
}

/// Whether to rebuild now: the wanted place is another than the session's
/// (`current`; `None`: started with no cable, on this machine only), and the
/// last rebuild was at least `REBUILD_GAP` ago. With no cable wanted, there
/// is nowhere better to go, so the session stays.
pub fn due(
    now: Instant,
    current: Option<&Place>,
    wanted: Option<&Place>,
    last: Option<Instant>,
) -> bool {
    wanted.is_some()
        && wanted != current
        && last.is_none_or(|last| now.duration_since(last) >= REBUILD_GAP)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on(cable: &str, ip: [u8; 4]) -> Place {
        Place {
            cable: cable.to_string(),
            ip: IpAddr::from(ip),
            listening: false,
        }
    }

    const ETH2: [u8; 4] = [192, 0, 2, 2];
    const ETH3: [u8; 4] = [192, 0, 2, 3];

    #[test]
    fn a_new_udp_cable_rebuilds() {
        let now = Instant::now();
        assert!(due(
            now,
            Some(&on("Ethernet 3", ETH3)),
            Some(&on("Ethernet 2", ETH2)),
            None
        ));
        assert!(
            due(now, None, Some(&on("Ethernet 2", ETH2)), None),
            "started with no cable: the first one moves it"
        );
    }

    #[test]
    fn the_same_cable_does_not() {
        let now = Instant::now();
        let two = on("Ethernet 2", ETH2);
        assert!(!due(now, Some(&two), Some(&two), None));
    }

    /// A tether back with a new address: the sockets are bound to the old.
    #[test]
    fn the_same_cable_on_a_new_address_rebuilds() {
        let now = Instant::now();
        assert!(due(
            now,
            Some(&on("Ethernet 2", ETH2)),
            Some(&on("Ethernet 2", [192, 0, 2, 9])),
            None
        ));
    }

    /// The listener opens or closes only with a session.
    #[test]
    fn the_listener_changing_rebuilds() {
        let now = Instant::now();
        let closed = on("Ethernet 2", ETH2);
        let open = Place {
            listening: true,
            ..closed.clone()
        };
        assert!(due(now, Some(&open), Some(&closed), None));
        assert!(due(now, Some(&closed), Some(&open), None));
    }

    #[test]
    fn no_cable_left_keeps_the_session() {
        let now = Instant::now();
        assert!(!due(now, Some(&on("Ethernet 2", ETH2)), None, None));
        assert!(!due(now, None, None, None));
    }

    /// Review Focus 4: a flapping cable moves the session at most once
    /// every 30 s.
    #[test]
    fn no_two_rebuilds_within_30_s() {
        let last = Instant::now();
        let at = |secs| last + Duration::from_secs(secs);
        let (two, three) = (on("Ethernet 2", ETH2), on("Ethernet 3", ETH3));
        let (two, three) = (Some(&two), Some(&three));
        assert!(!due(at(0), two, three, Some(last)));
        assert!(!due(at(29), two, three, Some(last)));
        assert!(due(at(30), two, three, Some(last)));
    }
}
