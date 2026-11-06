//! What every engine that fetches something offers (PLAN.md 3.3): add a
//! thing to fetch, watch every one of them, stop and resume one, and shut
//! down. The downloads (phase 3) and the torrents (phase 5) both offer it,
//! so neither's library types reach the API, and the torrent library can be
//! replaced behind it.
//!
//! The trait was deferred until the second engine arrived (the user's
//! choice, PR #4 review, 2026-10-30), and is written from the shape phase 3
//! proved. Each engine keeps its own words for what goes wrong, so the error
//! types are its own.

use std::future::Future;

use tokio::sync::watch;

use crate::bt::TorrentId;
use crate::fetch::DownloadId;
use crate::record::Priority;

/// One transfer of either kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    Web(DownloadId),
    Torrent(TorrentId),
}

pub trait TransferEngine {
    /// What to fetch: a URL, a magnet link or a .torrent file.
    type Request;
    type Id: Copy;
    type Status: Clone;
    type AddError;
    type ControlError;

    /// Start fetching; returns at once, and the status says how it goes.
    fn add(
        &self,
        request: Self::Request,
    ) -> impl Future<Output = Result<Self::Id, Self::AddError>> + Send;

    /// Every one added since the engine started, kept up to date.
    fn status(&self) -> watch::Receiver<Vec<Self::Status>>;

    /// Stop one, keeping what it has.
    fn stop(&self, id: Self::Id) -> impl Future<Output = Result<(), Self::ControlError>> + Send;

    /// Carry on from what a stopped one has.
    fn resume(&self, id: Self::Id) -> impl Future<Output = Result<(), Self::ControlError>> + Send;

    /// List it without starting it: queued, until it is released.
    fn enqueue(
        &self,
        request: Self::Request,
    ) -> impl Future<Output = Result<Self::Id, Self::AddError>> + Send;

    /// Start a queued one.
    fn release(&self, id: Self::Id) -> impl Future<Output = Result<(), Self::ControlError>> + Send;

    /// Send a stopped one to the back of the line.
    fn requeue(&self, id: Self::Id) -> impl Future<Output = Result<(), Self::ControlError>> + Send;

    /// Change who goes next. One already running stays running.
    fn set_priority(
        &self,
        id: Self::Id,
        priority: Priority,
    ) -> impl Future<Output = Result<(), Self::ControlError>> + Send;

    /// Stop everything, saving what can be saved.
    fn shutdown(&self) -> impl Future<Output = ()> + Send;
}
