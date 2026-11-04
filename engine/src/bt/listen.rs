//! Whether the listening port is kept. It opens on the UDP cable and stays
//! only if a peer calls in: many connections cannot be reached from outside,
//! and a port nobody can reach only costs a socket and a router's mapping.
//! Time counts only while a torrent runs, as nobody has a reason to call
//! otherwise. A new UDP cable tries again. Pure: the torrents pass the time,
//! and rebuild the session with or without a listener to match.

use std::time::Duration;

use tokio::time::Instant;

/// How long a torrent runs with the port open before giving up on it.
pub const TRY_FOR: Duration = Duration::from_secs(10 * 60);

/// Where the listening port stands.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Listening {
    /// No cable to listen on, or listening is off.
    #[default]
    Off,
    /// Open, waiting for a first peer: `ran` is the running time so far,
    /// as of `at`.
    Trying {
        cable: String,
        ran: Duration,
        at: Instant,
    },
    /// A peer called in: it stays.
    Open { cable: String },
    /// Nobody did: closed, until the UDP cable changes.
    Closed { cable: String, why: String },
}

impl Listening {
    /// Whether the session should have a listener.
    pub fn wants_port(&self) -> bool {
        matches!(self, Self::Trying { .. } | Self::Open { .. })
    }

    /// Whether the two read the same to the user: a try's clock does not show.
    pub fn reads_like(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Trying { cable: a, .. }, Self::Trying { cable: b, .. }) => a == b,
            _ => self == other,
        }
    }
}

/// The state after `state`, at `now`: `incoming` is how many peers called in
/// since the last look, `running` whether any torrent runs, `udp_cable` the
/// cable to listen on.
pub fn next(
    state: &Listening,
    now: Instant,
    incoming: u64,
    running: bool,
    udp_cable: Option<&str>,
    try_for: Duration,
) -> Listening {
    let Some(udp_cable) = udp_cable else {
        return Listening::Off;
    };
    match state {
        Listening::Trying { cable, ran, at } if cable == udp_cable => {
            if incoming > 0 {
                return Listening::Open {
                    cable: cable.clone(),
                };
            }
            let ran = *ran + if running { now - *at } else { Duration::ZERO };
            if ran >= try_for {
                return Listening::Closed {
                    cable: cable.clone(),
                    why: format!(
                        "no peer called in within {} minutes; this PC cannot be reached \
                         from outside on {cable}",
                        try_for.as_secs() / 60
                    ),
                };
            }
            Listening::Trying {
                cable: cable.clone(),
                ran,
                at: now,
            }
        }
        Listening::Open { cable } | Listening::Closed { cable, .. } if cable == udp_cable => {
            state.clone()
        }
        _ => Listening::Trying {
            cable: udp_cable.to_string(),
            ran: Duration::ZERO,
            at: now,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ETH2: Option<&str> = Some("Ethernet 2");
    const ETH3: Option<&str> = Some("Ethernet 3");

    fn minutes(n: u64) -> Duration {
        Duration::from_secs(n * 60)
    }

    fn trying(cable: &str, at: Instant) -> Listening {
        Listening::Trying {
            cable: cable.to_string(),
            ran: Duration::ZERO,
            at,
        }
    }

    fn step(state: &Listening, now: Instant, incoming: u64, running: bool) -> Listening {
        next(state, now, incoming, running, ETH3, TRY_FOR)
    }

    #[test]
    fn a_cable_starts_the_try() {
        let now = Instant::now();
        assert_eq!(
            next(&Listening::Off, now, 0, false, ETH3, TRY_FOR),
            trying("Ethernet 3", now)
        );
    }

    #[test]
    fn a_peer_calling_in_opens_it() {
        let start = Instant::now();
        let open = step(&trying("Ethernet 3", start), start + minutes(1), 1, true);
        assert_eq!(
            open,
            Listening::Open {
                cable: "Ethernet 3".to_string()
            }
        );
        assert_eq!(
            step(&open, start + minutes(60), 0, true),
            open,
            "open stays open, with no one calling"
        );
    }

    #[test]
    fn ten_running_minutes_without_one_close_it() {
        let start = Instant::now();
        let mut state = trying("Ethernet 3", start);
        for minute in 1..10 {
            state = step(&state, start + minutes(minute), 0, true);
            assert!(
                matches!(state, Listening::Trying { .. }),
                "{minute}: {state:?}"
            );
        }
        let closed = step(&state, start + minutes(10), 0, true);
        assert_eq!(
            closed,
            Listening::Closed {
                cable: "Ethernet 3".to_string(),
                why: "no peer called in within 10 minutes; this PC cannot be reached \
                      from outside on Ethernet 3"
                    .to_string()
            }
        );
        assert_eq!(
            step(&closed, start + minutes(20), 1, true),
            closed,
            "closed stays closed on the same cable"
        );
    }

    #[test]
    fn time_with_nothing_running_does_not_count() {
        let start = Instant::now();
        let mut state = step(&trying("Ethernet 3", start), start + minutes(9), 0, true);
        state = step(&state, start + minutes(69), 0, false);
        assert!(matches!(state, Listening::Trying { .. }), "{state:?}");
        state = step(&state, start + minutes(70), 0, false);
        assert!(matches!(state, Listening::Trying { .. }), "{state:?}");
        state = step(&state, start + minutes(71), 0, true);
        assert!(matches!(state, Listening::Closed { .. }), "{state:?}");
    }

    #[test]
    fn a_new_udp_cable_tries_again() {
        let now = Instant::now();
        let moved = |state| next(&state, now, 0, true, ETH2, TRY_FOR);
        let again = trying("Ethernet 2", now);
        let cable = "Ethernet 3".to_string();
        assert_eq!(
            moved(Listening::Open {
                cable: cable.clone()
            }),
            again
        );
        assert_eq!(
            moved(Listening::Closed {
                cable,
                why: "nobody".to_string()
            }),
            again
        );
        assert_eq!(moved(trying("Ethernet 3", now - minutes(9))), again);
    }

    #[test]
    fn the_clock_does_not_read_as_a_change() {
        let now = Instant::now();
        let later = step(&trying("Ethernet 3", now), now + minutes(1), 0, true);
        assert_ne!(later, trying("Ethernet 3", now));
        assert!(later.reads_like(&trying("Ethernet 3", now)));
        assert!(!later.reads_like(&trying("Ethernet 2", now)));
        assert!(!later.reads_like(&Listening::Off));
    }

    #[test]
    fn no_cable_is_off() {
        let now = Instant::now();
        assert_eq!(
            next(&trying("Ethernet 3", now), now, 0, true, None, TRY_FOR),
            Listening::Off
        );
    }

    /// A cable gone and back is a new chance, even a closed one's.
    #[test]
    fn a_closed_cable_back_after_going_tries_again() {
        let now = Instant::now();
        let closed = Listening::Closed {
            cable: "Ethernet 3".to_string(),
            why: "nobody".to_string(),
        };
        let gone = next(&closed, now, 0, true, None, TRY_FOR);
        assert_eq!(step(&gone, now, 0, true), trying("Ethernet 3", now));
    }

    #[test]
    fn the_try_is_the_callers_length() {
        let start = Instant::now();
        let closed = next(
            &trying("Ethernet 3", start),
            start + Duration::from_secs(1),
            0,
            true,
            ETH3,
            Duration::from_secs(1),
        );
        assert!(matches!(closed, Listening::Closed { .. }), "{closed:?}");
    }
}
