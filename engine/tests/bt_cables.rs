//! A torrent's pieces shared between cables (phase 5 Task 7): peer
//! connections spread over "Ethernet 2" (127.0.0.2) and "Ethernet 3"
//! (127.0.0.3) by their speed tests, each cable's bytes counted both ways,
//! nothing uploaded through a metered cable, and a cable that leaves taking
//! its connections with it. Every peer is a local session on 127.0.0.1.

mod fake;
mod torrent;

use std::net::SocketAddr;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use barrage::LinkHandle;
use barrage::bt::{
    Source, TorrentCable, TorrentConfig, TorrentId, TorrentPhase, TorrentStatus, Torrents,
};
use barrage::fetch::Usage;
use barrage::links::Candidate;
use barrage::links::watch::WatchConfig;
use barrage::transfer::TransferEngine;
use fake::{Fake, Live, cand_idx, ms};
use torrent::{FILE, Seeds, fresh_dir, listening_on, pattern, seeds_with};

const ETH2: &str = "127.0.0.2";
const ETH3: &str = "127.0.0.3";

fn eth2(metered: bool) -> Candidate {
    Candidate {
        metered,
        ..cand_idx(2, "Ethernet 2", ETH2, true)
    }
}

fn eth3(metered: bool) -> Candidate {
    Candidate {
        metered,
        ..cand_idx(3, "Ethernet 3", ETH3, true)
    }
}

/// The cables in `first`, speed-tested at `speeds` (Mbps by address), once
/// every one has its figure.
async fn cables(first: Vec<Candidate>, speeds: &[(&str, u64)]) -> (LinkHandle, Live) {
    let (mut net, live) = Fake::live(first.clone());
    for (ip, mbps) in speeds {
        net = net.speed(ip, mbps * 1_000_000);
    }
    let handle =
        LinkHandle::start_with(Arc::new(net.loopback_links()), WatchConfig::every(ms(20))).unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while handle.usable().len() < first.len()
        || handle.usable().iter().any(|l| l.down_bps.is_none())
    {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the cables never came up measured"
        );
        tokio::time::sleep(ms(20)).await;
    }
    (handle, live)
}

async fn torrents(
    name: &str,
    links: &LinkHandle,
    peers: Vec<SocketAddr>,
) -> (Torrents, Arc<Usage>, std::path::PathBuf) {
    let dir = fresh_dir(name);
    let usage = Usage::counting_speed_tests(links.table());
    let config = TorrentConfig {
        network: false,
        listen: false,
        try_for: barrage::bt::listen::TRY_FOR,
        tick: ms(50),
        peers,
    };
    let torrents = Torrents::start(links.table(), dir.clone(), Arc::clone(&usage), config)
        .await
        .unwrap();
    (torrents, usage, dir)
}

/// Seeds that each send `kib` KiB a second.
async fn slow_seeds(name: &str, n: usize, len: usize, kib: u32) -> Seeds {
    seeds_with(name, n, &pattern(len), 64 << 10, move || {
        let mut options = listening_on("127.0.0.1");
        options.ratelimits.upload_bps = NonZeroU32::new(kib << 10);
        options
    })
    .await
}

/// Leechers with no peers of their own, listening, each taking at most
/// 128 KiB a second, each holding the file's first half already: the
/// engine needs them (it drops a peer with nothing it needs), and whatever
/// of the second half they get, they got from the engine. Each with what it
/// started with.
async fn leechers(
    name: &str,
    seeds: &Seeds,
    n: usize,
) -> Vec<(Arc<librqbit::Session>, Arc<librqbit::ManagedTorrent>, u64)> {
    let mut out = Vec::new();
    for i in 0..n {
        let mut options = listening_on("127.0.0.1");
        options.ratelimits.download_bps = NonZeroU32::new(128 << 10);
        let dir = fresh_dir(&format!("{name}-other-{i}"));
        let mut half = seeds.bytes.clone();
        let len = half.len();
        half[len / 2..].fill(0);
        std::fs::write(dir.join(FILE), &half).unwrap();
        let session = librqbit::Session::new_with_opts(dir.clone(), options)
            .await
            .unwrap();
        let handle = session
            .add_torrent(
                librqbit::AddTorrent::from_bytes(seeds.torrent.clone()),
                Some(librqbit::AddTorrentOptions {
                    output_folder: Some(dir.to_string_lossy().into_owned()),
                    overwrite: true,
                    ..Default::default()
                }),
            )
            .await
            .unwrap()
            .into_handle()
            .unwrap();
        handle.wait_until_initialized().await.unwrap();
        let start = handle.stats().progress_bytes;
        assert!(
            start >= (len / 2) as u64 - (64 << 10),
            "the leecher holds its first half: {start}"
        );
        out.push((session, handle, start));
    }
    out
}

/// What the leechers got beyond what they started with.
fn given(others: &[(Arc<librqbit::Session>, Arc<librqbit::ManagedTorrent>, u64)]) -> u64 {
    others
        .iter()
        .map(|(_, o, start)| o.stats().progress_bytes - start)
        .sum()
}

fn status(torrents: &Torrents, id: TorrentId) -> TorrentStatus {
    torrents
        .status()
        .borrow()
        .iter()
        .find(|t| t.id == id)
        .cloned()
        .unwrap()
}

fn cable(status: &TorrentStatus, name: &str) -> TorrentCable {
    status
        .cables
        .iter()
        .find(|c| c.name == name)
        .cloned()
        .unwrap_or(TorrentCable {
            name: name.to_string(),
            received: 0,
            sent: 0,
            bps: 0,
            connections: 0,
            uploads: true,
        })
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
        tokio::time::sleep(ms(20)).await;
    }
}

fn used(usage: &Usage, name: &str) -> (u64, u64) {
    usage
        .subscribe()
        .borrow()
        .iter()
        .find(|u| u.cable == name)
        .map_or((0, 0), |u| (u.bytes, u.sent))
}

#[tokio::test]
async fn both_cables_carry_pieces() {
    let seeds = slow_seeds("c-both", 4, 4 << 20, 512).await;
    let (links, _live) = cables(vec![eth2(false), eth3(false)], &[(ETH2, 30), (ETH3, 30)]).await;
    let (torrents, usage, dir) = torrents("c-both-leech", &links, seeds.addrs.clone()).await;

    let id = torrents
        .add(Source::File(seeds.torrent.clone()))
        .await
        .unwrap();
    let done = wait_for(&torrents, id, 60, "done", |t| t.phase == TorrentPhase::Done).await;
    assert_eq!(
        std::fs::read(dir.join(FILE)).unwrap(),
        seeds.bytes,
        "every piece checked and in place"
    );
    let (two, three) = (cable(&done, "Ethernet 2"), cable(&done, "Ethernet 3"));
    assert!(
        two.received > 0 && three.received > 0,
        "both carried: {:?}",
        done.cables
    );
    assert!(
        two.received + three.received >= 4 << 20,
        "they carried the file between them: {:?}",
        done.cables
    );
    assert!(
        used(&usage, "Ethernet 2").0 >= two.received,
        "the torrent's bytes are in the data used"
    );
    assert!(used(&usage, "Ethernet 3").0 >= three.received);
}

#[tokio::test]
async fn the_faster_cable_holds_more_peer_connections() {
    let seeds = slow_seeds("c-faster", 8, 8 << 20, 256).await;
    let (links, _live) = cables(vec![eth2(false), eth3(false)], &[(ETH2, 30), (ETH3, 10)]).await;
    let (torrents, _usage, _dir) = torrents("c-faster-leech", &links, seeds.addrs.clone()).await;

    let id = torrents
        .add(Source::File(seeds.torrent.clone()))
        .await
        .unwrap();
    let full = wait_for(&torrents, id, 30, "connected to all eight", |t| {
        cable(t, "Ethernet 2").connections + cable(t, "Ethernet 3").connections == 8
    })
    .await;
    let (fast, slow) = (
        cable(&full, "Ethernet 2").connections,
        cable(&full, "Ethernet 3").connections,
    );
    assert!(
        fast.abs_diff(6) <= 1 && slow.abs_diff(2) <= 1,
        "30 and 10 Mbps hold {fast} and {slow} of 8, not about 6 and 2"
    );
}

/// A metered cable uploads nothing: leechers that can only get pieces from
/// the engine get none through it, while the engine downloads in full.
#[tokio::test]
async fn a_metered_cable_uploads_nothing() {
    let seeds = slow_seeds("c-metered", 2, 2 << 20, 512).await;
    let others = leechers("c-metered", &seeds, 2).await;
    let (links, _live) = cables(vec![eth2(true)], &[(ETH2, 30)]).await;
    let mut peers = seeds.addrs.clone();
    peers.extend(others.iter().map(|(s, ..)| s.listen_addr().unwrap()));
    let (torrents, usage, _dir) = torrents("c-metered-leech", &links, peers).await;

    let id = torrents
        .add(Source::File(seeds.torrent.clone()))
        .await
        .unwrap();
    let done = wait_for(&torrents, id, 60, "done", |t| t.phase == TorrentPhase::Done).await;
    let two = cable(&done, "Ethernet 2");
    assert!(!two.uploads, "the status says it does not upload");
    assert!(two.received >= 2 << 20);
    assert!(
        two.sent < 32 << 10,
        "only the protocol's messages left through the metered cable: {} bytes",
        two.sent
    );
    assert_eq!(
        used(&usage, "Ethernet 2").1,
        two.sent,
        "what it sent is in the data used"
    );
    assert_eq!(
        given(&others),
        0,
        "a leecher got a piece through the metered cable"
    );
}

#[tokio::test]
async fn an_unmetered_cable_uploads_and_counts_it() {
    let seeds = slow_seeds("c-uploads", 2, 2 << 20, 256).await;
    let others = leechers("c-uploads", &seeds, 2).await;
    let (links, _live) = cables(vec![eth2(false)], &[(ETH2, 30)]).await;
    let mut peers = seeds.addrs.clone();
    peers.extend(others.iter().map(|(s, ..)| s.listen_addr().unwrap()));
    let (torrents, usage, _dir) = torrents("c-uploads-leech", &links, peers).await;

    let id = torrents
        .add(Source::File(seeds.torrent.clone()))
        .await
        .unwrap();
    let uploading = wait_for(&torrents, id, 30, "uploading", |t| {
        cable(t, "Ethernet 2").sent > 64 << 10
    })
    .await;
    assert!(cable(&uploading, "Ethernet 2").uploads);
    assert!(
        given(&others) > 0,
        "the leechers got pieces from the engine"
    );
    assert!(
        used(&usage, "Ethernet 2").1 > 0,
        "the upload is in the data used"
    );
}

/// Review Focus 1: a cable that leaves mid-torrent takes its connections
/// with it; the rest of the file comes through the other, nothing is lost,
/// and what it carried stays counted.
#[tokio::test]
async fn a_cable_leaving_moves_its_peers_to_the_other() {
    let seeds = slow_seeds("c-leaving", 4, 4 << 20, 256).await;
    let (links, live) = cables(vec![eth2(false), eth3(false)], &[(ETH2, 30), (ETH3, 30)]).await;
    let (torrents, usage, dir) = torrents("c-leaving-leech", &links, seeds.addrs.clone()).await;

    let id = torrents
        .add(Source::File(seeds.torrent.clone()))
        .await
        .unwrap();
    wait_for(&torrents, id, 30, "a third done, through both", |t| {
        t.done * 3 >= 4 << 20 && cable(t, "Ethernet 3").received > 0
    })
    .await;
    live.set(vec![eth2(false)]);
    let gone = wait_for(&torrents, id, 5, "Ethernet 3's connections ended", |t| {
        cable(t, "Ethernet 3").connections == 0
    })
    .await;
    let left_with = cable(&gone, "Ethernet 3").received;

    let done = wait_for(&torrents, id, 60, "done", |t| t.phase == TorrentPhase::Done).await;
    assert_eq!(std::fs::read(dir.join(FILE)).unwrap(), seeds.bytes);
    let three = cable(&done, "Ethernet 3");
    assert!(
        three.received >= left_with && three.received < left_with + (128 << 10),
        "Ethernet 3 kept its count and took no more"
    );
    assert!(
        used(&usage, "Ethernet 3").0 >= three.received,
        "what it carried is still in the data used"
    );
}
