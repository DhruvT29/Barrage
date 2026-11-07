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
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use librqbit::{
    AddTorrent, AddTorrentOptions, AddTorrentResponse, ConnectionOptions, DhtSessionConfig,
    ListenerMode, ListenerOptions, ManagedTorrent, Session, SessionOptions, TorrentStats,
    TorrentStatsState,
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
use crate::fetch::{Usage, replace};
use crate::links::watch::LinkTable;
use crate::record::{self, KeptState, Priority, What};
use crate::transfer::TransferEngine;

/// The note on a magnet nobody has given the details of.
const STILL_FINDING: &str = "no peer has given this torrent's details yet. It is holding a place \
                             in the queue; stop it to let the next start";

/// The note on a torrent brought back without what it takes to start it.
const LOST_FILE: &str = "its saved .torrent file has gone: remove it and add it again";

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
    /// The order of adding, shared with the downloads.
    pub seq: Arc<AtomicU64>,
    /// The engine's own folder, where each `.torrent` and its fast resume
    /// data are kept. `None`: neither is.
    pub state: Option<PathBuf>,
    /// How long a magnet may go without its details before it says it is
    /// holding a place in the queue.
    pub finding_note_after: Duration,
}

impl Default for TorrentConfig {
    fn default() -> Self {
        Self {
            network: true,
            listen: true,
            try_for: TRY_FOR,
            tick: Duration::from_secs(1),
            peers: Vec::new(),
            seq: Arc::new(AtomicU64::new(0)),
            state: None,
            finding_note_after: Duration::from_secs(5 * 60),
        }
    }
}

/// What the user asked of a torrent, which librqbit's state alone does not
/// say: a paused torrent may be stopped or done.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wanted {
    /// In line: not yet told to run.
    Queued,
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
    seq: u64,
    priority: Priority,
    /// The folder its files go under: the session's when it was added.
    dir: PathBuf,
    /// The folder librqbit chose in `dir`, once known, so it stays there
    /// whatever the session's folder is later.
    output: Option<PathBuf>,
    /// Whether it has been given to a session. One brought back stopped or
    /// done has not, and needs none until it runs again.
    given: bool,
    /// When it was given to a session, to tell a magnet nobody answers.
    given_at: Option<Instant>,
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
        let options = |place| session_options(Arc::clone(&connector), &config, place);
        let (session, on, listening) =
            match Session::new_with_opts(dir.clone(), options(on.as_ref())).await {
                Ok(session) => (session, on, listening),
                Err(why) => {
                    // Its address gone already: the next tick moves it on.
                    tracing::warn!("torrents: starting on this machine only: {why:#}");
                    let session = Session::new_with_opts(dir.clone(), options(None)).await?;
                    (session, None, Listening::Off)
                }
            };
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

    /// Put every torrent of `kept`, from an earlier engine, back on the list
    /// with its id. What should run goes in line; a stopped or done one waits
    /// as a row, and one whose `.torrent` has gone says so.
    /// `next` is the highest id that engine ever gave.
    pub async fn restore(&self, kept: Vec<record::Kept>, next: u64) {
        let inner = &self.inner;
        inner.next.fetch_max(next, Ordering::SeqCst);
        let generation = inner.generation.load(Ordering::SeqCst);
        for kept in kept {
            let What::Torrent {
                id,
                info_hash,
                magnet,
                output,
            } = kept.what
            else {
                continue;
            };
            inner.next.fetch_max(id, Ordering::SeqCst);
            inner.config.seq.fetch_max(kept.seq, Ordering::SeqCst);
            let saved = match (magnet, inner.kept_file(&info_hash)) {
                (Some(link), _) => Some(Source::Magnet(link)),
                (None, Some(path)) => std::fs::read(path).ok().map(Source::File),
                (None, None) => None,
            };
            let id = TorrentId(id);
            // A done torrent is never given again, so only the others need it.
            let lost = saved.is_none() && matches!(kept.state, KeptState::Run | KeptState::Stopped);
            let name = kept.name.or_else(|| match &saved {
                Some(Source::Magnet(link)) => {
                    librqbit::Magnet::parse(link).ok().and_then(|m| m.name)
                }
                _ => None,
            });
            let source = saved.unwrap_or(Source::File(Vec::new()));
            inner.list.lock().unwrap().push(Entry {
                id,
                seq: kept.seq,
                priority: kept.priority,
                dir: kept.dir,
                output,
                given: false,
                given_at: None,
                info_hash,
                source,
                name,
                handle: None,
                generation,
                kept: (kept.length.is_some() || kept.done > 0).then_some(Kept {
                    length: kept.length,
                    done: kept.done,
                }),
                wanted: match kept.state {
                    KeptState::Stopped => Wanted::Stopped,
                    KeptState::Done => Wanted::Done,
                    KeptState::Failed => Wanted::Running,
                    KeptState::Run => Wanted::Queued,
                },
                failed: match kept.state {
                    KeptState::Failed => Some(kept.note.unwrap_or_else(|| "it failed".into())),
                    _ if lost => Some(LOST_FILE.to_string()),
                    _ => None,
                },
            });
        }
        inner.publish().await;
    }

    /// Every torrent as saved.
    pub fn kept(&self) -> Vec<record::Kept> {
        let shown = self.inner.status.borrow();
        let list = self.inner.list.lock().unwrap();
        list.iter()
            .filter_map(|e| {
                let s = shown.iter().find(|s| s.id == e.id)?;
                Some(record::Kept {
                    seq: e.seq,
                    priority: e.priority,
                    state: match (s.phase, e.wanted) {
                        (TorrentPhase::Failed, _) => KeptState::Failed,
                        (_, Wanted::Stopped) => KeptState::Stopped,
                        (TorrentPhase::Done, _) | (_, Wanted::Done) => KeptState::Done,
                        _ => KeptState::Run,
                    },
                    dir: e.dir.clone(),
                    name: s.name.clone(),
                    length: s.length,
                    done: s.done,
                    note: s.note.clone(),
                    what: What::Torrent {
                        id: e.id.as_u64(),
                        info_hash: e.info_hash.clone(),
                        magnet: match &e.source {
                            Source::Magnet(link) => Some(link.clone()),
                            Source::File(_) => None,
                        },
                        output: e.output.clone(),
                    },
                })
            })
            .collect()
    }

    /// The highest id given so far.
    pub fn last_id(&self) -> u64 {
        self.inner.next.load(Ordering::SeqCst)
    }

    /// The address peers can call in on, while the session listens.
    pub fn listen_addr(&self) -> Option<SocketAddr> {
        self.inner.session().listen_addr()
    }
}

/// librqbit's options: BARRAGE opens every peer connection; UDP and any
/// listener at `place`, or on this machine, with no listener, without one;
/// no local service discovery (it multicasts on every interface, not one
/// cable); fast resume data beside the record, and no session store:
/// BARRAGE's own list says what runs.
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
        fastresume_folder: config.state.as_deref().map(fastresume_folder),
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
        let output = self.entry(id, |e| {
            e.given = true;
            e.given_at.get_or_insert_with(Instant::now);
            e.output.clone()
        });
        tokio::spawn(async move {
            let add = match source {
                Source::Magnet(link) => AddTorrent::from_url(link),
                Source::File(bytes) => AddTorrent::from_bytes(bytes),
            };
            // No output folder: left to the session's, a multi-file torrent
            // gets a folder of its own, by its name, checked against traversal.
            let options = AddTorrentOptions {
                overwrite: true,
                paused,
                output_folder: output
                    .flatten()
                    .map(|folder| folder.to_string_lossy().into_owned()),
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
                    let bytes = handle
                        .with_metadata(|m| m.torrent_bytes.to_vec())
                        .ok()
                        .filter(|bytes| !bytes.is_empty());
                    let listed = giving.entry(id, |e| {
                        current(e).then(|| {
                            e.handle = Some(Arc::clone(&handle));
                            e.output = Some(handle.output_folder().to_path_buf());
                            if let Some(bytes) = &bytes {
                                giving.keep_file(&e.info_hash, bytes);
                                e.source = Source::File(bytes.clone());
                            }
                            e.wanted
                        })
                    });
                    if listed.is_none() {
                        // Removed while it was being given.
                        let _ = session.delete(handle.id().into(), false).await;
                        return;
                    }
                    // Stopped or resumed while it was being given.
                    match listed.flatten() {
                        Some(Wanted::Running) if paused => {
                            let _ = session.unpause(&handle).await;
                        }
                        Some(Wanted::Stopped | Wanted::Done | Wanted::Queued) if !paused => {
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

    /// Save a torrent's `.torrent` beside the record, so it can come back.
    fn keep_file(&self, hash: &str, bytes: &[u8]) {
        let Some(path) = self.kept_file(hash) else {
            return;
        };
        let saved = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| replace(&path, bytes));
        if let Err(e) = saved {
            tracing::warn!("cannot save {}: {e}", path.display());
        }
    }

    /// Where a torrent's `.torrent` is kept.
    fn kept_file(&self, hash: &str) -> Option<PathBuf> {
        let state = self.config.state.as_deref()?;
        Some(torrent_file(state, hash))
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
            .filter(|e| e.failed.is_none() && e.given)
            .map(|e| {
                if let Some(handle) = e.handle.take() {
                    let stats = handle.stats();
                    e.name = handle.name().or(e.name.take());
                    e.kept = Some(Kept {
                        length: Some(stats.total_bytes),
                        done: have(&stats).max(e.kept.map_or(0, |k| k.done)),
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
                    let mut status =
                        status_of(entry, self.connector.cables_of(&entry.info_hash, &links));
                    if status.phase == TorrentPhase::Finding
                        && status.note.is_none()
                        && entry
                            .given_at
                            .is_some_and(|at| at.elapsed() >= self.config.finding_note_after)
                    {
                        status.note = Some(STILL_FINDING.to_string());
                    }
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

/// Where the `.torrent` of the torrent named `hash` is kept in `state`.
fn torrent_file(state: &Path, hash: &str) -> PathBuf {
    state.join("torrents").join(format!("{hash}.torrent"))
}

/// Where the fork keeps the fast resume data of every torrent in `state`.
pub(crate) fn fastresume_folder(state: &Path) -> PathBuf {
    state.join("resume")
}

/// Where the fork keeps the saved bitfield of the torrent named `hash`, or
/// `None` if `hash` is not 40 hex digits: the record is a file anyone can
/// edit, and a name with `..` or a drive in it must not reach outside.
pub(crate) fn fastresume_file(state: &Path, hash: &str) -> Option<PathBuf> {
    let plain = hash.len() == 40 && hash.bytes().all(|b| b.is_ascii_hexdigit());
    plain.then(|| fastresume_folder(state).join(format!("{hash}.bitv")))
}

/// The bytes a torrent has, checked. While librqbit checks its files, its
/// progress is how far the check has read, right or wrong, so none are known
/// yet: what it had before (`Kept`) shows meanwhile.
fn have(stats: &TorrentStats) -> u64 {
    match stats.state {
        TorrentStatsState::Initializing { .. } => 0,
        _ => stats.progress_bytes,
    }
}

/// The phase of a torrent its session is not running, from what the user
/// asked.
fn phase_asked(wanted: Wanted) -> TorrentPhase {
    match wanted {
        Wanted::Queued => TorrentPhase::Queued,
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
        seq: entry.seq,
        priority: entry.priority,
    };
    if entry.failed.is_some() {
        status.phase = TorrentPhase::Failed;
        return status;
    }
    let Some(handle) = &entry.handle else {
        // With what it had: brought back, or being given to a rebuilt session.
        if kept.is_some() || matches!(entry.wanted, Wanted::Stopped | Wanted::Queued) {
            status.phase = phase_asked(entry.wanted);
        }
        return status;
    };
    let stats = handle.stats();
    status.name = handle.name().or(status.name);
    status.length = Some(stats.total_bytes);
    status.done = have(&stats).max(status.done);
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
        let id = self.enqueue(source).await?;
        // Just listed as queued, unless a stop has come already.
        let _ = self.release(id).await;
        Ok(id)
    }

    async fn enqueue(&self, source: Source) -> Result<TorrentId, TorrentAddError> {
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
            // Read under the list's lock, as a rebuild sets it, so this
            // torrent is either given by the rebuild or meant for its session.
            let generation = inner.generation.load(Ordering::SeqCst);
            if let Source::File(bytes) = &source {
                inner.keep_file(&hash, bytes);
            }
            list.push(Entry {
                id,
                seq: inner.config.seq.fetch_add(1, Ordering::SeqCst) + 1,
                priority: Priority::default(),
                dir: inner.dir.clone(),
                output: None,
                given: false,
                given_at: None,
                info_hash: hash,
                source: source.clone(),
                name,
                handle: None,
                generation,
                kept: None,
                wanted: Wanted::Queued,
                failed: None,
            });
            id
        };
        inner.publish().await;
        Ok(id)
    }

    async fn release(&self, id: TorrentId) -> Result<(), TorrentControlError> {
        let inner = &self.inner;
        let (handle, source) = inner
            .entry(id, |e| {
                if e.wanted != Wanted::Queued {
                    return Err(TorrentControlError::NotQueued);
                }
                e.wanted = Wanted::Running;
                e.generation = inner.generation.load(Ordering::SeqCst);
                let source = (!e.given).then(|| (e.source.clone(), e.generation));
                Ok((e.handle.clone(), source))
            })
            .ok_or(TorrentControlError::NoSuchTorrent)??;
        if let Some((source, generation)) = source {
            inner.give(id, source, generation, false);
        } else if let Some(handle) = handle {
            let _ = inner.session().unpause(&handle).await;
        }
        inner.publish().await;
        Ok(())
    }

    async fn forget(&self, id: TorrentId) -> Result<(), TorrentControlError> {
        let inner = &self.inner;
        let entry = {
            let mut list = inner.list.lock().unwrap();
            let at = list
                .iter()
                .position(|e| e.id == id)
                .ok_or(TorrentControlError::NoSuchTorrent)?;
            list.remove(at)
        };
        if let Some(handle) = &entry.handle {
            let _ = inner.session().delete(handle.id().into(), false).await;
        }
        if let Some(state) = inner.config.state.as_deref() {
            let _ = std::fs::remove_file(torrent_file(state, &entry.info_hash));
            if let Some(bitfield) = fastresume_file(state, &entry.info_hash) {
                let _ = std::fs::remove_file(bitfield);
            }
        }
        inner.publish().await;
        Ok(())
    }

    async fn set_priority(
        &self,
        id: TorrentId,
        priority: Priority,
    ) -> Result<(), TorrentControlError> {
        let inner = &self.inner;
        inner
            .entry(id, |e| {
                if e.wanted == Wanted::Done || e.failed.is_some() {
                    return Err(TorrentControlError::Finished);
                }
                e.priority = priority;
                Ok(())
            })
            .ok_or(TorrentControlError::NoSuchTorrent)??;
        inner.publish().await;
        Ok(())
    }

    async fn requeue(&self, id: TorrentId) -> Result<(), TorrentControlError> {
        let inner = &self.inner;
        inner
            .entry(id, |e| {
                if e.wanted != Wanted::Stopped {
                    return Err(TorrentControlError::NotStopped);
                }
                // Never hand librqbit empty bytes: note it as lost instead.
                if matches!(&e.source, Source::File(bytes) if bytes.is_empty()) {
                    e.failed = Some(LOST_FILE.to_string());
                    return Err(TorrentControlError::Finished);
                }
                e.wanted = Wanted::Queued;
                e.seq = inner.config.seq.fetch_add(1, Ordering::SeqCst) + 1;
                Ok(())
            })
            .ok_or(TorrentControlError::NoSuchTorrent)??;
        inner.publish().await;
        Ok(())
    }

    fn status(&self) -> watch::Receiver<Vec<TorrentStatus>> {
        self.inner.status.subscribe()
    }

    async fn stop(&self, id: TorrentId) -> Result<(), TorrentControlError> {
        let inner = &self.inner;
        let handle = inner
            .entry(id, |e| {
                let running = matches!(e.wanted, Wanted::Running | Wanted::Queued);
                if !running || e.failed.is_some() {
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
        self.requeue(id).await?;
        // Just queued, unless a stop has come already.
        let _ = self.release(id).await;
        Ok(())
    }

    async fn shutdown(&self) {
        self.inner.stopping.store(true, Ordering::SeqCst);
        self.inner.session().stop().await;
    }
}
