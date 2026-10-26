//! One download's coordinator, the only owner of its state (PLAN.md section
//! 3.5): which links take part, which bytes are done, which chunks are still
//! to fetch. Workers keep only their own connection and report by message.
//!
//! HOW IT RUNS
//!
//! It follows the link table, not link events: every change is a fresh look
//! at which links are usable now, so a link joining or leaving is never
//! missed, and an event for a link it never saw cannot reach it (PLAN.md
//! item 14). Every usable link, or only the named cables, finds the file
//! from the URL itself, through its own redirects. The first to answer fixes
//! the file: its name, its length, its version, and a full-size `.part` in
//! the download folder. Every chunk of [`plan`] goes on one pile, and each
//! link that found the same file gets its workers, which take chunks until
//! none are left. A worker reports what it wrote and the rest of its chunk
//! goes back on the pile. Once every byte is written the file is flushed to
//! disk and named, and the download is done.
//!
//! WHEN THE SERVER AND THE LINKS DISAGREE
//!
//! A link whose node sent another version or length of the file, or no
//! pieces, sits out with a note ([`differs`]). A file that changes on the
//! server mid-download ends the download, the `.part` left in place, rather
//! than be stitched from two versions. A server that sends no pieces, or no
//! length, is fetched whole through the link that found it first, and the
//! download cannot resume if that link drops.
//!
//! WHEN A CONNECTION OR A LINK FAILS
//!
//! Detection is local, diagnosis is global (PLAN.md section 3.5). A worker
//! reports its connection broken and ends; the coordinator, which sees every
//! connection, decides what that means. If another connection on the same
//! link received bytes within the read deadline, the connection was bad, not
//! the link: a fresh one takes its place after `retry_connection`. If none
//! did, the link is failing for this download: all its connections stop,
//! what they wrote stays written and the rest of their chunks go back on the
//! pile for the other links, and it rests, `rest_first` doubling with each
//! rest in a row up to `rest_max`, before one connection tries again. That
//! one working brings back the rest. A link that leaves the table, or stops
//! being usable, is stopped at once, without waiting for a deadline; if it
//! comes back, it does so under a new id, as a new member.
//!
//! WHEN THE SERVER LIMITS CONNECTIONS
//!
//! A `429 Too Many Requests` is a ceiling, not a failure (PLAN.md item 8):
//! servers cap connections per address, and each link has an address of its
//! own. Each 429 ends the connection that got it and nothing takes its
//! place, so the link settles at what the server takes, never below one,
//! while the others keep theirs. When its last connection is refused, the
//! link waits as long as `Retry-After` asks, up to `max_retry_after`, and
//! sits the download out if asked to wait longer; with no `Retry-After` it
//! rests as a failing link. A link that works again after waiting may hold
//! all its connections again, and learns the ceiling afresh.
//!
//! With no usable link it waits. When every usable link has given up, on a
//! file it could not find or answers it could not use, and none is resting,
//! it fails, and says why.
//!
//! WHEN THE ENGINE STOPS
//!
//! Progress is saved beside the `.part` (`saved.rs`) at most every
//! `save_every`, with what each connection has written of the chunk it is
//! on, and once more at the end: when the engine stops, every connection
//! stops and reports what it wrote first, and a download that fails saves
//! what it had before it shows as failed. Each save flushes the `.part`
//! first, and runs in the background, one at a time, so a slow disk holds
//! back the progress on it, never the download.
//!
//! The next engine, given the same URL, looks for that progress under the
//! names the file would be given, in the order they are tried, and if the
//! server's file is still the same length and version, fetches only what is
//! missing. Otherwise it starts over and says why. A file the server sends
//! only whole cannot be picked up again, so no progress is kept for it.

use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::net::IpAddr;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use reqwest::Url;
use tokio::sync::{mpsc, watch};
use tokio::task::{self, JoinError, JoinSet};
use tokio::time::{Instant, MissedTickBehavior};

use super::disk::{create_part, name_finished, reopen_part};
use super::name::{names_like, taken};
use super::remote::{Remote, differs, find};
use super::saved::{Saved, load, progress_path, save};
use super::worker::{Exit, Outcome, Report, Worker, stamp};
use super::{
    DownloadConfig, DownloadId, DownloadStatus, LinkShare, Phase, Ranges, plan, same_file,
    size_words, wait_words,
};
use crate::links::watch::LinkTable;
use crate::links::{Link, LinkId};

/// The download's note when the server sends the file only whole.
const ONLY_WHOLE: &str = "the server sends this file only whole, not in pieces, so it comes \
                          through one cable, and cannot resume if that cable drops";

/// Everything one download starts from.
pub(crate) struct Job {
    pub id: DownloadId,
    /// As the user gave it, for the status.
    pub given: String,
    pub url: Url,
    pub cables: Option<Vec<String>>,
    pub table: LinkTable,
    pub dir: PathBuf,
    pub config: DownloadConfig,
    pub status: Arc<watch::Sender<Vec<DownloadStatus>>>,
    /// Becomes true when the engine stops.
    pub stopping: watch::Receiver<bool>,
    /// Becomes true when this download alone is asked to stop.
    pub stop: watch::Receiver<bool>,
}

/// Why a download stops before its end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stop {
    /// This download, by [`super::Downloads::stop`].
    Asked,
    /// The whole engine.
    Engine,
}

/// Run the download to its end, publishing its status as it goes.
pub(crate) async fn run(job: Job) {
    Download::new(job).run().await;
}

struct Download {
    job: Job,
    /// Every link that has taken part, in the order they first appeared, so
    /// a cable that has gone still shows what it carried.
    members: Vec<Member>,
    file: Option<Open>,
    ended: Option<Phase>,
    name: Option<String>,
    length: Option<u64>,
    note: Option<String>,
    finds: JoinSet<anyhow::Result<Remote>>,
    finding: HashMap<task::Id, LinkId>,
    workers: JoinSet<Exit>,
    /// Each running worker's seat, by its task.
    working: HashMap<task::Id, u64>,
    /// Every worker that has neither ended nor reported its end, by seat:
    /// its link's connections.
    seats: HashMap<u64, Seat>,
    next_seat: u64,
    /// What the workers' `heard` is measured from.
    epoch: Instant,
    reports_tx: mpsc::UnboundedSender<Report>,
    reports: mpsc::UnboundedReceiver<Report>,
    table_open: bool,
    /// Bytes an earlier engine had written, picked up on resuming.
    resumed: u64,
    /// Where the progress is saved; `None` before the file is found, and for
    /// a file fetched whole.
    progress_file: Option<PathBuf>,
    saved_at: Instant,
    /// The save running in the background, if one is: never more than one.
    saving: Option<task::JoinHandle<()>>,
    /// The progress as it stood when the download failed, saved once its
    /// loop has ended.
    last_progress: Option<Snapshot>,
    /// Stopping on purpose: its connections stop because they were told
    /// to, not because their cable went.
    stopping: bool,
}

/// The progress at one moment, and what saving it needs.
struct Snapshot {
    saved: Saved,
    /// The `.part`, flushed to disk before progress counting its bytes is
    /// saved.
    part: Arc<File>,
    path: PathBuf,
}

impl Snapshot {
    /// Flush the `.part`, then save the progress: never the other way round,
    /// or a power cut could leave progress claiming bytes the disk lost.
    fn write(self) {
        let saved = self
            .part
            .sync_data()
            .and_then(|()| save(&self.path, &self.saved));
        if let Err(e) = saved {
            // The next save may manage: one missed costs a second's progress.
            tracing::debug!(error = %e, "saving a download's progress failed");
        }
    }
}

/// One worker, as the coordinator follows it.
struct Seat {
    link: LinkId,
    /// When its connection last received bytes (`Worker::heard`).
    heard: Arc<AtomicU64>,
    /// What it has written of the chunk it is on (`Worker::doing`).
    doing: Arc<Mutex<Option<Range<u64>>>>,
}

/// One link's part in the download.
struct Member {
    id: LinkId,
    name: String,
    local: IpAddr,
    /// Usable, and one of the cables asked for, at the last look.
    present: bool,
    finding: bool,
    /// The file as this link found it.
    remote: Option<Remote>,
    /// Found the file the download is fetching, so its workers may take
    /// pieces of it.
    agrees: bool,
    /// Its connections: workers that have neither ended nor reported their
    /// end.
    workers: u32,
    /// When fresh connections are due, one each, in place of connections
    /// that failed while the link kept working.
    refills: Vec<Instant>,
    /// Resting until then, with no connections.
    resting: Option<Instant>,
    /// Its next rest: `rest_first`, doubled by each rest in a row up to
    /// `rest_max`.
    backoff: Duration,
    /// One connection is trying the link after a rest; its first chunk
    /// brings back the rest.
    trial: bool,
    /// Bytes this link's workers wrote, counted by them.
    written: Arc<AtomicU64>,
    stop: watch::Sender<bool>,
    note: Option<String>,
    speed: Speed,
}

/// The file once a link has found it.
struct Open {
    /// The file as the link that found it first described it.
    fixed: Remote,
    part: PathBuf,
    file: Arc<File>,
    done: Ranges,
    mode: Mode,
}

enum Mode {
    /// Pieces, off one pile, from every link that agrees about the file.
    Pieces {
        length: u64,
        pile: flume::Sender<Range<u64>>,
        taken: flume::Receiver<Range<u64>>,
    },
    /// The whole file through one link, for a server that sends no pieces
    /// or no length.
    Whole { carrier: LinkId, started: bool },
}

enum Event {
    Found(Result<(task::Id, anyhow::Result<Remote>), JoinError>),
    Report(Report),
    WorkerEnded(Result<(task::Id, Exit), JoinError>),
    Table(bool),
    Wake,
    Tick,
    Stop(Stop),
}

impl Download {
    fn new(job: Job) -> Self {
        let (reports_tx, reports) = mpsc::unbounded_channel();
        Self {
            job,
            members: Vec::new(),
            file: None,
            ended: None,
            name: None,
            length: None,
            note: None,
            finds: JoinSet::new(),
            finding: HashMap::new(),
            workers: JoinSet::new(),
            working: HashMap::new(),
            seats: HashMap::new(),
            next_seat: 0,
            epoch: Instant::now(),
            reports_tx,
            reports,
            table_open: true,
            resumed: 0,
            progress_file: None,
            saved_at: Instant::now(),
            saving: None,
            last_progress: None,
            stopping: false,
        }
    }

    async fn run(mut self) {
        let links = self.job.table.latest();
        self.look_at(&links);
        self.publish();
        let mut tick = tokio::time::interval(self.job.config.publish_every);
        tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        while self.ended.is_none() {
            let wake = self.next_wake();
            let event = tokio::select! {
                found = self.finds.join_next_with_id(), if !self.finds.is_empty() => {
                    found.map(Event::Found)
                }
                report = self.reports.recv() => report.map(Event::Report),
                ended = self.workers.join_next_with_id(), if !self.workers.is_empty() => {
                    ended.map(Event::WorkerEnded)
                }
                open = self.job.table.changed(), if self.table_open => Some(Event::Table(open)),
                () = tokio::time::sleep_until(wake.unwrap_or_else(Instant::now)), if wake.is_some() => {
                    Some(Event::Wake)
                }
                // This download asked to stop, or the registry gone.
                _ = self.job.stop.wait_for(|&stop| stop) => Some(Event::Stop(Stop::Asked)),
                // The engine stopping, or the registry gone with it.
                _ = self.job.stopping.wait_for(|&stop| stop) => Some(Event::Stop(Stop::Engine)),
                _ = tick.tick() => Some(Event::Tick),
            };
            if let Some(event) = event {
                self.handle(event).await;
            }
            self.check_stuck();
            if self.ended.is_none() {
                self.publish();
            }
        }
        // A download that failed keeps what it had for the next attempt,
        // saved before it shows as ended, so adding it again at once finds it.
        let last = self.last_progress.take();
        self.save_now(last).await;
        self.publish();
    }

    async fn handle(&mut self, event: Event) {
        match event {
            Event::Found(found) => {
                let (task, found) = match found {
                    Ok(done) => done,
                    Err(e) => (
                        e.id(),
                        Err(anyhow::anyhow!("finding the file stopped: {e}")),
                    ),
                };
                if let Some(link) = self.finding.remove(&task) {
                    self.found(link, found).await;
                }
            }
            Event::Report(report) => self.reported(report).await,
            Event::WorkerEnded(ended) => {
                // A worker sends its last report before it ends, and both can
                // be ready at once. Hear the report first, or its link could
                // look idle with its work not yet handed back.
                while let Ok(report) = self.reports.try_recv() {
                    self.reported(report).await;
                    if self.ended.is_some() {
                        return;
                    }
                }
                let (task, exit) = match ended {
                    Ok(done) => done,
                    Err(e) => (e.id(), Some(format!("a connection stopped: {e}"))),
                };
                if let Some(seat) = self.working.remove(&task) {
                    // Still seated means it ended without a last report: it
                    // failed before it asked for anything.
                    let link = self.seats.get(&seat).map(|s| s.link);
                    self.release(seat);
                    if let (Some(link), Some(why)) = (link, exit) {
                        self.broken(link, why);
                    }
                }
            }
            Event::Table(true) => {
                let links = self.job.table.latest();
                self.look_at(&links);
            }
            Event::Table(false) => {
                self.table_open = false;
                self.fail("the link watcher stopped".to_string());
            }
            Event::Wake => self.wake(),
            Event::Tick => {
                let (now, window) = (Instant::now(), self.job.config.speed_window);
                for member in &mut self.members {
                    let bytes = member.written.load(Ordering::SeqCst);
                    member.speed.sample(now, bytes, window);
                }
                // A link trying again after a rest works once its connection
                // receives bytes: the rest need not wait for a whole chunk.
                let trying: Vec<LinkId> = self
                    .members
                    .iter()
                    .filter(|m| m.trial)
                    .map(|m| m.id)
                    .collect();
                for link in trying {
                    let heard = self
                        .seats
                        .values()
                        .any(|s| s.link == link && s.heard.load(Ordering::SeqCst) > 0);
                    if heard {
                        self.worked(link);
                    }
                }
                if self.saved_at.elapsed() >= self.job.config.save_every {
                    self.save_soon();
                }
            }
            Event::Stop(why) => self.shut_down(why).await,
        }
    }

    /// This download, or the whole engine, is stopping: every connection
    /// stops and reports what it wrote, and the progress is saved, for
    /// resuming it or for the next engine.
    async fn shut_down(&mut self, why: Stop) {
        self.stopping = true;
        for member in &self.members {
            member.stop.send_replace(true);
        }
        while let Some(ended) = self.workers.join_next_with_id().await {
            while let Ok(report) = self.reports.try_recv() {
                self.reported(report).await;
            }
            if let Ok((task, _)) | Err(task) = ended.map_err(|e| e.id())
                && let Some(seat) = self.working.remove(&task)
            {
                self.release(seat);
            }
        }
        while let Ok(report) = self.reports.try_recv() {
            self.reported(report).await;
        }
        if self.ended.is_some() {
            // The last report finished it, or failed it.
            return;
        }
        let snapshot = self.snapshot();
        self.save_now(snapshot).await;
        self.file = None;
        // With no progress kept, it was not yet found, or is fetched whole.
        let note = match (why, self.progress_file.is_some()) {
            (Stop::Asked, true) => "stopped; resuming carries on from here",
            (Stop::Asked, false) => "stopped; resuming starts it over",
            (Stop::Engine, true) => "stopped with barrage; adding it again carries on from here",
            (Stop::Engine, false) => "stopped with barrage; adding it again starts it over",
        };
        self.note = Some(note.to_string());
        self.ended = Some(Phase::Stopped);
    }

    /// The progress now, with what saving it needs: the bytes written, every
    /// chunk reported and what each connection has written of the chunk it
    /// is on. `None` for a file fetched whole, or not yet found.
    fn snapshot(&self) -> Option<Snapshot> {
        let open = self.file.as_ref()?;
        let Mode::Pieces { length, .. } = open.mode else {
            return None;
        };
        let path = self.progress_file.clone()?;
        let mut done = open.done.clone();
        for seat in self.seats.values() {
            if let Some(range) = seat.doing.lock().unwrap().clone() {
                done.insert(range);
            }
        }
        let saved = Saved {
            url: self.job.url.to_string(),
            length,
            validator: open.fixed.validator.clone(),
            done: done.as_slice().iter().map(|r| (r.start, r.end)).collect(),
        };
        Some(Snapshot {
            saved,
            part: open.file.clone(),
            path,
        })
    }

    /// Save the progress in the background, unless the last save is still
    /// going: a slow disk holds back the progress on it, never the download.
    fn save_soon(&mut self) {
        if self.saving.as_ref().is_some_and(|s| !s.is_finished()) {
            return;
        }
        self.saved_at = Instant::now();
        if let Some(snapshot) = self.snapshot() {
            self.saving = Some(tokio::task::spawn_blocking(move || snapshot.write()));
        }
    }

    /// Wait for the save going in the background, if one is.
    async fn saved(&mut self) {
        if let Some(saving) = self.saving.take() {
            let _ = saving.await;
        }
    }

    /// Save `snapshot` now, after any save already going, so the last word
    /// on disk is this one.
    async fn save_now(&mut self, snapshot: Option<Snapshot>) {
        self.saved().await;
        if let Some(snapshot) = snapshot {
            let _ = tokio::task::spawn_blocking(move || snapshot.write()).await;
        }
    }

    /// Bring the members in line with the table: links that became usable
    /// join and find the file, links that stopped being usable are stopped.
    fn look_at(&mut self, links: &[Link]) {
        let wanted: Vec<&Link> = links
            .iter()
            .filter(|link| link.state.is_usable() && self.wants(&link.name))
            .collect();
        for member in &mut self.members {
            if member.present && !wanted.iter().any(|link| link.id == member.id) {
                member.present = false;
                member.stop.send_replace(true);
                // Whatever it was waiting to do no longer applies.
                member.refills.clear();
                member.resting = None;
                member.trial = false;
            }
        }
        for link in wanted {
            match self.members.iter().position(|m| m.id == link.id) {
                Some(at) if self.members[at].present => {}
                Some(at) => {
                    let member = &mut self.members[at];
                    member.present = true;
                    member.stop = watch::channel(false).0;
                    member.backoff = self.job.config.rest_first;
                    self.join(link.id);
                }
                None => {
                    self.members.push(Member {
                        id: link.id,
                        name: link.name.clone(),
                        local: link.addr.ip(),
                        present: true,
                        finding: false,
                        remote: None,
                        agrees: false,
                        workers: 0,
                        refills: Vec::new(),
                        resting: None,
                        backoff: self.job.config.rest_first,
                        trial: false,
                        written: Arc::new(AtomicU64::new(0)),
                        stop: watch::channel(false).0,
                        note: None,
                        speed: Speed::default(),
                    });
                    self.join(link.id);
                }
            }
        }
    }

    fn wants(&self, name: &str) -> bool {
        self.job
            .cables
            .as_ref()
            .is_none_or(|cables| cables.iter().any(|c| c == name))
    }

    /// A present link starts taking part: at once if it has found the file
    /// already, else once it finds it.
    fn join(&mut self, link: LinkId) {
        let Some(member) = self.members.iter().find(|m| m.id == link) else {
            return;
        };
        if member.remote.is_some() && self.file.is_some() {
            self.take_part(link);
            return;
        }
        let (local, url) = (member.local, self.job.url.clone());
        let deadline = self.job.config.find_deadline;
        let task = self.finds.spawn(async move {
            match tokio::time::timeout(deadline, find(local, &url)).await {
                Ok(found) => found,
                Err(_) => Err(anyhow::anyhow!(
                    "finding {url} through {local}: no answer within {deadline:?}"
                )),
            }
        });
        self.finding.insert(task.id(), link);
        if let Some(member) = self.member(link) {
            member.finding = true;
        }
    }

    async fn found(&mut self, link: LinkId, found: anyhow::Result<Remote>) {
        let Some(member) = self.member(link) else {
            return;
        };
        member.finding = false;
        if !member.present {
            return;
        }
        let remote = match found {
            Ok(remote) => remote,
            Err(e) => {
                member.note = Some(format!("{e:#}"));
                return;
            }
        };
        member.note = None;
        member.remote = Some(remote.clone());
        if self.file.is_none() {
            if let Err(note) = self.open(&remote, link) {
                self.fail(note);
                return;
            }
            if self.nothing_missing() {
                // An empty file, or one an earlier engine had all of.
                self.finish().await;
                return;
            }
        }
        self.take_part(link);
    }

    fn nothing_missing(&self) -> bool {
        self.length == Some(0)
            || self.file.as_ref().is_some_and(
                |o| matches!(o.mode, Mode::Pieces { length, .. } if o.done.is_complete(length)),
            )
    }

    /// Fix the file as `remote`, found through `link`, describes it: a
    /// full-size `.part` and the chunks still missing on the pile, or the
    /// whole file for `link` alone when the server sends no pieces or no
    /// length.
    fn open(&mut self, remote: &Remote, link: LinkId) -> Result<(), String> {
        let pieces = remote.length.filter(|_| remote.ranges);
        let (name, part, file, done) = match pieces {
            Some(length) => self.resume(remote, length)?,
            None => {
                let size = remote.length.unwrap_or(0);
                let (name, part, file) = create_part(&self.job.dir, &remote.name, size)?;
                (name, part, file, Ranges::new())
            }
        };
        let mode = match pieces {
            Some(length) => {
                let (pile, taken) = flume::unbounded();
                for chunk in plan(length, &done) {
                    let _ = pile.send(chunk);
                }
                self.progress_file = Some(progress_path(&part));
                Mode::Pieces {
                    length,
                    pile,
                    taken,
                }
            }
            None => {
                self.note = Some(ONLY_WHOLE.to_string());
                Mode::Whole {
                    carrier: link,
                    started: false,
                }
            }
        };
        self.name = Some(name);
        self.length = remote.length;
        self.file = Some(Open {
            fixed: remote.clone(),
            part,
            file: Arc::new(file),
            done,
            mode,
        });
        Ok(())
    }

    /// The `.part` for a file `length` bytes long fetched in pieces, and what
    /// is done of it: the one an earlier engine left for this URL, with what
    /// it had written, if the server's file is still the same; else a new one.
    fn resume(
        &mut self,
        remote: &Remote,
        length: u64,
    ) -> Result<(String, PathBuf, File, Ranges), String> {
        let dir = self.job.dir.clone();
        if let Some((name, saved)) = self.earlier(&remote.name) {
            let part = dir.join(format!("{name}.part"));
            let kept = agrees(&saved, length, remote).and_then(|done| {
                reopen_part(&part, length)
                    .map(|file| (file, done))
                    .map_err(|_| "what the last attempt left could not be opened; started again")
            });
            match kept {
                Ok((file, done)) => {
                    self.resumed = done.covered();
                    self.note = Some(format!(
                        "resumed: {} were already here",
                        size_words(self.resumed)
                    ));
                    return Ok((name, part, file, done));
                }
                Err(why) => {
                    // Its own leftovers, for this very URL: it starts again
                    // in their place.
                    let _ = std::fs::remove_file(progress_path(&part));
                    let _ = std::fs::remove_file(&part);
                    self.note = Some(why.to_string());
                }
            }
        }
        let (name, part, file) = create_part(&dir, &remote.name, length)?;
        Ok((name, part, file, Ranges::new()))
    }

    /// Progress an earlier engine saved for this URL, under one of the names
    /// a file called `name` would be given, in the order they are tried, up
    /// to the first that is free: where a new download would start.
    fn earlier(&self, name: &str) -> Option<(String, Saved)> {
        let dir = &self.job.dir;
        for candidate in names_like(name) {
            if !taken(dir, &candidate) {
                return None;
            }
            let part = dir.join(format!("{candidate}.part"));
            if let Some(saved) = load(&progress_path(&part))
                && Url::parse(&saved.url).is_ok_and(|url| same_file(&url, &self.job.url))
            {
                return Some((candidate, saved));
            }
        }
        None
    }

    /// Start `link`'s workers, if it agrees about the file: as many as it
    /// may hold, taking pieces off the pile, or the one fetching the whole
    /// file.
    fn take_part(&mut self, link: LinkId) {
        let Some(open) = &mut self.file else {
            return;
        };
        let Some(member) = self.members.iter_mut().find(|m| m.id == link) else {
            return;
        };
        let Some(remote) = &member.remote else {
            return;
        };
        let whole = match &mut open.mode {
            Mode::Pieces { .. } => {
                if let Some(why) = differs(&open.fixed, remote) {
                    member.note = Some(why);
                    return;
                }
                member.agrees = true;
                false
            }
            Mode::Whole {
                carrier,
                started: going,
            } => {
                if *carrier != link {
                    member.note = Some(
                        "sits out: the server sends the file only whole, so one cable fetches it"
                            .to_string(),
                    );
                    return;
                }
                if *going {
                    return;
                }
                *going = true;
                true
            }
        };
        if whole {
            let length = self.length;
            if let Some((seat, worker)) = self.seat(link) {
                let task = self.workers.spawn(worker.run_whole(length));
                self.working.insert(task.id(), seat);
            }
        } else {
            self.add_workers(link, usize::MAX);
        }
    }

    /// Start up to `n` more workers on `link`, taking pieces, never past as
    /// many as it may hold: one while it is trying after a rest, else
    /// `workers_per_link`. None while it rests.
    fn add_workers(&mut self, link: LinkId, n: usize) {
        let Some(Open {
            mode: Mode::Pieces { length, taken, .. },
            ..
        }) = &self.file
        else {
            return;
        };
        let (length, taken) = (*length, taken.clone());
        let Some(member) = self.members.iter().find(|m| m.id == link) else {
            return;
        };
        if !member.present || !member.agrees || member.resting.is_some() {
            return;
        }
        let most = if member.trial {
            1
        } else {
            self.job.config.workers_per_link
        };
        let room = most.saturating_sub(member.workers) as usize;
        for _ in 0..n.min(room) {
            let Some((seat, worker)) = self.seat(link) else {
                return;
            };
            let task = self.workers.spawn(worker.run(taken.clone(), length));
            self.working.insert(task.id(), seat);
        }
    }

    /// A new worker for `link`, given a seat, counted among its connections.
    fn seat(&mut self, link: LinkId) -> Option<(u64, Worker)> {
        let open = self.file.as_ref()?;
        let member = self.members.iter_mut().find(|m| m.id == link)?;
        let remote = member.remote.as_ref()?;
        let seat = self.next_seat;
        self.next_seat += 1;
        let heard = Arc::new(AtomicU64::new(0));
        let doing = Arc::new(Mutex::new(None));
        self.seats.insert(
            seat,
            Seat {
                link,
                heard: heard.clone(),
                doing: doing.clone(),
            },
        );
        member.workers += 1;
        let worker = Worker {
            link,
            seat,
            heard,
            epoch: self.epoch,
            doing,
            local: member.local,
            url: remote.url.clone(),
            validator: remote.validator.clone(),
            file: open.file.clone(),
            reports: self.reports_tx.clone(),
            written: member.written.clone(),
            stop: member.stop.subscribe(),
            read_deadline: self.job.config.read_deadline,
        };
        Some((seat, worker))
    }

    /// `seat`'s worker has ended, or reported an end: it is no longer one of
    /// its link's connections.
    fn release(&mut self, seat: u64) {
        if let Some(gone) = self.seats.remove(&seat)
            && let Some(member) = self.member(gone.link)
        {
            member.workers -= 1;
        }
    }

    async fn reported(&mut self, report: Report) {
        let Some(open) = &mut self.file else {
            return;
        };
        let Range { start, end } = report.range;
        let written_to = start + report.written;
        open.done.insert(start..written_to);
        // Counted in `done` now. The worker may have begun its next chunk
        // already, which it says itself, so only this one's is cleared.
        if let Some(seat) = self.seats.get(&report.seat) {
            let mut doing = seat.doing.lock().unwrap();
            if doing.as_ref().is_some_and(|r| r.start == start) {
                *doing = None;
            }
        }
        let whole = match &open.mode {
            Mode::Pieces { pile, .. } => {
                if written_to < end {
                    // Only what is still missing goes back.
                    let _ = pile.send(written_to..end);
                }
                false
            }
            Mode::Whole { .. } => true,
        };
        let complete = match &open.mode {
            Mode::Pieces { length, .. } => open.done.is_complete(*length),
            Mode::Whole { .. } => report.outcome == Outcome::Done,
        };
        let part = open.part.clone();
        // A worker carries on after a chunk it got; after anything else it
        // is ending, and no longer one of its link's connections.
        if !matches!(report.outcome, Outcome::Done | Outcome::Short) {
            self.release(report.seat);
        }
        match (whole, report.outcome) {
            (_, Outcome::Changed(why)) => return self.fail(why),
            (_, Outcome::Disk(why)) => {
                return self.fail(format!("cannot write {}: {why}", part.display()));
            }
            (false, Outcome::Failed(why)) => {
                if let Some(member) = self.member(report.link) {
                    member.note = Some(why);
                }
            }
            (false, Outcome::Broken(why)) => self.broken(report.link, why),
            (false, Outcome::Capped(wait)) => self.capped(report.link, wait),
            (false, Outcome::Done | Outcome::Short) => self.worked(report.link),
            (false, Outcome::Stopped) | (true, Outcome::Done) => {}
            (true, Outcome::Stopped) if self.stopping => {}
            (true, Outcome::Stopped) => {
                return self.fail(
                    "the cable carrying the file went away, and this server sends it only \
                     whole, so the download cannot resume"
                        .to_string(),
                );
            }
            (true, outcome) => {
                let why = match outcome {
                    Outcome::Failed(why) | Outcome::Broken(why) => why,
                    _ => "the file ended early".to_string(),
                };
                return self.fail(format!(
                    "{why}; this server sends the file only whole, so the download cannot resume"
                ));
            }
        }
        if complete {
            if whole && self.length.is_none() {
                // Only now is the length known: what arrived before the end.
                self.length = Some(written_to);
            }
            self.finish().await;
        }
    }

    /// A chunk came through `link`. After a rest, that brings back the rest
    /// of its connections, all it may hold, so that what a server allowed
    /// before it asked the link to wait is learned again; and its rests
    /// start short again.
    fn worked(&mut self, link: LinkId) {
        let rest_first = self.job.config.rest_first;
        let Some(member) = self.member(link) else {
            return;
        };
        if !member.trial {
            return;
        }
        member.trial = false;
        member.backoff = rest_first;
        member.note = None;
        self.add_workers(link, usize::MAX);
    }

    /// The server refused a connection on `link` with 429: it allows fewer
    /// connections through this address (PLAN.md item 8). The refused one
    /// ends and nothing takes its place, so the link settles at what the
    /// server takes, and says so, while the other links keep their own.
    /// Counting refusals, rather than looking at who is receiving, does not
    /// depend on which answer arrives first. Its last connection is never
    /// counted away: refused too, the link waits as long as the server asked,
    /// sits the download out if that is longer than `max_retry_after`, or
    /// rests as a failing link when the server did not say.
    fn capped(&mut self, link: LinkId, wait: Option<Duration>) {
        let most = self.job.config.max_retry_after;
        let Some(member) = self.member(link) else {
            return;
        };
        if !member.present || member.resting.is_some() {
            return;
        }
        let left = member.workers;
        if left > 0 {
            member.note = Some(format!(
                "the server allows {left} connection{} on this cable",
                if left == 1 { "" } else { "s" }
            ));
            return;
        }
        match wait {
            Some(wait) if wait > most => {
                member.refills.clear();
                member.note = Some(format!(
                    "sits out: the server asked this cable to wait {}, longer than {}",
                    wait_words(wait),
                    wait_words(most)
                ));
            }
            Some(wait) => {
                let note = format!(
                    "the server asked this cable to wait {} before trying again",
                    wait_words(wait)
                );
                self.pause(link, wait, note);
            }
            None => self.rest(
                link,
                "the server refused this cable's connections (429 Too Many Requests)".to_string(),
            ),
        }
    }

    /// A connection on `link` failed, and has already given up its seat. If
    /// another of its connections received bytes within the read deadline,
    /// the link works and the connection was bad: a fresh one follows after
    /// `retry_connection`. If none did, the link is failing, and rests.
    fn broken(&mut self, link: LinkId, why: String) {
        let others_heard = self.others_heard(link);
        let due = Instant::now() + self.job.config.retry_connection;
        let Some(member) = self.member(link) else {
            return;
        };
        // Stopped already: a rest, or the link leaving the table, came first.
        if !member.present || member.resting.is_some() {
            return;
        }
        if others_heard && !member.trial {
            member.refills.push(due);
        } else {
            self.rest(link, why);
        }
    }

    /// Whether a connection still seated on `link` received bytes within the
    /// read deadline. The one that failed has given up its seat, so only the
    /// others can vouch for the link.
    fn others_heard(&self, link: LinkId) -> bool {
        let now = stamp(self.epoch);
        let window = self.job.config.read_deadline;
        self.seats
            .values()
            .any(|s| s.link == link && heard_within(s.heard.load(Ordering::SeqCst), now, window))
    }

    /// Let a failing `link` rest before one connection tries again, each rest
    /// in a row twice the last, saying why.
    fn rest(&mut self, link: LinkId, why: String) {
        let rest_max = self.job.config.rest_max;
        let Some(member) = self.member(link) else {
            return;
        };
        let rest = member.backoff;
        member.backoff = next_rest(rest, rest_max);
        let note = format!("{why}; it rests {} and tries again", wait_words(rest));
        self.pause(link, rest, note);
    }

    /// Stop every connection of `link` for `rest`, with `note` saying why,
    /// after which one connection tries again.
    fn pause(&mut self, link: LinkId, rest: Duration, note: String) {
        let Some(member) = self.member(link) else {
            return;
        };
        member.resting = Some(Instant::now() + rest);
        member.refills.clear();
        member.trial = false;
        // Its workers stop on the old signal; the next ones listen to a new.
        member.stop.send_replace(true);
        member.stop = watch::channel(false).0;
        member.note = Some(note);
    }

    /// When something is next due: a rest ending, or a fresh connection.
    fn next_wake(&self) -> Option<Instant> {
        self.members
            .iter()
            .flat_map(|m| m.resting.into_iter().chain(m.refills.iter().copied()))
            .min()
    }

    /// Start whatever is due: one connection trying each link whose rest is
    /// over, and fresh connections in place of failed ones.
    fn wake(&mut self) {
        let now = Instant::now();
        let mut due = Vec::new();
        for member in &mut self.members {
            if member.resting.is_some_and(|until| until <= now) {
                // Trying, it may hold one connection.
                member.resting = None;
                member.trial = true;
                due.push((member.id, usize::MAX));
            }
            let waiting = member.refills.len();
            member.refills.retain(|&at| at > now);
            let ready = waiting - member.refills.len();
            if ready > 0 {
                due.push((member.id, ready));
            }
        }
        for (link, n) in due {
            self.add_workers(link, n);
        }
    }

    /// Every byte is written: let the workers go, flush the file to disk, and
    /// give it its name.
    async fn finish(&mut self) {
        let Some(open) = self.file.take() else {
            return;
        };
        let Open {
            part, file, mode, ..
        } = open;
        // A closed pile ends the workers waiting on it, and a stop the rest.
        drop(mode);
        for member in &self.members {
            member.stop.send_replace(true);
        }
        // No worker may still hold the file when it is renamed, nor a save,
        // which could also bring back the progress about to be removed.
        while self.workers.join_next().await.is_some() {}
        self.saved().await;
        self.working.clear();
        self.seats.clear();
        for member in &mut self.members {
            member.workers = 0;
        }
        let synced = match tokio::task::spawn_blocking(move || file.sync_all()).await {
            Ok(synced) => synced.map_err(|e| e.to_string()),
            Err(e) => Err(e.to_string()),
        };
        if let Err(e) = synced {
            self.fail(format!("cannot save {}: {e}", part.display()));
            return;
        }
        let wanted = self.name.clone().unwrap_or_default();
        match tokio::task::spawn_blocking(move || name_finished(&part, &wanted)).await {
            Ok(Ok(name)) => {
                self.name = Some(name);
                self.ended = Some(Phase::Done);
                // Done: nothing is left to pick up.
                if let Some(path) = self.progress_file.take() {
                    let _ = tokio::task::spawn_blocking(move || std::fs::remove_file(path)).await;
                }
            }
            Ok(Err(note)) => self.fail(note),
            Err(e) => self.fail(e.to_string()),
        }
    }

    /// End the download with `note`. The `.part` is left where it is, and
    /// what it has is saved once the download has stopped, for a later
    /// attempt to decide about.
    fn fail(&mut self, note: String) {
        if self.ended.is_some() {
            return;
        }
        for member in &self.members {
            member.stop.send_replace(true);
        }
        self.last_progress = self.snapshot();
        self.file = None;
        self.note = Some(note);
        self.ended = Some(Phase::Failed);
    }

    /// Links are usable, yet none is finding the file, fetching it, or
    /// resting before it tries again: every one has failed or sits out, so
    /// the download fails, with a reason.
    fn check_stuck(&mut self) {
        if self.ended.is_some() {
            return;
        }
        let mut present = self.members.iter().filter(|m| m.present).peekable();
        if present.peek().is_none() {
            return;
        }
        let busy =
            |m: &Member| m.finding || m.workers > 0 || m.resting.is_some() || !m.refills.is_empty();
        if self.members.iter().any(|m| m.present && busy(m)) {
            return;
        }
        let note = self
            .members
            .iter()
            .filter(|m| m.present)
            .find_map(|m| m.note.clone())
            .unwrap_or_else(|| "no link could download the file".to_string());
        self.fail(note);
    }

    fn phase(&self) -> Phase {
        match self.ended {
            Some(phase) => phase,
            None if !self.members.iter().any(|m| m.present) => Phase::Waiting,
            None if self.file.is_some() => Phase::Downloading,
            None => Phase::Starting,
        }
    }

    fn member(&mut self, link: LinkId) -> Option<&mut Member> {
        self.members.iter_mut().find(|m| m.id == link)
    }

    fn publish(&self) {
        let phase = self.phase();
        let moving = phase == Phase::Downloading;
        let links: Vec<LinkShare> = self
            .members
            .iter()
            .map(|m| LinkShare {
                link: m.id,
                name: m.name.clone(),
                bytes: m.written.load(Ordering::SeqCst),
                bps: if moving { m.speed.bps } else { 0 },
                connections: m.workers,
                note: m.note.clone(),
            })
            .collect();
        let status = DownloadStatus {
            id: self.job.id,
            url: self.job.given.clone(),
            name: self.name.clone(),
            phase,
            length: self.length,
            // What was already here counts toward the file, but is no
            // cable's share.
            done: self.resumed + links.iter().map(|l| l.bytes).sum::<u64>(),
            bps: links.iter().map(|l| l.bps).sum(),
            links,
            note: self.note.clone(),
        };
        // Every tick publishes, but only a change wakes the readers: a
        // download that is waiting or done would otherwise send each open
        // page the same status several times a second.
        self.job
            .status
            .send_if_modified(|all| match all.iter_mut().find(|s| s.id == status.id) {
                Some(slot) if *slot == status => false,
                Some(slot) => {
                    *slot = status;
                    true
                }
                None => {
                    all.push(status);
                    true
                }
            });
    }
}

/// What `saved` says is done of a file `length` bytes long, if it is still
/// the file `remote` describes: the same length, and the same version, which
/// the server must have named. Otherwise why it starts again: bytes of two
/// versions are never stitched into one file.
fn agrees(saved: &Saved, length: u64, remote: &Remote) -> Result<Ranges, &'static str> {
    if saved.validator.is_none() {
        return Err(
            "nothing from the server told whether the file was still the same, so it started \
             again",
        );
    }
    if saved.length != length || saved.validator != remote.validator {
        return Err("the file changed on the server; started again");
    }
    let mut done = Ranges::new();
    for &(start, end) in &saved.done {
        if start >= end || end > length {
            return Err("the progress saved for it did not fit the file; started again");
        }
        done.insert(start..end);
    }
    Ok(done)
}

/// The rest after `rest`, for a link failing again: twice as long, up to
/// `most`.
fn next_rest(rest: Duration, most: Duration) -> Duration {
    rest.saturating_mul(2).min(most)
}

/// Whether a connection whose `heard` is this, as [`stamp`] made it, received
/// bytes within `window` of `now`. 0 is never.
fn heard_within(heard: u64, now: u64, window: Duration) -> bool {
    let window = u64::try_from(window.as_millis()).unwrap_or(u64::MAX);
    heard > 0 && now.saturating_sub(heard) < window
}

/// A link's speed over the last window, from its byte count at each tick.
#[derive(Default)]
struct Speed {
    samples: VecDeque<(Instant, u64)>,
    bps: u64,
}

impl Speed {
    fn sample(&mut self, now: Instant, bytes: u64, window: Duration) {
        self.samples.push_back((now, bytes));
        // The oldest kept is the newest at least a window old.
        while self.samples.len() > 1 && now.duration_since(self.samples[1].0) >= window {
            self.samples.pop_front();
        }
        let (then, before) = self.samples[0];
        let secs = now.duration_since(then).as_secs_f64();
        self.bps = if secs > 0.0 {
            ((bytes - before) as f64 * 8.0 / secs).round() as u64
        } else {
            0
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_rest_in_a_row_doubles_up_to_the_most() {
        let most = Duration::from_secs(1);
        let mut rest = Duration::from_millis(200);
        let mut rests = Vec::new();
        for _ in 0..5 {
            rests.push(rest);
            rest = next_rest(rest, most);
        }
        assert_eq!(
            rests,
            [200, 400, 800, 1000, 1000].map(Duration::from_millis)
        );
    }

    #[test]
    fn heard_means_bytes_within_the_window() {
        let window = Duration::from_millis(300);
        // Never heard at all, early in a download or late.
        assert!(!heard_within(0, 100, window));
        assert!(!heard_within(0, 1000, window));
        assert!(heard_within(1000, 1000, window));
        assert!(heard_within(701, 1000, window));
        // Exactly a window ago is too long.
        assert!(!heard_within(700, 1000, window));
        assert!(!heard_within(1, 1000, window));
    }
}
