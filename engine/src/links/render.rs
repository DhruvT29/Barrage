//! Text views of the link table, for `brg-links`: the live table, and one
//! line per change for its log.
//!
//! Pure, so every layout rule is tested without a terminal. Here rather than
//! in the binary because only this crate can build a `Link` (its id is minted
//! by the watcher alone), so only tests in here can feed the table.
//!
//! LAYOUT
//!
//! One row per address, the rows of one cable kept together under its name,
//! in the order cables first appeared, so rows do not jump about between
//! redraws. **A cable's speed is shown once**, on its first row: on
//! 2026-10-13 the phone held three addresses over one cable, and three
//! copies of one figure invite adding them up (PLAN.md phase 3 item 11).
//! Links on different cables behind one internet connection carry a `*`
//! after their public address and a note under the table.

use std::net::IpAddr;

use super::model::{Link, LinkState};
use super::upstream::shared_upstreams;

const HEADER: [&str; 6] = ["NAME", "ADDRESS", "STATE", "RTT", "DOWN", "PUBLIC"];

/// The table: a header, one row per link, and a note per group of links that
/// share one internet connection. An empty table is the header alone.
pub fn render(links: &[Link]) -> String {
    let shared = sharing(links);
    let marked: Vec<_> = shared_upstreams(links).into_iter().flatten().collect();

    let mut rows: Vec<[String; 6]> = vec![HEADER.map(String::from)];
    for name in cables_in_order(links) {
        let cable: Vec<&Link> = links.iter().filter(|l| l.name == name).collect();
        // The cable's speed, from its working addresses: a dead address can
        // still hold the figure from before it died.
        let speed = cable
            .iter()
            .filter(|l| l.state == LinkState::Up)
            .find_map(|l| l.down_bps);
        for (i, link) in cable.iter().enumerate() {
            let first = i == 0;
            let mut public = public_text(link);
            if marked.contains(&link.id) {
                public.push_str(" *");
            }
            rows.push([
                if first {
                    name.to_string()
                } else {
                    String::new()
                },
                link.addr.ip().to_string(),
                state_word(link.state).to_string(),
                link.rtt
                    .map_or_else(dash, |rtt| format!("{} ms", rtt.as_millis())),
                if first {
                    speed.map_or_else(dash, mbps)
                } else {
                    String::new()
                },
                public,
            ]);
        }
    }

    let widths: Vec<usize> = (0..HEADER.len())
        .map(|c| rows.iter().map(|r| r[c].chars().count()).max().unwrap_or(0))
        .collect();
    let mut out = String::new();
    for row in &rows {
        let cells: Vec<String> = row
            .iter()
            .zip(&widths)
            .map(|(cell, &width)| format!("{cell:<width$}"))
            .collect();
        out.push_str(cells.join("  ").trim_end());
        out.push('\n');
    }
    for (names, public) in shared {
        out.push_str(&format!(
            "* {names} reach the internet through one connection ({public}), so using both gains nothing.\n"
        ));
    }
    out
}

/// What changed between two snapshots, one line per change: links appearing
/// and going, state, speed and public address changes, and sharing starting
/// or ending. A changed RTT alone is not a line: every probe moves it.
pub fn changes(before: &[Link], after: &[Link]) -> Vec<String> {
    let mut out = Vec::new();
    for old in before {
        if !after.iter().any(|l| l.id == old.id) {
            out.push(format!("gone: {}", label(old)));
        }
    }
    for new in after {
        let Some(old) = before.iter().find(|l| l.id == new.id) else {
            let mut line = format!("appeared: {} {}", label(new), state_word(new.state));
            if let Some(rtt) = new.rtt {
                line.push_str(&format!(", rtt {} ms", rtt.as_millis()));
            }
            if let Some(bps) = new.down_bps {
                line.push_str(&format!(", speed {}", mbps(bps)));
            }
            if let Some(public) = new.public_ip {
                line.push_str(&format!(", public {public}"));
            }
            out.push(line);
            continue;
        };
        if old.state != new.state {
            let mut line = format!(
                "{}: {} -> {}",
                label(new),
                state_word(old.state),
                state_word(new.state)
            );
            if let Some(rtt) = new.rtt {
                line.push_str(&format!(", rtt {} ms", rtt.as_millis()));
            }
            out.push(line);
        }
        if old.down_bps != new.down_bps {
            let speed = new.down_bps.map_or_else(|| "cleared".to_string(), mbps);
            out.push(format!("{}: speed {speed}", label(new)));
        }
        if old.public_ip != new.public_ip {
            let public = new
                .public_ip
                .map_or_else(|| "cleared".to_string(), |p| p.to_string());
            out.push(format!("{}: public {public}", label(new)));
        }
    }

    let was = sharing(before);
    let now = sharing(after);
    for (names, public) in &now {
        if !was.contains(&(names.clone(), *public)) {
            out.push(format!("shared internet: {names} ({public})"));
        }
    }
    for (names, public) in &was {
        if !now.contains(&(names.clone(), *public)) {
            out.push(format!("no longer shared: {names} ({public})"));
        }
    }
    out
}

/// Each group of links sharing one internet connection, as the names of its
/// cables ("Wi-Fi and Ethernet 2") and the public address they share.
fn sharing(links: &[Link]) -> Vec<(String, IpAddr)> {
    shared_upstreams(links)
        .into_iter()
        .filter_map(|group| {
            let members: Vec<&Link> = group
                .iter()
                .filter_map(|id| links.iter().find(|l| l.id == *id))
                .collect();
            let public = members.first()?.public_ip?;
            let mut names: Vec<&str> = Vec::new();
            for m in &members {
                if !names.contains(&m.name.as_str()) {
                    names.push(&m.name);
                }
            }
            Some((join_names(&names), public))
        })
        .collect()
}

/// "A", "A and B", "A, B and C".
fn join_names(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [only] => only.to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// Cable names in the order each first appears.
fn cables_in_order(links: &[Link]) -> Vec<&str> {
    let mut names: Vec<&str> = Vec::new();
    for link in links {
        if !names.contains(&link.name.as_str()) {
            names.push(&link.name);
        }
    }
    names
}

fn public_text(link: &Link) -> String {
    match link.public_ip {
        None => dash(),
        Some(public) if public == link.addr.ip() => "same as address".to_string(),
        Some(public) => public.to_string(),
    }
}

fn label(link: &Link) -> String {
    format!("{} {}", link.name, link.addr.ip())
}

/// The word for a state, in the table and in the UI's JSON alike.
pub(crate) fn state_word(state: LinkState) -> &'static str {
    match state {
        LinkState::Probing => "probing",
        LinkState::Up => "up",
        LinkState::Portal => "portal",
        LinkState::Degraded => "degraded",
        LinkState::Down => "down",
    }
}

fn mbps(bps: u64) -> String {
    format!("{:.1} Mbps", bps as f64 / 1e6)
}

fn dash() -> String {
    "-".to_string()
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;
    use std::time::Duration;

    use super::*;
    use crate::links::model::{Addr, LinkId};

    use LinkState::{Down, Portal, Probing, Up};

    fn link(
        id: u64,
        name: &str,
        ip: &str,
        state: LinkState,
        rtt_ms: Option<u64>,
        bps: Option<u64>,
    ) -> Link {
        Link {
            id: LinkId::new(id),
            name: name.to_string(),
            addr: Addr::from(ip.parse::<IpAddr>().unwrap()),
            state,
            rtt: rtt_ms.map(Duration::from_millis),
            down_bps: bps,
            public_ip: None,
            metered: false,
        }
    }

    fn with_public(mut l: Link, public: &str) -> Link {
        l.public_ip = Some(public.parse().unwrap());
        l
    }

    // The plan's three tests.

    #[test]
    fn renders_a_header_and_one_row_per_link() {
        let out = render(&[link(
            1,
            "Wi-Fi",
            "192.0.2.10",
            Up,
            Some(14),
            Some(41_200_000),
        )]);
        assert!(out.contains("NAME"), "{out}");
        assert!(out.contains("Wi-Fi"), "{out}");
        assert!(out.contains("14 ms"), "{out}");
        assert!(out.contains("41.2 Mbps"), "{out}");
        assert_eq!(out.lines().count(), 2, "{out}");
    }

    #[test]
    fn renders_portal_state_distinctly_from_up() {
        let out = render(&[link(1, "Hotel", "192.0.2.30", Portal, Some(40), None)]);
        assert!(out.contains("portal"), "{out}");
        assert!(!out.contains(" up "), "{out}");
    }

    // Review Focus 4
    #[test]
    fn renders_a_header_and_no_rows_when_there_are_no_links() {
        let out = render(&[]);
        assert!(out.contains("NAME"), "{out}");
        assert_eq!(out.lines().count(), 1, "{out}");
        assert_eq!(out.lines().filter(|l| l.contains("Mbps")).count(), 0);
    }

    // The table, beyond the plan.

    /// Phase 3 item 11: the phone's addresses are one cable. Its speed is
    /// shown once, on the cable's first line, so nobody adds three copies.
    /// Its rows stay together even when another cable appeared in between.
    #[test]
    fn a_cables_addresses_are_grouped_and_its_speed_shown_once() {
        let out = render(&[
            link(
                1,
                "Ethernet 2",
                "192.0.2.20",
                Up,
                Some(120),
                Some(12_100_000),
            ),
            link(2, "Wi-Fi", "192.0.2.10", Up, Some(37), Some(62_800_000)),
            link(
                3,
                "Ethernet 2",
                "2001:db8::20",
                Up,
                Some(130),
                Some(12_100_000),
            ),
        ]);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 4, "{out}");
        assert!(lines[1].starts_with("Ethernet 2"), "{out}");
        assert!(lines[2].trim_start().starts_with("2001:db8::20"), "{out}");
        assert!(lines[3].starts_with("Wi-Fi"), "{out}");
        assert_eq!(out.matches("12.1 Mbps").count(), 1, "{out}");
    }

    /// The speed belongs to the cable and comes from its working addresses,
    /// not from whichever row happens to be first.
    #[test]
    fn a_cables_speed_comes_from_its_working_addresses() {
        let out = render(&[
            link(1, "Ethernet 2", "192.0.2.20", Down, None, Some(99_000_000)),
            link(
                2,
                "Ethernet 2",
                "2001:db8::20",
                Up,
                Some(130),
                Some(12_100_000),
            ),
        ]);
        assert!(out.lines().nth(1).unwrap().contains("12.1 Mbps"), "{out}");
        assert!(!out.contains("99.0 Mbps"), "{out}");
    }

    #[test]
    fn unknown_values_show_a_dash() {
        let out = render(&[link(1, "Ethernet 2", "192.0.2.20", Probing, None, None)]);
        let row = out.lines().nth(1).unwrap();
        assert!(row.contains("probing"), "{out}");
        assert_eq!(
            row.split_whitespace().filter(|c| *c == "-").count(),
            3,
            "{out}"
        );
    }

    /// IPv6 has no address sharing, so a link's public address is its own.
    #[test]
    fn a_public_address_equal_to_the_links_own_says_so() {
        let v6 = with_public(
            link(1, "Ethernet 2", "2001:db8::20", Up, Some(130), None),
            "2001:db8::20",
        );
        let v4 = with_public(
            link(2, "Wi-Fi", "192.0.2.10", Up, Some(37), None),
            "198.51.100.7",
        );
        let out = render(&[v6, v4]);
        assert!(out.contains("same as address"), "{out}");
        assert!(out.contains("198.51.100.7"), "{out}");
    }

    /// Seen 2026-10-14: the phone joined to the laptop's Wi-Fi.
    #[test]
    fn links_sharing_one_internet_are_marked_and_explained() {
        let out = render(&[
            with_public(
                link(1, "Wi-Fi", "192.0.2.10", Up, Some(37), Some(62_800_000)),
                "198.51.100.99",
            ),
            with_public(
                link(2, "Ethernet 2", "192.0.2.20", Up, Some(90), Some(7_400_000)),
                "198.51.100.99",
            ),
        ]);
        let lines: Vec<&str> = out.lines().collect();
        assert!(lines[1].contains("198.51.100.99 *"), "{out}");
        assert!(lines[2].contains("198.51.100.99 *"), "{out}");
        let note = lines.last().unwrap();
        assert!(note.starts_with('*'), "{out}");
        assert!(note.contains("Wi-Fi and Ethernet 2"), "{out}");
        assert!(note.contains("198.51.100.99"), "{out}");
    }

    #[test]
    fn the_columns_line_up() {
        let out = render(&[
            link(1, "Wi-Fi", "192.0.2.10", Up, Some(37), Some(62_800_000)),
            link(2, "Ethernet 2", "2001:db8::20", Probing, None, None),
        ]);
        let lines: Vec<&str> = out.lines().collect();
        let column = lines[0].find("ADDRESS").unwrap();
        assert_eq!(lines[1].find("192.0.2.10"), Some(column), "{out}");
        assert_eq!(lines[2].find("2001:db8::20"), Some(column), "{out}");
        let state = lines[0].find("STATE").unwrap();
        assert_eq!(lines[1].find("up"), Some(state), "{out}");
        assert_eq!(lines[2].find("probing"), Some(state), "{out}");
    }

    // The log.

    #[test]
    fn a_new_link_is_logged_as_appearing() {
        let after = [link(1, "Ethernet 2", "192.0.2.20", Probing, None, None)];
        assert_eq!(
            changes(&[], &after),
            ["appeared: Ethernet 2 192.0.2.20 probing"]
        );
    }

    #[test]
    fn a_state_change_is_logged_with_its_rtt() {
        let before = [link(1, "Ethernet 2", "192.0.2.20", Probing, None, None)];
        let after = [link(1, "Ethernet 2", "192.0.2.20", Up, Some(87), None)];
        assert_eq!(
            changes(&before, &after),
            ["Ethernet 2 192.0.2.20: probing -> up, rtt 87 ms"]
        );
    }

    #[test]
    fn speed_and_public_address_are_logged_when_set_and_when_cleared() {
        let bare = link(1, "Ethernet 2", "192.0.2.20", Up, Some(87), None);
        let known = with_public(
            link(
                1,
                "Ethernet 2",
                "192.0.2.20",
                Up,
                Some(87),
                Some(12_100_000),
            ),
            "198.51.100.7",
        );
        assert_eq!(
            changes(std::slice::from_ref(&bare), std::slice::from_ref(&known)),
            [
                "Ethernet 2 192.0.2.20: speed 12.1 Mbps",
                "Ethernet 2 192.0.2.20: public 198.51.100.7",
            ]
        );
        assert_eq!(
            changes(&[known], &[bare]),
            [
                "Ethernet 2 192.0.2.20: speed cleared",
                "Ethernet 2 192.0.2.20: public cleared",
            ]
        );
    }

    #[test]
    fn a_vanished_link_is_logged_as_gone() {
        let before = [link(1, "Ethernet 2", "192.0.2.20", Up, Some(87), None)];
        assert_eq!(changes(&before, &[]), ["gone: Ethernet 2 192.0.2.20"]);
    }

    /// Every probe moves the RTT a little; logging each would bury the
    /// changes that matter.
    #[test]
    fn an_rtt_change_alone_is_not_logged() {
        let before = [link(1, "Wi-Fi", "192.0.2.10", Up, Some(37), None)];
        let after = [link(1, "Wi-Fi", "192.0.2.10", Up, Some(41), None)];
        assert!(changes(&before, &after).is_empty());
    }

    #[test]
    fn sharing_is_logged_when_it_starts_and_when_it_ends() {
        let wifi = with_public(
            link(1, "Wi-Fi", "192.0.2.10", Up, Some(37), None),
            "198.51.100.99",
        );
        let phone = link(2, "Ethernet 2", "192.0.2.20", Up, Some(90), None);
        let phone_on_wifi = with_public(phone.clone(), "198.51.100.99");
        let alone = [wifi.clone(), phone];
        let sharing = [wifi, phone_on_wifi];

        let started = changes(&alone, &sharing);
        assert!(
            started.contains(&"shared internet: Wi-Fi and Ethernet 2 (198.51.100.99)".to_string()),
            "{started:?}"
        );
        let ended = changes(&sharing, &alone);
        assert!(
            ended.contains(&"no longer shared: Wi-Fi and Ethernet 2 (198.51.100.99)".to_string()),
            "{ended:?}"
        );
    }
}
