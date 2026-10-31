//! How many connections each link holds: a download's connections shared out
//! by what each link delivers (PLAN.md phase 4 item 2).
//!
//! A 40 Mbps link and a 5 Mbps one must not get equal counts: the slow one's
//! extra connections would only split its few megabits, and cost requests,
//! cuts and data thrown away with them. Links on one cable are one path, its
//! weight its links' speeds added (or, before they have carried, the cable's
//! speed test), and cables on one internet connection are one path, carried
//! by the fastest while the others sit out; the download's connections are
//! shared between paths by weight, then equally between a path's links. A
//! link the server limits keeps the ceiling it learned. Pure: the
//! coordinator passes the facts.

use crate::links::render::cable_speed;
use crate::links::{Link, LinkId, shared_connections};

/// One path's, or one link's, claim on the connections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Want {
    /// Its speed, or 0 when not measured.
    pub weight: u64,
    /// The most it may hold.
    pub most: u32,
}

/// One internet connection as a download uses it: the usable links on one
/// interface, carried by that cable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Path {
    pub carrier: String,
    pub links: Vec<LinkId>,
    /// Other cables taking part on the same internet connection: they sit
    /// out, holding no connections.
    pub others: Vec<String>,
}

/// Each want's share of `total` by weight, by largest remainder, ties to the
/// earlier; each at least `least` while `total` reaches, earlier first, and
/// its `most` allows; never over `most`. What a capped want cannot take is
/// left unused, not pushed onto slower ones. A weight of 0 counts as the
/// mean of the measured weights, or 1 when none is.
pub fn allocate(total: u32, least: u32, wants: &[Want]) -> Vec<u32> {
    if wants.is_empty() {
        return Vec::new();
    }
    let measured: Vec<u64> = wants.iter().map(|w| w.weight).filter(|&w| w > 0).collect();
    let mean = if measured.is_empty() {
        1.0
    } else {
        measured.iter().sum::<u64>() as f64 / measured.len() as f64
    };
    let weights: Vec<f64> = wants
        .iter()
        .map(|w| if w.weight > 0 { w.weight as f64 } else { mean })
        .collect();
    let sum: f64 = weights.iter().sum();
    let ideal: Vec<f64> = weights.iter().map(|w| f64::from(total) * w / sum).collect();
    let mut out: Vec<u32> = ideal.iter().map(|x| x.floor() as u32).collect();

    // What the floors leave, one each to the largest remainders.
    let mut order: Vec<usize> = (0..wants.len()).collect();
    let remainder = |i: usize| ideal[i] - ideal[i].floor();
    order.sort_by(|&a, &b| remainder(b).total_cmp(&remainder(a)).then(a.cmp(&b)));
    let mut left = total.saturating_sub(out.iter().sum());
    for &i in &order {
        if left == 0 {
            break;
        }
        out[i] += 1;
        left -= 1;
    }

    // The least, earlier first, each taken from the one holding most above
    // its own least, the later of equals.
    for i in 0..wants.len() {
        while out[i] < least.min(wants[i].most) {
            let donor = (0..wants.len())
                .filter(|&j| j != i && out[j] > least)
                .max_by_key(|&j| (out[j], j));
            let Some(j) = donor else {
                break;
            };
            out[j] -= 1;
            out[i] += 1;
        }
    }

    for (got, want) in out.iter_mut().zip(wants) {
        *got = (*got).min(want.most);
    }
    out
}

/// The paths the links in `links` that are `taking` part give a download, in
/// the order each first appears. A cable's usable links are one path, and
/// the cables [`shared_connections`] puts on one internet connection are one
/// too: its carrier is the cable with the fastest speed test, the first of
/// equals, and the other cables taking part are its `others`, holding no
/// connections, since using both gains nothing. Which cables share is read
/// from the whole table, so a cable not taking part still joins the two it
/// links.
pub fn paths(links: &[Link], taking: impl Fn(LinkId) -> bool) -> Vec<Path> {
    // One cable per interface: its links taking part, in table order.
    let mut cables: Vec<(&str, Vec<LinkId>)> = Vec::new();
    for link in links.iter().filter(|l| l.state.is_usable() && taking(l.id)) {
        match cables.iter_mut().find(|(name, _)| *name == link.name) {
            Some((_, ids)) => ids.push(link.id),
            None => cables.push((&link.name, vec![link.id])),
        }
    }
    let shared = shared_connections(links);
    let speed = |name: &str| {
        let cable: Vec<&Link> = links.iter().filter(|l| l.name == name).collect();
        cable_speed(&cable)
    };

    let mut out = Vec::new();
    let mut placed = vec![false; cables.len()];
    for first in 0..cables.len() {
        if placed[first] {
            continue;
        }
        let on = shared
            .iter()
            .find(|connection| connection.iter().any(|c| c == cables[first].0));
        let members: Vec<usize> = (first..cables.len())
            .filter(|&i| {
                i == first
                    || on.is_some_and(|connection| connection.iter().any(|c| c == cables[i].0))
            })
            .collect();
        for &i in &members {
            placed[i] = true;
        }
        // A cable not metered, whatever its speed: the connection is the
        // same, and what comes down a metered cable costs. Then the fastest
        // speed test, a figure before none; the first of equals.
        let metered = |name: &str| links.iter().any(|l| l.name == name && l.metered);
        let rank = |i: usize| (!metered(cables[i].0), speed(cables[i].0));
        let carrier = members
            .iter()
            .copied()
            .reduce(|best, i| if rank(i) > rank(best) { i } else { best })
            .expect("a path has its first cable");
        out.push(Path {
            carrier: cables[carrier].0.to_string(),
            links: cables[carrier].1.clone(),
            others: members
                .iter()
                .filter(|&&i| i != carrier)
                .map(|&i| cables[i].0.to_string())
                .collect(),
        });
    }
    out
}

/// A target moved only when the new one is two or more away, or the link
/// has none yet: what a link carried decides its weight, and when a server
/// limits each connection that feeds back, one connection lost becoming one
/// fewer wanted for good. The same feedback would keep a link one short for
/// good, so with `room` (connections the download is not using) it takes
/// one more.
pub fn settle(current: u32, computed: u32, room: bool) -> u32 {
    if current == 0 || computed.abs_diff(current) >= 2 || (room && computed == current + 1) {
        computed
    } else {
        current
    }
}

/// Each link's new target, in order, from its `current` one and the one
/// `computed`: [`settle`], with room while the targets so far leave some of
/// the download's `connections` unused. A link that takes one more uses it
/// up for those after; one that falls frees what it gave up.
pub fn settle_all(current: &[u32], computed: &[u32], connections: u32) -> Vec<u32> {
    let mut room = connections.saturating_sub(current.iter().sum());
    current
        .iter()
        .zip(computed)
        .map(|(&now, &n)| {
            let target = settle(now, n, room > 0);
            room = (room + now).saturating_sub(target);
            target
        })
        .collect()
}

/// A link taking part in a download, as its allowance sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Taker<'a> {
    pub id: LinkId,
    pub cable: &'a str,
    pub degraded: bool,
    /// The most it may hold otherwise (`most_per_link`, a ceiling, one on
    /// trial after a rest).
    pub most: u32,
}

/// The most each of `takers` may hold, in order. A degraded cable, back
/// from down and not trusted yet, holds one connection over all its
/// addresses, the first of them that may hold any: if it drops again it
/// takes one connection's part with it, and its addresses go down
/// together.
pub fn allowances(takers: &[Taker]) -> Vec<u32> {
    let mut holding_one: Vec<&str> = Vec::new();
    takers
        .iter()
        .map(|t| match (t.degraded, t.most) {
            (false, most) => most,
            (true, 0) => 0,
            (true, _) if !holding_one.contains(&t.cable) => {
                holding_one.push(t.cable);
                1
            }
            (true, _) => 0,
        })
        .collect()
}

/// Whether a link may hold more than at the last retarget, `before` being
/// `None` when it was not taking part: what it carried held to less says
/// nothing of what it carries now. A link joining keeps its speed test.
pub fn allowed_more(before: Option<u32>, now: u32) -> bool {
    before.is_some_and(|before| now > before)
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;

    use super::*;
    use crate::links::{Addr, LinkState};

    fn want(weight: u64, most: u32) -> Want {
        Want { weight, most }
    }

    #[test]
    fn a_40_mbps_link_and_a_5_mbps_link_do_not_get_equal_counts() {
        assert_eq!(allocate(16, 1, &[want(40, 8), want(5, 8)]), [8, 2]);
    }

    #[test]
    fn shares_follow_speed_by_largest_remainder() {
        // 13.37 and 2.63: the one left goes to the larger remainder.
        assert_eq!(allocate(16, 1, &[want(300, 24), want(59, 8)]), [13, 3]);
        // Equal remainders: the earlier.
        assert_eq!(allocate(3, 0, &[want(1, 8), want(1, 8)]), [2, 1]);
    }

    /// Review Focus 3: a link with no measured speed still gets connections.
    #[test]
    fn unmeasured_counts_as_the_mean_and_nothing_measured_is_equal() {
        assert_eq!(allocate(16, 1, &[want(0, 8), want(0, 8)]), [8, 8]);
        assert_eq!(allocate(16, 1, &[want(10, 8), want(0, 8)]), [8, 8]);
        assert_eq!(
            allocate(9, 1, &[want(10, 8), want(20, 8), want(0, 8)]),
            [2, 4, 3]
        );
    }

    #[test]
    fn least_goes_to_the_earlier_when_total_runs_short() {
        let five = [want(1, 8); 5];
        assert_eq!(allocate(3, 1, &five), [1, 1, 1, 0, 0]);
        // A share under one still gets the least, taken from the largest.
        assert_eq!(allocate(16, 1, &[want(1000, 16), want(1, 16)]), [15, 1]);
        // Never more than its most allows, even the least.
        assert_eq!(allocate(4, 1, &[want(1, 0), want(1, 8)]), [0, 2]);
    }

    #[test]
    fn a_capped_want_leaves_its_excess_unused() {
        // 14.2 for the first, capped at 8: the 6 over are not the slow one's.
        assert_eq!(
            allocate(16, 1, &[want(40, 8), want(5, 8)])
                .iter()
                .sum::<u32>(),
            10
        );
        assert_eq!(allocate(8, 1, &[want(4, 4), want(3, 4)]), [4, 3]);
        assert_eq!(allocate(5, 1, &[]), Vec::<u32>::new());
    }

    fn link(id: u64, name: &str, addr: &str, state: LinkState) -> Link {
        Link {
            id: LinkId::new(id),
            name: name.to_string(),
            addr: Addr::from(addr.parse::<IpAddr>().unwrap()),
            state,
            rtt: None,
            down_bps: None,
            public_ip: None,
            metered: false,
            tested: 0,
        }
    }

    #[test]
    fn a_cables_addresses_are_one_path() {
        let links = [
            link(1, "Wi-Fi", "192.0.2.10", LinkState::Up),
            link(2, "Ethernet 2", "198.51.100.7", LinkState::Up),
            link(3, "Wi-Fi", "2001:db8::10", LinkState::Degraded),
            link(4, "Wi-Fi", "2001:db8::11", LinkState::Probing),
            link(5, "Ethernet 3", "203.0.113.9", LinkState::Down),
        ];
        let got = paths(&links, |_| true);
        assert_eq!(
            got,
            [
                Path {
                    carrier: "Wi-Fi".into(),
                    links: ids(&[1, 3]),
                    others: Vec::new(),
                },
                Path {
                    carrier: "Ethernet 2".into(),
                    links: ids(&[2]),
                    others: Vec::new(),
                },
            ]
        );
        assert!(paths(&[], |_| true).is_empty());
    }

    fn ids(ns: &[u64]) -> Vec<LinkId> {
        ns.iter().map(|&n| LinkId::new(n)).collect()
    }

    /// An up link with its speed test, if any, and its public address.
    fn tested(id: u64, name: &str, addr: &str, bps: Option<u64>, public: &str) -> Link {
        Link {
            down_bps: bps,
            public_ip: Some(public.parse().unwrap()),
            ..link(id, name, addr, LinkState::Up)
        }
    }

    fn path(carrier: &str, links: &[u64], others: &[&str]) -> Path {
        Path {
            carrier: carrier.into(),
            links: ids(links),
            others: others.iter().map(|o| o.to_string()).collect(),
        }
    }

    /// The phone on the laptop's own Wi-Fi: two cables, one internet
    /// connection, so one path, on the faster cable's speed test.
    #[test]
    fn two_cables_behind_one_public_address_are_one_path_carried_by_the_faster() {
        let links = [
            tested(1, "Wi-Fi", "192.0.2.10", Some(20_000_000), "198.51.100.7"),
            tested(
                2,
                "Ethernet 2",
                "192.0.2.20",
                Some(50_000_000),
                "198.51.100.7",
            ),
        ];
        assert_eq!(
            paths(&links, |_| true),
            [path("Ethernet 2", &[2], &["Wi-Fi"])]
        );
    }

    /// A tether Windows calls metered, on the same connection as the Wi-Fi:
    /// faster or not, first in the table or not, the Wi-Fi carries it. Both
    /// metered, the faster again.
    #[test]
    fn a_shared_path_is_carried_by_the_cable_not_metered() {
        let phone = |metered| Link {
            metered,
            ..tested(
                1,
                "Ethernet 3",
                "192.0.2.40",
                Some(50_000_000),
                "198.51.100.7",
            )
        };
        let wifi = |metered| Link {
            metered,
            ..tested(2, "Wi-Fi", "192.0.2.10", Some(20_000_000), "198.51.100.7")
        };
        assert_eq!(
            paths(&[phone(true), wifi(false)], |_| true),
            [path("Wi-Fi", &[2], &["Ethernet 3"])]
        );
        assert_eq!(
            paths(&[wifi(false), phone(true)], |_| true),
            [path("Wi-Fi", &[2], &["Ethernet 3"])]
        );
        assert_eq!(
            paths(&[wifi(true), phone(true)], |_| true),
            [path("Ethernet 3", &[1], &["Wi-Fi"])]
        );
    }

    /// Ethernet 2's IPv4 address shares the Wi-Fi's public address, its IPv6
    /// address Ethernet 3's: all three are on one connection.
    #[test]
    fn a_cable_joined_through_a_third_is_on_the_same_path() {
        let links = [
            tested(1, "Wi-Fi", "192.0.2.10", Some(30_000_000), "198.51.100.7"),
            tested(
                2,
                "Ethernet 2",
                "192.0.2.20",
                Some(10_000_000),
                "198.51.100.7",
            ),
            tested(
                3,
                "Ethernet 2",
                "2001:db8::20",
                Some(10_000_000),
                "2001:db8::1",
            ),
            tested(
                4,
                "Ethernet 3",
                "2001:db8::30",
                Some(40_000_000),
                "2001:db8::1",
            ),
            tested(
                5,
                "Ethernet 4",
                "203.0.113.40",
                Some(5_000_000),
                "203.0.113.40",
            ),
        ];
        assert_eq!(
            paths(&links, |_| true),
            [
                path("Ethernet 3", &[4], &["Wi-Fi", "Ethernet 2"]),
                path("Ethernet 4", &[5], &[]),
            ]
        );
    }

    #[test]
    fn equal_speeds_carry_on_the_first_cable() {
        let shared = |wifi, ethernet| {
            [
                tested(1, "Wi-Fi", "192.0.2.10", wifi, "198.51.100.7"),
                tested(2, "Ethernet 2", "192.0.2.20", ethernet, "198.51.100.7"),
            ]
        };
        let equal = shared(Some(20_000_000), Some(20_000_000));
        assert_eq!(
            paths(&equal, |_| true),
            [path("Wi-Fi", &[1], &["Ethernet 2"])]
        );
        // Neither measured: alike, so the first.
        assert_eq!(
            paths(&shared(None, None), |_| true),
            [path("Wi-Fi", &[1], &["Ethernet 2"])]
        );
        // A speed test beats none.
        assert_eq!(
            paths(&shared(None, Some(1)), |_| true),
            [path("Ethernet 2", &[2], &["Wi-Fi"])]
        );
    }

    /// A link not taking part (resting, finding nothing, not asked for, held
    /// to no connections) neither carries a path nor sits out behind one; its
    /// public address still joins the cables it links.
    #[test]
    fn a_link_not_taking_part_neither_carries_nor_sits_out() {
        let links = [
            tested(1, "Wi-Fi", "192.0.2.10", Some(20_000_000), "198.51.100.7"),
            tested(
                2,
                "Ethernet 2",
                "192.0.2.20",
                Some(50_000_000),
                "198.51.100.7",
            ),
        ];
        assert_eq!(
            paths(&links, |id| id != LinkId::new(2)),
            [path("Wi-Fi", &[1], &[])]
        );

        let through = [
            tested(1, "Wi-Fi", "192.0.2.10", Some(20_000_000), "198.51.100.7"),
            tested(
                2,
                "Ethernet 2",
                "192.0.2.20",
                Some(90_000_000),
                "198.51.100.7",
            ),
            tested(
                3,
                "Ethernet 2",
                "2001:db8::20",
                Some(90_000_000),
                "2001:db8::1",
            ),
            tested(
                4,
                "Ethernet 3",
                "2001:db8::30",
                Some(40_000_000),
                "2001:db8::1",
            ),
        ];
        let resting = [LinkId::new(2), LinkId::new(3)];
        assert_eq!(
            paths(&through, |id| !resting.contains(&id)),
            [path("Ethernet 3", &[4], &["Wi-Fi"])]
        );

        // One address of a cable out: its other still carries.
        let dual = [
            tested(1, "Wi-Fi", "192.0.2.10", None, "198.51.100.7"),
            tested(2, "Wi-Fi", "2001:db8::10", None, "2001:db8::10"),
        ];
        assert_eq!(
            paths(&dual, |id| id != LinkId::new(1)),
            [path("Wi-Fi", &[2], &[])]
        );
    }

    #[test]
    fn a_target_moves_by_two_or_falls() {
        assert_eq!(settle(4, 3, false), 4, "one fewer is noise");
        assert_eq!(settle(4, 5, false), 4, "one more too");
        assert_eq!(settle(4, 2, false), 2);
        assert_eq!(settle(2, 4, false), 4);
        assert_eq!(settle(0, 3, false), 3, "a link with none yet takes it");
        assert_eq!(settle(3, 0, false), 0);
    }

    /// Review: a server limiting each connection makes what a link carries
    /// follow its count, so a link one short would stay one short while
    /// connections sit unused. With room it takes the one more; one fewer
    /// is still noise.
    /// The links' targets together: one more for a link one short only while
    /// the targets before it leave connections unused, the room it takes
    /// gone for those after.
    #[test]
    fn room_is_shared_out_in_order() {
        assert_eq!(settle_all(&[8, 5], &[8, 6], 16), [8, 6], "3 unused");
        assert_eq!(settle_all(&[8, 5], &[8, 6], 13), [8, 5], "none unused");
        assert_eq!(
            settle_all(&[5, 5, 5], &[6, 6, 6], 16),
            [6, 5, 5],
            "one unused, the first takes it"
        );
        assert_eq!(
            settle_all(&[8, 5], &[6, 7], 13),
            [6, 7],
            "a fall makes room"
        );
        assert_eq!(settle_all(&[], &[], 16), Vec::<u32>::new());
    }

    #[test]
    fn with_connections_free_a_target_grows_by_one() {
        assert_eq!(settle(5, 6, true), 6);
        assert_eq!(settle(5, 4, true), 5, "one fewer is noise, room or not");
        assert_eq!(settle(5, 8, true), 8);
    }

    fn taker(id: u64, cable: &'static str, degraded: bool, most: u32) -> Taker<'static> {
        Taker {
            id: LinkId::new(id),
            cable,
            degraded,
            most,
        }
    }

    /// A degraded cable holds one connection over all its addresses, the
    /// first that may hold any; another cable, and one trusted, keep theirs.
    #[test]
    fn a_degraded_cable_holds_one_over_all_its_addresses() {
        let got = allowances(&[
            taker(1, "Wi-Fi", false, 8),
            taker(2, "Ethernet 2", true, 8),
            taker(3, "Ethernet 2", true, 8),
            taker(4, "Ethernet 2", true, 8),
            taker(5, "Ethernet 3", true, 8),
        ]);
        assert_eq!(got, [8, 1, 0, 0, 1]);
        // One the server holds to none passes the one to the next.
        let got = allowances(&[
            taker(2, "Ethernet 2", true, 0),
            taker(3, "Ethernet 2", true, 8),
        ]);
        assert_eq!(got, [0, 1]);
        // One trusted beside one still degraded: the degraded one holds one.
        let got = allowances(&[
            taker(2, "Ethernet 2", false, 8),
            taker(3, "Ethernet 2", true, 8),
        ]);
        assert_eq!(got, [8, 1]);
    }

    /// Allowed more than at the last retarget: held to less, or none behind
    /// a degraded address of its cable. Not a link joining, which keeps its
    /// speed test, nor one allowed the same or less.
    #[test]
    fn allowed_more_is_a_rise_while_taking_part() {
        assert!(allowed_more(Some(0), 4), "held behind its cable's one");
        assert!(allowed_more(Some(1), 4), "held to one");
        assert!(!allowed_more(None, 4), "joining");
        assert!(!allowed_more(Some(4), 4));
        assert!(!allowed_more(Some(4), 2));
    }
}
