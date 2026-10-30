//! Downloads: one file fetched through every working link at once.
//!
//! Each link's workers are given a long part of the file each, fetch it with
//! one `Range` request through their own address, and write it at its place
//! in the file. One with nothing left to do takes half of what another has
//! left, so a faster link simply fetches more, with tens of requests per
//! download rather than hundreds.
//!
//! | module | role |
//! |---|---|
//! | `ranges` | [`Ranges`]: the bytes done (pure) |
//! | `spans` | one connection's part of the file, and where to cut one (pure) |
//! | `rate` | a speed that follows a change in seconds (pure) |
//! | `name` | [`file_name`], [`free_name`]: safe names, never one taken |
//! | `remote` | [`find`]: what one link sees at the URL; [`retry_after`] |
//! | `disk` | the `.part`: full size at once, written at offsets, named last; the Downloads folder |
//! | `saved` | the progress beside the `.part`, so the next engine resumes it |
//! | `worker` | one connection: its part, its bytes, a report |
//! | `download` | one download's coordinator, the owner of its state |
//!
//! [`Downloads`] starts a coordinator per URL and gathers their status.

mod disk;
mod download;
pub mod name;
pub mod ranges;
mod rate;
pub mod remote;
mod saved;
mod spans;
mod worker;

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use reqwest::Url;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::links::LinkId;
use crate::links::watch::LinkTable;

pub use disk::downloads_folder;
pub use name::{file_name, free_name};
pub use ranges::Ranges;
pub use remote::{MAX_REDIRECTS, PROBE_BYTES, Refused, Remote, Validator, find, retry_after};

/// Every download the engine runs, and what each is doing. Cheap to clone:
/// the clones share the downloads.
#[derive(Clone)]
pub struct Downloads {
    inner: Arc<Inner>,
}

struct Inner {
    table: LinkTable,
    dir: PathBuf,
    config: DownloadConfig,
    next: AtomicU64,
    status: Arc<watch::Sender<Vec<DownloadStatus>>>,
    /// Becomes true when the engine stops: every download stops and saves
    /// its progress.
    stopping: watch::Sender<bool>,
    /// What stopping and resuming each download needs, by id.
    controls: Mutex<HashMap<DownloadId, Control>>,
}

/// One download's way to be stopped, and started again.
struct Control {
    /// The URL as given, and parsed.
    given: String,
    url: Url,
    cables: Option<Vec<String>>,
    /// Becomes true when [`Downloads::stop`] asks it to stop.
    stop: watch::Sender<bool>,
    /// Its coordinator, until the engine stopping waits for it.
    task: Option<JoinHandle<()>>,
}

/// A URL to download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddRequest {
    pub url: String,
    /// Only the cables with these names take part. `None`: every cable.
    pub cables: Option<Vec<String>>,
}

/// Why a URL was not added. The words are the engine's, for the page to show
/// as they are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddError {
    /// Not an `http` or `https` address; the text says what is wrong.
    BadUrl(String),
    /// `cables` names no cable at all, so no cable could ever fetch it.
    NoCables,
    /// The same file is already starting, waiting or downloading, as this
    /// download.
    AlreadyRunning(DownloadId),
}

impl fmt::Display for AddError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadUrl(why) => f.write_str(why),
            Self::NoCables => f.write_str(
                "cables names no cable, so none could fetch it. Leave cables out to use every cable",
            ),
            Self::AlreadyRunning(id) => {
                write!(f, "already downloading, as download {}", id.as_u64())
            }
        }
    }
}

impl std::error::Error for AddError {}

/// Why a download was not stopped or resumed. The words are the engine's,
/// for the page to show as they are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlError {
    NoSuchDownload,
    /// Stopping: it is not starting, waiting or downloading.
    NotRunning,
    /// Resuming: it is not stopped.
    NotStopped,
}

impl fmt::Display for ControlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSuchDownload => f.write_str("no such download"),
            Self::NotRunning => f.write_str("it is not running, so there is nothing to stop"),
            Self::NotStopped => f.write_str("it is not stopped, so there is nothing to resume"),
        }
    }
}

impl std::error::Error for ControlError {}

/// How downloads run. Tests shrink the times.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadConfig {
    /// Connections each link opens for one download.
    pub workers_per_link: u32,
    /// How long a connection may go without data before it is given up.
    pub read_deadline: Duration,
    /// How long one link may take to find the file, redirects and all.
    pub find_deadline: Duration,
    /// How often each link's speed is measured and the status published at
    /// the least. What a download does between, a chunk written, a link
    /// joining, a note, is published as it happens; the live socket merges
    /// what a slow page has not read yet.
    pub publish_every: Duration,
    /// How quickly a measured speed follows a change: half of it within
    /// this long (rate.rs).
    pub speed_half_life: Duration,
    /// How long a fresh connection waits to take the place of one that
    /// failed while the rest of its link kept working.
    pub retry_connection: Duration,
    /// How long a link whose connections all failed rests before one tries
    /// again. Each failure in a row doubles it, up to `rest_max`.
    pub rest_first: Duration,
    pub rest_max: Duration,
    /// The longest `Retry-After` a link waits out when the server refuses
    /// its last connection; asked to wait longer, it sits the download out.
    pub max_retry_after: Duration,
    /// How often a download's progress is saved beside its `.part`, so an
    /// engine that is killed loses no more than this much of it.
    pub save_every: Duration,
    /// Before a connection's speed is known, the least a cut leaves each
    /// side: a part with less than twice this left is not cut for another.
    /// Every cut costs a request, and throws away what is on its way past it.
    pub min_split: u64,
    /// Once speeds are known, how much sooner a cut must bring a part's end:
    /// a slow link's last part is cut whatever its size, and one about to
    /// finish is left alone.
    pub min_split_time: Duration,
}

impl Default for DownloadConfig {
    fn default() -> Self {
        Self {
            workers_per_link: 4,
            read_deadline: Duration::from_secs(8),
            // Several round trips per hop, and the tether's reached 934 ms.
            find_deadline: Duration::from_secs(20),
            publish_every: Duration::from_millis(250),
            speed_half_life: Duration::from_millis(1500),
            retry_connection: Duration::from_secs(1),
            rest_first: Duration::from_secs(5),
            rest_max: Duration::from_secs(60),
            // Cloudflare answered Retry-After: 3311 in phase 2.
            max_retry_after: Duration::from_secs(60),
            save_every: Duration::from_secs(1),
            min_split: 512 << 10,
            min_split_time: Duration::from_millis(500),
        }
    }
}

/// One download, as long as the engine runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DownloadId(u64);

impl DownloadId {
    /// Only [`Downloads::add`] gives out new ids, hence `pub(crate)`, as for
    /// [`LinkId`]. The API makes one from the number in a path only to look
    /// it up: an id from outside is never trusted to name a download.
    pub(crate) fn new(raw: u64) -> Self {
        Self(raw)
    }

    pub fn as_u64(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Finding the file.
    Starting,
    /// No usable link to fetch it through yet.
    Waiting,
    Downloading,
    Done,
    /// Ended without the file; the note says why.
    Failed,
    /// Stopped, by [`Downloads::stop`] or the engine stopping, with what it
    /// had saved: resuming carries on from there.
    Stopped,
}

/// What one download is doing, as last published.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadStatus {
    pub id: DownloadId,
    /// The URL as it was given.
    pub url: String,
    /// The file's name in the download folder, once known.
    pub name: Option<String>,
    pub phase: Phase,
    pub length: Option<u64>,
    /// Bytes written: every link's share added up.
    pub done: u64,
    /// The download's speed: every link's added up.
    pub bps: u64,
    pub links: Vec<LinkShare>,
    pub note: Option<String>,
}

/// One link's part in a download. A link that has gone keeps its share, so
/// what it carried still shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkShare {
    pub link: LinkId,
    /// The link's name when it took part.
    pub name: String,
    /// Bytes this link wrote to the file.
    pub bytes: u64,
    /// Its speed over the last speed window, 0 unless downloading.
    pub bps: u64,
    /// Its connections open for this download right now.
    pub connections: u32,
    pub note: Option<String>,
}

impl Downloads {
    pub fn new(table: LinkTable, dir: PathBuf) -> Self {
        Self::with_config(table, dir, DownloadConfig::default())
    }

    pub fn with_config(table: LinkTable, dir: PathBuf, config: DownloadConfig) -> Self {
        let (status, _) = watch::channel(Vec::new());
        Self {
            inner: Arc::new(Inner {
                table,
                dir,
                config,
                next: AtomicU64::new(0),
                status: Arc::new(status),
                stopping: watch::channel(false).0,
                controls: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// Start downloading `request.url` into the download folder. Returns at
    /// once; the status says how it goes.
    ///
    /// Only an `http` or `https` address is taken, and not while the same
    /// file is still starting, waiting or downloading: two downloads of one
    /// file would race each other for the same links. A stopped download of
    /// the file is resumed, under its own id, through the cables asked for
    /// now: one download of a file, never two, so its progress is never
    /// taken from under it. Once it is done or has failed, adding it again
    /// starts a new download.
    pub async fn add(&self, request: AddRequest) -> Result<DownloadId, AddError> {
        let url = web_address(&request.url)?;
        if request.cables.as_ref().is_some_and(Vec::is_empty) {
            return Err(AddError::NoCables);
        }
        // Checked and started under the controls' lock, then the status's,
        // as resuming is: the same URL posted twice at once still starts
        // once, and a stop that comes meanwhile waits for the new download.
        let mut controls = self.inner.controls.lock().unwrap();
        let mut added = None;
        self.inner.status.send_if_modified(|all| {
            let same =
                |s: &DownloadStatus| Url::parse(s.url.trim()).is_ok_and(|u| same_file(&u, &url));
            if let Some(running) = all.iter().find(|s| running(s.phase) && same(s)) {
                added = Some(Err(AddError::AlreadyRunning(running.id)));
                return false;
            }
            if let Some(at) = all
                .iter()
                .position(|s| s.phase == Phase::Stopped && same(s))
            {
                let id = all[at].id;
                all[at] = starting(id, &request.url);
                added = Some(Ok(id));
                return true;
            }
            let id = DownloadId::new(self.inner.next.fetch_add(1, Ordering::SeqCst) + 1);
            all.push(starting(id, &request.url));
            added = Some(Ok(id));
            true
        });
        let id = added.expect("the status closure always runs")?;
        self.launch(&mut controls, id, request.url, url, request.cables);
        Ok(id)
    }

    /// Start download `id`'s coordinator, with its own way to be stopped.
    /// Called with the controls' lock held from before `id` showed as
    /// starting, so a stop never finds the coordinator it replaces.
    fn launch(
        &self,
        controls: &mut HashMap<DownloadId, Control>,
        id: DownloadId,
        given: String,
        url: Url,
        cables: Option<Vec<String>>,
    ) {
        let (stop, asked) = watch::channel(false);
        let task = tokio::spawn(download::run(download::Job {
            id,
            given: given.clone(),
            url: url.clone(),
            cables: cables.clone(),
            table: self.inner.table.clone(),
            dir: self.inner.dir.clone(),
            config: self.inner.config.clone(),
            status: self.inner.status.clone(),
            stopping: self.inner.stopping.subscribe(),
            stop: asked,
        }));
        let control = Control {
            given,
            url,
            cables,
            stop,
            task: Some(task),
        };
        controls.insert(id, control);
    }

    /// Every download's status, updated as they run.
    pub fn status(&self) -> watch::Receiver<Vec<DownloadStatus>> {
        self.inner.status.subscribe()
    }

    /// Stop download `id`: its connections stop and report what they wrote,
    /// and its progress is saved, so resuming it carries on from there.
    /// Returns once it has stopped, the progress on disk; or, if it finished
    /// or failed before it heard, says it was not running.
    pub async fn stop(&self, id: DownloadId) -> Result<(), ControlError> {
        {
            let controls = self.inner.controls.lock().unwrap();
            let control = controls.get(&id).ok_or(ControlError::NoSuchDownload)?;
            if !self.phase(id).is_some_and(running) {
                return Err(ControlError::NotRunning);
            }
            control.stop.send_replace(true);
        }
        // Stopped shows only once its last save is on disk.
        let mut status = self.inner.status.subscribe();
        let _ = status
            .wait_for(|all| all.iter().any(|s| s.id == id && !running(s.phase)))
            .await;
        match self.phase(id) {
            Some(Phase::Stopped) => Ok(()),
            _ => Err(ControlError::NotRunning),
        }
    }

    /// Resume download `id`, stopped: it starts again under the same id and
    /// picks up from the progress it saved, as a download added again would.
    /// Returns once it has started.
    pub async fn resume(&self, id: DownloadId) -> Result<(), ControlError> {
        // Checked and started under the controls' lock, then the status's,
        // as adding is: a URL added at the same moment still runs once, and
        // a stop that comes meanwhile waits for the new coordinator rather
        // than signal the one that has ended.
        let mut controls = self.inner.controls.lock().unwrap();
        let control = controls.get(&id).ok_or(ControlError::NoSuchDownload)?;
        let (given, url, cables) = (
            control.given.clone(),
            control.url.clone(),
            control.cables.clone(),
        );
        let mut outcome = Err(ControlError::NoSuchDownload);
        self.inner.status.send_if_modified(|all| {
            let Some(at) = all.iter().position(|s| s.id == id) else {
                return false;
            };
            // Its file cannot be running as another download: adding the URL
            // of a stopped download resumes that one rather than start a
            // second.
            if all[at].phase != Phase::Stopped {
                outcome = Err(ControlError::NotStopped);
                return false;
            }
            all[at] = starting(id, &given);
            outcome = Ok(());
            true
        });
        outcome?;
        self.launch(&mut controls, id, given, url, cables);
        Ok(())
    }

    /// Download `id`'s phase, as last published.
    fn phase(&self, id: DownloadId) -> Option<Phase> {
        let all = self.inner.status.borrow();
        all.iter().find(|s| s.id == id).map(|s| s.phase)
    }

    /// Stop every download: each one's connections stop and report what they
    /// wrote, and its progress is saved, so the next engine resumes it.
    /// Returns once every download has.
    pub async fn shutdown(&self) {
        self.inner.stopping.send_replace(true);
        let tasks: Vec<JoinHandle<()>> = {
            let mut controls = self.inner.controls.lock().unwrap();
            controls
                .values_mut()
                .filter_map(|c| c.task.take())
                .collect()
        };
        for task in tasks {
            let _ = task.await;
        }
    }
}

/// A download's status as it starts, before anything is known of the file.
fn starting(id: DownloadId, given: &str) -> DownloadStatus {
    DownloadStatus {
        id,
        url: given.to_string(),
        name: None,
        phase: Phase::Starting,
        length: None,
        done: 0,
        bps: 0,
        links: Vec::new(),
        note: None,
    }
}

/// `given` as an address to download from: `http` or `https`. Either always
/// has a host: the parser refuses them without one.
fn web_address(given: &str) -> Result<Url, AddError> {
    let url = Url::parse(given.trim())
        .map_err(|e| AddError::BadUrl(format!("{given:?} is not a web address: {e}")))?;
    match url.scheme() {
        "http" | "https" => Ok(url),
        other => Err(AddError::BadUrl(format!(
            "only http and https addresses can be downloaded, not {other}:"
        ))),
    }
}

/// Whether a download in this phase still holds its URL.
fn running(phase: Phase) -> bool {
    matches!(phase, Phase::Starting | Phase::Waiting | Phase::Downloading)
}

/// Whether two addresses fetch the same file. Both are parsed, so the host's
/// case and a default port already agree. The fragment is never sent to the
/// server, so it cannot make another file.
pub(crate) fn same_file(a: &Url, b: &Url) -> bool {
    let sent = |url: &Url| {
        let mut url = url.clone();
        url.set_fragment(None);
        url
    };
    sent(a) == sent(b)
}

/// A wait as the notes write it: "300 ms", "5 s", "1.5 s", "1 min",
/// "55 min 11 s".
pub(crate) fn wait_words(wait: Duration) -> String {
    let ms = wait.as_millis();
    if ms < 1000 {
        return format!("{ms} ms");
    }
    let secs = wait.as_secs();
    if secs < 60 {
        return if ms.is_multiple_of(1000) {
            format!("{secs} s")
        } else {
            format!("{:.1} s", wait.as_secs_f64())
        };
    }
    match (secs / 60, secs % 60) {
        (min, 0) => format!("{min} min"),
        (min, s) => format!("{min} min {s} s"),
    }
}

/// A size as the notes write it, as the page writes sizes: whole bytes below
/// a kB, then one decimal, in powers of 1000, so "8.8 MB" and "1.2 GB". A
/// size that rounds up to 1000 of one unit is 1.0 of the next.
pub(crate) fn size_words(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["kB", "MB", "GB", "TB", "PB"];
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1000.0;
    let mut unit = 0;
    while format!("{value:.1}").parse::<f64>().unwrap_or(value) >= 1000.0 && unit < UNITS.len() - 1
    {
        value /= 1000.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_size_is_written_as_the_page_writes_it() {
        assert_eq!(size_words(0), "0 B");
        assert_eq!(size_words(999), "999 B");
        assert_eq!(size_words(1000), "1.0 kB");
        assert_eq!(size_words(8_762_927), "8.8 MB");
        assert_eq!(size_words(1_200_000_000), "1.2 GB");
        assert_eq!(size_words(999_949), "999.9 kB");
        assert_eq!(size_words(999_950), "1.0 MB");
    }

    #[test]
    fn a_wait_is_written_in_the_unit_that_reads_best() {
        let words = |ms| wait_words(Duration::from_millis(ms));
        assert_eq!(words(0), "0 ms");
        assert_eq!(words(300), "300 ms");
        assert_eq!(words(999), "999 ms");
        assert_eq!(words(1000), "1 s");
        assert_eq!(words(1500), "1.5 s");
        assert_eq!(words(8000), "8 s");
        assert_eq!(words(59_000), "59 s");
        assert_eq!(words(60_000), "1 min");
        assert_eq!(words(3_311_000), "55 min 11 s");
    }
}
