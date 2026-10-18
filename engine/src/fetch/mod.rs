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
//! | `disk` | the `.part`: full size at once, written at offsets, named last |
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

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::Context;
use reqwest::Url;
use tokio::sync::watch;

use crate::links::LinkId;
use crate::links::watch::LinkTable;

pub use chunk::plan;
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
        }
    }
}

/// One download, as long as the engine runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DownloadId(u64);

impl DownloadId {
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
    pub async fn add(&self, request: AddRequest) -> anyhow::Result<DownloadId> {
        let url = Url::parse(request.url.trim())
            .with_context(|| format!("{:?} is not a URL", request.url))?;
        let id = DownloadId(self.inner.next.fetch_add(1, Ordering::SeqCst) + 1);
        self.inner.status.send_modify(|all| {
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
            })
        });
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
