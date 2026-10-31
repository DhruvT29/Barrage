//! DHT, UDP trackers and uTP on one cable (phase 5 Task 8): the session's
//! UDP sockets bind to the UDP cable, the fastest not metered, and move,
//! by rebuilding the session, when it changes, every torrent keeping what
//! it has. Cables are "Ethernet 2" (127.0.0.2) and "Ethernet 3"
//! (127.0.0.3); every peer is a local session on 127.0.0.1.

mod fake;
mod torrent;

use std::net::{IpAddr, SocketAddr};
use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use barrage::LinkHandle;
use barrage::bt::{Source, TorrentConfig, TorrentId, TorrentPhase, TorrentStatus, Torrents};
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
) -> (Torrents, std::path::PathBuf) {
    let dir = fresh_dir(name);
    let usage = Usage::counting_speed_tests(links.table());
    let config = TorrentConfig {
        network: false,
        listen: false,
        tick: ms(50),
        peers,
    };
    let torrents = Torrents::start(links.table(), dir.clone(), usage, config)
        .await
        .unwrap();
    (torrents, dir)
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

fn status(torrents: &Torrents, id: TorrentId) -> TorrentStatus {
    torrents
        .status()
        .borrow()
        .iter()
        .find(|t| t.id == id)
        .cloned()
        .unwrap()
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

/// Waits until the session's UDP is on `cable`, at `ip`.
async fn udp_moves_to(torrents: &Torrents, cable: &str, ip: &str, secs: u64) {
    let ip: IpAddr = ip.parse().unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    loop {
        let named = torrents.swarm().borrow().udp_cable.clone();
        let addrs = torrents.udp_addrs();
        if named.as_deref() == Some(cable)
            && !addrs.is_empty()
            && addrs.iter().all(|a| a.ip() == ip)
        {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "UDP never moved to {cable}: named {named:?}, bound {addrs:?}"
        );
        tokio::time::sleep(ms(20)).await;
    }
}

#[tokio::test]
async fn udp_runs_on_the_unmetered_cable() {
    let (links, _live) = cables(vec![eth2(true), eth3(false)], &[(ETH2, 80), (ETH3, 20)]).await;
    let (torrents, _) = torrents("u-unmetered", &links, Vec::new()).await;
    udp_moves_to(&torrents, "Ethernet 3", ETH3, 5).await;
}

/// With no working cable, UDP stays on this machine, and moves to the first
/// cable that comes.
#[tokio::test]
async fn udp_waits_on_this_machine_until_a_cable_comes() {
    let (links, live) = cables(Vec::new(), &[]).await;
    let (torrents, _) = torrents("u-waits", &links, Vec::new()).await;
    assert_eq!(torrents.swarm().borrow().udp_cable, None);
    let addrs = torrents.udp_addrs();
    assert!(!addrs.is_empty(), "the DHT socket opens");
    assert!(
        addrs.iter().all(|a| a.ip() == IpAddr::from([127, 0, 0, 1])),
        "nothing leaves through Windows' choice: {addrs:?}"
    );
    live.set(vec![eth3(false)]);
    udp_moves_to(&torrents, "Ethernet 3", ETH3, 10).await;
}

/// Review Focus 4: the UDP cable leaving mid-torrent moves UDP to the
/// next; the torrent keeps what it has (nothing fetched twice, `done`
/// never going back) and finishes.
#[tokio::test]
async fn the_udp_cable_leaving_moves_udp_and_keeps_progress() {
    let len: usize = 4 << 20;
    let seeds = slow_seeds("u-leaving", 4, len, 256).await;
    let (links, live) = cables(vec![eth2(false), eth3(false)], &[(ETH2, 10), (ETH3, 30)]).await;
    let (torrents, dir) = torrents("u-leaving-leech", &links, seeds.addrs.clone()).await;
    udp_moves_to(&torrents, "Ethernet 3", ETH3, 5).await;

    let id = torrents
        .add(Source::File(seeds.torrent.clone()))
        .await
        .unwrap();
    let shown = Arc::new(Mutex::new(Vec::new()));
    let watching = {
        let shown = Arc::clone(&shown);
        let mut status = torrents.status();
        tokio::spawn(async move {
            while status.changed().await.is_ok() {
                if let Some(t) = status.borrow().iter().find(|t| t.id == id) {
                    shown.lock().unwrap().push(t.done);
                }
            }
        })
    };

    wait_for(&torrents, id, 30, "40% done", |t| {
        t.done * 5 >= len as u64 * 2
    })
    .await;
    live.set(vec![eth2(false)]);
    udp_moves_to(&torrents, "Ethernet 2", ETH2, 10).await;

    let done = wait_for(&torrents, id, 60, "done", |t| t.phase == TorrentPhase::Done).await;
    watching.abort();
    assert_eq!(std::fs::read(dir.join(FILE)).unwrap(), seeds.bytes);
    let shown = shown.lock().unwrap();
    assert!(
        shown.windows(2).all(|w| w[0] <= w[1]),
        "done went back: {shown:?}"
    );
    let received: u64 = done.cables.iter().map(|c| c.received).sum();
    assert!(
        received < len as u64 * 5 / 4,
        "fetched again what it had: {received} bytes for a {len} byte file"
    );
}

/// A stopped torrent is given to the rebuilt session stopped: it takes
/// nothing more until resumed, and then finishes.
#[tokio::test]
async fn a_stopped_torrent_stays_stopped_across_a_rebuild() {
    let len: usize = 4 << 20;
    let seeds = slow_seeds("u-stopped", 4, len, 256).await;
    let (links, live) = cables(vec![eth2(false), eth3(false)], &[(ETH2, 10), (ETH3, 30)]).await;
    let (torrents, dir) = torrents("u-stopped-leech", &links, seeds.addrs.clone()).await;
    udp_moves_to(&torrents, "Ethernet 3", ETH3, 5).await;

    let id = torrents
        .add(Source::File(seeds.torrent.clone()))
        .await
        .unwrap();
    wait_for(&torrents, id, 30, "a quarter done", |t| {
        t.done * 4 >= len as u64
    })
    .await;
    torrents.stop(id).await.unwrap();
    live.set(vec![eth2(false)]);
    udp_moves_to(&torrents, "Ethernet 2", ETH2, 10).await;
    let stopped = wait_for(&torrents, id, 10, "checked by the new session", |t| {
        t.phase == TorrentPhase::Stopped && t.length == Some(len as u64)
    })
    .await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let later = status(&torrents, id);
    assert_eq!(later.phase, TorrentPhase::Stopped);
    assert_eq!(later.done, stopped.done, "a stopped torrent took more");

    torrents.resume(id).await.unwrap();
    wait_for(&torrents, id, 60, "done", |t| t.phase == TorrentPhase::Done).await;
    assert_eq!(std::fs::read(dir.join(FILE)).unwrap(), seeds.bytes);
}
