//! The link table as the UI reads it.
//!
//! Every figure says the same as `brg-links`: the state words, whole
//! milliseconds, and the change lines, which come from
//! [`links::render::changes`](crate::links::render::changes) rather than being
//! worked out again in the UI. An unknown figure is `null`, never zero.
//! Addresses are strings, so the JSON does not depend on how serde encodes an
//! `IpAddr`.

use serde::Serialize;

use crate::links::render::{changes, state_word};
use crate::links::{Link, shared_upstreams};

/// One link: one address on one interface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LinkView {
    pub id: u64,
    /// The interface. Links with the same name share one cable.
    pub name: String,
    pub address: String,
    /// `"ipv4"` or `"ipv6"`.
    pub family: &'static str,
    /// `"probing"`, `"up"`, `"portal"`, `"degraded"` or `"down"`.
    pub state: &'static str,
    pub rtt_ms: Option<u64>,
    /// The cable's measured speed, in bits per second.
    pub down_bps: Option<u64>,
    pub public_ip: Option<String>,
}

/// The whole table, in the order links appeared.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TableView {
    pub links: Vec<LinkView>,
    /// Groups of link ids on different cables that reach the internet
    /// through one connection. See [`shared_upstreams`].
    pub shared: Vec<Vec<u64>>,
}

/// What the live socket sends: the whole table when a client connects, then
/// the table again on every change, with the lines saying what changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LiveMessage {
    Table {
        table: TableView,
    },
    Change {
        table: TableView,
        lines: Vec<String>,
    },
}

pub fn table_view(links: &[Link]) -> TableView {
    TableView {
        links: links.iter().map(link_view).collect(),
        shared: shared_upstreams(links)
            .into_iter()
            .map(|group| group.into_iter().map(|id| id.as_u64()).collect())
            .collect(),
    }
}

/// The table after a change, with the lines `brg-links` would log for it.
/// The lines may be empty: an RTT moving logs nothing, but the page still
/// shows the new figure.
pub fn change_message(before: &[Link], after: &[Link]) -> LiveMessage {
    LiveMessage::Change {
        table: table_view(after),
        lines: changes(before, after),
    }
}

fn link_view(link: &Link) -> LinkView {
    let ip = link.addr.ip();
    LinkView {
        id: link.id.as_u64(),
        name: link.name.clone(),
        address: ip.to_string(),
        family: if ip.is_ipv4() { "ipv4" } else { "ipv6" },
        state: state_word(link.state),
        rtt_ms: link.rtt.map(|rtt| rtt.as_millis() as u64),
        down_bps: link.down_bps,
        public_ip: link.public_ip.map(|ip| ip.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;
    use std::time::Duration;

    use serde_json::json;

    use super::*;
    use crate::links::model::{Addr, LinkId};
    use crate::links::render::changes;
    use crate::links::{Link, LinkState};

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

    fn up(id: u64, name: &str, addr: &str, rtt_ms: u64, bps: u64, public: &str) -> Link {
        Link {
            rtt: Some(Duration::from_millis(rtt_ms)),
            down_bps: Some(bps),
            public_ip: Some(public.parse().unwrap()),
            ..link(id, name, addr, LinkState::Up)
        }
    }

    fn json_of(links: &[Link]) -> serde_json::Value {
        serde_json::to_value(table_view(links)).unwrap()
    }

    #[test]
    fn a_link_becomes_the_json_the_ui_reads() {
        let wifi = up(1, "Wi-Fi", "192.0.2.10", 40, 57_600_000, "203.0.113.7");
        assert_eq!(
            json_of(&[wifi]),
            json!({
                "links": [{
                    "id": 1, "name": "Wi-Fi", "address": "192.0.2.10", "family": "ipv4",
                    "state": "up", "rtt_ms": 40, "down_bps": 57600000,
                    "public_ip": "203.0.113.7"
                }],
                "shared": []
            })
        );
    }

    /// Absent and zero are different facts (`Link::rtt`'s doc). A UI that
    /// shows 0 Mbps for "not measured yet" says something false.
    #[test]
    fn unknown_figures_are_null_not_zero() {
        let json = json_of(&[link(1, "Wi-Fi", "192.0.2.10", LinkState::Probing)]);
        let view = &json["links"][0];
        for field in ["rtt_ms", "down_bps", "public_ip"] {
            assert!(view[field].is_null(), "{field} is {}", view[field]);
        }
    }

    #[test]
    fn every_state_has_its_word() {
        let states = [
            (LinkState::Probing, "probing"),
            (LinkState::Up, "up"),
            (LinkState::Portal, "portal"),
            (LinkState::Degraded, "degraded"),
            (LinkState::Down, "down"),
        ];
        for (state, word) in states {
            let json = json_of(&[link(1, "Wi-Fi", "192.0.2.10", state)]);
            assert_eq!(json["links"][0]["state"], word);
        }
    }

    #[test]
    fn an_ipv6_link_says_so() {
        let json = json_of(&[link(1, "Ethernet 2", "2001:db8::20", LinkState::Up)]);
        assert_eq!(json["links"][0]["family"], "ipv6");
        assert_eq!(json["links"][0]["address"], "2001:db8::20");
    }

    /// Whole milliseconds, as `brg-links` shows them: 40.7 ms is "40".
    #[test]
    fn rtt_is_whole_milliseconds() {
        let mut wifi = link(1, "Wi-Fi", "192.0.2.10", LinkState::Up);
        wifi.rtt = Some(Duration::from_micros(40_700));
        assert_eq!(json_of(&[wifi])["links"][0]["rtt_ms"], 40);
    }

    #[test]
    fn shared_groups_are_ids() {
        let links = [
            up(1, "Wi-Fi", "192.0.2.10", 40, 57_600_000, "203.0.113.7"),
            up(2, "Ethernet 2", "192.0.2.20", 60, 9_000_000, "203.0.113.7"),
        ];
        assert_eq!(json_of(&links)["shared"], json!([[1, 2]]));
    }

    #[test]
    fn a_message_is_tagged_by_its_type() {
        let wifi = [up(1, "Wi-Fi", "192.0.2.10", 40, 57_600_000, "203.0.113.7")];
        let table = serde_json::to_value(LiveMessage::Table {
            table: table_view(&wifi),
        })
        .unwrap();
        assert_eq!(table["type"], "table");
        assert_eq!(table["table"]["links"][0]["name"], "Wi-Fi");

        let change = serde_json::to_value(change_message(&[], &wifi)).unwrap();
        assert_eq!(change["type"], "change");
        assert_eq!(change["table"]["links"][0]["name"], "Wi-Fi");
        assert!(change["lines"].is_array());
    }

    /// The feed on the page and the log `brg-links --log` writes are the
    /// same lines, worked out once, here.
    #[test]
    fn change_lines_are_the_ones_brg_links_logs() {
        let before = [link(1, "Wi-Fi", "192.0.2.10", LinkState::Probing)];
        let after = [up(1, "Wi-Fi", "192.0.2.10", 40, 57_600_000, "203.0.113.7")];
        let LiveMessage::Change { lines, .. } = change_message(&before, &after) else {
            panic!("expected a change");
        };
        assert!(!lines.is_empty());
        assert_eq!(lines, changes(&before, &after));
    }

    /// An RTT-only change logs nothing, but the table still moves, so the
    /// page shows the new figure.
    #[test]
    fn a_change_with_no_lines_still_carries_the_table() {
        let before = [up(1, "Wi-Fi", "192.0.2.10", 40, 57_600_000, "203.0.113.7")];
        let after = [up(1, "Wi-Fi", "192.0.2.10", 55, 57_600_000, "203.0.113.7")];
        let LiveMessage::Change { table, lines } = change_message(&before, &after) else {
            panic!("expected a change");
        };
        assert!(lines.is_empty());
        assert_eq!(table.links[0].rtt_ms, Some(55));
    }
}
