//! One line for every transfer, and one limit on how many run at once.
//!
//! The engines know nothing of each other or of the limit. Each can list a
//! transfer without starting it (`enqueue`), and start it when told
//! (`release`). This module watches both, and tells them which to start.
//!
//! A transfer that is starting, waiting for a cable, finding a torrent's
//! details or downloading holds a place. One that is stopped, done or failed
//! holds none, so the next starts as it frees its place. A lower limit stops
//! nothing: it just starts nothing until enough have ended.
//!
//! [`line_up`] and [`to_start`] decide, and are pure. [`Queue`] carries it out.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::watch;

use crate::bt::{Source, TorrentAddError, TorrentControlError, TorrentId, TorrentPhase, Torrents};
use crate::fetch::{AddError, AddRequest, ControlError, DownloadId, Downloads, Phase};
use crate::record::Priority;
use crate::transfer::{Key, TransferEngine};

/// The highest limit a user can set.
pub const MOST_LIMIT: u32 = 20;

/// Whether a transfer holds a place, wants one, or neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Slot {
    Held,
    Waiting,
    Out,
}

/// One transfer, as the line sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Line {
    pub key: Key,
    pub priority: Priority,
    pub seq: u64,
    pub slot: Slot,
}

/// The waiting transfers, the next first.
pub(crate) fn line_up(lines: &[Line]) -> Vec<Key> {
    let mut waiting: Vec<&Line> = lines.iter().filter(|l| l.slot == Slot::Waiting).collect();
    waiting.sort_by_key(|l| l.seq);
    waiting.into_iter().map(|l| l.key).collect()
}

/// The transfers to start now: as many of the line as there are free places.
pub(crate) fn to_start(lines: &[Line], limit: u32) -> Vec<Key> {
    let holding = lines.iter().filter(|l| l.slot == Slot::Held).count();
    let free = (limit as usize).saturating_sub(holding);
    line_up(lines).into_iter().take(free).collect()
}

/// Why a queue setting was refused, in the engine's words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueError {
    BadLimit,
}

impl fmt::Display for QueueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadLimit => write!(f, "the limit is a number from 1 to {MOST_LIMIT}"),
        }
    }
}

impl std::error::Error for QueueError {}

/// The queue as it stands.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QueueView {
    pub limit: u32,
    /// How many hold a place now.
    pub holding: u32,
    /// The waiting transfers and their places in line, from 1.
    pub places: Vec<(Key, u32)>,
}

/// Starts what the limit allows. Cheap to clone: the clones share the queue.
#[derive(Clone)]
pub struct Queue {
    inner: Arc<Inner>,
}

struct Inner {
    downloads: Downloads,
    torrents: Torrents,
    limit: watch::Sender<u32>,
    view: watch::Sender<QueueView>,
    /// Once true, nothing more starts: the engine is stopping.
    frozen: AtomicBool,
}

impl Queue {
    /// Start the queue on both engines, with `limit`, as read from the record.
    pub fn start(downloads: Downloads, torrents: Torrents, limit: u32) -> Self {
        let queue = Self {
            inner: Arc::new(Inner {
                downloads,
                torrents,
                limit: watch::channel(limit).0,
                view: watch::channel(QueueView {
                    limit,
                    ..QueueView::default()
                })
                .0,
                frozen: AtomicBool::new(false),
            }),
        };
        tokio::spawn(run(queue.clone()));
        queue
    }

    /// The engine's downloads.
    pub fn downloads(&self) -> &Downloads {
        &self.inner.downloads
    }

    /// The engine's torrents.
    pub fn torrents(&self) -> &Torrents {
        &self.inner.torrents
    }

    /// Put a URL in line.
    pub async fn add_download(&self, request: AddRequest) -> Result<DownloadId, AddError> {
        TransferEngine::enqueue(&self.inner.downloads, request).await
    }

    /// Put a torrent in line.
    pub async fn add_torrent(&self, source: Source) -> Result<TorrentId, TorrentAddError> {
        TransferEngine::enqueue(&self.inner.torrents, source).await
    }

    /// Send a stopped download to the back of the line.
    pub async fn resume_download(&self, id: DownloadId) -> Result<(), ControlError> {
        TransferEngine::requeue(&self.inner.downloads, id).await
    }

    /// Send a stopped torrent to the back of the line.
    pub async fn resume_torrent(&self, id: TorrentId) -> Result<(), TorrentControlError> {
        TransferEngine::requeue(&self.inner.torrents, id).await
    }

    /// The limit now.
    pub fn limit(&self) -> u32 {
        *self.inner.limit.borrow()
    }

    /// Change the limit. A higher one starts the next at once; a lower one
    /// stops nothing.
    pub fn set_limit(&self, limit: u32) -> Result<(), QueueError> {
        if !(1..=MOST_LIMIT).contains(&limit) {
            return Err(QueueError::BadLimit);
        }
        self.inner.limit.send_replace(limit);
        Ok(())
    }

    /// The queue, kept up to date.
    pub fn view(&self) -> watch::Receiver<QueueView> {
        self.inner.view.subscribe()
    }

    /// Start nothing more: the engine is stopping, and a download that ends
    /// now must not start the next.
    pub fn freeze(&self) {
        self.inner.frozen.store(true, Ordering::SeqCst);
    }

    /// Every transfer, as the line sees it.
    fn lines(&self) -> Vec<Line> {
        let mut lines: Vec<Line> = self
            .inner
            .downloads
            .status()
            .borrow()
            .iter()
            .map(|d| Line {
                key: Key::Web(d.id),
                priority: d.priority,
                seq: d.seq,
                slot: match d.phase {
                    Phase::Queued => Slot::Waiting,
                    Phase::Starting | Phase::Waiting | Phase::Downloading => Slot::Held,
                    Phase::Done | Phase::Failed | Phase::Stopped => Slot::Out,
                },
            })
            .collect();
        lines.extend(self.inner.torrents.status().borrow().iter().map(|t| Line {
            key: Key::Torrent(t.id),
            priority: t.priority,
            seq: t.seq,
            slot: match t.phase {
                TorrentPhase::Queued => Slot::Waiting,
                TorrentPhase::Finding | TorrentPhase::Downloading => Slot::Held,
                TorrentPhase::Done | TorrentPhase::Failed | TorrentPhase::Stopped => Slot::Out,
            },
        }));
        lines
    }

    /// Start what there is room for, and show where everything stands.
    async fn step(&self) {
        let limit = self.limit();
        if !self.inner.frozen.load(Ordering::SeqCst) {
            for key in to_start(&self.lines(), limit) {
                // One stopped since is no longer queued, and is passed over.
                let _ = match key {
                    Key::Web(id) => TransferEngine::release(&self.inner.downloads, id)
                        .await
                        .map_err(|_| ()),
                    Key::Torrent(id) => TransferEngine::release(&self.inner.torrents, id)
                        .await
                        .map_err(|_| ()),
                };
            }
        }
        let lines = self.lines();
        let view = QueueView {
            limit,
            holding: lines.iter().filter(|l| l.slot == Slot::Held).count() as u32,
            places: line_up(&lines).into_iter().zip(1..).collect(),
        };
        self.inner.view.send_if_modified(|shown| {
            let changed = *shown != view;
            if changed {
                *shown = view;
            }
            changed
        });
    }
}

/// Look again whenever either engine or the limit changes.
async fn run(queue: Queue) {
    let mut downloads = queue.inner.downloads.status();
    let mut torrents = queue.inner.torrents.status();
    let mut limit = queue.inner.limit.subscribe();
    loop {
        queue.step().await;
        tokio::select! {
            changed = downloads.changed() => if changed.is_err() { return },
            changed = torrents.changed() => if changed.is_err() { return },
            changed = limit.changed() => if changed.is_err() { return },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn web(id: u64) -> Key {
        Key::Web(DownloadId::new(id))
    }

    fn line(id: u64, seq: u64, slot: Slot) -> Line {
        Line {
            key: web(id),
            priority: Priority::Normal,
            seq,
            slot,
        }
    }

    #[test]
    fn the_earliest_added_goes_first() {
        let lines = [
            line(3, 30, Slot::Waiting),
            line(1, 10, Slot::Waiting),
            line(2, 20, Slot::Waiting),
        ];
        assert_eq!(line_up(&lines), [web(1), web(2), web(3)]);
    }

    #[test]
    fn only_as_many_as_there_are_free_places() {
        let lines = [
            line(1, 1, Slot::Held),
            line(2, 2, Slot::Waiting),
            line(3, 3, Slot::Waiting),
            line(4, 4, Slot::Waiting),
        ];
        assert_eq!(to_start(&lines, 3), [web(2), web(3)]);
        assert_eq!(to_start(&lines, 2), [web(2)]);
        assert_eq!(to_start(&lines, 1), []);
    }

    #[test]
    fn web_and_torrent_share_one_limit() {
        let torrent = Line {
            key: Key::Torrent(TorrentId(1)),
            ..line(0, 1, Slot::Held)
        };
        let waiting = Line {
            key: Key::Torrent(TorrentId(2)),
            ..line(0, 3, Slot::Waiting)
        };
        let lines = [
            torrent,
            line(1, 2, Slot::Held),
            waiting,
            line(2, 4, Slot::Waiting),
        ];
        assert_eq!(to_start(&lines, 2), []);
        assert_eq!(to_start(&lines, 3), [waiting.key]);
    }

    #[test]
    fn a_lower_limit_starts_nothing_and_stops_nothing() {
        let lines = [
            line(1, 1, Slot::Held),
            line(2, 2, Slot::Held),
            line(3, 3, Slot::Held),
            line(4, 4, Slot::Waiting),
        ];
        assert_eq!(to_start(&lines, 2), []);
    }

    #[test]
    fn nothing_waiting_starts_nothing() {
        let lines = [line(1, 1, Slot::Held), line(2, 2, Slot::Out)];
        assert_eq!(to_start(&lines, 5), []);
        assert_eq!(to_start(&[], 5), []);
    }

    #[test]
    fn what_ended_holds_no_place() {
        let lines = [line(1, 1, Slot::Out), line(2, 2, Slot::Waiting)];
        assert_eq!(to_start(&lines, 1), [web(2)]);
    }
}
