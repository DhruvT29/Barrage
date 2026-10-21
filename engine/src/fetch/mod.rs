//! Downloads: one file fetched through every working link at once.
//!
//! Each link's workers pull the next chunk from a shared pile, fetch it with
//! a `Range` request through their own address, and write it at its place in
//! the file, so a faster link simply fetches more.
//!
//! | module | role |
//! |---|---|
//! | `ranges` | [`Ranges`]: the bytes done (pure) |
//! | `chunk` | [`plan`]: the chunks still to fetch (pure) |
//! | `name` | [`file_name`], [`free_name`]: safe names, never one taken |
//! | `remote` | [`find`]: what one link sees at the URL |
//! | `disk` | the `.part`: full size at once, written at offsets, named last; the Downloads folder |
//! | `worker` | one connection: a chunk, its bytes, a report |
//! | `download` | one download's coordinator, the owner of its state |
//!
//! [`Downloads`] starts a coordinator per URL and gathers their status.

pub mod chunk;
mod disk;
mod download;
pub mod name;
pub mod ranges;
pub mod remote;
mod worker;

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use reqwest::Url;
use tokio::sync::watch;

use crate::links::LinkId;
use crate::links::watch::LinkTable;

pub use chunk::plan;
pub use disk::downloads_folder;
pub use name::{file_name, free_name};
pub use ranges::Ranges;
pub use remote::{MAX_REDIRECTS, PROBE_BYTES, Remote, Validator, find};

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

/// How downloads run. Tests shrink the times.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadConfig {
    /// Connections each link opens for one download.
    pub workers_per_link: u32,
    /// How long a connection may go without data before it is given up.
    pub read_deadline: Duration,
    /// How long one link may take to find the file, redirects and all.
    pub find_deadline: Duration,
    /// How often the status is published.
    pub publish_every: Duration,
    /// The span a speed is measured over.
    pub speed_window: Duration,
    /// How long a fresh connection waits to take the place of one that
    /// failed while the rest of its link kept working.
    pub retry_connection: Duration,
    /// How long a link whose connections all failed rests before one tries
    /// again. Each failure in a row doubles it, up to `rest_max`.
    pub rest_first: Duration,
    pub rest_max: Duration,
}

impl Default for DownloadConfig {
    fn default() -> Self {
        Self {
            workers_per_link: 4,
            read_deadline: Duration::from_secs(8),
            // Several round trips per hop, and the tether's reached 934 ms.
            find_deadline: Duration::from_secs(20),
            publish_every: Duration::from_millis(250),
            speed_window: Duration::from_secs(2),
            retry_connection: Duration::from_secs(1),
            rest_first: Duration::from_secs(5),
            rest_max: Duration::from_secs(60),
        }
    }
}

/// One download, as long as the engine runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DownloadId(u64);

impl DownloadId {
    /// Only [`Downloads::add`] mints ids, hence `pub(crate)`, as for
    /// [`LinkId`].
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
            }),
        }
    }

    /// Start downloading `request.url` into the download folder. Returns at
    /// once; the status says how it goes.
    ///
    /// Only an `http` or `https` address is taken, and not while the same
    /// file is still starting, waiting or downloading: two downloads of one
    /// file would race each other for the same links. Once it is done or has
    /// failed, adding it again starts a new download.
    pub async fn add(&self, request: AddRequest) -> Result<DownloadId, AddError> {
        let url = web_address(&request.url)?;
        if request.cables.as_ref().is_some_and(Vec::is_empty) {
            return Err(AddError::NoCables);
        }
        // Checked and added under the status's one lock, so the same URL
        // posted twice at once still starts once.
        let mut added = None;
        self.inner.status.send_if_modified(|all| {
            let running = all.iter().find(|s| {
                running(s.phase) && Url::parse(s.url.trim()).is_ok_and(|u| same_file(&u, &url))
            });
            if let Some(running) = running {
                added = Some(Err(AddError::AlreadyRunning(running.id)));
                return false;
            }
            let id = DownloadId::new(self.inner.next.fetch_add(1, Ordering::SeqCst) + 1);
            all.push(DownloadStatus {
                id,
                url: request.url.clone(),
                name: None,
                phase: Phase::Starting,
                length: None,
                done: 0,
                bps: 0,
                links: Vec::new(),
                note: None,
            });
            added = Some(Ok(id));
            true
        });
        let id = added.expect("the status closure always runs")?;
        tokio::spawn(download::run(download::Job {
            id,
            given: request.url,
            url,
            cables: request.cables,
            table: self.inner.table.clone(),
            dir: self.inner.dir.clone(),
            config: self.inner.config.clone(),
            status: self.inner.status.clone(),
        }));
        Ok(id)
    }

    /// Every download's status, updated as they run.
    pub fn status(&self) -> watch::Receiver<Vec<DownloadStatus>> {
        self.inner.status.subscribe()
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
fn same_file(a: &Url, b: &Url) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;

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
