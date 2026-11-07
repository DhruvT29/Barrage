//! Keeps the saved record (`record.rs`) up to date as the engine runs, and
//! writes it one last time as the engine stops.
//!
//! The status alone cannot say what to write at the end: a download stopped
//! because the engine is stopping shows as stopped, the same as one the user
//! stopped, and only the second should stay stopped at the next start. So
//! `serve` asks [`Keeper::running`] before it stops anything, and hands the
//! answer to [`Keeper::finish`] afterwards.
//!
//! A save that fails never stops the engine. The page is told, and told when
//! saving works again.

use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use tokio::sync::watch;
use tokio::time::MissedTickBehavior;

use crate::bt::{TorrentId, Torrents};
use crate::fetch::{Downloads, Usage};
use crate::queue::Queue;
use crate::record::{KeptState, Opened, Record, Store, What};
use crate::transfer::Key;

/// What the record is read from.
pub struct Engines {
    pub downloads: Downloads,
    pub torrents: Torrents,
    pub queue: Queue,
    pub usage: Arc<Usage>,
}

/// Saves the record. Dropping it stops the saving, and frees the folder.
pub struct Keeper {
    inner: Arc<Inner>,
}

struct Inner {
    /// `None` once the engine has stopped.
    store: Mutex<Option<Store>>,
    /// The parts of the record nothing else holds, and the last one saved.
    base: Record,
    last: Mutex<Option<Record>>,
    /// Why the record was set aside at the start, if it was.
    opened: Option<String>,
    notice: watch::Sender<Option<String>>,
    engines: Engines,
}

/// The key of a saved transfer.
fn key_of(what: &What) -> Key {
    match *what {
        What::Web { id, .. } => Key::Web(crate::fetch::DownloadId::new(id)),
        What::Torrent { id, .. } => Key::Torrent(TorrentId(id)),
    }
}

impl Keeper {
    /// Start saving the record `opened` holds, at most once per `every` and
    /// only when something has changed.
    pub fn start(opened: Opened, engines: Engines, every: Duration) -> Self {
        let (store, record, note) = opened;
        let inner = Arc::new(Inner {
            store: Mutex::new(Some(store)),
            base: record,
            last: Mutex::new(None),
            notice: watch::channel(note.clone()).0,
            opened: note.clone(),
            engines,
        });
        inner.engines.queue.set_used_since(inner.base.used_since);
        inner.engines.queue.set_notice(note);
        tokio::spawn(run(Arc::downgrade(&inner), every));
        Self { inner }
    }

    /// What the engine has running now, for [`Keeper::finish`].
    pub fn running(&self) -> Vec<Key> {
        let record = self.inner.record();
        record
            .transfers
            .iter()
            .filter(|k| k.state == KeptState::Run)
            .map(|k| key_of(&k.what))
            .collect()
    }

    /// Save the record one last time and let go of the folder. What is in
    /// `ran` and has merely stopped goes on at the next start.
    pub fn finish(&self, ran: &[Key]) {
        let mut record = self.inner.record();
        for kept in &mut record.transfers {
            if kept.state == KeptState::Stopped && ran.contains(&key_of(&kept.what)) {
                kept.state = KeptState::Run;
            }
        }
        if let Some(store) = self.inner.store.lock().unwrap().take()
            && let Err(e) = store.save(&record)
        {
            tracing::warn!("the saved list could not be saved at the end: {e}");
        }
    }

    /// What the user should know about the saved list, if anything.
    pub fn notice(&self) -> watch::Receiver<Option<String>> {
        self.inner.notice.subscribe()
    }
}

impl Inner {
    fn record(&self) -> Record {
        let engines = &self.engines;
        let mut transfers = engines.downloads.kept();
        transfers.extend(engines.torrents.kept());
        transfers.sort_by_key(|k| k.seq);
        Record {
            limit: engines.queue.limit(),
            used_since: engines.queue.used_since(),
            used: engines.usage.kept(),
            next_download: engines.downloads.last_id(),
            next_torrent: engines.torrents.last_id(),
            transfers,
            ..self.base.clone()
        }
    }

    /// Save the record if it has changed. False once the engine has stopped.
    fn save(&self) -> bool {
        let record = self.record();
        let store = self.store.lock().unwrap();
        let Some(store) = store.as_ref() else {
            return false;
        };
        let mut last = self.last.lock().unwrap();
        if last.as_ref() == Some(&record) {
            return true;
        }
        match store.save(&record) {
            Ok(()) => {
                *last = Some(record);
                self.notice.send_if_modified(|shown| {
                    let changed = *shown != self.opened;
                    shown.clone_from(&self.opened);
                    changed
                });
            }
            Err(e) => {
                let said = format!(
                    "The transfers could not be saved ({e}), so a restart would lose what has \
                     changed since."
                );
                self.notice.send_if_modified(|shown| {
                    if shown.as_deref() == Some(said.as_str()) {
                        return false;
                    }
                    tracing::warn!("{said}");
                    *shown = Some(said.clone());
                    true
                });
            }
        }
        let shown = self.notice.borrow().clone();
        self.engines.queue.set_notice(shown);
        true
    }
}

/// Save every `every` for as long as the keeper lives. Off the runtime's
/// threads, since a save writes to the disk.
async fn run(keeper: Weak<Inner>, every: Duration) {
    let mut tick = tokio::time::interval(every);
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        let Some(inner) = keeper.upgrade() else {
            return;
        };
        match tokio::task::spawn_blocking(move || inner.save()).await {
            Ok(true) => {}
            _ => return,
        }
    }
}
