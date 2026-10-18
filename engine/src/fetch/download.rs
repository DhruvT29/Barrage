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
//! With no usable link it waits. With usable links that have all failed, it
//! fails, and says why.

use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::net::IpAddr;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use reqwest::Url;
use tokio::sync::{mpsc, watch};
use tokio::task::{self, JoinError, JoinSet};
use tokio::time::{Instant, MissedTickBehavior};

use super::disk::{create_part, name_finished};
use super::remote::{Remote, differs, find};
use super::worker::{Exit, Outcome, Report, Worker};
use super::{DownloadConfig, DownloadId, DownloadStatus, LinkShare, Phase, Ranges, plan};
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
    working: HashMap<task::Id, LinkId>,
    reports_tx: mpsc::UnboundedSender<Report>,
    reports: mpsc::UnboundedReceiver<Report>,
    table_open: bool,
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
    workers: u32,
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
    Tick,
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
            reports_tx,
            reports,
            table_open: true,
        }
    }

    async fn run(mut self) {
        let links = self.job.table.latest();
        self.look_at(&links);
        self.publish();
        let mut tick = tokio::time::interval(self.job.config.publish_every);
        tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        while self.ended.is_none() {
            let event = tokio::select! {
                found = self.finds.join_next_with_id(), if !self.finds.is_empty() => {
                    found.map(Event::Found)
                }
                report = self.reports.recv() => report.map(Event::Report),
                ended = self.workers.join_next_with_id(), if !self.workers.is_empty() => {
                    ended.map(Event::WorkerEnded)
                }
                open = self.job.table.changed(), if self.table_open => Some(Event::Table(open)),
                _ = tick.tick() => Some(Event::Tick),
            };
            if let Some(event) = event {
                self.handle(event).await;
            }
            self.check_stuck();
            self.publish();
        }
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
                if let Some(link) = self.working.remove(&task)
                    && let Some(member) = self.member(link)
                {
                    member.workers -= 1;
                    if exit.is_some() {
                        member.note = exit;
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
            Event::Tick => {
                let (now, window) = (Instant::now(), self.job.config.speed_window);
                for member in &mut self.members {
                    let bytes = member.written.load(Ordering::SeqCst);
                    member.speed.sample(now, bytes, window);
                }
            }
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
            }
        }
        for link in wanted {
            match self.members.iter().position(|m| m.id == link.id) {
                Some(at) if self.members[at].present => {}
                Some(at) => {
                    self.members[at].present = true;
                    self.members[at].stop = watch::channel(false).0;
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
                        workers: 0,
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
            if self.length == Some(0) {
                // Nothing to fetch.
                self.finish().await;
                return;
            }
        }
        self.take_part(link);
    }

    /// Fix the file as `remote`, found through `link`, describes it: a
    /// full-size `.part`, and every chunk on the pile, or the whole file for
    /// `link` alone when the server sends no pieces or no length.
    fn open(&mut self, remote: &Remote, link: LinkId) -> Result<(), String> {
        let size = remote.length.unwrap_or(0);
        let (name, part, file) = create_part(&self.job.dir, &remote.name, size)?;
        let done = Ranges::new();
        let mode = match remote.length.filter(|_| remote.ranges) {
            Some(length) => {
                let (pile, taken) = flume::unbounded();
                for chunk in plan(length, &done) {
                    let _ = pile.send(chunk);
                }
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

    /// Start `link`'s workers, if it agrees about the file: its own
    /// `workers_per_link` on the pile, or the one fetching the whole file.
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
        let worker = || Worker {
            link,
            local: member.local,
            url: remote.url.clone(),
            validator: remote.validator.clone(),
            file: open.file.clone(),
            reports: self.reports_tx.clone(),
            written: member.written.clone(),
            stop: member.stop.subscribe(),
            read_deadline: self.job.config.read_deadline,
        };
        let mut started = Vec::new();
        match &mut open.mode {
            Mode::Pieces { length, taken, .. } => {
                if let Some(why) = differs(&open.fixed, remote) {
                    member.note = Some(why);
                    return;
                }
                for _ in 0..self.job.config.workers_per_link {
                    started.push(self.workers.spawn(worker().run(taken.clone(), *length)));
                }
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
                if !*going {
                    *going = true;
                    started.push(self.workers.spawn(worker().run_whole(self.length)));
                }
            }
        }
        for task in started {
            self.working.insert(task.id(), link);
            member.workers += 1;
        }
    }

    async fn reported(&mut self, report: Report) {
        let Some(open) = &mut self.file else {
            return;
        };
        let Range { start, end } = report.range;
        let written_to = start + report.written;
        open.done.insert(start..written_to);
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
            (false, _) | (true, Outcome::Done) => {}
            (true, Outcome::Stopped) => {
                return self.fail(
                    "the cable carrying the file went away, and this server sends it only \
                     whole, so the download cannot resume"
                        .to_string(),
                );
            }
            (true, outcome) => {
                let why = match outcome {
                    Outcome::Failed(why) => why,
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
        // No worker may still hold the file when it is renamed.
        while self.workers.join_next().await.is_some() {}
        self.working.clear();
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
            }
            Ok(Err(note)) => self.fail(note),
            Err(e) => self.fail(e.to_string()),
        }
    }

    /// End the download with `note`. The `.part` is left where it is.
    fn fail(&mut self, note: String) {
        if self.ended.is_some() {
            return;
        }
        for member in &self.members {
            member.stop.send_replace(true);
        }
        self.file = None;
        self.note = Some(note);
        self.ended = Some(Phase::Failed);
    }

    /// Links are usable, yet none is finding the file or fetching it: every
    /// one has failed or sits out, so the download fails, with a reason.
    fn check_stuck(&mut self) {
        if self.ended.is_some() {
            return;
        }
        let mut present = self.members.iter().filter(|m| m.present).peekable();
        if present.peek().is_none() {
            return;
        }
        if self
            .members
            .iter()
            .any(|m| m.present && (m.finding || m.workers > 0))
        {
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
            done: links.iter().map(|l| l.bytes).sum(),
            bps: links.iter().map(|l| l.bps).sum(),
            links,
            note: self.note.clone(),
        };
        self.job
            .status
            .send_modify(|all| match all.iter_mut().find(|s| s.id == status.id) {
                Some(slot) => *slot = status,
                None => all.push(status),
            });
    }
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
