//! A restart with torrents: two engines, one after the other, on the same
//! state and download folders. Local seeds, no internet. "Killed" means every
//! task of the first engine dropped where it stood.

mod fake;
mod fetching;
mod scratch;
mod serve;
mod torrent;

use std::net::SocketAddr;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::Duration;

use barrage::LinkHandle;
use barrage::bt::{
    Source, TorrentAddError, TorrentConfig, TorrentId, TorrentPhase, TorrentStatus, Torrents,
};
use barrage::fetch::{DownloadConfig, Downloads, Usage};
use barrage::keeper::{Engines, Keeper};
use barrage::queue::Queue;
use barrage::record::{Kept, KeptState, Priority, Record, Store, What};
use barrage::transfer::TransferEngine;
use fetching::{ethernet, links};
use serde_json::Value;
use tokio::runtime::Runtime;
use torrent::{FILE, Seeds, folder_seeds, fresh_dir, listening_on, pattern, seeds_with};

const LEN: usize = 4 << 20;

/// "Ethernet 2", up.
async fn one_cable() -> LinkHandle {
    let handle = links(vec![vec![ethernet()]]);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while handle.usable().is_empty() {
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    handle
}

async fn slow_seeds(name: &str, kib: u32) -> Seeds {
    seeds_with(name, 4, &pattern(LEN), 64 << 10, move || {
        let mut options = listening_on("127.0.0.1");
        options.ratelimits.upload_bps = NonZeroU32::new(kib << 10);
        options
    })
    .await
}

async fn quick_seeds(name: &str) -> Seeds {
    seeds_with(name, 2, &pattern(LEN), 64 << 10, || {
        listening_on("127.0.0.1")
    })
    .await
}

/// One engine: what `serve` builds, saving every 50 ms.
struct Engine {
    torrents: Torrents,
    downloads: Downloads,
    keeper: Keeper,
    usage: Arc<Usage>,
}

async fn open(cables: &LinkHandle, state: &Path, dir: &Path, peers: Vec<SocketAddr>) -> Engine {
    let table = cables.table();
    let (store, record, note) = Store::open(state).unwrap();
    let usage = Usage::restored(table.clone(), &record.used);
    let seq = Arc::new(AtomicU64::new(0));
    let downloads = Downloads::sharing(
        table.clone(),
        dir.to_path_buf(),
        DownloadConfig::default(),
        Arc::clone(&usage),
        Arc::clone(&seq),
    );
    downloads.restore(record.transfers.clone(), record.next_download);
    let config = TorrentConfig {
        network: false,
        listen: false,
        tick: Duration::from_millis(50),
        peers,
        seq,
        state: Some(state.to_path_buf()),
        ..TorrentConfig::default()
    };
    let torrents = Torrents::start(table, dir.to_path_buf(), Arc::clone(&usage), config)
        .await
        .unwrap();
    torrents
        .restore(record.transfers.clone(), record.next_torrent)
        .await;
    let queue = Queue::start(downloads.clone(), torrents.clone(), record.limit);
    let keeper = Keeper::start(
        (store, record, note),
        Engines {
            downloads: downloads.clone(),
            torrents: torrents.clone(),
            queue,
            usage: Arc::clone(&usage),
        },
        Duration::from_millis(50),
    );
    Engine {
        torrents,
        downloads,
        keeper,
        usage,
    }
}

impl Engine {
    /// A normal close.
    async fn close(self) {
        let ran = self.keeper.running();
        self.downloads.shutdown().await;
        self.torrents.shutdown().await;
        self.keeper.finish(&ran);
    }

    fn received(&self) -> u64 {
        self.usage.kept().iter().map(|u| u.bytes).sum()
    }
}

fn status(torrents: &Torrents, id: TorrentId) -> TorrentStatus {
    torrents
        .status()
        .borrow()
        .iter()
        .find(|t| t.id == id)
        .cloned()
        .expect("the torrent is listed")
}

async fn wait_for(
    torrents: &Torrents,
    id: TorrentId,
    secs: u64,
    said: &str,
    what: impl Fn(&TorrentStatus) -> bool,
) -> TorrentStatus {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    loop {
        let now = status(torrents, id);
        if what(&now) {
            return now;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "never {said}: {now:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// How many pieces the fast resume data on disk says are here.
fn pieces_flushed(state: &Path) -> u64 {
    let Ok(files) = std::fs::read_dir(state.join("resume")) else {
        return 0;
    };
    files
        .flatten()
        .filter_map(|file| std::fs::read(file.path()).ok())
        .map(|bytes| bytes.iter().map(|b| u64::from(b.count_ones())).sum::<u64>())
        .sum()
}

/// What state.json says of its first transfer.
fn first_saved(state: &Path) -> Option<Value> {
    let bytes = std::fs::read(state.join("state.json")).ok()?;
    let all: Value = serde_json::from_slice(&bytes).ok()?;
    Some(all["transfers"][0].clone())
}

/// A torrent's row, as a test writes it into a record.
fn row(id: u64, hash: &str, dir: PathBuf, state: KeptState, magnet: Option<String>) -> Kept {
    Kept {
        seq: id,
        priority: Priority::Normal,
        state,
        dir,
        name: None,
        length: None,
        done: 0,
        note: None,
        what: What::Torrent {
            id,
            info_hash: hash.to_string(),
            magnet,
            output: None,
        },
    }
}

fn left_behind(state: &Path, transfers: Vec<Kept>) {
    let (store, _, _) = Store::open(state).unwrap();
    let next_torrent = transfers.len() as u64;
    store
        .save(&Record {
            transfers,
            next_torrent,
            ..Record::default()
        })
        .unwrap();
}

#[tokio::test]
async fn a_torrent_cut_by_a_restart_finishes_every_piece_checked() {
    let seeds = slow_seeds("r-cut", 256).await;
    let cables = one_cable().await;
    let (state, dir) = (fresh_dir("r-cut-state"), fresh_dir("r-cut"));

    let first = open(&cables, &state, &dir, seeds.addrs.clone()).await;
    let id = first
        .torrents
        .add(Source::File(seeds.torrent.clone()))
        .await
        .unwrap();
    let had = wait_for(&first.torrents, id, 30, "a quarter done", |t| {
        t.done * 4 >= LEN as u64
    })
    .await;
    let received = first.received();
    assert!(
        received >= LEN as u64 / 4,
        "a quarter had arrived before the close, not only been read: {received}"
    );
    first.close().await;

    let next = open(&cables, &state, &dir, seeds.addrs.clone()).await;
    let shown = status(&next.torrents, id);
    assert_eq!(shown.id, id, "the same torrent, under the same id");
    assert_eq!(shown.info_hash, had.info_hash);
    assert!(shown.done >= had.done, "its progress shows at once");

    wait_for(&next.torrents, id, 60, "done", |t| {
        t.phase == TorrentPhase::Done
    })
    .await;
    assert_eq!(std::fs::read(dir.join(FILE)).unwrap(), seeds.bytes);
    let fetched = next.received() - received.min(next.received());
    assert!(
        fetched < LEN as u64,
        "fetched {fetched} of {LEN} after the restart"
    );
}

#[tokio::test]
async fn a_killed_engine_brings_its_torrent_back() {
    let seeds = slow_seeds("r-killed", 128).await;
    let cables = one_cable().await;
    let (state, dir) = (fresh_dir("r-killed-state"), fresh_dir("r-killed"));

    let doomed = Runtime::new().unwrap();
    let (first, id) = {
        let (table, peers, torrent) = (cables.table(), seeds.addrs.clone(), seeds.torrent.clone());
        let (state, dir) = (state.clone(), dir.clone());
        doomed
            .spawn(async move {
                let (store, record, note) = Store::open(&state).unwrap();
                let usage = Usage::restored(table.clone(), &record.used);
                let seq = Arc::new(AtomicU64::new(0));
                let downloads = Downloads::sharing(
                    table.clone(),
                    dir.clone(),
                    DownloadConfig::default(),
                    Arc::clone(&usage),
                    Arc::clone(&seq),
                );
                let config = TorrentConfig {
                    network: false,
                    listen: false,
                    tick: Duration::from_millis(50),
                    peers,
                    seq,
                    state: Some(state),
                    ..TorrentConfig::default()
                };
                let torrents = Torrents::start(table, dir, Arc::clone(&usage), config)
                    .await
                    .unwrap();
                let id = torrents.add(Source::File(torrent)).await.unwrap();
                let queue = Queue::start(downloads.clone(), torrents.clone(), record.limit);
                let keeper = Keeper::start(
                    (store, record, note),
                    Engines {
                        downloads,
                        torrents: torrents.clone(),
                        queue,
                        usage,
                    },
                    Duration::from_millis(50),
                );
                ((torrents, keeper), id)
            })
            .await
            .unwrap()
    };
    wait_for(&first.0, id, 30, "a quarter done", |t| {
        t.done * 4 >= LEN as u64
    })
    .await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while first_saved(&state).is_none_or(|t| t["state"] != "run") {
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // A kill loses what came since the last flush, so wait for one.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while pieces_flushed(&state) == 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the pieces never reached the disk while it ran"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // Part of the file: finishing flushes anyway, which proves nothing.
    assert!(
        pieces_flushed(&state) < LEN as u64 / (64 << 10),
        "killed mid-way, with pieces already flushed"
    );
    doomed.shutdown_background();
    drop(first);
    let kept = pieces_flushed(&state) * (64 << 10);

    let next = open(&cables, &state, &dir, seeds.addrs.clone()).await;
    wait_for(&next.torrents, id, 60, "done", |t| {
        t.phase == TorrentPhase::Done
    })
    .await;
    assert_eq!(std::fs::read(dir.join(FILE)).unwrap(), seeds.bytes);
    let fetched: u64 = next.usage.kept().iter().map(|u| u.bytes).sum();
    assert!(
        fetched < 2 * LEN as u64 - kept,
        "both runs fetched {fetched} of {LEN}, with {kept} kept through the kill"
    );
}

#[tokio::test]
async fn a_stopped_torrent_stays_stopped() {
    let seeds = slow_seeds("r-stopped", 256).await;
    let cables = one_cable().await;
    let (state, dir) = (fresh_dir("r-stopped-state"), fresh_dir("r-stopped"));

    let first = open(&cables, &state, &dir, seeds.addrs.clone()).await;
    let id = first
        .torrents
        .add(Source::File(seeds.torrent.clone()))
        .await
        .unwrap();
    wait_for(&first.torrents, id, 30, "a quarter done", |t| {
        t.done * 4 >= LEN as u64
    })
    .await;
    first.torrents.stop(id).await.unwrap();
    let had = status(&first.torrents, id);
    first.close().await;
    assert_eq!(first_saved(&state).unwrap()["state"], "stopped");

    let next = open(&cables, &state, &dir, seeds.addrs.clone()).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let back = status(&next.torrents, id);
    assert_eq!(back.phase, TorrentPhase::Stopped, "{back:?}");
    assert_eq!(back.done, had.done);
    assert_eq!(back.peers, 0);

    next.torrents.resume(id).await.unwrap();
    wait_for(&next.torrents, id, 60, "done", |t| {
        t.phase == TorrentPhase::Done
    })
    .await;
    assert_eq!(std::fs::read(dir.join(FILE)).unwrap(), seeds.bytes);
}

#[tokio::test]
async fn a_magnet_with_no_metadata_comes_back_as_a_magnet() {
    let cables = one_cable().await;
    let (state, dir) = (fresh_dir("r-magnet-state"), fresh_dir("r-magnet"));
    let hash = "cd".repeat(20);
    let magnet = format!("magnet:?xt=urn:btih:{hash}&dn=never.iso");

    let first = open(&cables, &state, &dir, Vec::new()).await;
    let id = first
        .torrents
        .add(Source::Magnet(magnet.clone()))
        .await
        .unwrap();
    first.close().await;
    let saved = first_saved(&state).unwrap();
    assert_eq!(saved["what"]["magnet"], magnet.as_str());

    let next = open(&cables, &state, &dir, Vec::new()).await;

    let back = wait_for(&next.torrents, id, 10, "finding", |t| {
        t.phase == TorrentPhase::Finding
    })
    .await;
    assert_eq!(back.info_hash, hash);
    assert_eq!(back.name.as_deref(), Some("never.iso"));
}

#[tokio::test]
async fn a_magnet_with_its_metadata_comes_back_from_its_torrent_file() {
    let seeds = quick_seeds("r-learned").await;
    let cables = one_cable().await;
    let (state, dir) = (fresh_dir("r-learned-state"), fresh_dir("r-learned"));

    let first = open(&cables, &state, &dir, seeds.addrs.clone()).await;
    let id = first
        .torrents
        .add(Source::Magnet(seeds.magnet.clone()))
        .await
        .unwrap();
    wait_for(&first.torrents, id, 30, "done", |t| {
        t.phase == TorrentPhase::Done
    })
    .await;
    first.close().await;

    let saved = first_saved(&state).unwrap();
    assert!(saved["what"]["magnet"].is_null(), "{saved}");
    let hash = saved["what"]["info_hash"].as_str().unwrap();
    assert!(
        state
            .join("torrents")
            .join(format!("{hash}.torrent"))
            .exists()
    );
}

#[tokio::test]
async fn a_multi_file_torrent_keeps_its_folder() {
    let (one, two) = (pattern(1 << 20), pattern(2 << 20));
    let seeds = folder_seeds("r-pack", &[("a.bin", &one), ("sub/b.bin", &two)]).await;
    let cables = one_cable().await;
    let (state, dir) = (fresh_dir("r-pack-state"), fresh_dir("r-pack"));

    let first = open(&cables, &state, &dir, seeds.addrs.clone()).await;
    let id = first
        .torrents
        .add(Source::File(seeds.torrent.clone()))
        .await
        .unwrap();
    wait_for(&first.torrents, id, 30, "done", |t| {
        t.phase == TorrentPhase::Done
    })
    .await;
    first.close().await;
    let output = first_saved(&state).unwrap()["what"]["output"].clone();
    assert_eq!(
        output.as_str().map(PathBuf::from),
        Some(dir.join("r-pack")),
        "{output}"
    );

    // Started again with another download folder: it stays where it is.
    let other = fresh_dir("r-pack-other");
    let next = open(&cables, &state, &other, seeds.addrs.clone()).await;
    assert_eq!(status(&next.torrents, id).phase, TorrentPhase::Done);
    assert_eq!(std::fs::read(dir.join("r-pack/a.bin")).unwrap(), one);
    assert_eq!(std::fs::read_dir(&other).unwrap().count(), 0);
}

#[tokio::test]
async fn a_done_torrent_comes_back_done_and_touches_nothing() {
    let seeds = quick_seeds("r-done").await;
    let cables = one_cable().await;
    let (state, dir) = (fresh_dir("r-done-state"), fresh_dir("r-done"));

    let first = open(&cables, &state, &dir, seeds.addrs.clone()).await;
    let id = first
        .torrents
        .add(Source::File(seeds.torrent.clone()))
        .await
        .unwrap();
    let done = wait_for(&first.torrents, id, 30, "done", |t| {
        t.phase == TorrentPhase::Done
    })
    .await;
    first.close().await;
    // Moved away by the user: it is theirs now.
    std::fs::remove_file(dir.join(FILE)).unwrap();

    let next = open(&cables, &state, &dir, seeds.addrs.clone()).await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    let back = status(&next.torrents, id);
    assert_eq!(back.phase, TorrentPhase::Done, "{back:?}");
    assert_eq!((back.done, back.length), (done.done, done.length));
    assert_eq!(back.peers, 0);
    assert!(!dir.join(FILE).exists(), "nothing fetched it back");
}

#[tokio::test]
async fn a_saved_torrent_file_gone_is_noted_and_the_others_run() {
    let seeds = quick_seeds("r-gone").await;
    let cables = one_cable().await;
    let (state, dir) = (fresh_dir("r-gone-state"), fresh_dir("r-gone"));
    let first = open(&cables, &state, &dir, seeds.addrs.clone()).await;
    let good = first
        .torrents
        .add(Source::File(seeds.torrent.clone()))
        .await
        .unwrap();
    first.torrents.stop(good).await.unwrap();
    first.close().await;
    let (store, mut record, _) = Store::open(&state).unwrap();
    record.transfers.insert(
        0,
        row(9, &"ef".repeat(20), dir.clone(), KeptState::Run, None),
    );
    record.next_torrent = 9;
    store.save(&record).unwrap();
    drop(store);

    let next = open(&cables, &state, &dir, seeds.addrs.clone()).await;

    let all = next.torrents.status().borrow().clone();
    let bad = all.iter().find(|t| t.id.as_u64() == 9).unwrap();
    assert_eq!(bad.phase, TorrentPhase::Failed, "{bad:?}");
    assert!(
        bad.note.as_deref().unwrap().contains("add it again"),
        "{bad:?}"
    );
    next.torrents.resume(good).await.unwrap();
    wait_for(&next.torrents, good, 30, "done", |t| {
        t.phase == TorrentPhase::Done
    })
    .await;
}

#[tokio::test]
async fn a_stopped_torrent_whose_saved_file_is_gone_is_noted_and_stays_so_on_resume() {
    let seeds = quick_seeds("r-gone-stopped").await;
    let cables = one_cable().await;
    let (state, dir) = (
        fresh_dir("r-gone-stopped-state"),
        fresh_dir("r-gone-stopped"),
    );
    let first = open(&cables, &state, &dir, seeds.addrs.clone()).await;
    first.close().await;
    let (store, mut record, _) = Store::open(&state).unwrap();
    record.transfers.insert(
        0,
        row(9, &"ef".repeat(20), dir.clone(), KeptState::Stopped, None),
    );
    record.next_torrent = 9;
    store.save(&record).unwrap();
    drop(store);

    let next = open(&cables, &state, &dir, seeds.addrs.clone()).await;
    let note = |next: &Engine| {
        let all = next.torrents.status().borrow().clone();
        all.into_iter().find(|t| t.id.as_u64() == 9).unwrap()
    };
    let bad = note(&next);
    assert_eq!(bad.phase, TorrentPhase::Failed, "{bad:?}");
    assert!(
        bad.note.as_deref().unwrap().contains("add it again"),
        "{bad:?}"
    );
    // Refused or not, it is not queued and the note stays the same.
    let _ = next.torrents.resume(bad.id).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let after = note(&next);
    assert_eq!(after.phase, TorrentPhase::Failed, "{after:?}");
    assert_eq!(after.note, bad.note, "{after:?}");
    assert_eq!(after.peers, 0);
}

#[tokio::test]
async fn adding_a_torrent_already_back_is_refused_as_before() {
    let seeds = quick_seeds("r-twice").await;
    let cables = one_cable().await;
    let (state, dir) = (fresh_dir("r-twice-state"), fresh_dir("r-twice"));
    let first = open(&cables, &state, &dir, seeds.addrs.clone()).await;
    let id = first
        .torrents
        .add(Source::File(seeds.torrent.clone()))
        .await
        .unwrap();
    first.torrents.stop(id).await.unwrap();
    first.close().await;

    let next = open(&cables, &state, &dir, seeds.addrs.clone()).await;
    let again = next.torrents.add(Source::File(seeds.torrent.clone())).await;

    assert_eq!(again, Err(TorrentAddError::AlreadyAdded(id)));
}

#[tokio::test]
async fn ids_and_order_carry_on_past_the_highest() {
    let seeds = quick_seeds("r-ids").await;
    let cables = one_cable().await;
    let (state, dir) = (fresh_dir("r-ids-state"), fresh_dir("r-ids"));
    let earlier = Kept {
        seq: 41,
        ..row(
            6,
            &"ab".repeat(20),
            dir.clone(),
            KeptState::Done,
            Some(format!("magnet:?xt=urn:btih:{}", "ab".repeat(20))),
        )
    };
    left_behind(&state, vec![earlier]);

    let next = open(&cables, &state, &dir, seeds.addrs.clone()).await;
    let id = next
        .torrents
        .add(Source::File(seeds.torrent.clone()))
        .await
        .unwrap();

    assert_eq!(id.as_u64(), 7);
    assert_eq!(status(&next.torrents, id).seq, 42);
}

#[tokio::test]
async fn a_removed_torrent_stays_gone_and_can_be_added_again() {
    let seeds = slow_seeds("r-remove", 256).await;
    let cables = one_cable().await;
    let (state, dir) = (fresh_dir("r-remove-state"), fresh_dir("r-remove"));
    let first = open(&cables, &state, &dir, seeds.addrs.clone()).await;
    let id = first
        .torrents
        .add(Source::File(seeds.torrent.clone()))
        .await
        .unwrap();
    wait_for(&first.torrents, id, 30, "a quarter done", |t| {
        t.done * 4 >= LEN as u64
    })
    .await;

    // Still running: it stops and leaves the list, its file kept.
    first.torrents.forget(id).await.unwrap();

    assert!(first.torrents.status().borrow().is_empty());
    assert!(dir.join(FILE).exists());
    let leftovers = |folder: &str| std::fs::read_dir(state.join(folder)).unwrap().count();
    assert_eq!(leftovers("torrents"), 0, "its saved .torrent has gone");
    assert_eq!(leftovers("resume"), 0, "and its fast resume data");
    assert_eq!(
        first.torrents.forget(id).await,
        Err(barrage::bt::TorrentControlError::NoSuchTorrent)
    );

    let again = first
        .torrents
        .add(Source::File(seeds.torrent.clone()))
        .await
        .unwrap();
    assert_eq!(again.as_u64(), id.as_u64() + 1, "ids are not used twice");
    wait_for(&first.torrents, again, 60, "done", |t| {
        t.phase == TorrentPhase::Done
    })
    .await;
    assert_eq!(std::fs::read(dir.join(FILE)).unwrap(), seeds.bytes);
    first.close().await;

    let next = open(&cables, &state, &dir, seeds.addrs.clone()).await;
    let list = next.torrents.status().borrow().clone();
    assert_eq!(list.len(), 1, "{list:?}");
    assert_eq!(list[0].id, again);
}

/// A failed torrent's info hash used to block it for good.
#[tokio::test]
async fn a_failed_torrent_can_be_removed_and_added_again() {
    let seeds = quick_seeds("r-failed").await;
    let cables = one_cable().await;
    let (state, dir) = (fresh_dir("r-failed-state"), fresh_dir("r-failed"));
    let hash = seeds.info_hash.as_string();
    left_behind(
        &state,
        vec![row(1, &hash, dir.clone(), KeptState::Failed, None)],
    );

    let next = open(&cables, &state, &dir, seeds.addrs.clone()).await;
    let id = next.torrents.status().borrow()[0].id;
    next.torrents.forget(id).await.unwrap();

    let again = next
        .torrents
        .add(Source::File(seeds.torrent.clone()))
        .await
        .unwrap();
    wait_for(&next.torrents, again, 30, "done", |t| {
        t.phase == TorrentPhase::Done
    })
    .await;
}

/// The fork never syncs pieces, so after a kill or a power cut its saved
/// bitfield may be ahead of the disk: the next engine drops it.
#[tokio::test]
async fn an_engine_that_did_not_close_has_its_torrents_checked_in_full() {
    let cables = one_cable().await;
    let (state, dir) = (fresh_dir("r-unclean-state"), fresh_dir("r-unclean"));
    let hash = "ab".repeat(20);
    left_behind(
        &state,
        vec![row(1, &hash, dir.clone(), KeptState::Stopped, None)],
    );
    let bitfield = state.join("resume").join(format!("{hash}.bitv"));
    std::fs::create_dir_all(bitfield.parent().unwrap()).unwrap();
    std::fs::write(&bitfield, [0xff]).unwrap();

    // left_behind dropped its store without closing it.
    let next = open(&cables, &state, &dir, Vec::new()).await;
    assert!(!bitfield.exists(), "the saved bitfield was trusted");
    next.close().await;

    std::fs::write(&bitfield, [0xff]).unwrap();
    let again = open(&cables, &state, &dir, Vec::new()).await;
    assert!(bitfield.exists(), "after a normal close it is kept");
    again.close().await;
}
