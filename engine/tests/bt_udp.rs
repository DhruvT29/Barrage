//! DHT, UDP trackers and uTP on one cable (phase 5 Task 8): the session's
//! UDP sockets bind to the UDP cable, the fastest not metered, and move,
//! by rebuilding the session, when it changes, every torrent keeping what
//! it has. The listening port opens there too, and stays only if a peer
//! calls in (Task 9). Cables are "Ethernet 2" (127.0.0.2) and "Ethernet 3"
//! (127.0.0.3); every peer is a local session on 127.0.0.1.

mod fake;
mod torrent;

use std::net::{IpAddr, SocketAddr};
use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use barrage::LinkHandle;
use barrage::bt::{
    Listening, Source, TorrentConfig, TorrentId, TorrentPhase, TorrentStatus, Torrents,
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
) -> (Torrents, std::path::PathBuf) {
    let config = TorrentConfig {
        listen: false,
        peers,
        ..TorrentConfig::default()
    };
    started(name, links, config).await
}

/// Listening, closing after `try_for` of a torrent running.
async fn listening(
    name: &str,
    links: &LinkHandle,
    try_for: Duration,
    peers: Vec<SocketAddr>,
) -> (Torrents, std::path::PathBuf) {
    let config = TorrentConfig {
        try_for,
        peers,
        ..TorrentConfig::default()
    };
    started(name, links, config).await
}

async fn started(
    name: &str,
    links: &LinkHandle,
    config: TorrentConfig,
) -> (Torrents, std::path::PathBuf) {
    let dir = fresh_dir(name);
    let usage = Usage::counting_speed_tests(links.table());
    let config = TorrentConfig {
        network: false,
        tick: ms(50),
        ..config
    };
    let torrents = Torrents::start(links.table(), dir.clone(), usage, config)
        .await
        .unwrap();
    (torrents, dir)
}

/// Waits until the port stands as `what` says.
async fn port_is(
    torrents: &Torrents,
    secs: u64,
    said: &str,
    what: impl Fn(&Listening) -> bool,
) -> Listening {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    loop {
        let now = torrents.swarm().borrow().listening.clone();
        if what(&now) {
            return now;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the port never {said}: {now:?}"
        );
        tokio::time::sleep(ms(20)).await;
    }
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
    tokio::time::sleep(ms(300)).await;
    assert_eq!(torrents.swarm().borrow().listening, Listening::Off);
    assert_eq!(torrents.listen_addr(), None, "listening is off");
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

/// The port opens on the UDP cable; a peer calling in keeps it past the try.
#[tokio::test]
async fn a_local_peer_calling_in_keeps_the_port() {
    let seeds = seeds_with("l-kept", 1, &pattern(1 << 20), 64 << 10, || {
        listening_on("127.0.0.1")
    })
    .await;
    let (links, _live) = cables(vec![eth2(true), eth3(false)], &[(ETH2, 80), (ETH3, 20)]).await;
    let (torrents, _) = listening("l-kept-leech", &links, Duration::from_secs(4), Vec::new()).await;
    let addr = torrents.listen_addr().expect("the port opens");
    assert_eq!(addr.ip(), ETH3.parse::<IpAddr>().unwrap());
    assert!(matches!(
        torrents.swarm().borrow().listening,
        Listening::Trying { ref cable, .. } if cable == "Ethernet 3"
    ));

    torrents
        .add(Source::File(seeds.torrent.clone()))
        .await
        .unwrap();
    let _caller = seeds.calling(addr).await;
    let open = port_is(&torrents, 3, "opened", |l| {
        matches!(l, Listening::Open { .. })
    })
    .await;
    assert_eq!(
        open,
        Listening::Open {
            cable: "Ethernet 3".to_string()
        }
    );
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(torrents.swarm().borrow().listening, open, "past the try");
    assert_eq!(torrents.listen_addr(), Some(addr), "the same port");
    assert!(tokio::net::TcpStream::connect(addr).await.is_ok());
}

/// Nobody calling in while a torrent runs closes the port, says why, and
/// costs the torrent nothing.
#[tokio::test]
async fn nobody_calling_in_closes_the_port_and_says_why() {
    let len: usize = 2 << 20;
    let seeds = slow_seeds("l-closes", 2, len, 256).await;
    let (links, _live) = cables(vec![eth3(false)], &[(ETH3, 30)]).await;
    let (torrents, dir) = listening("l-closes-leech", &links, ms(300), seeds.addrs.clone()).await;
    let addr = torrents.listen_addr().expect("the port opens");

    let id = torrents
        .add(Source::File(seeds.torrent.clone()))
        .await
        .unwrap();
    let closed = port_is(&torrents, 5, "closed", |l| {
        matches!(l, Listening::Closed { .. })
    })
    .await;
    let Listening::Closed { cable, why } = closed else {
        unreachable!()
    };
    assert_eq!(cable, "Ethernet 3");
    assert!(
        why.contains("cannot be reached from outside on Ethernet 3"),
        "{why}"
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while torrents.listen_addr().is_some() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the session still listens"
        );
        tokio::time::sleep(ms(20)).await;
    }
    assert!(
        tokio::net::TcpStream::connect(addr).await.is_err(),
        "the old port is shut"
    );

    wait_for(&torrents, id, 60, "done", |t| t.phase == TorrentPhase::Done).await;
    assert_eq!(std::fs::read(dir.join(FILE)).unwrap(), seeds.bytes);
}

/// With nothing running nobody has a reason to call: the port waits.
#[tokio::test]
async fn with_nothing_running_the_port_waits() {
    let (links, _live) = cables(vec![eth3(false)], &[(ETH3, 30)]).await;
    let (torrents, _) = listening("l-waits", &links, ms(200), Vec::new()).await;
    let addr = torrents.listen_addr().expect("the port opens");
    let mut swarm = torrents.swarm();
    swarm.mark_unchanged();
    tokio::time::sleep(ms(800)).await;
    assert!(
        !swarm.has_changed().unwrap(),
        "the try's clock is not news to anyone watching"
    );
    let now = torrents.swarm().borrow().listening.clone();
    assert!(matches!(now, Listening::Trying { .. }), "{now:?}");
    assert_eq!(torrents.listen_addr(), Some(addr));
}

/// Started with no cable nothing listens; the first cable opens the port.
#[tokio::test]
async fn the_port_waits_for_a_cable() {
    let (links, live) = cables(Vec::new(), &[]).await;
    let (torrents, _) = listening("l-no-cable", &links, ms(200), Vec::new()).await;
    assert_eq!(torrents.swarm().borrow().listening, Listening::Off);
    assert_eq!(torrents.listen_addr(), None);

    live.set(vec![eth3(false)]);
    udp_moves_to(&torrents, "Ethernet 3", ETH3, 10).await;
    let addr = torrents.listen_addr().expect("the port opens");
    assert_eq!(addr.ip(), ETH3.parse::<IpAddr>().unwrap());
}

/// A closed port opens again on the next UDP cable, however soon after
/// closing it comes: the move waits out the rebuild gap, about 30 s here.
#[tokio::test]
async fn a_closed_port_tries_again_on_a_new_udp_cable() {
    let seeds = slow_seeds("l-again", 1, 1 << 20, 64).await;
    let (links, live) = cables(vec![eth2(false), eth3(false)], &[(ETH2, 10), (ETH3, 30)]).await;
    let (torrents, _) = listening("l-again-leech", &links, ms(200), seeds.addrs.clone()).await;
    let id = torrents
        .add(Source::File(seeds.torrent.clone()))
        .await
        .unwrap();
    port_is(&torrents, 5, "closed", |l| {
        matches!(l, Listening::Closed { .. })
    })
    .await;
    torrents.stop(id).await.unwrap();

    live.set(vec![eth2(false)]);
    port_is(
        &torrents,
        5,
        "tried again",
        |l| matches!(l, Listening::Trying { cable, .. } if cable == "Ethernet 2"),
    )
    .await;
    udp_moves_to(&torrents, "Ethernet 2", ETH2, 40).await;
    let addr = torrents.listen_addr().expect("the port opens again");
    assert_eq!(addr.ip(), ETH2.parse::<IpAddr>().unwrap());
}

/// While the session waits out the gap to move, a peer reaching its old
/// listener says nothing of the cable it is moving to.
#[tokio::test]
async fn a_peer_on_the_old_cable_opens_nothing() {
    let seeds = seeds_with("l-old", 1, &pattern(1 << 20), 64 << 10, || {
        listening_on("127.0.0.1")
    })
    .await;
    let (links, live) = cables(vec![eth2(false), eth3(false)], &[(ETH2, 10), (ETH3, 30)]).await;
    let (torrents, _) = listening("l-old-leech", &links, Duration::from_secs(60), Vec::new()).await;
    live.set(vec![eth2(false)]);
    udp_moves_to(&torrents, "Ethernet 2", ETH2, 10).await;
    let old = torrents.listen_addr().expect("listening on Ethernet 2");
    assert_eq!(old.ip(), ETH2.parse::<IpAddr>().unwrap());
    let id = torrents
        .add(Source::File(seeds.torrent.clone()))
        .await
        .unwrap();
    wait_for(&torrents, id, 5, "checked", |t| t.length.is_some()).await;

    live.set(vec![eth3(false)]);
    port_is(
        &torrents,
        5,
        "tried again",
        |l| matches!(l, Listening::Trying { cable, .. } if cable == "Ethernet 3"),
    )
    .await;
    assert_eq!(torrents.listen_addr(), Some(old), "still inside the gap");
    let _caller = seeds.calling(old).await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let now = torrents.swarm().borrow().listening.clone();
    assert!(matches!(now, Listening::Trying { .. }), "{now:?}");
}

/// A cable whose address is gone by the time the session binds it leaves
/// the engine running, on this machine, for the next tick to move.
#[tokio::test]
async fn a_cable_that_will_not_bind_waits_on_this_machine() {
    let wifi = cand_idx(1, "Wi-Fi", "192.0.2.10", true);
    let (links, _live) = cables(vec![wifi], &[("192.0.2.10", 30)]).await;
    let (torrents, _) = listening("l-no-bind", &links, ms(200), Vec::new()).await;
    assert_eq!(torrents.swarm().borrow().udp_cable, None);
    assert_eq!(torrents.listen_addr(), None);
    let addrs = torrents.udp_addrs();
    assert!(
        addrs.iter().all(|a| a.ip() == IpAddr::from([127, 0, 0, 1])),
        "{addrs:?}"
    );
}
