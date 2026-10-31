//! Connections by speed (phase 4, Task 3): a download's connections shared
//! between links by what each delivers, moved one at a time, within what a
//! server allows. Against the scripted server and network, as in
//! fetch_download.rs.

mod fake;
mod fetching;
mod serve;

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use barrage::LinkHandle;
use barrage::fetch::{CableUse, DownloadConfig, DownloadStatus, Downloads, Phase};
use barrage::links::LinkState;
use barrage::links::probe::ProbeVerdict;
use barrage::links::watch::{LinkTable, WatchConfig};
use fake::{Fake, cand_idx, ms};
use fetching::*;
use serve::{Answer, File, Server};

/// Wi-Fi and Ethernet 2 over `net`, once both are up and both speed tests
/// have run, `measured` of them with a figure: a download shares its
/// connections by those from the start.
async fn speed_tested(net: Fake, measured: usize) -> LinkHandle {
    let net = Arc::new(net.loopback_links());
    let links = LinkHandle::start_with(net.clone(), WatchConfig::every(ms(20))).unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let table = links.table().latest();
        let up = table.iter().filter(|l| l.state.is_usable()).count();
        let figures = table.iter().filter(|l| l.down_bps.is_some()).count();
        if up == 2 && figures == measured && net.samples_finished.load(Ordering::SeqCst) >= 2 {
            return links;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "never speed-tested: {table:?}"
        );
        tokio::time::sleep(ms(20)).await;
    }
}

fn two_links() -> Fake {
    Fake::scripted(vec![vec![wifi(), ethernet()]])
}

/// Rebalanced every 100 ms, so a test sees connections move.
fn quick() -> DownloadConfig {
    DownloadConfig {
        rebalance_every: ms(100),
        ..config()
    }
}

/// `name`'s share in each status while downloading, once it is wanted.
fn shares<'a>(
    history: &'a [(tokio::time::Instant, DownloadStatus)],
    name: &'a str,
) -> impl Iterator<Item = &'a barrage::fetch::LinkShare> + 'a {
    history
        .iter()
        .filter(|(_, s)| s.phase == Phase::Downloading)
        .filter_map(move |(_, s)| s.links.iter().find(|l| l.name == name))
        .filter(|l| l.wanted > 0)
}

#[tokio::test]
async fn a_faster_link_gets_more_connections() {
    let net = two_links()
        .speed(WIFI, 40_000_000)
        .speed(ETHERNET, 5_000_000);
    let links = speed_tested(net, 2).await;
    let config = DownloadConfig {
        connections: 16,
        most_per_link: 8,
        ..config()
    };
    const LENGTH: u64 = 24 * MIB;
    let file = File::new(LENGTH).rate(2 * MIB);
    let (_server, dir, downloads, id) = start_with("faster.bin", file, &links, config).await;
    let history = record(&downloads, id);

    let end = ended(&downloads, id).await;
    assert_eq!(end.phase, Phase::Done, "{end:?}");
    assert_same(&dir.read("faster.bin"), LENGTH);
    let history = history.await.unwrap();
    // 16 by 40 to 5: 14 and 2, the Wi-Fi's capped at 8 and its six over
    // left unused rather than given to the slow one.
    for (name, want) in [("Wi-Fi", 8), ("Ethernet 2", 2)] {
        let wanted: Vec<u32> = shares(&history, name).map(|l| l.wanted).collect();
        assert!(
            !wanted.is_empty() && wanted.iter().all(|&n| n == want),
            "{name}: {wanted:?}"
        );
        let most = shares(&history, name).map(|l| l.connections).max();
        assert_eq!(most, Some(want), "{name}");
    }
}

/// Review Focus 3: a link whose speed test failed counts as the average.
#[tokio::test]
async fn a_link_never_measured_still_gets_connections() {
    let links = speed_tested(two_links().failing_samples(ETHERNET), 1).await;
    const LENGTH: u64 = 16 * MIB;
    let file = File::new(LENGTH).rate(2 * MIB);
    let (_server, dir, downloads, id) = start_with("unmeasured.bin", file, &links, config()).await;
    let history = record(&downloads, id);

    let end = ended(&downloads, id).await;
    assert_eq!(end.phase, Phase::Done, "{end:?}");
    assert_same(&dir.read("unmeasured.bin"), LENGTH);
    assert!(share(&end, "Ethernet 2").bytes > 0, "{end:?}");
    let history = history.await.unwrap();
    let wanted: Vec<u32> = shares(&history, "Ethernet 2").map(|l| l.wanted).collect();
    assert!(
        !wanted.is_empty() && wanted.iter().all(|&n| n == 4),
        "{wanted:?}"
    );
    let most = shares(&history, "Ethernet 2").map(|l| l.connections).max();
    assert_eq!(most, Some(4));
}

/// The Ethernet served at a quarter of the Wi-Fi's speed a connection, both
/// speed tests alike: once it has carried, it is wanted fewer connections,
/// and each status moves a link by one at the most.
async fn slow_ethernet(
    name: &str,
) -> (Vec<(tokio::time::Instant, DownloadStatus)>, DownloadStatus) {
    const LENGTH: u64 = 32 * MIB;
    let links = both();
    let file = File::new(LENGTH).rate(2 * MIB).rate_for(ETHERNET, MIB / 2);
    let (_server, dir, downloads, id) = start_with(name, file, &links, quick()).await;
    let history = record(&downloads, id);
    let end = ended(&downloads, id).await;
    assert_eq!(end.phase, Phase::Done, "{end:?}");
    assert_same(&dir.read(name), LENGTH);
    (history.await.unwrap(), end)
}

#[tokio::test]
async fn what_a_link_delivers_moves_its_connections_one_at_a_time() {
    let (history, _) = slow_ethernet("delivers.bin").await;
    let fewer = history.iter().any(|(_, s)| {
        let wanted = |name| s.links.iter().find(|l| l.name == name).map(|l| l.wanted);
        matches!((wanted("Ethernet 2"), wanted("Wi-Fi")), (Some(e), Some(w)) if e > 0 && e < w)
    });
    assert!(fewer, "the Ethernet was never wanted fewer than the Wi-Fi");
    for name in ["Wi-Fi", "Ethernet 2"] {
        // From the first connections it started with, a step at a time.
        let counts: Vec<u32> = shares(&history, name)
            .map(|l| l.connections)
            .skip_while(|&n| n == 0)
            .collect();
        let jumps: Vec<(u32, u32)> = counts
            .windows(2)
            .filter(|w| w[0].abs_diff(w[1]) > 1)
            .map(|w| (w[0], w[1]))
            .collect();
        assert!(
            jumps.is_empty(),
            "{name} moved by more than one: {jumps:?} in {counts:?}"
        );
    }
}

/// Both links find the file at about the same moment, in either order: the
/// one still finding it already counts, so the first starts with its own
/// share, not every connection a link may hold, and the second starts with
/// its share at once rather than one at a time as the first gives them up.
#[tokio::test]
async fn each_link_starts_with_its_share_whichever_finds_the_file_first() {
    const LENGTH: u64 = 24 * MIB;
    let links = both();
    let config = DownloadConfig {
        most_per_link: 8,
        ..quick()
    };
    let file = File::new(LENGTH).rate(2 * MIB);
    let (_server, dir, downloads, id) = start_with("first.bin", file, &links, config).await;
    let history = record(&downloads, id);

    let end = ended(&downloads, id).await;
    assert_eq!(end.phase, Phase::Done, "{end:?}");
    assert_same(&dir.read("first.bin"), LENGTH);
    let history = history.await.unwrap();
    for name in ["Wi-Fi", "Ethernet 2"] {
        let first = history
            .iter()
            .filter_map(|(_, s)| s.links.iter().find(|l| l.name == name))
            .map(|l| l.connections)
            .find(|&n| n > 0);
        assert_eq!(first, Some(4), "{name}");
    }
}

/// A link that rests hands its share to the other one connection a
/// rebalance, never all at once: 4 to 8 in four steps.
#[tokio::test]
async fn a_resting_links_share_moves_over_one_connection_at_a_time() {
    const LENGTH: u64 = 24 * MIB;
    let links = both();
    let config = DownloadConfig {
        most_per_link: 8,
        rebalance_every: ms(500),
        // Out for longer than the download takes.
        rest_first: Duration::from_secs(30),
        rest_max: Duration::from_secs(30),
        ..config()
    };
    // Every first connection of the Ethernet gets a 502: none heard, so the
    // link rests.
    let file = File::new(LENGTH)
        .rate(MIB)
        .refuse_with(ETHERNET, Some(4), 502, None);
    let (_server, dir, downloads, id) = start_with("moves.bin", file, &links, config).await;
    let history = record(&downloads, id);

    let end = ended(&downloads, id).await;
    assert_eq!(end.phase, Phase::Done, "{end:?}");
    assert_same(&dir.read("moves.bin"), LENGTH);
    assert_eq!(share(&end, "Ethernet 2").bytes, 0, "{end:?}");
    let history = history.await.unwrap();
    let counts: Vec<u32> = shares(&history, "Wi-Fi")
        .map(|l| l.connections)
        .skip_while(|&n| n == 0)
        .collect();
    assert_eq!(counts.iter().max(), Some(&8), "{counts:?}");
    let jumps: Vec<(u32, u32)> = counts
        .windows(2)
        .filter(|w| w[0].abs_diff(w[1]) > 1)
        .map(|w| (w[0], w[1]))
        .collect();
    assert!(
        jumps.is_empty(),
        "moved by more than one: {jumps:?} in {counts:?}"
    );
}

#[tokio::test]
async fn a_retired_connection_hands_its_rest_back() {
    let (history, end) = slow_ethernet("retired.bin").await;
    let retired = shares(&history, "Ethernet 2").any(|l| l.connections < 4);
    assert!(retired, "no connection of the Ethernet retired");
    // Each share counts what its own link wrote: adding up to exactly the
    // file, what a retired connection gave back was fetched once, by another.
    assert_eq!(end.links.iter().map(|l| l.bytes).sum::<u64>(), 32 * MIB);
}

#[tokio::test]
async fn a_server_ceiling_still_caps_a_link() {
    // One answer at a time for the Ethernet, at four times the Wi-Fi's speed
    // a connection: its single connection is never cut for a Wi-Fi one, so
    // it never drops an answer and asks again while the server still counts
    // the old one.
    const LENGTH: u64 = 24 * MIB;
    let links = both();
    let file = File::new(LENGTH)
        .rate(2 * MIB)
        .rate_for(ETHERNET, 8 * MIB)
        .cap(ETHERNET, 1, None);
    let (server, dir, downloads, id) = start_with("ceiling.bin", file, &links, quick()).await;
    let history = record(&downloads, id);

    let end = ended(&downloads, id).await;
    assert_eq!(end.phase, Phase::Done, "{end:?}");
    assert_same(&dir.read("ceiling.bin"), LENGTH);
    // Three of its four refused in the first round, and never asked again
    // for more than the one the server allows.
    let refused = server
        .seen()
        .iter()
        .filter(|s| s.source == ip(ETHERNET) && s.status == 429)
        .count();
    assert!(refused <= 3, "{refused} refusals");
    let history = history.await.unwrap();
    // Its ceiling falls with each refusal, 3, 2, then 1, and stays.
    let one = "the server allows 1 connection on this cable";
    let after: Vec<u32> = shares(&history, "Ethernet 2")
        .skip_while(|l| l.note.as_deref() != Some(one))
        .map(|l| l.wanted)
        .collect();
    assert!(
        !after.is_empty() && after.iter().all(|&n| n == 1),
        "{after:?}"
    );
    // The Wi-Fi keeps its four: on a loaded machine its figure dips for a
    // moment under the Ethernet's one connection, so a moment at 2 is
    // allowed, never 1, never more than its most.
    let wifi: Vec<u32> = shares(&history, "Wi-Fi").map(|l| l.wanted).collect();
    assert!(wifi.contains(&4), "{wifi:?}");
    assert!(wifi.iter().all(|&n| (2..=4).contains(&n)), "{wifi:?}");
}

/// The public address both cables show the internet below: the phone joined
/// to the laptop's own Wi-Fi and tethered.
const SHARED: &str = "198.51.100.7";

const BEHIND_WIFI: &str = "sits out: it reaches the internet through the same connection as \
                           Wi-Fi, so using both gains nothing";

/// The Wi-Fi (40 Mbps) and Ethernet 2 (5 Mbps) behind one public address
/// over `net`, once both are up, speed-tested and their public addresses
/// known: a download sees them as one path from the start.
async fn one_connection(net: Fake) -> LinkHandle {
    behind_one(net, &[]).await
}

/// As [`one_connection`], with `more` addresses behind the same public
/// address.
async fn behind_one(net: Fake, more: &[&str]) -> LinkHandle {
    let mut net = net
        .speed(WIFI, 40_000_000)
        .speed(ETHERNET, 5_000_000)
        .public(WIFI, &[Some(SHARED)])
        .public(ETHERNET, &[Some(SHARED)]);
    for address in more {
        net = net.public(address, &[Some(SHARED)]);
    }
    let net = Arc::new(net.loopback_links());
    let links = LinkHandle::start_with(net, WatchConfig::every(ms(20))).unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let table = links.table().latest();
        let ready = |l: &&barrage::links::Link| {
            l.state.is_usable() && l.down_bps.is_some() && l.public_ip.is_some()
        };
        if table.iter().filter(ready).count() == 2 + more.len() {
            return links;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "never ready: {table:?}"
        );
        tokio::time::sleep(ms(20)).await;
    }
}

/// The pieces asked of the server through `source`, its probe aside.
fn pieces_through(seen: &[serve::Seen], source: &str) -> usize {
    seen.iter()
        .filter(|s| s.source == ip(source) && !is_probe(s))
        .count()
}

#[tokio::test]
async fn a_cable_sharing_the_wifis_connection_sits_out() {
    let links = one_connection(two_links()).await;
    const LENGTH: u64 = 16 * MIB;
    let file = File::new(LENGTH).rate(2 * MIB);
    let (server, dir, downloads, id) = start_with("one-path.bin", file, &links, quick()).await;
    let history = record(&downloads, id);

    let end = ended(&downloads, id).await;
    assert_eq!(end.phase, Phase::Done, "{end:?}");
    assert_same(&dir.read("one-path.bin"), LENGTH);
    let ethernet = share(&end, "Ethernet 2");
    assert_eq!(ethernet.bytes, 0, "{end:?}");
    assert_eq!(ethernet.note.as_deref(), Some(BEHIND_WIFI));
    assert_eq!(share(&end, "Wi-Fi").bytes, LENGTH);
    assert_eq!(pieces_through(&server.seen(), ETHERNET), 0);
    let history = history.await.unwrap();
    for (_, status) in &history {
        if let Some(e) = status.links.iter().find(|l| l.name == "Ethernet 2") {
            assert_eq!((e.connections, e.wanted), (0, 0), "{status:?}");
        }
    }
    // The path's share is all the Wi-Fi's: as many as one link may hold.
    let wifi: Vec<u32> = shares(&history, "Wi-Fi").map(|l| l.wanted).collect();
    assert!(!wifi.is_empty() && wifi.iter().all(|&n| n == 4), "{wifi:?}");
}

/// The Wi-Fi goes mid-download: the Ethernet, on the same connection until
/// then, carries the rest, starting with all it is wanted at once, as a link
/// joining does, not one connection a rebalance (every 2 s here).
#[tokio::test]
async fn when_the_carrier_goes_the_other_cable_takes_over() {
    let (net, live) = Fake::live(vec![wifi(), ethernet()]);
    let links = one_connection(net).await;
    const LENGTH: u64 = 24 * MIB;
    let file = File::new(LENGTH).rate(MIB);
    let (_server, dir, downloads, id) = start_with("takeover.bin", file, &links, config()).await;
    let history = record(&downloads, id);

    wait_for(&downloads, id, "4 MiB down", |s| s.done >= 4 * MIB).await;
    live.set(vec![ethernet()]);

    let end = ended(&downloads, id).await;
    assert_eq!(end.phase, Phase::Done, "{end:?}");
    assert_same(&dir.read("takeover.bin"), LENGTH);
    let ethernet = share(&end, "Ethernet 2");
    assert!(ethernet.bytes > 0, "{end:?}");
    assert!(share(&end, "Wi-Fi").bytes > 0, "{end:?}");
    // No longer sitting out (a connection that failed on the way may leave
    // a note of its own).
    assert_ne!(ethernet.note.as_deref(), Some(BEHIND_WIFI));
    let history = history.await.unwrap();
    let first = history
        .iter()
        .filter_map(|(_, s)| s.links.iter().find(|l| l.name == "Ethernet 2"))
        .map(|l| l.connections)
        .find(|&n| n > 0);
    assert_eq!(first, Some(4));
}

/// The server asks the Wi-Fi to wait 55 minutes: it sits the download out,
/// and the Ethernet behind it carries the file rather than the download
/// failing with nobody holding a connection.
#[tokio::test]
async fn a_carrier_the_server_turns_away_hands_its_path_over() {
    let links = one_connection(two_links()).await;
    const LENGTH: u64 = 16 * MIB;
    let file = File::new(LENGTH)
        .rate(2 * MIB)
        .refuse(WIFI, None, Some("3311"));
    let (_server, dir, downloads, id) = start_with("handed.bin", file, &links, quick()).await;

    let end = ended(&downloads, id).await;
    assert_eq!(end.phase, Phase::Done, "{end:?}");
    assert_same(&dir.read("handed.bin"), LENGTH);
    let wifi = share(&end, "Wi-Fi");
    assert_eq!(wifi.bytes, 0, "{end:?}");
    assert_eq!(
        wifi.note.as_deref(),
        Some("sits out: the server asked this cable to wait 55 min 11 s, longer than 1 min")
    );
    let ethernet = share(&end, "Ethernet 2");
    assert_eq!(ethernet.bytes, LENGTH, "{end:?}");
    assert_ne!(ethernet.note.as_deref(), Some(BEHIND_WIFI));
}

/// A second address on the Ethernet that cannot find the file is not taking
/// part, so it keeps its own note: only the addresses taking part sit out
/// behind the Wi-Fi, and a note of their own is not lost when they stop.
#[tokio::test]
async fn an_address_not_taking_part_keeps_its_own_note() {
    const SECOND: &str = "127.0.0.3";
    let net = Fake::scripted(vec![vec![
        wifi(),
        ethernet(),
        cand_idx(2, "Ethernet 2", SECOND, true),
    ]]);
    let links = behind_one(net, &[SECOND]).await;
    const LENGTH: u64 = 16 * MIB;
    let file = File::new(LENGTH)
        .rate(2 * MIB)
        .refuse_probe(SECOND, 99, 400, None);
    let (_server, dir, downloads, id) = start_with("own-note.bin", file, &links, quick()).await;

    let end = ended(&downloads, id).await;
    assert_eq!(end.phase, Phase::Done, "{end:?}");
    assert_same(&dir.read("own-note.bin"), LENGTH);
    let notes: Vec<Option<&str>> = end
        .links
        .iter()
        .filter(|l| l.name == "Ethernet 2")
        .map(|l| l.note.as_deref())
        .collect();
    assert_eq!(notes.len(), 2, "{end:?}");
    assert!(notes.contains(&Some(BEHIND_WIFI)), "{notes:?}");
    assert!(
        notes.iter().any(|n| n.is_some_and(|n| n.contains("400"))),
        "{notes:?}"
    );
}

/// The Wi-Fi and Ethernet 2, the Ethernet's probe answer set by the test
/// through the returned switch; probed every 50 ms, a failure retried after
/// 50 ms, trusted again after `cooldown`.
fn switchable(cooldown: Duration) -> (LinkHandle, fake::Switch) {
    // Each speed test one connection's figure, as a real one is: about the
    // 256 KiB/s the tests below serve each connection.
    let (net, probe) = two_links()
        .speed(WIFI, 2_000_000)
        .speed(ETHERNET, 2_000_000)
        .loopback_links()
        .switched(ETHERNET);
    let watch = WatchConfig {
        reprobe: ms(50),
        retry: ms(50),
        cooldown,
        ..WatchConfig::every(ms(20))
    };
    (LinkHandle::start_with(Arc::new(net), watch).unwrap(), probe)
}

/// The Ethernet's state as it changed, with when, `None` while it is not in
/// the table.
type States = Vec<(tokio::time::Instant, Option<LinkState>)>;

/// The Ethernet's state in every table the watcher publishes, with when it
/// was seen, until `stop` is set.
fn record_states(
    mut table: LinkTable,
) -> (tokio::task::JoinHandle<States>, Arc<tokio::sync::Notify>) {
    let stop = Arc::new(tokio::sync::Notify::new());
    let stopped = stop.clone();
    let task = tokio::spawn(async move {
        let mut seen = Vec::new();
        loop {
            let state = table
                .latest()
                .iter()
                .find(|l| l.name == "Ethernet 2")
                .map(|l| l.state);
            if seen.last().map(|(_, s)| *s) != Some(state) {
                seen.push((tokio::time::Instant::now(), state));
            }
            tokio::select! {
                open = table.changed() => if !open { return seen },
                () = stopped.notified() => return seen,
            }
        }
    });
    (task, stop)
}

async fn ethernet_is(links: &LinkHandle, state: LinkState) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let table = links.table().latest();
        if table
            .iter()
            .any(|l| l.name == "Ethernet 2" && l.state == state)
        {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "Ethernet 2 never {state:?}: {table:?}"
        );
        tokio::time::sleep(ms(10)).await;
    }
}

/// The Ethernet's connections from before it went down have ended.
async fn ethernet_holds_none(
    downloads: &barrage::fetch::Downloads,
    id: barrage::fetch::DownloadId,
) {
    wait_for(downloads, id, "the Ethernet's connections to end", |s| {
        s.links
            .iter()
            .filter(|l| l.name == "Ethernet 2")
            .all(|l| l.connections == 0)
    })
    .await;
}

/// The Ethernet stops answering mid-download and answers again once its
/// connections have ended: while it is degraded it holds one connection at
/// most, and once trusted, more. At 256 KiB/s a connection, so the file
/// outlasts the cooldown without moving many bytes through a test build.
#[tokio::test]
async fn a_link_back_from_down_holds_one_connection_until_trusted() {
    let (links, probe) = switchable(Duration::from_secs(1));
    let (states, stop) = record_states(links.table());
    const LENGTH: u64 = 10 * MIB;
    let file = File::new(LENGTH).rate(256 * KIB);
    let (_server, dir, downloads, id) = start_with("degraded.bin", file, &links, quick()).await;
    let history = record(&downloads, id);

    wait_for(&downloads, id, "the Ethernet carrying", |s| {
        s.links
            .iter()
            .any(|l| l.name == "Ethernet 2" && l.bytes >= MIB / 2)
    })
    .await;
    probe.set(ProbeVerdict::Broken);
    ethernet_is(&links, LinkState::Down).await;
    ethernet_holds_none(&downloads, id).await;
    probe.set(ProbeVerdict::Working);

    let end = ended(&downloads, id).await;
    assert_eq!(end.phase, Phase::Done, "{end:?}");
    assert_same(&dir.read("degraded.bin"), LENGTH);
    stop.notify_one();
    let states = states.await.unwrap();
    // The first time it is degraded, and the first time it is up after.
    let when = |state, after: tokio::time::Instant| {
        states
            .iter()
            .find(|(at, s)| *s == Some(state) && *at > after)
            .map(|(at, _)| *at)
            .unwrap_or_else(|| panic!("never {state:?}: {states:?}"))
    };
    let degraded = when(LinkState::Degraded, states[0].0 - ms(1));
    let up = when(LinkState::Up, degraded);
    let history = history.await.unwrap();
    let ethernet = |from: tokio::time::Instant, to: tokio::time::Instant| {
        history
            .iter()
            .filter(move |(at, _)| *at > from && *at < to)
            .filter_map(|(_, s)| s.links.iter().find(|l| l.name == "Ethernet 2"))
    };
    // Judged a little inside each window, so a status the download published
    // just before the table showed the change is not counted against it.
    let margin = ms(50);
    let while_degraded: Vec<(u32, u32)> = ethernet(degraded + margin, up - margin)
        .map(|l| (l.connections, l.wanted))
        .collect();
    assert!(
        while_degraded.iter().all(|&(c, w)| c <= 1 && w <= 1),
        "{while_degraded:?}"
    );
    assert!(
        while_degraded.iter().any(|&(c, _)| c == 1),
        "it never took part while degraded: {while_degraded:?}"
    );
    // Trusted, it takes its whole share at once, as a link joining does,
    // not one connection a rebalance.
    let after: Vec<u32> = ethernet(up, tokio::time::Instant::now())
        .map(|l| l.connections)
        .collect();
    assert_eq!(after.iter().find(|&&n| n > 1), Some(&4), "{after:?}");
    // And keeps it: what it carried held to one connection does not weigh it
    // once trusted. Both links are served alike a connection, so it is
    // wanted as many as the Wi-Fi again; on a loaded machine one link's
    // figure dips for a moment, so a moment at 2 is allowed, never 1.
    let settled: Vec<u32> = ethernet(up + ms(500), tokio::time::Instant::now())
        .filter(|l| l.connections > 0)
        .map(|l| l.wanted)
        .collect();
    assert!(settled.contains(&4), "{settled:?}");
    assert!(settled.iter().all(|&n| n >= 2), "{settled:?}");
}

/// The Ethernet down and back four times, inside one long cooldown: each
/// return costs it a request or two, not a connection's worth each. At
/// 256 KiB/s a connection, as above.
#[tokio::test]
async fn a_link_flapping_costs_a_request_or_two_per_return() {
    let (links, probe) = switchable(Duration::from_secs(60));
    const LENGTH: u64 = 10 * MIB;
    let file = File::new(LENGTH).rate(256 * KIB);
    let (server, dir, downloads, id) = start_with("flapping.bin", file, &links, quick()).await;

    wait_for(&downloads, id, "the Ethernet carrying", |s| {
        s.links
            .iter()
            .any(|l| l.name == "Ethernet 2" && l.bytes >= MIB / 2)
    })
    .await;
    const RETURNS: usize = 4;
    let mut before = 0;
    for round in 0..RETURNS {
        probe.set(ProbeVerdict::Broken);
        ethernet_is(&links, LinkState::Down).await;
        ethernet_holds_none(&downloads, id).await;
        if round == 0 {
            before = pieces_through(&server.seen(), ETHERNET);
        }
        probe.set(ProbeVerdict::Working);
        ethernet_is(&links, LinkState::Degraded).await;
        wait_for(&downloads, id, "the Ethernet back with a connection", |s| {
            s.links
                .iter()
                .any(|l| l.name == "Ethernet 2" && l.connections == 1)
        })
        .await;
        tokio::time::sleep(ms(200)).await;
    }

    let end = ended(&downloads, id).await;
    assert_eq!(end.phase, Phase::Done, "{end:?}");
    assert_same(&dir.read("flapping.bin"), LENGTH);
    let since = pieces_through(&server.seen(), ETHERNET) - before;
    assert!(
        since <= 2 * RETURNS,
        "{since} piece requests over {RETURNS} returns"
    );
}

/// The Ethernet with two addresses, as a phone's tether has: down, then back
/// on both. While it is degraded the cable holds one connection between its
/// addresses, not one each, since they go down together; trusted, both take
/// their share at once.
#[tokio::test]
async fn a_cable_back_from_down_holds_one_connection_over_all_its_addresses() {
    const SECOND: &str = "127.0.0.3";
    let (net, first) = Fake::scripted(vec![vec![
        wifi(),
        ethernet(),
        cand_idx(2, "Ethernet 2", SECOND, true),
    ]])
    .speed(WIFI, 2_000_000)
    .speed(ETHERNET, 2_000_000)
    .speed(SECOND, 2_000_000)
    .loopback_links()
    .switched(ETHERNET);
    let (net, second) = net.switched(SECOND);
    let watch = WatchConfig {
        reprobe: ms(50),
        retry: ms(50),
        cooldown: Duration::from_millis(1500),
        ..WatchConfig::every(ms(20))
    };
    let links = LinkHandle::start_with(Arc::new(net), watch).unwrap();
    let ethernet_all = |state| {
        let table = links.table().latest();
        let cable: Vec<_> = table.iter().filter(|l| l.name == "Ethernet 2").collect();
        cable.len() == 2 && cable.iter().all(|l| l.state == state)
    };
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !ethernet_all(LinkState::Up) {
        assert!(tokio::time::Instant::now() < deadline, "never both up");
        tokio::time::sleep(ms(10)).await;
    }
    const LENGTH: u64 = 12 * MIB;
    let file = File::new(LENGTH).rate(256 * KIB);
    let (_server, dir, downloads, id) = start_with("two-back.bin", file, &links, quick()).await;
    let history = record(&downloads, id);

    wait_for(&downloads, id, "the Ethernet carrying", |s| {
        s.links
            .iter()
            .filter(|l| l.name == "Ethernet 2")
            .map(|l| l.bytes)
            .sum::<u64>()
            >= MIB / 2
    })
    .await;
    first.set(ProbeVerdict::Broken);
    second.set(ProbeVerdict::Broken);
    while !ethernet_all(LinkState::Down) {
        tokio::time::sleep(ms(10)).await;
    }
    ethernet_holds_none(&downloads, id).await;
    first.set(ProbeVerdict::Working);
    second.set(ProbeVerdict::Working);
    while !ethernet_all(LinkState::Degraded) {
        tokio::time::sleep(ms(10)).await;
    }
    let degraded = tokio::time::Instant::now();
    while !ethernet_all(LinkState::Up) {
        tokio::time::sleep(ms(10)).await;
    }
    let up = tokio::time::Instant::now();

    let end = ended(&downloads, id).await;
    assert_eq!(end.phase, Phase::Done, "{end:?}");
    assert_same(&dir.read("two-back.bin"), LENGTH);
    let history = history.await.unwrap();
    // The cable's connections and wanted, its two addresses added.
    let cable = |from: tokio::time::Instant, to: tokio::time::Instant| -> Vec<(u32, u32)> {
        history
            .iter()
            .filter(|(at, s)| *at > from && *at < to && s.phase == Phase::Downloading)
            .map(|(_, s)| {
                let ethernet = s.links.iter().filter(|l| l.name == "Ethernet 2");
                ethernet.fold((0, 0), |(c, w), l| (c + l.connections, w + l.wanted))
            })
            .collect()
    };
    let margin = ms(50);
    let while_degraded = cable(degraded + margin, up - margin);
    assert!(
        while_degraded.iter().all(|&(c, w)| c <= 1 && w <= 1),
        "{while_degraded:?}"
    );
    assert!(
        while_degraded.iter().any(|&(c, _)| c == 1),
        "it never took part while degraded: {while_degraded:?}"
    );
    // Trusted, both addresses take their share at once: the cable's half of
    // the eight, two each, not the one holding growing and the other
    // joining one at a time.
    let after: Vec<u32> = cable(up, tokio::time::Instant::now())
        .iter()
        .map(|&(c, _)| c)
        .collect();
    assert_eq!(after.iter().find(|&&n| n > 1), Some(&4), "{after:?}");
}

/// What `cable` has received, every download together.
fn used_by(downloads: &Downloads, cable: &str) -> u64 {
    downloads
        .used()
        .borrow()
        .iter()
        .find(|u| u.cable == cable)
        .map_or(0, |u| u.bytes)
}

/// What `cable` wrote of the download, 0 if it took no part.
fn wrote(status: &DownloadStatus, cable: &str) -> u64 {
    status
        .links
        .iter()
        .filter(|l| l.name == cable)
        .map(|l| l.bytes)
        .sum()
}

#[tokio::test]
async fn each_cable_counts_what_it_carried() {
    let links = both();
    const LENGTH: u64 = 4 * MIB;
    let (server, _dir, downloads, id) = start("used.bin", File::new(LENGTH), &links).await;
    let end = ended(&downloads, id).await;
    assert_eq!(end.phase, Phase::Done, "{end:?}");

    let seen = server.seen();
    let used: Vec<CableUse> = downloads.used().borrow().clone();
    for (cable, source) in [("Wi-Fi", WIFI), ("Ethernet 2", ETHERNET)] {
        let got = used_by(&downloads, cable);
        assert!(got >= wrote(&end, cable), "{cable}: {used:?} {end:?}");
        // Nothing arrives that the server did not send.
        assert!(got <= served(&seen, source), "{cable}: {used:?}");
    }
    assert!(
        used.iter().map(|u| u.bytes).sum::<u64>() >= LENGTH,
        "{used:?}"
    );
}

/// Review Focus 4. The Ethernet four times slower a connection, so the
/// Wi-Fi cuts its claims while their answers run on: what arrives past a cut
/// is thrown away, but it came down the cable, so it counts. Each answer
/// dropped before its end had at most a piece or two still on its way (the
/// server sends 64 KiB at a time, paced), which never arrived.
#[tokio::test]
async fn bytes_past_a_cut_are_counted_as_used() {
    let links = both();
    const LENGTH: u64 = 4 * MIB + 12_345;
    let file = File::new(LENGTH)
        .rate_for(WIFI, MIB)
        .rate_for(ETHERNET, 256 * KIB);
    let (server, _dir, downloads, id) = start("cut.bin", file, &links).await;
    let end = ended(&downloads, id).await;
    assert_eq!(end.phase, Phase::Done, "{end:?}");

    let seen = server.seen();
    let used: Vec<CableUse> = downloads.used().borrow().clone();
    for (cable, source) in [("Wi-Fi", WIFI), ("Ethernet 2", ETHERNET)] {
        let dropped = seen
            .iter()
            .filter(|s| s.source == ip(source) && !is_probe(s))
            .filter(|s| s.asked().is_some_and(|(a, b)| s.sent() < b + 1 - a))
            .count() as u64;
        let got = used_by(&downloads, cable);
        let sent = served(&seen, source);
        assert!(
            got + dropped * 128 * KIB >= sent,
            "{cable}: used {got}, sent {sent}, {dropped} answers dropped: {used:?}"
        );
        assert!(got <= sent, "{cable}: used {got}, sent {sent}");
    }
    let total: u64 = used.iter().map(|u| u.bytes).sum();
    assert!(total > LENGTH, "nothing past a cut was counted: {used:?}");
}

/// One count per cable for the engine, every download adding to it.
#[tokio::test]
async fn two_downloads_add_to_one_count() {
    let links = both();
    let server = Server::start().await;
    server.at("/one.bin", Answer::File(File::new(2 * MIB)));
    server.at("/two.bin", Answer::File(File::new(3 * MIB)));
    let dir = Folder::new("two-downloads");
    let downloads = Downloads::with_config(links.table(), dir.0.clone(), config());
    let one = downloads
        .add(request(&server.url("/one.bin")))
        .await
        .unwrap();
    let two = downloads
        .add(request(&server.url("/two.bin")))
        .await
        .unwrap();
    let (one, two) = (ended(&downloads, one).await, ended(&downloads, two).await);
    assert_eq!((one.phase, two.phase), (Phase::Done, Phase::Done));

    let used: Vec<CableUse> = downloads.used().borrow().clone();
    let mut names: Vec<&str> = used.iter().map(|u| u.cable.as_str()).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), used.len(), "a cable counted twice: {used:?}");
    for cable in ["Wi-Fi", "Ethernet 2"] {
        let both_wrote = wrote(&one, cable) + wrote(&two, cable);
        assert!(
            used_by(&downloads, cable) >= both_wrote,
            "{cable}: {used:?}"
        );
    }
    assert!(
        used.iter().map(|u| u.bytes).sum::<u64>() >= 5 * MIB,
        "{used:?}"
    );
}
