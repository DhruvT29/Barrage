//! What the download tests share: the links ("Wi-Fi" is 127.0.0.1 and
//! "Ethernet 2" is 127.0.0.2, so real bytes flow over each and the server in
//! `serve/mod.rs` tells them apart by source address), a folder per test,
//! the server's view of what it sent, and waiting on a download's status.
//!
//! Each test file that uses it also declares `mod fake` and `mod serve`.
#![allow(dead_code)]

use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use barrage::LinkHandle;
use barrage::fetch::{
    AddRequest, DownloadConfig, DownloadId, DownloadStatus, Downloads, LinkShare, PROBE_BYTES,
    Phase, Ranges,
};
use barrage::links::Candidate;
use barrage::links::watch::WatchConfig;

use crate::fake::{Fake, Live, cand_idx, ms};
use crate::serve::{Answer, File, Seen, Server, content};

pub const KIB: u64 = 1 << 10;
pub const MIB: u64 = 1 << 20;
pub const WIFI: &str = "127.0.0.1";
pub const ETHERNET: &str = "127.0.0.2";

pub fn wifi() -> Candidate {
    cand_idx(1, "Wi-Fi", WIFI, true)
}

pub fn ethernet() -> Candidate {
    cand_idx(2, "Ethernet 2", ETHERNET, true)
}

pub fn ip(text: &str) -> IpAddr {
    text.parse().unwrap()
}

/// The links, one table per poll of the watcher, the last one repeating.
pub fn links(steps: Vec<Vec<Candidate>>) -> LinkHandle {
    let net = Fake::scripted(steps).loopback_links();
    LinkHandle::start_with(Arc::new(net), WatchConfig::every(ms(20))).unwrap()
}

/// The links `first` holds, until the test changes them through the
/// returned [`Live`].
pub fn live_links(first: Vec<Candidate>) -> (LinkHandle, Live) {
    let (net, live) = Fake::live(first);
    let handle =
        LinkHandle::start_with(Arc::new(net.loopback_links()), WatchConfig::every(ms(20))).unwrap();
    (handle, live)
}

pub fn both() -> LinkHandle {
    links(vec![vec![wifi(), ethernet()]])
}

/// The default worker count, with short times so a test takes a second.
pub fn config() -> DownloadConfig {
    DownloadConfig {
        read_deadline: Duration::from_secs(5),
        find_deadline: Duration::from_secs(5),
        publish_every: ms(20),
        speed_window: ms(200),
        ..DownloadConfig::default()
    }
}

/// A folder of its own for one test's downloads, removed afterwards.
pub struct Folder(pub PathBuf);

impl Folder {
    pub fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("barrage-download-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    pub fn read(&self, name: &str) -> Vec<u8> {
        std::fs::read(self.0.join(name)).unwrap()
    }
}

impl Drop for Folder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn request(url: &reqwest::Url) -> AddRequest {
    AddRequest {
        url: url.to_string(),
        cables: None,
    }
}

/// The download's status once `wanted` holds, failing the test after 20 s.
pub async fn wait_for(
    downloads: &Downloads,
    id: DownloadId,
    what: &str,
    wanted: impl Fn(&DownloadStatus) -> bool,
) -> DownloadStatus {
    let mut status = downloads.status();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let now = status
            .borrow_and_update()
            .iter()
            .find(|s| s.id == id)
            .cloned();
        if let Some(now) = now.as_ref().filter(|s| wanted(s)) {
            return now.clone();
        }
        match tokio::time::timeout_at(deadline, status.changed()).await {
            Ok(Ok(())) => {}
            Ok(Err(_)) => panic!("the status channel closed while waiting for {what}"),
            Err(_) => panic!("no {what} within 20 s; last status: {now:?}"),
        }
    }
}

/// The download's status as last published.
pub fn status_of(downloads: &Downloads, id: DownloadId) -> DownloadStatus {
    let status = downloads
        .status()
        .borrow()
        .iter()
        .find(|s| s.id == id)
        .cloned();
    status.unwrap_or_else(|| panic!("no status for {id:?}"))
}

pub async fn ended(downloads: &Downloads, id: DownloadId) -> DownloadStatus {
    wait_for(downloads, id, "end", |s| {
        matches!(s.phase, Phase::Done | Phase::Failed)
    })
    .await
}

/// Every status of `id` the download publishes, with when it was seen, until
/// it ends: a running record of notes and connections that the last status
/// alone cannot show.
pub fn record(
    downloads: &Downloads,
    id: DownloadId,
) -> tokio::task::JoinHandle<Vec<(tokio::time::Instant, DownloadStatus)>> {
    let mut status = downloads.status();
    tokio::spawn(async move {
        let mut seen = Vec::new();
        loop {
            let now = status
                .borrow_and_update()
                .iter()
                .find(|s| s.id == id)
                .cloned();
            if let Some(now) = now {
                let over = matches!(now.phase, Phase::Done | Phase::Failed);
                seen.push((tokio::time::Instant::now(), now));
                if over {
                    return seen;
                }
            }
            if status.changed().await.is_err() {
                return seen;
            }
        }
    })
}

pub fn is_probe(seen: &Seen) -> bool {
    seen.asked() == Some((0, PROBE_BYTES - 1))
}

/// Every byte of the file these requests asked for, probes aside, each
/// counted once. Since phase 4 a connection's request can be cut short and
/// another asks for the back of it, so what the server sent includes what
/// was on its way past a cut; what was asked says which bytes the engine
/// wanted.
pub fn asked(seen: &[Seen]) -> Ranges {
    let mut asked = Ranges::new();
    for (start, last) in seen.iter().filter(|s| !is_probe(s)).filter_map(Seen::asked) {
        asked.insert(start..last + 1);
    }
    asked
}

/// The file's bytes the server sent to `source`, its probes aside.
pub fn served(seen: &[Seen], source: &str) -> u64 {
    seen.iter()
        .filter(|s| s.source == ip(source) && !is_probe(s))
        .map(Seen::sent)
        .sum()
}

/// The file on disk is the server's, byte for byte.
pub fn assert_same(got: &[u8], length: u64) {
    let want = content(0..length);
    assert_eq!(got.len(), want.len(), "the file's length");
    if let Some(at) = got.iter().zip(&want).position(|(a, b)| a != b) {
        panic!("the file differs from the server's at byte {at}");
    }
}

/// `file` on a new server, downloaded into a new folder.
pub async fn start(
    name: &str,
    file: File,
    links: &LinkHandle,
) -> (Server, Folder, Downloads, DownloadId) {
    start_with(name, file, links, config()).await
}

/// As [`start`], with these times.
pub async fn start_with(
    name: &str,
    file: File,
    links: &LinkHandle,
    config: DownloadConfig,
) -> (Server, Folder, Downloads, DownloadId) {
    let server = Server::start().await;
    server.at(&format!("/{name}"), Answer::File(file));
    let dir = Folder::new(name);
    let downloads = Downloads::with_config(links.table(), dir.0.clone(), config);
    let id = downloads
        .add(request(&server.url(&format!("/{name}"))))
        .await
        .unwrap();
    (server, dir, downloads, id)
}

/// The cable's share in `status`, by name.
pub fn share<'a>(status: &'a DownloadStatus, name: &str) -> &'a LinkShare {
    status
        .links
        .iter()
        .find(|l| l.name == name)
        .unwrap_or_else(|| panic!("no share for {name}: {status:?}"))
}
