//! Torrents (phase 5): librqbit, through BARRAGE's fork, behind BARRAGE's
//! own types. No librqbit type leaves this module (PLAN.md 3.3: the library
//! is meant to be replaced).

pub mod choose;
mod connect;
mod counted;
pub mod listen;
mod rebuild;
pub mod source;
mod status;
mod torrents;

pub use listen::Listening;
pub use source::{Source, TorrentAddError};
pub use status::{
    Swarm, TorrentCable, TorrentControlError, TorrentId, TorrentPhase, TorrentStatus,
};
pub(crate) use torrents::fastresume_file;
pub use torrents::{TorrentConfig, Torrents};
