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
use barrage::fetch::{DownloadConfig, DownloadStatus, Phase};
use barrage::links::watch::WatchConfig;
use fake::{Fake, ms};
use fetching::*;
use serve::File;

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
    let wifi: Vec<u32> = shares(&history, "Wi-Fi").map(|l| l.wanted).collect();
    assert!(wifi.iter().all(|&n| n == 4), "{wifi:?}");
}
