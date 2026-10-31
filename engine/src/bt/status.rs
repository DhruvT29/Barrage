//! A torrent as the API and the page see it: BARRAGE's own types, built
//! from librqbit's state in `torrents.rs`, never librqbit's.

use std::fmt;

/// A torrent's id, from 1 in the order torrents were added.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TorrentId(pub(crate) u64);

impl TorrentId {
    pub fn as_u64(self) -> u64 {
        self.0
    }
}

/// Where a torrent is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TorrentPhase {
    /// Fetching its metadata (a magnet link), before its files are known.
    Finding,
    Downloading,
    /// Every piece checked. It then shares nothing (no seeding).
    Done,
    /// Stopped by the user, keeping what it has; shares nothing.
    Stopped,
    Failed,
}

/// One torrent.
#[derive(Debug, Clone, PartialEq)]
pub struct TorrentStatus {
    pub id: TorrentId,
    /// Its name, once known (a magnet's `dn` until the metadata arrives).
    pub name: Option<String>,
    /// 40 lowercase hex characters.
    pub info_hash: String,
    pub phase: TorrentPhase,
    /// In bytes, once the metadata is known.
    pub length: Option<u64>,
    /// Bytes downloaded and checked against their piece's hash.
    pub done: u64,
    /// In bits per second, 0 unless downloading.
    pub down_bps: u64,
    pub up_bps: u64,
    /// Peers connected right now.
    pub peers: u32,
    /// Why it failed, or what to know about it, in the engine's words.
    pub note: Option<String>,
    pub cables: Vec<TorrentCable>,
}

/// One cable's part in a torrent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TorrentCable {
    pub name: String,
    pub received: u64,
    pub sent: u64,
    /// In bits per second.
    pub bps: u64,
    pub connections: u32,
    /// Whether its connections may upload (not on a metered connection).
    pub uploads: bool,
}

/// What the whole session does, apart from any one torrent.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Swarm {
    /// The cable carrying DHT, UDP trackers and uTP; `None` while no cable
    /// works, when they stay on this machine.
    pub udp_cable: Option<String>,
}

/// Why a torrent was not stopped or resumed, in the engine's words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TorrentControlError {
    NoSuchTorrent,
    /// Stopping: it is not finding or downloading.
    NotRunning,
    /// Resuming: it is not stopped.
    NotStopped,
}

impl fmt::Display for TorrentControlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSuchTorrent => f.write_str("no such torrent"),
            Self::NotRunning => f.write_str("it is not running, so there is nothing to stop"),
            Self::NotStopped => f.write_str("it is not stopped, so there is nothing to resume"),
        }
    }
}

impl std::error::Error for TorrentControlError {}
