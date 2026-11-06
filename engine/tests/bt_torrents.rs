//! Torrents behind `TransferEngine` (phase 5 Task 6): added from a .torrent
//! file or a magnet link, downloaded through the fake cable "Ethernet 2"
//! (127.0.0.2) from local seeds on 127.0.0.1, stopped and resumed. Nothing
//! leaves this machine.

mod fake;
mod fetching;
mod scratch;
mod serve;
mod torrent;

use std::net::SocketAddr;
use std::num::NonZeroU32;
use std::time::Duration;

use barrage::LinkHandle;
use barrage::bt::{
    Source, TorrentAddError, TorrentConfig, TorrentId, TorrentPhase, TorrentStatus, Torrents,
};
use barrage::fetch::Usage;
use barrage::transfer::TransferEngine;
use fetching::{ethernet, links};
use torrent::{FILE, Seeds, folder_seeds, fresh_dir, listening_on, pattern, seeds_with};

/// "Ethernet 2", up.
async fn one_cable() -> LinkHandle {
    let handle = links(vec![vec![ethernet()]]);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while handle.usable().is_empty() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the cable never came up"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    handle
}

/// Torrents in their own folder, starting every torrent with `peers`.
async fn torrents(
    name: &str,
    cables: &LinkHandle,
    peers: Vec<SocketAddr>,
) -> (Torrents, std::path::PathBuf) {
    let dir = fresh_dir(name);
    let config = TorrentConfig {
        network: false,
        listen: false,
        try_for: barrage::bt::listen::TRY_FOR,
        tick: Duration::from_millis(50),
        peers,
    };
    let usage = Usage::counting_speed_tests(cables.table());
    let torrents = Torrents::start(cables.table(), dir.clone(), usage, config)
        .await
        .unwrap();
    (torrents, dir)
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

/// Waits until `id`'s status satisfies `what`, for at most `secs`.
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

async fn plain_seeds(name: &str, len: usize) -> Seeds {
    seeds_with(name, 2, &pattern(len), 64 << 10, || {
        listening_on("127.0.0.1")
    })
    .await
}

#[tokio::test]
async fn a_torrent_file_downloads_whole() {
    let seeds = plain_seeds("t-file", 2 << 20).await;
    let cables = one_cable().await;
    let (torrents, dir) = torrents("t-file-leech", &cables, seeds.addrs.clone()).await;

    let id = torrents
        .add(Source::File(seeds.torrent.clone()))
        .await
        .unwrap();
    let done = wait_for(&torrents, id, 30, "done", |t| t.phase == TorrentPhase::Done).await;
    assert_eq!(done.length, Some(2 << 20));
    assert_eq!(done.done, 2 << 20);
    assert_eq!(done.name.as_deref(), Some(FILE));
    assert_eq!(done.info_hash, seeds.info_hash.as_string());
    assert_eq!(std::fs::read(dir.join(FILE)).unwrap(), seeds.bytes);
}

#[tokio::test]
async fn a_magnet_finds_its_metadata_from_a_peer() {
    let seeds = plain_seeds("t-magnet", 1 << 20).await;
    let cables = one_cable().await;
    let (torrents, dir) = torrents("t-magnet-leech", &cables, seeds.addrs.clone()).await;

    let id = torrents
        .add(Source::Magnet(seeds.magnet.clone()))
        .await
        .unwrap();
    // Listed at once, before its metadata is known.
    let first = status(&torrents, id);
    assert!(
        matches!(
            first.phase,
            TorrentPhase::Finding | TorrentPhase::Downloading | TorrentPhase::Done
        ),
        "{first:?}"
    );
    wait_for(&torrents, id, 30, "done", |t| t.phase == TorrentPhase::Done).await;
    assert_eq!(std::fs::read(dir.join(FILE)).unwrap(), seeds.bytes);
}

#[tokio::test]
async fn stop_keeps_what_it_has_and_resume_finishes() {
    // Seeds that send 512 KiB a second each, so a 4 MiB file takes seconds.
    let slow = || {
        let mut options = listening_on("127.0.0.1");
        options.ratelimits.upload_bps = NonZeroU32::new(512 << 10);
        options
    };
    let seeds = seeds_with("t-stop", 2, &pattern(4 << 20), 64 << 10, slow).await;
    let cables = one_cable().await;
    let (torrents, dir) = torrents("t-stop-leech", &cables, seeds.addrs.clone()).await;

    let id = torrents
        .add(Source::File(seeds.torrent.clone()))
        .await
        .unwrap();
    wait_for(&torrents, id, 30, "part way", |t| t.done > 0).await;
    torrents.stop(id).await.unwrap();
    let stopped = wait_for(&torrents, id, 5, "stopped", |t| {
        t.phase == TorrentPhase::Stopped
    })
    .await;
    assert!(stopped.done < 4 << 20, "it stopped part way: {stopped:?}");
    tokio::time::sleep(Duration::from_millis(500)).await;
    let later = status(&torrents, id);
    assert_eq!(
        later.done, stopped.done,
        "nothing more arrives while stopped"
    );
    assert_eq!(later.peers, 0, "a stopped torrent holds no peers");

    torrents.resume(id).await.unwrap();
    wait_for(&torrents, id, 60, "done", |t| t.phase == TorrentPhase::Done).await;
    assert_eq!(std::fs::read(dir.join(FILE)).unwrap(), seeds.bytes);
}

/// A finished torrent shares nothing: a peer that still wants pieces, and
/// can only get them from this engine, gets no more once it is done.
#[tokio::test]
async fn a_finished_torrent_stops_sharing() {
    let seeds = plain_seeds("t-finished", 1 << 20).await;
    // A slow leecher with no peers of its own, listening for us.
    let mut options = listening_on("127.0.0.1");
    options.ratelimits.download_bps = NonZeroU32::new(128 << 10);
    let leecher_dir = fresh_dir("t-finished-other");
    let leecher = librqbit::Session::new_with_opts(leecher_dir.clone(), options)
        .await
        .unwrap();
    let other = leecher
        .add_torrent(
            librqbit::AddTorrent::from_bytes(seeds.torrent.clone()),
            Some(librqbit::AddTorrentOptions {
                output_folder: Some(leecher_dir.to_string_lossy().into_owned()),
                ..Default::default()
            }),
        )
        .await
        .unwrap()
        .into_handle()
        .unwrap();

    let cables = one_cable().await;
    let mut peers = seeds.addrs.clone();
    peers.push(leecher.listen_addr().unwrap());
    let (torrents, _dir) = torrents("t-finished-leech", &cables, peers).await;

    let id = torrents
        .add(Source::File(seeds.torrent.clone()))
        .await
        .unwrap();
    wait_for(&torrents, id, 30, "done", |t| t.phase == TorrentPhase::Done).await;
    let after = wait_for(&torrents, id, 5, "without peers", |t| t.peers == 0).await;
    assert_eq!((after.down_bps, after.up_bps), (0, 0));
    let before = other.stats().progress_bytes;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let later = other.stats().progress_bytes;
    assert!(
        later < 1 << 20,
        "the other leecher could not have finished from us alone"
    );
    assert_eq!(
        later, before,
        "nothing more went to the other leecher once this torrent was done"
    );
}

#[tokio::test]
async fn controls_answer_in_the_engines_words() {
    let seeds = plain_seeds("t-controls", 1 << 20).await;
    let cables = one_cable().await;
    let (torrents, _dir) = torrents("t-controls-leech", &cables, seeds.addrs.clone()).await;

    let id = torrents
        .add(Source::File(seeds.torrent.clone()))
        .await
        .unwrap();
    wait_for(&torrents, id, 30, "done", |t| t.phase == TorrentPhase::Done).await;
    assert_eq!(
        torrents.stop(id).await.unwrap_err().to_string(),
        "it is not running, so there is nothing to stop"
    );
    assert_eq!(
        torrents.resume(id).await.unwrap_err().to_string(),
        "it is not stopped, so there is nothing to resume"
    );
}

/// Review Focus 5: the same torrent twice, as its file and as its magnet,
/// is refused with the first one's id, not added again.
#[tokio::test]
async fn the_same_torrent_twice_is_refused() {
    let seeds = plain_seeds("t-twice", 1 << 20).await;
    let cables = one_cable().await;
    let (torrents, _dir) = torrents("t-twice-leech", &cables, seeds.addrs.clone()).await;

    let id = torrents
        .add(Source::File(seeds.torrent.clone()))
        .await
        .unwrap();
    let again = torrents.add(Source::Magnet(seeds.magnet.clone())).await;
    assert_eq!(again, Err(TorrentAddError::AlreadyAdded(id)));
    assert_eq!(torrents.status().borrow().len(), 1);
}

#[tokio::test]
async fn shutdown_ends_the_session() {
    let cables = one_cable().await;
    let (torrents, _dir) = torrents("t-shutdown", &cables, vec![]).await;
    torrents.shutdown().await;
    let refused = torrents
        .add(Source::Magnet(format!(
            "magnet:?xt=urn:btih:{}",
            "ab".repeat(20)
        )))
        .await;
    assert_eq!(
        refused,
        Err(TorrentAddError::Stopped),
        "nothing is added once the engine has stopped"
    );
}

/// A multi-file torrent keeps to a folder of its own, its name, so two with
/// a file of one name do not overwrite each other, or scatter through the
/// download folder. One comes by a magnet link, its layout known only from
/// a peer.
#[tokio::test]
async fn multi_file_torrents_keep_to_a_folder_of_their_own() {
    let (poster_one, poster_two) = (pattern(70_000), pattern(90_000));
    let track = pattern(200_000);
    let one = folder_seeds(
        "album-one",
        &[("poster.jpg", &poster_one), ("disc 1/track.bin", &track)],
    )
    .await;
    let two = folder_seeds(
        "album-two",
        &[("poster.jpg", &poster_two), ("notes.txt", b"liner notes")],
    )
    .await;
    let cables = one_cable().await;
    let peers = [one.addrs.clone(), two.addrs.clone()].concat();
    let (torrents, dir) = torrents("multi-file", &cables, peers).await;

    let first = torrents
        .add(Source::File(one.torrent.clone()))
        .await
        .unwrap();
    let second = torrents
        .add(Source::Magnet(two.magnet.clone()))
        .await
        .unwrap();
    for id in [first, second] {
        wait_for(&torrents, id, 30, "done", |t| t.phase == TorrentPhase::Done).await;
    }

    assert_eq!(
        std::fs::read(dir.join("album-one/poster.jpg")).unwrap(),
        poster_one
    );
    assert_eq!(
        std::fs::read(dir.join("album-one/disc 1/track.bin")).unwrap(),
        track
    );
    assert_eq!(
        std::fs::read(dir.join("album-two/poster.jpg")).unwrap(),
        poster_two
    );
    assert_eq!(
        std::fs::read(dir.join("album-two/notes.txt")).unwrap(),
        b"liner notes"
    );
    assert!(
        !dir.join("poster.jpg").exists(),
        "loose in the download folder"
    );
}
