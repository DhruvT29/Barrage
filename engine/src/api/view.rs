//! The link table and the downloads as the UI reads them.
//!
//! Every figure says the same as `brg-links`: the state words, whole
//! milliseconds, and the change lines, which come from
//! [`links::render::changes`](crate::links::render::changes) rather than being
//! worked out again in the UI. An unknown figure is `null`, never zero.
//! Addresses are strings, so the JSON does not depend on how serde encodes an
//! `IpAddr`. A download is shown by cable, not by address: a cable's bytes
//! and speed are what its addresses really carried, added up.

use serde::Serialize;

use crate::fetch::{CableUse, DownloadStatus, LinkShare, Phase};
use crate::links::render::{cable_speed, cables_in_order, changes, state_word};
use crate::links::{Link, LinkState, shared_connections, shared_upstreams};

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
    /// Whether Windows says the connection it is on is metered.
    pub metered: bool,
}

/// The whole table, in the order links appeared.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TableView {
    pub links: Vec<LinkView>,
    /// Groups of link ids on different cables that reach the internet
    /// through one connection. See [`shared_upstreams`].
    pub shared: Vec<Vec<u64>>,
    /// The cables on each shared connection, by name, in table order: joined
    /// by [`shared_upstreams`] directly or through another cable, measured or
    /// not. Only connections of two or more cables are listed. See
    /// [`shared_connections`].
    pub connections: Vec<Vec<String>>,
    pub combined: CombinedView,
}

/// The combined speed available: what using every connection at once could
/// give, from what has been measured.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CombinedView {
    /// The sum of the parts, in bits per second. 0 only when there are no parts.
    pub bps: u64,
    /// One per independent connection, in the order its first cable appeared.
    pub parts: Vec<CombinedPart>,
    /// The cables left out of the sum, in table order, and why.
    pub not_counted: Vec<NotCounted>,
}

/// One independent connection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CombinedPart {
    /// Its fastest cable.
    pub name: String,
    /// That cable's measured speed, in bits per second.
    pub bps: u64,
    /// The other measured cables that reach the internet through the same
    /// connection, so are counted once, here.
    pub shared_with: Vec<String>,
}

/// A cable that adds nothing to the sum.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NotCounted {
    pub name: String,
    /// `"not measured yet"` when it works, else its state word: `"portal"`,
    /// `"probing"` or `"down"`.
    pub why: &'static str,
}

/// Every download since the engine started, and the speed they make together.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DownloadsView {
    pub live: LiveSpeed,
    /// In the order they were added.
    pub list: Vec<DownloadView>,
    /// What each cable has received for every download since the engine
    /// started, written or not, in the order cables first carried.
    pub used: Vec<UseView>,
}

/// The data one cable has received.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UseView {
    pub cable: String,
    pub bytes: u64,
}

/// The live download speed: what the downloads that are downloading are
/// fetching right now, all together.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LiveSpeed {
    /// The sum of the parts, in bits per second.
    pub bps: u64,
    /// One per cable carrying data, its speed in every download added up, in
    /// the order it first appears in the list. A cable carrying nothing has
    /// no part.
    pub parts: Vec<SpeedPart>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SpeedPart {
    pub name: String,
    pub bps: u64,
}

/// One download.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DownloadView {
    pub id: u64,
    /// The URL as it was given.
    pub url: String,
    /// The file's name in the download folder; null until the file is found.
    pub name: Option<String>,
    /// `"starting"`, `"waiting"`, `"downloading"`, `"done"`, `"failed"` or
    /// `"stopped"`.
    pub state: &'static str,
    /// In bytes; null while the server has not said.
    pub length: Option<u64>,
    /// Bytes written.
    pub done: u64,
    /// In bits per second.
    pub bps: u64,
    /// Why it failed, or what to know about it.
    pub note: Option<String>,
    pub cables: Vec<CableShare>,
}

/// One cable's part in a download: its addresses' shares added up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CableShare {
    pub name: String,
    /// Bytes its addresses wrote to the file.
    pub bytes: u64,
    /// In bits per second, 0 unless downloading.
    pub bps: u64,
    /// Connections open for this download right now.
    pub connections: u32,
    /// The connections the download wants it to hold, by what it delivers:
    /// `connections` moves toward it one at a time.
    pub wanted: u32,
    /// Each of its addresses' notes, in order, each said once.
    pub notes: Vec<String>,
}

/// What the live socket sends: the whole table when a client connects, then
/// the downloads, then each change to either: the table with the lines saying
/// what changed, or the downloads again.
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
    Downloads {
        downloads: DownloadsView,
    },
}

pub fn table_view(links: &[Link]) -> TableView {
    TableView {
        links: links.iter().map(link_view).collect(),
        shared: shared_upstreams(links)
            .into_iter()
            .map(|group| group.into_iter().map(|id| id.as_u64()).collect())
            .collect(),
        connections: shared_connections(links),
        combined: combined(links),
    }
}

/// Each cable's speed, by [`cable_speed`]; the measured cables on one of the
/// [`shared_connections`] are one part, at the speed of the fastest.
fn combined(links: &[Link]) -> CombinedView {
    let mut measured: Vec<(&str, u64)> = Vec::new();
    let mut not_counted = Vec::new();
    for name in cables_in_order(links) {
        let cable: Vec<&Link> = links.iter().filter(|l| l.name == name).collect();
        match cable_speed(&cable) {
            Some(bps) => measured.push((name, bps)),
            None => not_counted.push(NotCounted {
                name: name.to_string(),
                why: why_not_counted(&cable),
            }),
        }
    }

    // Each measured cable is numbered by the first measured cable on its
    // connection, or by itself when it shares with nobody. Parts come out in
    // the order their first measured cable appeared.
    let shared = shared_connections(links);
    let connection: Vec<usize> = measured
        .iter()
        .enumerate()
        .map(|(i, &(name, _))| {
            let on = shared
                .iter()
                .find(|cables| cables.iter().any(|c| c == name));
            measured
                .iter()
                .position(|m| on.is_some_and(|cables| cables.iter().any(|c| c == m.0)))
                .unwrap_or(i)
        })
        .collect();

    let mut parts = Vec::new();
    for first in 0..measured.len() {
        if connection[first] != first {
            continue;
        }
        let members: Vec<(&str, u64)> = (0..measured.len())
            .filter(|&i| connection[i] == first)
            .map(|i| measured[i])
            .collect();
        // The fastest; the first of equals.
        let (name, bps) = members
            .iter()
            .copied()
            .reduce(|best, m| if m.1 > best.1 { m } else { best })
            .expect("a connection has its first cable");
        parts.push(CombinedPart {
            name: name.to_string(),
            bps,
            shared_with: members
                .iter()
                .filter(|m| m.0 != name)
                .map(|m| m.0.to_string())
                .collect(),
        });
    }

    CombinedView {
        bps: parts.iter().map(|p| p.bps).sum(),
        parts,
        not_counted,
    }
}

/// Why a cable with no usable figure adds nothing: it works but has not been
/// measured, or its best state says it cannot be used.
fn why_not_counted(cable: &[&Link]) -> &'static str {
    if cable.iter().any(|l| l.state.is_usable()) {
        return "not measured yet";
    }
    for state in [LinkState::Portal, LinkState::Probing] {
        if cable.iter().any(|l| l.state == state) {
            return state_word(state);
        }
    }
    state_word(LinkState::Down)
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
        metered: link.metered,
    }
}

pub fn downloads_view(all: &[DownloadStatus], used: &[CableUse]) -> DownloadsView {
    let list: Vec<DownloadView> = all.iter().map(download_view).collect();

    let mut parts: Vec<SpeedPart> = Vec::new();
    let downloading = all
        .iter()
        .zip(&list)
        .filter(|(status, _)| status.phase == Phase::Downloading);
    for (_, download) in downloading {
        for cable in download.cables.iter().filter(|c| c.bps > 0) {
            match parts.iter_mut().find(|p| p.name == cable.name) {
                Some(part) => part.bps += cable.bps,
                None => parts.push(SpeedPart {
                    name: cable.name.clone(),
                    bps: cable.bps,
                }),
            }
        }
    }

    DownloadsView {
        live: LiveSpeed {
            bps: parts.iter().map(|p| p.bps).sum(),
            parts,
        },
        list,
        used: used
            .iter()
            .map(|u| UseView {
                cable: u.cable.clone(),
                bytes: u.bytes,
            })
            .collect(),
    }
}

fn download_view(status: &DownloadStatus) -> DownloadView {
    DownloadView {
        id: status.id.as_u64(),
        url: status.url.clone(),
        name: status.name.clone(),
        state: phase_word(status.phase),
        length: status.length,
        done: status.done,
        bps: status.bps,
        note: status.note.clone(),
        cables: cable_shares(&status.links),
    }
}

fn phase_word(phase: Phase) -> &'static str {
    match phase {
        Phase::Starting => "starting",
        Phase::Waiting => "waiting",
        Phase::Downloading => "downloading",
        Phase::Done => "done",
        Phase::Failed => "failed",
        Phase::Stopped => "stopped",
    }
}

/// Each cable's addresses' shares added up, in the order the cable's first
/// address joined.
fn cable_shares(links: &[LinkShare]) -> Vec<CableShare> {
    let mut cables: Vec<CableShare> = Vec::new();
    for link in links {
        let i = match cables.iter().position(|c| c.name == link.name) {
            Some(i) => i,
            None => {
                cables.push(CableShare {
                    name: link.name.clone(),
                    bytes: 0,
                    bps: 0,
                    connections: 0,
                    wanted: 0,
                    notes: Vec::new(),
                });
                cables.len() - 1
            }
        };
        let cable = &mut cables[i];
        cable.bytes += link.bytes;
        cable.bps += link.bps;
        cable.connections += link.connections;
        cable.wanted += link.wanted;
        if let Some(note) = &link.note
            && !cable.notes.contains(note)
        {
            cable.notes.push(note.clone());
        }
    }
    cables
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;
    use std::time::Duration;

    use serde_json::json;

    use super::*;
    use crate::fetch::DownloadId;
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
                    "public_ip": "203.0.113.7", "metered": false
                }],
                "shared": [],
                "connections": [],
                "combined": {
                    "bps": 57600000,
                    "parts": [{ "name": "Wi-Fi", "bps": 57600000, "shared_with": [] }],
                    "not_counted": []
                }
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

        let downloads = serde_json::to_value(LiveMessage::Downloads {
            downloads: downloads_view(&[download(1, Phase::Starting, vec![])], &[]),
        })
        .unwrap();
        assert_eq!(downloads["type"], "downloads");
        assert_eq!(downloads["downloads"]["list"][0]["id"], 1);
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

    fn with_state(mut l: Link, state: LinkState) -> Link {
        l.state = state;
        l
    }

    fn combined_of(links: &[Link]) -> CombinedView {
        table_view(links).combined
    }

    fn part(name: &str, bps: u64, shared_with: &[&str]) -> CombinedPart {
        CombinedPart {
            name: name.to_string(),
            bps,
            shared_with: shared_with.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn left_out(name: &str, why: &'static str) -> NotCounted {
        NotCounted {
            name: name.to_string(),
            why,
        }
    }

    #[test]
    fn two_independent_cables_add_up() {
        let links = [
            up(1, "Wi-Fi", "192.0.2.10", 40, 57_600_000, "203.0.113.7"),
            up(
                2,
                "Ethernet 2",
                "198.51.100.20",
                61,
                162_500_000,
                "203.0.113.9",
            ),
        ];
        assert_eq!(
            combined_of(&links),
            CombinedView {
                bps: 220_100_000,
                parts: vec![
                    part("Wi-Fi", 57_600_000, &[]),
                    part("Ethernet 2", 162_500_000, &[])
                ],
                not_counted: vec![],
            }
        );
    }

    /// Two cables behind one public address are one connection: adding both
    /// would promise speed that is not there.
    #[test]
    fn a_shared_group_counts_once_at_its_fastest() {
        let links = [
            up(1, "Wi-Fi", "192.0.2.10", 40, 41_000_000, "203.0.113.7"),
            up(
                2,
                "Ethernet 2",
                "198.51.100.20",
                61,
                162_500_000,
                "203.0.113.9",
            ),
            up(
                3,
                "Ethernet 3",
                "198.51.100.30",
                48,
                57_600_000,
                "203.0.113.7",
            ),
        ];
        let combined = combined_of(&links);
        assert_eq!(combined.bps, 57_600_000 + 162_500_000);
        // Named after its fastest cable, placed where its first cable appeared.
        assert_eq!(
            combined.parts,
            vec![
                part("Ethernet 3", 57_600_000, &["Wi-Fi"]),
                part("Ethernet 2", 162_500_000, &[])
            ]
        );
    }

    /// Wi-Fi and Ethernet 2 share by IPv6, Ethernet 2 and Ethernet 3 by IPv4:
    /// all three leave by one connection. The IPv4 group comes first, so one
    /// pass over the groups is not enough to join Ethernet 3 to Wi-Fi.
    #[test]
    fn cables_joined_through_a_third_are_one_connection() {
        let links = [
            up(1, "Wi-Fi", "2001:db8::10", 40, 30_000_000, "2001:db8::1"),
            up(
                2,
                "Ethernet 2",
                "198.51.100.20",
                61,
                50_000_000,
                "203.0.113.7",
            ),
            up(
                3,
                "Ethernet 2",
                "2001:db8::20",
                61,
                50_000_000,
                "2001:db8::1",
            ),
            up(
                4,
                "Ethernet 3",
                "198.51.100.30",
                48,
                20_000_000,
                "203.0.113.7",
            ),
        ];
        let combined = combined_of(&links);
        assert_eq!(combined.bps, 50_000_000);
        assert_eq!(
            combined.parts,
            vec![part("Ethernet 2", 50_000_000, &["Wi-Fi", "Ethernet 3"])]
        );
    }

    /// Working, with a public address, but no speed yet.
    fn unmeasured(id: u64, name: &str, addr: &str, public: &str) -> Link {
        Link {
            public_ip: Some(public.parse().unwrap()),
            ..link(id, name, addr, LinkState::Up)
        }
    }

    /// The cable in the middle joins the other two whether or not it has
    /// been measured, so measuring it cannot lower the total.
    #[test]
    fn an_unmeasured_cable_still_joins_the_two_it_links() {
        let links = [
            up(1, "Wi-Fi", "2001:db8::10", 40, 30_000_000, "2001:db8::1"),
            unmeasured(2, "Ethernet 2", "198.51.100.20", "203.0.113.7"),
            unmeasured(3, "Ethernet 2", "2001:db8::20", "2001:db8::1"),
            up(
                4,
                "Ethernet 3",
                "198.51.100.30",
                48,
                20_000_000,
                "203.0.113.7",
            ),
        ];
        let combined = combined_of(&links);
        assert_eq!(combined.bps, 30_000_000);
        assert_eq!(
            combined.parts,
            vec![part("Wi-Fi", 30_000_000, &["Ethernet 3"])]
        );
        assert_eq!(
            combined.not_counted,
            vec![left_out("Ethernet 2", "not measured yet")]
        );
    }

    /// The cables on one connection, as `links::upstream::shared_connections`
    /// finds them (its tests hold the rules): the cards and the combined
    /// speed read the same list.
    #[test]
    fn connections_are_the_cables_on_one_internet_connection() {
        let links = [
            unmeasured(1, "Wi-Fi", "192.0.2.10", "203.0.113.7"),
            unmeasured(2, "Ethernet 2", "198.51.100.20", "203.0.113.9"),
            unmeasured(3, "Ethernet 3", "198.51.100.30", "203.0.113.7"),
        ];
        assert_eq!(
            json_of(&links)["connections"],
            json!([["Wi-Fi", "Ethernet 3"]])
        );
    }

    #[test]
    fn a_down_or_unmeasured_cable_adds_nothing() {
        let mut stale = up(
            2,
            "Ethernet 4",
            "198.51.100.40",
            1,
            99_000_000,
            "203.0.113.4",
        );
        stale.state = LinkState::Down;
        let mut portal = up(
            5,
            "Ethernet 7",
            "198.51.100.70",
            1,
            88_000_000,
            "203.0.113.5",
        );
        portal.state = LinkState::Portal;
        let links = [
            up(1, "Wi-Fi", "192.0.2.10", 40, 57_600_000, "203.0.113.7"),
            // A dead address can still hold the figure from before it died.
            stale,
            link(3, "Ethernet 5", "198.51.100.50", LinkState::Up),
            link(4, "Ethernet 6", "198.51.100.60", LinkState::Probing),
            portal,
            // Working but unmeasured beats down when naming the reason.
            link(6, "Ethernet 8", "198.51.100.80", LinkState::Down),
            link(7, "Ethernet 8", "2001:db8::80", LinkState::Up),
        ];
        let combined = combined_of(&links);
        assert_eq!(combined.bps, 57_600_000);
        assert_eq!(combined.parts, vec![part("Wi-Fi", 57_600_000, &[])]);
        assert_eq!(
            combined.not_counted,
            vec![
                left_out("Ethernet 4", "down"),
                left_out("Ethernet 5", "not measured yet"),
                left_out("Ethernet 6", "probing"),
                left_out("Ethernet 7", "portal"),
                left_out("Ethernet 8", "not measured yet"),
            ]
        );
    }

    /// Degraded is usable (`LinkState::is_usable`): it still carries data,
    /// so its speed is part of what is available.
    #[test]
    fn a_degraded_cable_counts() {
        let links = [
            up(1, "Wi-Fi", "192.0.2.10", 40, 57_600_000, "203.0.113.7"),
            with_state(
                up(
                    2,
                    "Ethernet 2",
                    "198.51.100.20",
                    900,
                    9_000_000,
                    "203.0.113.9",
                ),
                LinkState::Degraded,
            ),
        ];
        assert_eq!(combined_of(&links).bps, 66_600_000);
    }

    #[test]
    fn nothing_measured_gives_zero_and_no_parts() {
        assert_eq!(
            combined_of(&[]),
            CombinedView {
                bps: 0,
                parts: vec![],
                not_counted: vec![]
            }
        );
        let probing = [link(1, "Wi-Fi", "192.0.2.10", LinkState::Probing)];
        let combined = combined_of(&probing);
        assert_eq!((combined.bps, combined.parts.len()), (0, 0));
        assert_eq!(combined.not_counted, vec![left_out("Wi-Fi", "probing")]);
    }

    #[test]
    fn the_combined_speed_in_json() {
        let links = [
            up(1, "Wi-Fi", "192.0.2.10", 40, 57_600_000, "203.0.113.7"),
            up(
                2,
                "Ethernet 3",
                "198.51.100.30",
                48,
                41_000_000,
                "203.0.113.7",
            ),
            link(3, "Ethernet 4", "198.51.100.40", LinkState::Down),
        ];
        assert_eq!(
            json_of(&links)["combined"],
            json!({
                "bps": 57600000,
                "parts": [{ "name": "Wi-Fi", "bps": 57600000, "shared_with": ["Ethernet 3"] }],
                "not_counted": [{ "name": "Ethernet 4", "why": "down" }]
            })
        );
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

    fn share(
        link: u64,
        name: &str,
        bytes: u64,
        bps: u64,
        connections: u32,
        note: Option<&str>,
    ) -> LinkShare {
        LinkShare {
            link: LinkId::new(link),
            name: name.to_string(),
            bytes,
            bps,
            connections,
            wanted: connections,
            note: note.map(String::from),
        }
    }

    /// A download whose totals are its links' shares, as the engine sends.
    fn download(id: u64, phase: Phase, links: Vec<LinkShare>) -> DownloadStatus {
        DownloadStatus {
            id: DownloadId::new(id),
            url: format!("https://example.com/{id}.iso"),
            name: Some(format!("{id}.iso")),
            phase,
            length: Some(400_000_000),
            done: links.iter().map(|l| l.bytes).sum(),
            bps: links.iter().map(|l| l.bps).sum(),
            links,
            note: None,
        }
    }

    /// Wanting what it holds, as `share` does.
    fn cable(name: &str, bytes: u64, bps: u64, connections: u32, notes: &[&str]) -> CableShare {
        CableShare {
            name: name.to_string(),
            bytes,
            bps,
            connections,
            wanted: connections,
            notes: notes.iter().map(|n| n.to_string()).collect(),
        }
    }

    /// A Windows "Set as metered" tether, a cable growing toward more
    /// connections than it holds, and what each cable has received.
    #[test]
    fn metered_used_and_wanted_become_the_json_the_ui_reads() {
        let tether = Link {
            metered: true,
            ..up(
                4,
                "Ethernet 3",
                "198.51.100.30",
                48,
                41_000_000,
                "203.0.113.9",
            )
        };
        assert_eq!(json_of(&[tether])["links"][0]["metered"], true);

        let growing = download(
            1,
            Phase::Downloading,
            vec![LinkShare {
                wanted: 6,
                ..share(1, "Wi-Fi", 3_000_000, 20_000_000, 4, None)
            }],
        );
        let used = [
            CableUse {
                cable: "Ethernet 3".to_string(),
                bytes: 7_000_000,
            },
            CableUse {
                cable: "Wi-Fi".to_string(),
                bytes: 3_100_000,
            },
        ];
        let json = serde_json::to_value(downloads_view(&[growing], &used)).unwrap();
        assert_eq!(
            json["list"][0]["cables"][0],
            json!({
                "name": "Wi-Fi", "bytes": 3000000, "bps": 20000000,
                "connections": 4, "wanted": 6, "notes": []
            })
        );
        assert_eq!(
            json["used"],
            json!([
                { "cable": "Ethernet 3", "bytes": 7000000 },
                { "cable": "Wi-Fi", "bytes": 3100000 }
            ])
        );
    }

    #[test]
    fn a_cables_wanted_is_its_addresses_added_up() {
        let status = download(
            1,
            Phase::Downloading,
            vec![
                LinkShare {
                    wanted: 3,
                    ..share(2, "Ethernet 2", 1, 1, 2, None)
                },
                LinkShare {
                    wanted: 1,
                    ..share(1, "Wi-Fi", 1, 1, 1, None)
                },
                LinkShare {
                    wanted: 2,
                    ..share(3, "Ethernet 2", 1, 1, 2, None)
                },
            ],
        );
        let cables = &downloads_view(&[status], &[]).list[0].cables;
        let wanted: Vec<(&str, u32)> = cables.iter().map(|c| (c.name.as_str(), c.wanted)).collect();
        assert_eq!(wanted, [("Ethernet 2", 5), ("Wi-Fi", 1)]);
    }

    fn speed(name: &str, bps: u64) -> SpeedPart {
        SpeedPart {
            name: name.to_string(),
            bps,
        }
    }

    /// Several addresses can be one cable (PLAN.md section 3.6, item 11). On
    /// screen a cable is what its addresses really carried, added up, with
    /// each address's note, in the order the cable first joined.
    #[test]
    fn a_cables_addresses_add_up_to_one_share() {
        let whole = "sits out: the server sends the file only whole, so one cable fetches it";
        let status = download(
            1,
            Phase::Downloading,
            vec![
                share(
                    2,
                    "Ethernet 2",
                    6_000_000,
                    40_000_000,
                    4,
                    Some("the IPv4 note"),
                ),
                share(1, "Wi-Fi", 3_000_000, 20_000_000, 4, None),
                share(
                    3,
                    "Ethernet 2",
                    2_000_000,
                    9_000_000,
                    2,
                    Some("the IPv6 note"),
                ),
                // The same words from two addresses are said once.
                share(4, "Ethernet 3", 0, 0, 0, Some(whole)),
                share(5, "Ethernet 3", 0, 0, 0, Some(whole)),
            ],
        );
        assert_eq!(
            downloads_view(&[status], &[]).list[0].cables,
            vec![
                cable(
                    "Ethernet 2",
                    8_000_000,
                    49_000_000,
                    6,
                    &["the IPv4 note", "the IPv6 note"]
                ),
                cable("Wi-Fi", 3_000_000, 20_000_000, 4, &[]),
                cable("Ethernet 3", 0, 0, 0, &[whole]),
            ]
        );
    }

    /// The hero's "Downloading at": every download that is downloading, by
    /// cable. The engine already sends 0 for the others; the view does not
    /// count on it.
    #[test]
    fn the_live_speed_counts_only_downloads_that_are_downloading() {
        let all = [
            download(
                1,
                Phase::Downloading,
                vec![
                    share(1, "Wi-Fi", 1, 30_000_000, 4, None),
                    share(2, "Ethernet 2", 1, 50_000_000, 4, None),
                ],
            ),
            download(
                2,
                Phase::Done,
                vec![share(1, "Wi-Fi", 1, 99_000_000, 0, None)],
            ),
            download(
                3,
                Phase::Waiting,
                vec![share(3, "Ethernet 4", 1, 77_000_000, 0, None)],
            ),
            download(
                4,
                Phase::Failed,
                vec![share(1, "Wi-Fi", 1, 66_000_000, 0, None)],
            ),
            download(
                5,
                Phase::Starting,
                vec![share(1, "Wi-Fi", 1, 55_000_000, 0, None)],
            ),
            download(
                7,
                Phase::Stopped,
                vec![share(1, "Wi-Fi", 1, 44_000_000, 0, None)],
            ),
            // Ethernet 2 in a second download adds to its part. Ethernet 3,
            // carrying nothing, has no part.
            download(
                6,
                Phase::Downloading,
                vec![
                    share(2, "Ethernet 2", 1, 20_000_000, 4, None),
                    share(4, "Ethernet 3", 0, 0, 0, Some("sits out")),
                ],
            ),
        ];
        assert_eq!(
            downloads_view(&all, &[]).live,
            LiveSpeed {
                bps: 100_000_000,
                parts: vec![speed("Wi-Fi", 30_000_000), speed("Ethernet 2", 70_000_000)],
            }
        );
    }

    #[test]
    fn every_phase_has_its_word() {
        let phases = [
            (Phase::Starting, "starting"),
            (Phase::Waiting, "waiting"),
            (Phase::Downloading, "downloading"),
            (Phase::Done, "done"),
            (Phase::Failed, "failed"),
            (Phase::Stopped, "stopped"),
        ];
        for (phase, word) in phases {
            let view = downloads_view(&[download(1, phase, vec![])], &[]);
            assert_eq!(view.list[0].state, word);
        }
    }

    /// Before the file is found its name and length are unknown: null, never
    /// an empty name or 0 bytes.
    #[test]
    fn a_download_becomes_the_json_the_ui_reads() {
        let starting = DownloadStatus {
            id: DownloadId::new(7),
            url: "https://example.com/a.iso".to_string(),
            name: None,
            phase: Phase::Starting,
            length: None,
            done: 0,
            bps: 0,
            links: vec![],
            note: None,
        };
        let done = download(
            8,
            Phase::Done,
            vec![share(1, "Wi-Fi", 400_000_000, 0, 0, None)],
        );
        assert_eq!(
            serde_json::to_value(downloads_view(&[starting, done], &[])).unwrap(),
            json!({
                "live": { "bps": 0, "parts": [] },
                "list": [
                    {
                        "id": 7, "url": "https://example.com/a.iso", "name": null,
                        "state": "starting", "length": null, "done": 0, "bps": 0,
                        "note": null, "cables": []
                    },
                    {
                        "id": 8, "url": "https://example.com/8.iso", "name": "8.iso",
                        "state": "done", "length": 400000000, "done": 400000000, "bps": 0,
                        "note": null,
                        "cables": [{
                            "name": "Wi-Fi", "bytes": 400000000, "bps": 0,
                            "connections": 0, "wanted": 0, "notes": []
                        }]
                    }
                ],
                "used": []
            })
        );
    }

    /// The page's copy of the contract: messages exactly as this engine
    /// sends them, written out as TypeScript, so the page's type check holds
    /// every field to `types.ts` and `contract.test.tsx` renders them.
    const CONTRACT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../ui/src/api/contract.ts");

    const CONTRACT_HEADER: &str = concat!(
        "// Messages exactly as the engine sends them, written by its test\n",
        "// `the_ui_contract_file_is_up_to_date` (engine/src/api/view.rs). Do not\n",
        "// edit: `satisfies` holds them to types.ts whenever the page is\n",
        "// type-checked (`pnpm build`), and contract.test.tsx renders them. After\n",
        "// changing the engine's JSON, set BARRAGE_WRITE_CONTRACT=1 and run, from\n",
        "// engine/: cargo test --lib the_ui_contract_file\n",
        "import type { LiveMessage } from './types'\n",
        "\n",
    );

    /// One of everything the page draws: every state word and every reason a
    /// cable is left out of the sum, a cable with an IPv4 and an IPv6
    /// address, a degraded address beside a portal one, and two cables on one
    /// connection. The change adds an address and a speed, so it has lines.
    fn contract_sample() -> (Vec<Link>, Vec<Link>) {
        let portal = |id, name, addr, rtt_ms| Link {
            rtt: Some(Duration::from_millis(rtt_ms)),
            ..link(id, name, addr, LinkState::Portal)
        };
        let after = vec![
            up(1, "Wi-Fi", "192.0.2.10", 40, 57_600_000, "203.0.113.7"),
            up(
                2,
                "Ethernet 2",
                "198.51.100.20",
                120,
                141_000_000,
                "203.0.113.50",
            ),
            up(
                3,
                "Ethernet 2",
                "2001:db8::20",
                95,
                141_000_000,
                "2001:db8::20",
            ),
            Link {
                metered: true,
                ..up(
                    4,
                    "Ethernet 3",
                    "198.51.100.30",
                    48,
                    41_000_000,
                    "203.0.113.7",
                )
            },
            with_state(
                up(
                    5,
                    "Ethernet 4",
                    "198.51.100.40",
                    300,
                    9_400_000,
                    "198.51.100.40",
                ),
                LinkState::Degraded,
            ),
            portal(6, "Ethernet 4", "2001:db8::40", 60),
            unmeasured(7, "Ethernet 5", "192.0.2.50", "203.0.113.90"),
            portal(8, "Ethernet 6", "192.0.2.60", 80),
            link(9, "Ethernet 7", "192.0.2.70", LinkState::Probing),
            link(10, "Ethernet 8", "192.0.2.80", LinkState::Down),
        ];
        let before = after
            .iter()
            .filter(|l| l.id.as_u64() != 3)
            .map(|l| match l.id.as_u64() {
                4 => Link {
                    down_bps: None,
                    ..l.clone()
                },
                _ => l.clone(),
            })
            .collect();
        (before, after)
    }

    /// A download in every state, on the sample's cables: one downloading
    /// over Wi-Fi and both of Ethernet 2's addresses while Ethernet 3 sits
    /// out with its note, one failed with the note its link gave, and one
    /// stopped halfway.
    fn contract_downloads() -> Vec<DownloadStatus> {
        let gone = "https://example.com/files/gone.zip";
        let not_found = format!("finding {gone} through 192.0.2.10: {gone} answered 404 Not Found");
        let other = "sits out: the server gave this cable another version of the file \
                     (\"7b1d\", not \"a3f0\")";
        let mut downloading = download(
            1,
            Phase::Downloading,
            vec![
                // Growing toward six.
                LinkShare {
                    wanted: 6,
                    ..share(1, "Wi-Fi", 1_210_000_000, 38_400_000, 4, None)
                },
                share(2, "Ethernet 2", 905_000_000, 61_000_000, 4, None),
                share(3, "Ethernet 2", 412_000_000, 27_500_000, 4, None),
                share(4, "Ethernet 3", 0, 0, 0, Some(other)),
            ],
        );
        downloading.url = "https://example.com/files/ubuntu.iso".to_string();
        downloading.name = Some("ubuntu.iso".to_string());
        downloading.length = Some(6_114_656_256);

        let mut done = download(
            2,
            Phase::Done,
            vec![share(1, "Wi-Fi", 2_400_000, 0, 0, None)],
        );
        done.url = "https://example.com/files/photo.jpg".to_string();
        done.name = Some("photo (1).jpg".to_string());
        done.length = Some(2_400_000);

        let mut failed = download(
            3,
            Phase::Failed,
            vec![share(1, "Wi-Fi", 0, 0, 0, Some(&not_found))],
        );
        failed.url = gone.to_string();
        (failed.name, failed.length, failed.note) = (None, None, Some(not_found.clone()));

        let waiting = DownloadStatus {
            id: DownloadId::new(4),
            url: "https://example.com/files/notes.pdf".to_string(),
            name: None,
            phase: Phase::Waiting,
            length: None,
            done: 0,
            bps: 0,
            links: vec![],
            note: None,
        };
        let starting = DownloadStatus {
            id: DownloadId::new(5),
            url: "https://example.com/files/video.mp4".to_string(),
            phase: Phase::Starting,
            ..waiting.clone()
        };

        let mut stopped = download(
            6,
            Phase::Stopped,
            vec![
                share(1, "Wi-Fi", 112_500_000, 0, 0, None),
                share(2, "Ethernet 2", 538_600_000, 0, 0, None),
            ],
        );
        stopped.url = "https://example.com/files/installer.exe".to_string();
        stopped.name = Some("installer.exe".to_string());
        stopped.length = Some(1_302_200_000);
        stopped.note = Some("stopped; resuming carries on from here".to_string());
        vec![downloading, done, failed, waiting, starting, stopped]
    }

    /// What the sample's cables received: a little over what they wrote, as
    /// bytes past a cut count.
    fn contract_used() -> Vec<CableUse> {
        vec![
            CableUse {
                cable: "Wi-Fi".to_string(),
                bytes: 1_326_100_000,
            },
            CableUse {
                cable: "Ethernet 2".to_string(),
                bytes: 1_858_200_000,
            },
        ]
    }

    /// Fails when the engine's JSON changes and the page's copy does not,
    /// which is what made a renamed field leave both test suites green.
    /// The messages come in the order the socket sends them.
    #[test]
    fn the_ui_contract_file_is_up_to_date() {
        let (before, after) = contract_sample();
        let messages = [
            LiveMessage::Table {
                table: table_view(&before),
            },
            LiveMessage::Downloads {
                downloads: downloads_view(&contract_downloads(), &contract_used()),
            },
            change_message(&before, &after),
        ];
        let json = serde_json::to_string_pretty(&messages).unwrap();
        let wanted =
            format!("{CONTRACT_HEADER}export const messages = {json} satisfies LiveMessage[]\n");
        if std::env::var_os("BARRAGE_WRITE_CONTRACT").is_some() {
            std::fs::write(CONTRACT, &wanted).unwrap();
        }

        // Git may hand the file out with Windows line endings.
        let found = std::fs::read_to_string(CONTRACT)
            .unwrap_or_default()
            .replace("\r\n", "\n");
        if found != wanted {
            let line = found
                .lines()
                .zip(wanted.lines())
                .position(|(a, b)| a != b)
                .unwrap_or_else(|| found.lines().count().min(wanted.lines().count()));
            panic!(
                "ui/src/api/contract.ts no longer matches what this engine sends. First \
                 difference, line {}: the file has {:?}, the engine sends {:?}. If the JSON \
                 is meant to change, set BARRAGE_WRITE_CONTRACT=1 and run cargo test --lib \
                 the_ui_contract_file to refresh it, then make the page agree: pnpm build \
                 type-checks it and pnpm test renders it.",
                line + 1,
                found.lines().nth(line),
                wanted.lines().nth(line),
            );
        }
    }
}
