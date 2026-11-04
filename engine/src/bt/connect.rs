//! BARRAGE's side of the fork's `PeerConnector`: every peer connection a
//! torrent opens leaves from the address of the cable `choose::next_cable`
//! picks, and is counted against its cable while it lasts, so the next
//! choice sees how many each cable holds. Every byte is counted both ways
//! (`counted.rs`), into the cable's data used and the torrent's tally. All
//! connections through one cable share its upload switch, off while Windows
//! calls it metered, and its cut, which ends them when it stops working;
//! `refresh` applies both every tick, to the connections already open.

use std::collections::HashMap;
use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use librqbit::dht::Id20;
use librqbit::{PeerConnector, PeerSocket};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpSocket;
use tokio::time::Instant;

use super::choose::next_cable;
use super::counted::{CountedRead, CountedWrite, Counts, Cut, Tally};
use super::status::TorrentCable;
use crate::fetch::Usage;
use crate::links::Link;
use crate::links::watch::LinkTable;

/// How quickly a cable's speed in a torrent follows a change.
const SPEED_HALF_LIFE: Duration = Duration::from_secs(2);

/// Each cable's open peer connections, by name.
type Open = Arc<Mutex<HashMap<String, u32>>>;

/// What every connection through one cable shares.
struct Line {
    may_upload: Arc<AtomicBool>,
    cut: Arc<Cut>,
}

pub(crate) struct Connector {
    table: LinkTable,
    usage: Arc<Usage>,
    open: Open,
    lines: Mutex<HashMap<String, Line>>,
    /// Each torrent's part through each cable, by (info hash, cable), in
    /// the order each first carried.
    tallies: Mutex<Vec<(String, String, Arc<Tally>)>>,
    /// Peers that called in since the last `take_incoming`.
    incoming: AtomicU64,
}

impl Connector {
    pub fn new(table: LinkTable, usage: Arc<Usage>) -> Self {
        Self {
            table,
            usage,
            open: Arc::default(),
            lines: Mutex::default(),
            tallies: Mutex::default(),
            incoming: AtomicU64::new(0),
        }
    }

    /// How many peers called in since the last time this was asked.
    pub fn take_incoming(&self) -> u64 {
        self.incoming.swap(0, Ordering::SeqCst)
    }

    /// Apply the link table to the connections already open: a metered
    /// cable's stop uploading, a cable no longer working has its ended.
    pub fn refresh(&self, links: &[Link]) {
        let mut lines = self.lines.lock().unwrap();
        lines.retain(|cable, line| {
            let on: Vec<&Link> = links.iter().filter(|l| &l.name == cable).collect();
            if !on.iter().any(|l| l.state.is_usable()) {
                line.cut.cut();
                return false;
            }
            line.may_upload
                .store(!on.iter().any(|l| l.metered), Ordering::SeqCst);
            true
        });
    }

    /// Each cable's part in the torrent `info_hash`.
    pub fn cables_of(&self, info_hash: &str, links: &[Link]) -> Vec<TorrentCable> {
        let now = Instant::now();
        self.tallies
            .lock()
            .unwrap()
            .iter()
            .filter(|(hash, ..)| hash == info_hash)
            .map(|(_, cable, tally)| {
                let received = tally.received.load(Ordering::SeqCst);
                let mut rate = tally.rate.lock().unwrap();
                rate.sample(now, received, SPEED_HALF_LIFE);
                TorrentCable {
                    name: cable.clone(),
                    received,
                    sent: tally.sent.load(Ordering::SeqCst),
                    bps: rate.bps().unwrap_or(0),
                    connections: tally.open.load(Ordering::SeqCst),
                    uploads: !links.iter().any(|l| &l.name == cable && l.metered),
                }
            })
            .collect()
    }

    /// The cable's shared switch and cut, made when its first connection is.
    fn line(&self, cable: &str, links: &[Link]) -> (Arc<AtomicBool>, Arc<Cut>) {
        let mut lines = self.lines.lock().unwrap();
        let line = lines.entry(cable.to_string()).or_insert_with(|| Line {
            may_upload: Arc::new(AtomicBool::new(
                !links.iter().any(|l| l.name == cable && l.metered),
            )),
            cut: Arc::default(),
        });
        (Arc::clone(&line.may_upload), Arc::clone(&line.cut))
    }

    fn tally(&self, info_hash: &str, cable: &str) -> Arc<Tally> {
        let mut tallies = self.tallies.lock().unwrap();
        if let Some((.., tally)) = tallies
            .iter()
            .find(|(h, c, _)| h == info_hash && c == cable)
        {
            return Arc::clone(tally);
        }
        let tally = Arc::new(Tally::default());
        tallies.push((info_hash.to_string(), cable.to_string(), Arc::clone(&tally)));
        tally
    }

    /// The streams of a connection through `cable` for `info_hash`, counted.
    fn wrap<R, W>(
        &self,
        info_hash: Id20,
        cable: String,
        links: &[Link],
        read: R,
        write: W,
    ) -> PeerSocket
    where
        R: AsyncRead + Send + Unpin + 'static,
        W: AsyncWrite + Send + Unpin + 'static,
    {
        let (may_upload, cut) = self.line(&cable, links);
        let tally = self.tally(&info_hash.as_string(), &cable);
        let counts = Counts {
            counter: self.usage.cable(&cable),
            tally: Arc::clone(&tally),
            cut,
        };
        let held = Held::take(&self.open, cable, tally);
        PeerSocket {
            read: Box::new(CountedRead::new(read, counts.clone(), Box::new(held))),
            write: Box::new(CountedWrite::new(write, counts)),
            may_upload,
        }
    }
}

/// One connection's place in its cable's count and its torrent's tally,
/// given back when it ends.
struct Held {
    cable: String,
    open: Open,
    tally: Arc<Tally>,
}

impl Held {
    fn take(open: &Open, cable: String, tally: Arc<Tally>) -> Self {
        *open.lock().unwrap().entry(cable.clone()).or_insert(0) += 1;
        tally.open.fetch_add(1, Ordering::SeqCst);
        Self {
            cable,
            open: Arc::clone(open),
            tally,
        }
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        if let Some(n) = self.open.lock().unwrap().get_mut(&self.cable) {
            *n = n.saturating_sub(1);
        }
        self.tally.open.fetch_sub(1, Ordering::SeqCst);
    }
}

/// A connection chosen but not yet open: counted against its cable from the
/// moment it is chosen, so two opening at once see each other.
struct Reserved {
    cable: String,
    open: Open,
}

impl Drop for Reserved {
    fn drop(&mut self) {
        if let Some(n) = self.open.lock().unwrap().get_mut(&self.cable) {
            *n = n.saturating_sub(1);
        }
    }
}

impl PeerConnector for Connector {
    fn connect(
        &self,
        info_hash: Id20,
        peer: SocketAddr,
    ) -> Pin<Box<dyn Future<Output = std::io::Result<PeerSocket>> + Send>> {
        let links = self.table.snapshot();
        let chosen = {
            let mut open = self.open.lock().unwrap();
            next_cable(&links, &open, peer)
                .inspect(|(cable, _)| *open.entry(cable.clone()).or_insert(0) += 1)
        };
        let (cable, local) = match chosen {
            Ok(chosen) => chosen,
            Err(why) => {
                return Box::pin(async move { Err(std::io::Error::other(why.to_string())) });
            }
        };
        let reserved = Reserved {
            cable: cable.clone(),
            open: Arc::clone(&self.open),
        };
        let (may_upload, cut) = self.line(&cable, &links);
        let tally = self.tally(&info_hash.as_string(), &cable);
        let counts = Counts {
            counter: self.usage.cable(&cable),
            tally: Arc::clone(&tally),
            cut,
        };
        let open = Arc::clone(&self.open);
        Box::pin(async move {
            let socket = match local {
                IpAddr::V4(_) => TcpSocket::new_v4()?,
                IpAddr::V6(_) => TcpSocket::new_v6()?,
            };
            socket.bind(SocketAddr::new(local, 0))?;
            let (read, write) = socket.connect(peer).await?.into_split();
            // Open: the reservation becomes the connection's place.
            drop(reserved);
            let held = Held::take(&open, cable, tally);
            Ok(PeerSocket {
                read: Box::new(CountedRead::new(read, counts.clone(), Box::new(held))),
                write: Box::new(CountedWrite::new(write, counts)),
                may_upload,
            })
        })
    }

    fn accepted(
        &self,
        info_hash: Id20,
        local: SocketAddr,
        _peer: SocketAddr,
        read: Box<dyn AsyncRead + Send + Unpin>,
        write: Box<dyn AsyncWrite + Send + Unpin>,
    ) -> PeerSocket {
        self.incoming.fetch_add(1, Ordering::SeqCst);
        let links = self.table.snapshot();
        let cable = links
            .iter()
            .find(|l| l.addr.ip() == local.ip())
            .map_or_else(|| local.ip().to_string(), |l| l.name.clone());
        self.wrap(info_hash, cable, &links, read, write)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::links::{Addr, LinkId, LinkState};

    fn link(name: &str, ip: &str, state: LinkState, metered: bool) -> Link {
        Link {
            id: LinkId::new(1),
            name: name.to_string(),
            addr: Addr::from(ip.parse::<IpAddr>().unwrap()),
            state,
            rtt: None,
            down_bps: Some(30_000_000),
            public_ip: None,
            metered,
            tested: 0,
        }
    }

    fn connector(links: Vec<Link>) -> Connector {
        let (_, rx) = tokio::sync::watch::channel(links);
        Connector::new(LinkTable::of(rx), Arc::new(Usage::new()))
    }

    /// Review Focus 3: Windows' metered setting changed while a torrent
    /// runs reaches the connections already open, through the switch they
    /// share, at the next refresh.
    #[test]
    fn refresh_turns_upload_off_and_on_for_open_connections() {
        let up = |metered| vec![link("Ethernet 2", "192.0.2.7", LinkState::Up, metered)];
        let connector = connector(up(false));
        let (may_upload, _) = connector.line("Ethernet 2", &up(false));
        assert!(may_upload.load(Ordering::SeqCst));
        connector.refresh(&up(true));
        assert!(
            !may_upload.load(Ordering::SeqCst),
            "metered now: open connections stop uploading"
        );
        connector.refresh(&up(false));
        assert!(
            may_upload.load(Ordering::SeqCst),
            "not metered again: they may upload"
        );
    }

    #[test]
    fn a_cables_first_connection_starts_with_its_metered_switch() {
        let links = vec![link("Ethernet 2", "192.0.2.7", LinkState::Up, true)];
        let (may_upload, _) = connector(links.clone()).line("Ethernet 2", &links);
        assert!(!may_upload.load(Ordering::SeqCst));
    }

    /// Review Focus 1: a cable that stops working has its connections ended,
    /// and its next connection gets a fresh line.
    #[test]
    fn refresh_cuts_a_cable_that_stopped_working() {
        let up = vec![link("Ethernet 3", "192.0.2.9", LinkState::Up, false)];
        let connector = connector(up.clone());
        let (_, cut) = connector.line("Ethernet 3", &up);
        connector.refresh(&[link("Ethernet 3", "192.0.2.9", LinkState::Down, false)]);
        assert!(cut.is_cut(), "its open connections are ended");
        let (_, next) = connector.line("Ethernet 3", &up);
        assert!(!next.is_cut(), "a connection once it is back starts afresh");
    }
}
