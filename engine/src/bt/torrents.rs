//! The torrent engine: one librqbit session, through BARRAGE's fork, every
//! peer connection opened by `connect::Connector`. Each torrent is listed
//! at once, before a magnet's metadata arrives, and its status published
//! every tick from librqbit's state. A finished torrent is paused, and so is
//! a stopped one: neither uploads (no seeding, the user's choice).

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use librqbit::{
    AddTorrent, AddTorrentOptions, AddTorrentResponse, ConnectionOptions, DhtSessionConfig,
    ManagedTorrent, Session, SessionOptions, TorrentStatsState,
};
use tokio::sync::watch;

use super::connect::Connector;
use super::source::{Source, TorrentAddError, info_hash};
use super::status::{TorrentCable, TorrentControlError, TorrentId, TorrentPhase, TorrentStatus};
use crate::fetch::Usage;
use crate::links::watch::LinkTable;
use crate::transfer::TransferEngine;

/// How the torrents run. Tests turn the network off and name their peers.
#[derive(Debug, Clone)]
pub struct TorrentConfig {
    /// DHT bootstrapping and trackers. Off, the DHT socket still opens but
    /// is told of nobody, and torrents find only `peers`.
    pub network: bool,
    /// Listen for peers calling in (Task 9).
    pub listen: bool,
    /// How often the status is published.
    pub tick: Duration,
    /// Peers every torrent starts with, besides what it finds.
    pub peers: Vec<SocketAddr>,
}

impl Default for TorrentConfig {
    fn default() -> Self {
        Self {
            network: true,
            listen: true,
            tick: Duration::from_secs(1),
            peers: Vec::new(),
        }
    }
}

/// What the user asked of a torrent, which librqbit's state alone does not
/// say: a paused torrent may be stopped or done.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wanted {
    Running,
    Stopped,
    Done,
}

struct Entry {
    id: TorrentId,
    info_hash: String,
    /// The magnet's `dn`, until librqbit knows the name.
    name: Option<String>,
    /// Set once librqbit has the torrent (a magnet: once its metadata came).
    handle: Option<Arc<ManagedTorrent>>,
    wanted: Wanted,
    failed: Option<String>,
}

struct Inner {
    session: Arc<Session>,
    dir: PathBuf,
    config: TorrentConfig,
    status: watch::Sender<Vec<TorrentStatus>>,
    list: Mutex<Vec<Entry>>,
    next: AtomicU64,
    stopping: AtomicBool,
    table: LinkTable,
    /// Opens and counts every peer connection.
    connector: Arc<Connector>,
    /// Data used, shared with the downloads.
    usage: Arc<Usage>,
}

#[derive(Clone)]
pub struct Torrents {
    inner: Arc<Inner>,
}

impl Torrents {
    /// Start the session; torrents download into `dir`.
    pub async fn start(
        table: LinkTable,
        dir: PathBuf,
        usage: Arc<Usage>,
        config: TorrentConfig,
    ) -> anyhow::Result<Self> {
        let connector = Arc::new(Connector::new(table.clone(), Arc::clone(&usage)));
        let session = Session::new_with_opts(
            dir.clone(),
            session_options(Arc::clone(&connector), &config),
        )
        .await?;
        let torrents = Self {
            inner: Arc::new(Inner {
                session,
                dir,
                config,
                status: watch::channel(Vec::new()).0,
                list: Mutex::new(Vec::new()),
                next: AtomicU64::new(0),
                stopping: AtomicBool::new(false),
                table,
                connector,
                usage,
            }),
        };
        let ticking = Arc::downgrade(&torrents.inner);
        let tick = torrents.inner.config.tick;
        tokio::spawn(async move {
            let mut every = tokio::time::interval(tick);
            loop {
                every.tick().await;
                let Some(inner) = ticking.upgrade() else {
                    return;
                };
                if inner.stopping.load(Ordering::SeqCst) {
                    return;
                }
                inner.publish().await;
            }
        });
        Ok(torrents)
    }
}

/// librqbit's options: BARRAGE opens every peer connection; no local
/// service discovery (it multicasts on every interface, not one cable); no
/// persistence before phase 6.
fn session_options(connector: Arc<Connector>, config: &TorrentConfig) -> SessionOptions {
    let mut options = SessionOptions {
        connect: Some(ConnectionOptions {
            peer_connector: Some(connector),
            ..Default::default()
        }),
        dht: Some(DhtSessionConfig {
            bootstrap_addrs: None,
            port: None,
            persistence: None,
        }),
        disable_local_service_discovery: true,
        fastresume: false,
        persistence: None,
        listen: None,
        ..Default::default()
    };
    if !config.network {
        options.dht = Some(DhtSessionConfig {
            bootstrap_addrs: Some(Vec::new()),
            port: None,
            persistence: None,
        });
        options.disable_trackers = true;
    }
    options
}

impl Inner {
    fn entry<T>(&self, id: TorrentId, f: impl FnOnce(&mut Entry) -> T) -> Option<T> {
        self.list
            .lock()
            .unwrap()
            .iter_mut()
            .find(|e| e.id == id)
            .map(f)
    }

    /// librqbit's state for every torrent, as BARRAGE's status; a torrent
    /// that has just finished is paused here.
    async fn publish(&self) {
        let links = self.table.snapshot();
        self.connector.refresh(&links);
        self.usage.publish();
        let mut finished = Vec::new();
        let statuses: Vec<TorrentStatus> = {
            let mut list = self.list.lock().unwrap();
            list.iter_mut()
                .map(|entry| {
                    let status =
                        status_of(entry, self.connector.cables_of(&entry.info_hash, &links));
                    if entry.wanted == Wanted::Running && status.phase == TorrentPhase::Done {
                        entry.wanted = Wanted::Done;
                        if let Some(handle) = &entry.handle {
                            finished.push(Arc::clone(handle));
                        }
                    }
                    status
                })
                .collect()
        };
        for handle in finished {
            let _ = self.session.pause(&handle).await;
        }
        self.status.send_if_modified(|shown| {
            if *shown == statuses {
                return false;
            }
            *shown = statuses;
            true
        });
    }
}

/// One torrent's status, from librqbit's stats and what the user asked.
fn status_of(entry: &Entry, cables: Vec<TorrentCable>) -> TorrentStatus {
    let mut status = TorrentStatus {
        id: entry.id,
        name: entry.name.clone(),
        info_hash: entry.info_hash.clone(),
        phase: TorrentPhase::Finding,
        length: None,
        done: 0,
        down_bps: 0,
        up_bps: 0,
        peers: 0,
        note: entry.failed.clone(),
        cables,
    };
    if entry.failed.is_some() {
        status.phase = TorrentPhase::Failed;
        return status;
    }
    let Some(handle) = &entry.handle else {
        if entry.wanted == Wanted::Stopped {
            status.phase = TorrentPhase::Stopped;
        }
        return status;
    };
    let stats = handle.stats();
    status.name = handle.name().or(status.name);
    status.length = Some(stats.total_bytes);
    status.done = stats.progress_bytes;
    if let Some(error) = stats.error {
        status.phase = TorrentPhase::Failed;
        status.note = Some(error);
        return status;
    }
    status.phase = match entry.wanted {
        Wanted::Stopped => TorrentPhase::Stopped,
        Wanted::Done => TorrentPhase::Done,
        Wanted::Running if stats.finished => TorrentPhase::Done,
        Wanted::Running => TorrentPhase::Downloading,
    };
    if let (TorrentStatsState::Live, Some(live)) = (&stats.state, &stats.live) {
        status.peers = live.snapshot.peer_stats.live;
        if status.phase == TorrentPhase::Downloading {
            status.down_bps = live.download_speed.as_bytes() * 8;
            status.up_bps = live.upload_speed.as_bytes() * 8;
        }
    }
    status
}

impl TransferEngine for Torrents {
    type Request = Source;
    type Id = TorrentId;
    type Status = TorrentStatus;
    type AddError = TorrentAddError;
    type ControlError = TorrentControlError;

    async fn add(&self, source: Source) -> Result<TorrentId, TorrentAddError> {
        let inner = &self.inner;
        if inner.stopping.load(Ordering::SeqCst) {
            return Err(TorrentAddError::Stopped);
        }
        let hash = info_hash(&source)?;
        let id = {
            let mut list = inner.list.lock().unwrap();
            if let Some(same) = list.iter().find(|e| e.info_hash == hash) {
                return Err(TorrentAddError::AlreadyAdded(same.id));
            }
            let id = TorrentId(inner.next.fetch_add(1, Ordering::SeqCst) + 1);
            let name = match &source {
                Source::Magnet(link) => librqbit::Magnet::parse(link).ok().and_then(|m| m.name),
                Source::File(_) => None,
            };
            list.push(Entry {
                id,
                info_hash: hash,
                name,
                handle: None,
                wanted: Wanted::Running,
                failed: None,
            });
            id
        };

        // A magnet's metadata can take a while to arrive, so librqbit is
        // given the torrent in the background; it is listed already.
        let adding = Arc::clone(inner);
        tokio::spawn(async move {
            let add = match source {
                Source::Magnet(link) => AddTorrent::from_url(link),
                Source::File(bytes) => AddTorrent::from_bytes(bytes),
            };
            let options = AddTorrentOptions {
                output_folder: Some(adding.dir.to_string_lossy().into_owned()),
                overwrite: true,
                initial_peers: (!adding.config.peers.is_empty())
                    .then(|| adding.config.peers.clone()),
                ..Default::default()
            };
            match adding.session.add_torrent(add, Some(options)).await {
                Ok(
                    AddTorrentResponse::Added(_, handle)
                    | AddTorrentResponse::AlreadyManaged(_, handle),
                ) => {
                    let stopped = adding.entry(id, |e| {
                        e.handle = Some(Arc::clone(&handle));
                        e.wanted == Wanted::Stopped
                    });
                    if stopped == Some(true) {
                        let _ = adding.session.pause(&handle).await;
                    }
                }
                Ok(AddTorrentResponse::ListOnly(_)) => {
                    adding.entry(id, |e| e.failed = Some("librqbit only listed it".into()));
                }
                Err(e) => {
                    adding.entry(id, |entry| entry.failed = Some(format!("{e:#}")));
                }
            }
            adding.publish().await;
        });
        inner.publish().await;
        Ok(id)
    }

    fn status(&self) -> watch::Receiver<Vec<TorrentStatus>> {
        self.inner.status.subscribe()
    }

    async fn stop(&self, id: TorrentId) -> Result<(), TorrentControlError> {
        let inner = &self.inner;
        let handle = inner
            .entry(id, |e| {
                if e.wanted != Wanted::Running || e.failed.is_some() {
                    return Err(TorrentControlError::NotRunning);
                }
                e.wanted = Wanted::Stopped;
                Ok(e.handle.clone())
            })
            .ok_or(TorrentControlError::NoSuchTorrent)??;
        if let Some(handle) = handle {
            let _ = inner.session.pause(&handle).await;
        }
        inner.publish().await;
        Ok(())
    }

    async fn resume(&self, id: TorrentId) -> Result<(), TorrentControlError> {
        let inner = &self.inner;
        let handle = inner
            .entry(id, |e| {
                if e.wanted != Wanted::Stopped {
                    return Err(TorrentControlError::NotStopped);
                }
                e.wanted = Wanted::Running;
                Ok(e.handle.clone())
            })
            .ok_or(TorrentControlError::NoSuchTorrent)??;
        if let Some(handle) = handle {
            let _ = inner.session.unpause(&handle).await;
        }
        inner.publish().await;
        Ok(())
    }

    async fn shutdown(&self) {
        self.inner.stopping.store(true, Ordering::SeqCst);
        self.inner.session.stop().await;
    }
}
