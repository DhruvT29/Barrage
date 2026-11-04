//! The torrent engine: one librqbit session, through BARRAGE's fork, every
//! peer connection opened by `connect::Connector`. Each torrent is listed
//! at once, before a magnet's metadata arrives, and its status published
//! every tick from librqbit's state. A finished torrent is paused, and so is
//! a stopped one: neither uploads (no seeding, the user's choice).
//!
//! The session's DHT, UDP tracker and uTP sockets and its listener are bound
//! to the UDP cable (`choose::udp_cable`), and librqbit binds them only when
//! a session starts: when the UDP cable changes, or the port opens or closes
//! (`listen::next`), the session is rebuilt (`rebuild::due`) and every
//! torrent given to it again in its phase. librqbit checks the files already
//! on disk, so each keeps what it has.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use librqbit::{
    AddTorrent, AddTorrentOptions, AddTorrentResponse, ConnectionOptions, DhtSessionConfig,
    ListenerMode, ListenerOptions, ManagedTorrent, Session, SessionOptions, TorrentStatsState,
};
use tokio::sync::watch;
use tokio::time::Instant;

use super::choose::udp_cable;
use super::connect::Connector;
use super::listen::{Listening, TRY_FOR, next};
use super::rebuild::{Place, due};
use super::source::{Source, TorrentAddError, info_hash};
use super::status::{
    Swarm, TorrentCable, TorrentControlError, TorrentId, TorrentPhase, TorrentStatus,
};
use crate::fetch::Usage;
use crate::links::watch::LinkTable;
use crate::transfer::TransferEngine;

/// Where the UDP sockets bind while no cable works: nothing leaves this
/// machine through Windows' choice of link.
const NO_CABLE: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

/// How the torrents run. Tests turn the network off and name their peers.
#[derive(Debug, Clone)]
pub struct TorrentConfig {
    /// DHT bootstrapping and trackers. Off, the DHT socket still opens but
    /// is told of nobody, and torrents find only `peers`.
    pub network: bool,
    /// Listen for peers calling in.
    pub listen: bool,
    /// How long a torrent runs with nobody calling in before the port closes.
    pub try_for: Duration,
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
            try_for: TRY_FOR,
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

/// What a torrent showed when its session was rebuilt, shown until the new
/// session has checked its files, so its progress does not drop to nothing
/// meanwhile.
#[derive(Debug, Clone, Copy)]
struct Kept {
    length: Option<u64>,
    done: u64,
}

struct Entry {
    id: TorrentId,
    info_hash: String,
    /// What to give librqbit again after a rebuild: once its metadata is
    /// known, the .torrent file's bytes, even for a magnet.
    source: Source,
    /// The magnet's `dn`, until librqbit knows the name.
    name: Option<String>,
    /// Set once librqbit has the torrent (a magnet: once its metadata came).
    handle: Option<Arc<ManagedTorrent>>,
    /// The session it was last given to; an answer from an older one is
    /// ignored.
    generation: u64,
    kept: Option<Kept>,
    wanted: Wanted,
    failed: Option<String>,
}

struct Inner {
    session: Mutex<Arc<Session>>,
    /// Counts the sessions, one more at each rebuild.
    generation: AtomicU64,
    /// Where the session is, where the port stands, and when the session
    /// was last rebuilt.
    udp: Mutex<Udp>,
    swarm: watch::Sender<Swarm>,
    dir: PathBuf,
    config: TorrentConfig,
    status: watch::Sender<Vec<TorrentStatus>>,
    list: Mutex<Vec<Entry>>,
    next: AtomicU64,
    stopping: AtomicBool,
    table: LinkTable,
    /// Opens and counts every peer connection, for every session in turn.
    connector: Arc<Connector>,
    /// Data used, shared with the downloads.
    usage: Arc<Usage>,
}

#[derive(Default)]
struct Udp {
    on: Option<Place>,
    listening: Listening,
    rebuilt: Option<Instant>,
}

#[derive(Clone)]
pub struct Torrents {
    inner: Arc<Inner>,
}

impl Torrents {
    /// Start the session on the UDP cable; torrents download into `dir`.
    pub async fn start(
        table: LinkTable,
        dir: PathBuf,
        usage: Arc<Usage>,
        config: TorrentConfig,
    ) -> anyhow::Result<Self> {
        let connector = Arc::new(Connector::new(table.clone(), Arc::clone(&usage)));
        let udp = udp_cable(&table.snapshot());
        let listening = if config.listen {
            let cable = udp.as_ref().map(|(cable, _)| cable.as_str());
            let now = Instant::now();
            next(&Listening::Off, now, 0, false, cable, config.try_for)
        } else {
            Listening::Off
        };
        let on = udp.map(|(cable, ip)| Place {
            cable,
            ip,
            listening: listening.wants_port(),
        });
        let session = Session::new_with_opts(
            dir.clone(),
            session_options(Arc::clone(&connector), &config, on.as_ref()),
        )
        .await?;
        let torrents = Self {
            inner: Arc::new(Inner {
                session: Mutex::new(session),
                generation: AtomicU64::new(0),
                swarm: watch::channel(Swarm {
                    udp_cable: on.as_ref().map(|on| on.cable.clone()),
                    listening: listening.clone(),
                })
                .0,
                udp: Mutex::new(Udp {
                    on,
                    listening,
                    rebuilt: None,
                }),
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
                inner.move_session().await;
            }
        });
        Ok(torrents)
    }

    /// The UDP cable and the listening port, kept up to date.
    pub fn swarm(&self) -> watch::Receiver<Swarm> {
        self.inner.swarm.subscribe()
    }

    /// The addresses the session's DHT, UDP tracker and uTP sockets are
    /// bound to.
    pub fn udp_addrs(&self) -> Vec<SocketAddr> {
        self.inner.session().udp_local_addrs()
    }

    /// The address peers can call in on, while the session listens.
    pub fn listen_addr(&self) -> Option<SocketAddr> {
        self.inner.session().listen_addr()
    }
}

/// librqbit's options: BARRAGE opens every peer connection; UDP and any
/// listener at `place`, or on this machine, with no listener, without one;
/// no local service discovery (it multicasts on every interface, not one
/// cable); no persistence before phase 6.
fn session_options(
    connector: Arc<Connector>,
    config: &TorrentConfig,
    place: Option<&Place>,
) -> SessionOptions {
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
        udp_bind_ip: Some(place.map_or(NO_CABLE, |on| on.ip)),
        disable_local_service_discovery: true,
        fastresume: false,
        persistence: None,
        listen: place.filter(|on| on.listening).map(|on| ListenerOptions {
            mode: ListenerMode::TcpAndUtp,
            listen_addr: SocketAddr::new(on.ip, 0),
            enable_upnp_port_forwarding: config.network,
            ..Default::default()
        }),
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
    fn session(&self) -> Arc<Session> {
        Arc::clone(&self.session.lock().unwrap())
    }

    fn entry<T>(&self, id: TorrentId, f: impl FnOnce(&mut Entry) -> T) -> Option<T> {
        self.list
            .lock()
            .unwrap()
            .iter_mut()
            .find(|e| e.id == id)
            .map(f)
    }

    /// Give torrent `id` to the session of `generation`, in the background:
    /// a magnet's metadata can take a while to arrive, and it is listed
    /// already. `paused`: it is given stopped or done.
    fn give(self: &Arc<Self>, id: TorrentId, source: Source, generation: u64, paused: bool) {
        let session = self.session();
        let giving = Arc::clone(self);
        tokio::spawn(async move {
            let add = match source {
                Source::Magnet(link) => AddTorrent::from_url(link),
                Source::File(bytes) => AddTorrent::from_bytes(bytes),
            };
            let options = AddTorrentOptions {
                output_folder: Some(giving.dir.to_string_lossy().into_owned()),
                overwrite: true,
                paused,
                initial_peers: (!giving.config.peers.is_empty())
                    .then(|| giving.config.peers.clone()),
                ..Default::default()
            };
            let added = session.add_torrent(add, Some(options)).await;
            let current = |e: &Entry| e.generation == generation;
            match added {
                Ok(
                    AddTorrentResponse::Added(_, handle)
                    | AddTorrentResponse::AlreadyManaged(_, handle),
                ) => {
                    let wanted = giving.entry(id, |e| {
                        current(e).then(|| {
                            e.handle = Some(Arc::clone(&handle));
                            e.wanted
                        })
                    });
                    // Stopped or resumed while it was being given.
                    match wanted.flatten() {
                        Some(Wanted::Running) if paused => {
                            let _ = session.unpause(&handle).await;
                        }
                        Some(Wanted::Stopped | Wanted::Done) if !paused => {
                            let _ = session.pause(&handle).await;
                        }
                        _ => {}
                    }
                }
                Ok(AddTorrentResponse::ListOnly(_)) => {
                    giving.entry(id, |e| {
                        if current(e) {
                            e.failed = Some("librqbit only listed it".into());
                        }
                    });
                }
                Err(why) => {
                    giving.entry(id, |e| {
                        if current(e) {
                            e.failed = Some(format!("{why:#}"));
                        }
                    });
                }
            }
            giving.publish().await;
        });
    }

    /// Where the port stands now, given the UDP cable wanted.
    fn look_at_port(&self, udp: &mut Udp, wanted: Option<(&str, IpAddr)>, now: Instant) {
        // A peer reaching the old listener says nothing of a cable the
        // session has yet to move to.
        let settled = udp.on.as_ref().map(|on| (on.cable.as_str(), on.ip)) == wanted;
        let incoming = self.connector.take_incoming();
        let running = self
            .list
            .lock()
            .unwrap()
            .iter()
            .any(|e| e.wanted == Wanted::Running && e.failed.is_none());
        let port = next(
            &udp.listening,
            now,
            if settled { incoming } else { 0 },
            running,
            wanted.map(|(cable, _)| cable),
            self.config.try_for,
        );
        if !port.reads_like(&udp.listening) {
            self.swarm
                .send_modify(|swarm| swarm.listening = port.clone());
        }
        udp.listening = port;
    }

    /// Rebuild the session if the UDP cable or the port has changed and the
    /// last rebuild is long enough ago.
    async fn move_session(self: &Arc<Self>) {
        let cable = udp_cable(&self.table.snapshot());
        let place = {
            let mut udp = self.udp.lock().unwrap();
            let now = Instant::now();
            if self.config.listen {
                let asked = cable.as_ref().map(|(cable, ip)| (cable.as_str(), *ip));
                self.look_at_port(&mut udp, asked, now);
            }
            let wanted = cable.map(|(cable, ip)| Place {
                cable,
                ip,
                listening: udp.listening.wants_port(),
            });
            if !due(now, udp.on.as_ref(), wanted.as_ref(), udp.rebuilt) {
                return;
            }
            udp.rebuilt = Some(now);
            wanted.expect("due wants a cable")
        };
        let cable = place.cable.clone();
        let options = session_options(Arc::clone(&self.connector), &self.config, Some(&place));
        let session = match Session::new_with_opts(self.dir.clone(), options).await {
            Ok(session) => session,
            Err(why) => {
                // Tried again after the gap; the old session carries on.
                tracing::warn!("torrents: no session on {cable}: {why:#}");
                return;
            }
        };
        let old = std::mem::replace(&mut *self.session.lock().unwrap(), session);
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let again: Vec<(TorrentId, Source, bool)> = self
            .list
            .lock()
            .unwrap()
            .iter_mut()
            .filter(|e| e.failed.is_none())
            .map(|e| {
                if let Some(handle) = e.handle.take() {
                    let stats = handle.stats();
                    e.name = handle.name().or(e.name.take());
                    e.kept = Some(Kept {
                        length: Some(stats.total_bytes),
                        done: stats.progress_bytes.max(e.kept.map_or(0, |k| k.done)),
                    });
                    if let Ok(bytes) = handle.with_metadata(|m| m.torrent_bytes.to_vec()) {
                        e.source = Source::File(bytes);
                    }
                }
                e.generation = generation;
                (e.id, e.source.clone(), e.wanted != Wanted::Running)
            })
            .collect();
        self.udp.lock().unwrap().on = Some(place);
        self.swarm
            .send_modify(|swarm| swarm.udp_cable = Some(cable));
        old.stop().await;
        if self.stopping.load(Ordering::SeqCst) {
            // Shut down meanwhile, perhaps before this session was in place.
            self.session().stop().await;
            return;
        }
        for (id, source, paused) in again {
            self.give(id, source, generation, paused);
        }
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
                    let checked = entry.handle.as_ref().is_some_and(|h| {
                        !matches!(h.stats().state, TorrentStatsState::Initializing { .. })
                    });
                    if checked {
                        entry.kept = None;
                    }
                    status
                })
                .collect()
        };
        let session = self.session();
        for handle in finished {
            let _ = session.pause(&handle).await;
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

/// The phase of a torrent its session is not running, from what the user
/// asked.
fn phase_asked(wanted: Wanted) -> TorrentPhase {
    match wanted {
        Wanted::Running => TorrentPhase::Downloading,
        Wanted::Stopped => TorrentPhase::Stopped,
        Wanted::Done => TorrentPhase::Done,
    }
}

/// One torrent's status, from librqbit's stats and what the user asked.
fn status_of(entry: &Entry, cables: Vec<TorrentCable>) -> TorrentStatus {
    let kept = entry.kept;
    let mut status = TorrentStatus {
        id: entry.id,
        name: entry.name.clone(),
        info_hash: entry.info_hash.clone(),
        phase: TorrentPhase::Finding,
        length: kept.and_then(|k| k.length),
        done: kept.map_or(0, |k| k.done),
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
        if kept.is_some() {
            // Being given to a rebuilt session.
            status.phase = phase_asked(entry.wanted);
        } else if entry.wanted == Wanted::Stopped {
            status.phase = TorrentPhase::Stopped;
        }
        return status;
    };
    let stats = handle.stats();
    status.name = handle.name().or(status.name);
    status.length = Some(stats.total_bytes);
    status.done = stats.progress_bytes.max(status.done);
    if let Some(error) = stats.error {
        status.phase = TorrentPhase::Failed;
        status.note = Some(error);
        return status;
    }
    status.phase = match entry.wanted {
        Wanted::Running if stats.finished => TorrentPhase::Done,
        wanted => phase_asked(wanted),
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
        let (id, generation) = {
            let mut list = inner.list.lock().unwrap();
            if let Some(same) = list.iter().find(|e| e.info_hash == hash) {
                return Err(TorrentAddError::AlreadyAdded(same.id));
            }
            let id = TorrentId(inner.next.fetch_add(1, Ordering::SeqCst) + 1);
            let name = match &source {
                Source::Magnet(link) => librqbit::Magnet::parse(link).ok().and_then(|m| m.name),
                Source::File(_) => None,
            };
            // Read under the list's lock, as a rebuild sets it, so this
            // torrent is either given by the rebuild or meant for its session.
            let generation = inner.generation.load(Ordering::SeqCst);
            list.push(Entry {
                id,
                info_hash: hash,
                source: source.clone(),
                name,
                handle: None,
                generation,
                kept: None,
                wanted: Wanted::Running,
                failed: None,
            });
            (id, generation)
        };
        inner.give(id, source, generation, false);
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
            let _ = inner.session().pause(&handle).await;
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
            let _ = inner.session().unpause(&handle).await;
        }
        inner.publish().await;
        Ok(())
    }

    async fn shutdown(&self) {
        self.inner.stopping.store(true, Ordering::SeqCst);
        self.inner.session().stop().await;
    }
}
