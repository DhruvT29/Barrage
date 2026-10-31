//! The data each cable has received and sent, every engine together (the
//! downloads and the torrents), and the cable's speed tests.
//!
//! What a metered tether costs is what came down it, so this counts every
//! byte a connection received, whether or not it was written: what arrived
//! past a cut, and what a request that broke off had received, count too.
//! One counter per cable for the whole engine, shared by every connection of
//! every download through it, and added to as each piece arrives, so no
//! download's end or restart loses any. What a torrent uploads is counted
//! apart, as sent: a tethered phone pays for that too.
//!
//! One count for the whole engine: built once with [`Usage::counting_speed_tests`]
//! and given to every engine, so each speed test is added once.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::watch;

use super::CableUse;
use crate::links::LinkId;
use crate::links::watch::LinkTable;

/// Add what each cable's speed tests pull (`Link::tested`) to its count as
/// it grows, until the link watcher stops. A speed test comes out of a
/// metered tether's data as a download does: 27 to 64 MB each time a phone
/// is measured.
pub(crate) async fn count_speed_tests(mut table: LinkTable, used: Arc<Usage>) {
    let mut seen: std::collections::HashMap<LinkId, u64> = std::collections::HashMap::new();
    loop {
        for link in table.latest().iter() {
            let before = seen.insert(link.id, link.tested).unwrap_or(0);
            if link.tested > before {
                used.cable(&link.name).add(link.tested - before);
            }
        }
        used.publish();
        if !table.changed().await {
            return;
        }
    }
}

/// One cable's count.
pub(crate) struct Counter {
    /// Received.
    bytes: AtomicU64,
    sent: AtomicU64,
    /// When it first carried, counted from 1 among the engine's cables; 0
    /// until then.
    first: AtomicU64,
    /// The engine's next `first`.
    order: Arc<AtomicU64>,
}

impl Counter {
    /// `n` more bytes arrived through the cable.
    pub fn add(&self, n: u64) {
        if n > 0 {
            self.bytes.fetch_add(n, Ordering::SeqCst);
            self.carried();
        }
    }

    /// `n` more bytes left through the cable.
    // The torrents call it from phase 5 Task 7; until then only the tests.
    #[allow(dead_code)]
    pub fn add_sent(&self, n: u64) {
        if n > 0 {
            self.sent.fetch_add(n, Ordering::SeqCst);
            self.carried();
        }
    }

    /// Note when the cable first carried, either way, once.
    fn carried(&self) {
        if self.first.load(Ordering::SeqCst) == 0 {
            let next = self.order.fetch_add(1, Ordering::SeqCst) + 1;
            let _ = self
                .first
                .compare_exchange(0, next, Ordering::SeqCst, Ordering::SeqCst);
        }
    }
}

/// Every cable's count, and its readers.
pub struct Usage {
    cables: Mutex<Vec<(String, Arc<Counter>)>>,
    order: Arc<AtomicU64>,
    sender: watch::Sender<Vec<CableUse>>,
}

impl Usage {
    /// The engine's one count, adding each cable's speed tests as the link
    /// watcher behind `table` reports them (outside a runtime there is no
    /// watcher, so nothing to add).
    pub fn counting_speed_tests(table: LinkTable) -> Arc<Self> {
        let usage = Arc::new(Self::new());
        if tokio::runtime::Handle::try_current().is_ok() {
            tokio::spawn(count_speed_tests(table, Arc::clone(&usage)));
        }
        usage
    }

    pub(crate) fn new() -> Self {
        Self {
            cables: Mutex::new(Vec::new()),
            order: Arc::new(AtomicU64::new(0)),
            sender: watch::channel(Vec::new()).0,
        }
    }

    /// The count for the cable named `name`, the same for every caller.
    pub(crate) fn cable(&self, name: &str) -> Arc<Counter> {
        let mut cables = self.cables.lock().unwrap();
        if let Some((_, counter)) = cables.iter().find(|(cable, _)| cable == name) {
            return counter.clone();
        }
        let counter = Arc::new(Counter {
            bytes: AtomicU64::new(0),
            sent: AtomicU64::new(0),
            first: AtomicU64::new(0),
            order: self.order.clone(),
        });
        cables.push((name.to_string(), counter.clone()));
        counter
    }

    /// Show the readers every cable that has carried, in the order they
    /// first did; only a change wakes them.
    pub(crate) fn publish(&self) {
        let mut carried: Vec<(u64, CableUse)> = self
            .cables
            .lock()
            .unwrap()
            .iter()
            .filter_map(|(cable, counter)| {
                let first = counter.first.load(Ordering::SeqCst);
                (first > 0).then(|| {
                    let bytes = counter.bytes.load(Ordering::SeqCst);
                    let sent = counter.sent.load(Ordering::SeqCst);
                    let cable = cable.clone();
                    (first, CableUse { cable, bytes, sent })
                })
            })
            .collect();
        carried.sort_by_key(|(first, _)| *first);
        let used: Vec<CableUse> = carried.into_iter().map(|(_, used)| used).collect();
        self.sender.send_if_modified(|shown| {
            if *shown == used {
                return false;
            }
            *shown = used;
            true
        });
    }

    pub fn subscribe(&self) -> watch::Receiver<Vec<CableUse>> {
        self.sender.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shown(usage: &Usage) -> Vec<(String, u64)> {
        usage.publish();
        let used = usage.subscribe().borrow().clone();
        used.into_iter().map(|u| (u.cable, u.bytes)).collect()
    }

    /// Asked for first is not carried first: a cable shows once bytes
    /// arrive through it.
    #[test]
    fn cables_show_in_the_order_they_first_carried() {
        let usage = Usage::new();
        let wifi = usage.cable("Wi-Fi");
        let tether = usage.cable("Ethernet 3");
        assert_eq!(shown(&usage), []);
        tether.add(10);
        wifi.add(0);
        assert_eq!(shown(&usage), [("Ethernet 3".to_string(), 10)]);
        wifi.add(5);
        tether.add(1);
        assert_eq!(
            shown(&usage),
            [("Ethernet 3".to_string(), 11), ("Wi-Fi".to_string(), 5)]
        );
    }

    #[test]
    fn sent_and_received_are_counted_apart() {
        let usage = Usage::new();
        usage.cable("Wi-Fi").add(10);
        usage.cable("Wi-Fi").add_sent(3);
        usage.publish();
        let used = usage.subscribe().borrow().clone();
        assert_eq!(
            used,
            [CableUse {
                cable: "Wi-Fi".to_string(),
                bytes: 10,
                sent: 3
            }]
        );
    }

    /// A cable that has only sent (a torrent's upload, before any piece
    /// arrives) has still cost something, so it shows.
    #[test]
    fn a_cable_that_only_sent_still_shows() {
        let usage = Usage::new();
        usage.cable("Ethernet 3").add_sent(7);
        usage.cable("Wi-Fi").add(1);
        usage.publish();
        let used = usage.subscribe().borrow().clone();
        let names: Vec<&str> = used.iter().map(|u| u.cable.as_str()).collect();
        assert_eq!(names, ["Ethernet 3", "Wi-Fi"]);
        assert_eq!((used[0].bytes, used[0].sent), (0, 7));
    }

    #[test]
    fn one_name_is_one_count() {
        let usage = Usage::new();
        usage.cable("Wi-Fi").add(3);
        usage.cable("Wi-Fi").add(4);
        assert_eq!(shown(&usage), [("Wi-Fi".to_string(), 7)]);
    }
}
