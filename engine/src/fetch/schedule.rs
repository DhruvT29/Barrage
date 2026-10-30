//! How many connections each link holds: a download's connections shared out
//! by what each link delivers (PLAN.md phase 4 item 2).
//!
//! A 40 Mbps link and a 5 Mbps one must not get equal counts: the slow one's
//! extra connections would only split its few megabits, and cost requests,
//! cuts and data thrown away with them. Links on one cable are one path, its
//! weight its links' speeds added (or, before they have carried, the cable's
//! speed test); the download's connections are shared between paths by
//! weight, then equally between a path's links. A link the server limits
//! keeps the ceiling it learned. Pure: the coordinator passes the facts.

use crate::links::{Link, LinkId};

/// One path's, or one link's, claim on the connections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Want {
    /// Its speed, or 0 when not measured.
    pub weight: u64,
    /// The most it may hold.
    pub most: u32,
}

/// The usable links on one interface, carried by that cable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Path {
    pub carrier: String,
    pub links: Vec<LinkId>,
    /// Other cables on the same internet connection (Task 4).
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

/// The usable links in `links`, one path per interface, in the order each
/// first appears.
pub fn paths(links: &[Link]) -> Vec<Path> {
    let mut out: Vec<Path> = Vec::new();
    for link in links.iter().filter(|l| l.state.is_usable()) {
        match out.iter_mut().find(|p| p.carrier == link.name) {
            Some(path) => path.links.push(link.id),
            None => out.push(Path {
                carrier: link.name.clone(),
                links: vec![link.id],
                others: Vec::new(),
            }),
        }
    }
    out
}

/// A target moved only when the new one is two or more away, or the link
/// has none yet: what a link carried decides its weight, and when a server
/// limits each connection that feeds back, one connection lost becoming one
/// fewer wanted for good.
pub fn settle(current: u32, computed: u32) -> u32 {
    if current == 0 || computed.abs_diff(current) >= 2 {
        computed
    } else {
        current
    }
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
        let got = paths(&links);
        let ids = |ns: &[u64]| ns.iter().map(|&n| LinkId::new(n)).collect::<Vec<_>>();
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
        assert!(paths(&[]).is_empty());
    }

    #[test]
    fn a_target_moves_by_two_or_falls() {
        assert_eq!(settle(4, 3), 4, "one fewer is noise");
        assert_eq!(settle(4, 5), 4, "one more too");
        assert_eq!(settle(4, 2), 2);
        assert_eq!(settle(2, 4), 4);
        assert_eq!(settle(0, 3), 3, "a link with none yet takes it");
        assert_eq!(settle(3, 0), 0);
    }
}
