//! A link failing mid-download (Task 9), and a server that limits
//! connections with `429 Too Many Requests` (Task 10), against the scripted
//! server and network as in fetch_download.rs. The times are short: a
//! connection is given up after 300 ms without data, a fresh one takes its
//! place 100 ms later, and a link whose every connection failed rests
//! 200 ms, doubling up to 1 s.

mod fake;
mod fetching;
mod serve;

use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use barrage::LinkHandle;
use barrage::fetch::{DownloadConfig, DownloadStatus, LinkShare, Phase, retry_after};
use barrage::links::probe::ProbeVerdict;
use barrage::links::watch::WatchConfig;
use fake::{Fake, ms};
use fetching::*;
use serve::{File, Seen, most_at_once};

/// Short failure times, so a test takes a few seconds.
fn failing() -> DownloadConfig {
    DownloadConfig {
        read_deadline: ms(300),
        retry_connection: ms(100),
        rest_first: ms(200),
        rest_max: ms(1000),
        ..config()
    }
}

/// 40 MiB at 4 MiB a second per connection: every worker of both links busy
/// for a second or two.
const LENGTH: u64 = 40 * MIB;

fn busy() -> File {
    File::new(LENGTH).rate(4 * MIB)
}

/// Ethernet 2's share in `status`, once it has one.
fn ethernet_in(status: &DownloadStatus) -> Option<&LinkShare> {
    status.links.iter().find(|l| l.name == "Ethernet 2")
}

/// Every note Ethernet 2 showed while the download ran.
fn ethernet_notes(history: &[(tokio::time::Instant, DownloadStatus)]) -> Vec<String> {
    history
        .iter()
        .filter_map(|(_, s)| ethernet_in(s)?.note.clone())
        .collect()
}

/// Ethernet 2's file requests, its probe aside, in the order they started.
fn ethernet_pieces(seen: &[Seen]) -> Vec<&Seen> {
    let mut pieces: Vec<&Seen> = seen
        .iter()
        .filter(|s| s.source == ip(ETHERNET) && !is_probe(s))
        .collect();
    pieces.sort_by_key(|s| s.started);
    pieces
}

#[tokio::test]
async fn one_stalled_connection_is_retried_while_its_link_keeps_working() {
    let links = both();
    let file = busy().stalls(ETHERNET, 256 * KIB, Some(1));
    // A full second for the deadline: on a loaded machine a healthy
    // connection can go 300 ms unscheduled, which would rightly read as
    // a link with no data.
    let patient = DownloadConfig {
        read_deadline: ms(1000),
        ..failing()
    };
    let (server, dir, downloads, id) = start_with("one-stall.bin", file, &links, patient).await;
    let history = record(&downloads, id);

    let end = ended(&downloads, id).await;
    assert_eq!(end.phase, Phase::Done, "{end:?}");
    assert_same(&dir.read("one-stall.bin"), LENGTH);
    let seen = server.seen();
    let stalled = seen
        .iter()
        .find(|s| s.stall.is_some())
        .expect("nothing stalled");
    assert_eq!(stalled.source, ip(ETHERNET));

    // A bad connection, not a bad link: Ethernet 2 never rested or said so,
    // and once it had its four connections it never fell below three.
    let history = history.await.unwrap();
    let notes = ethernet_notes(&history);
    assert!(notes.is_empty(), "{notes:?}");
    let counts: Vec<u32> = history
        .iter()
        .filter(|(_, s)| s.phase == Phase::Downloading)
        .filter_map(|(_, s)| ethernet_in(s).map(|l| l.connections))
        .skip_while(|&n| n < 4)
        .collect();
    assert!(!counts.is_empty(), "Ethernet 2 never had four connections");
    assert!(counts.iter().all(|&n| n >= 3), "{counts:?}");
    // Once the stalled one was given up it was three, then a fresh
    // connection made it four again.
    let given_up = tokio::time::Instant::from_std(stalled.started + ms(1000));
    let after: Vec<u32> = history
        .iter()
        .filter(|(at, s)| *at >= given_up && s.phase == Phase::Downloading)
        .filter_map(|(_, s)| ethernet_in(s).map(|l| l.connections))
        .collect();
    let dip = after.iter().position(|&n| n == 3);
    assert!(dip.is_some_and(|at| after[at..].contains(&4)), "{after:?}");
}

#[tokio::test]
async fn a_link_whose_every_connection_stalls_rests_and_its_work_moves() {
    let links = both();
    let file = busy().stalls(ETHERNET, 0, None);
    let (server, dir, downloads, id) = start_with("all-stall.bin", file, &links, failing()).await;
    let history = record(&downloads, id);

    let end = ended(&downloads, id).await;
    assert_eq!(end.phase, Phase::Done, "{end:?}");
    assert_same(&dir.read("all-stall.bin"), LENGTH);
    // Its work moved to the Wi-Fi.
    assert_eq!(share(&end, "Ethernet 2").bytes, 0);
    assert_eq!(share(&end, "Wi-Fi").bytes, LENGTH);
    // It said why while it rested.
    let notes = ethernet_notes(&history.await.unwrap());
    assert!(
        notes
            .iter()
            .any(|n| n.starts_with("no data from the server for 300 ms")),
        "{notes:?}"
    );

    // Each rest in a row twice the last, as the notes said them.
    let mut rests = notes.clone();
    rests.dedup();
    assert!(rests.len() >= 2, "{rests:?}");
    assert!(
        rests[0].ends_with("it rests 200 ms and tries again"),
        "{rests:?}"
    );
    assert!(
        rests[1].ends_with("it rests 400 ms and tries again"),
        "{rests:?}"
    );

    // Four connections at first; after a rest, one tries at a time.
    let seen = server.seen();
    let pieces: Vec<Seen> = ethernet_pieces(&seen).into_iter().cloned().collect();
    assert!(pieces.len() >= 5, "{} requests", pieces.len());
    assert_eq!(most_at_once(&pieces[..4], ip(ETHERNET), |_| true), 4);
    assert_eq!(most_at_once(&pieces[4..], ip(ETHERNET), |_| true), 1);
}

#[tokio::test]
async fn a_rested_link_that_works_again_gets_its_connections_back() {
    let links = both();
    // Its first four answers stall; after that the link works.
    let file = busy().stalls(ETHERNET, 0, Some(4));
    let (_server, dir, downloads, id) = start_with("recovers.bin", file, &links, failing()).await;
    let history = record(&downloads, id);

    let end = ended(&downloads, id).await;
    assert_eq!(end.phase, Phase::Done, "{end:?}");
    assert_same(&dir.read("recovers.bin"), LENGTH);
    let ethernet = share(&end, "Ethernet 2");
    assert!(ethernet.bytes > 0, "{ethernet:?}");
    // It rested, then its one trying connection worked, so all four came
    // back and its note went.
    let history = history.await.unwrap();
    let notes = ethernet_notes(&history);
    assert!(notes.iter().any(|n| n.contains("rests")), "{notes:?}");
    let rested = history
        .iter()
        .position(|(_, s)| ethernet_in(s).is_some_and(|l| l.note.is_some()))
        .unwrap();
    let back = history[rested..]
        .iter()
        .filter_map(|(_, s)| ethernet_in(s))
        .find(|l| l.connections == 4)
        .expect("Ethernet 2 never got its four connections back");
    // As soon as the trying connection received bytes, not once it had
    // fetched its whole 4 MiB chunk.
    assert!(back.bytes < 4 * MIB, "{back:?}");
    assert_eq!(ethernet.note, None);
}

/// With one connection there is no other to vouch for the link, so its
/// failure is the link's.
#[tokio::test]
async fn with_one_connection_a_failure_rests_the_link() {
    let links = both();
    let file = busy().cut(ETHERNET, 256 * KIB, 1);
    let alone = DownloadConfig {
        workers_per_link: 1,
        ..failing()
    };
    let (_server, dir, downloads, id) = start_with("one-each.bin", file, &links, alone).await;
    let history = record(&downloads, id);

    let end = ended(&downloads, id).await;
    assert_eq!(end.phase, Phase::Done, "{end:?}");
    assert_same(&dir.read("one-each.bin"), LENGTH);
    let notes = ethernet_notes(&history.await.unwrap());
    assert!(
        notes.iter().any(|n| n.starts_with("the answer broke off")
            && n.ends_with("it rests 200 ms and tries again")),
        "{notes:?}"
    );
}

/// A download whose only link rests is waiting for it, not stuck.
#[tokio::test]
async fn a_lone_link_that_rests_does_not_fail_the_download() {
    let links = links(vec![vec![wifi()]]);
    let file = File::new(8 * MIB).rate(4 * MIB).stalls(WIFI, 0, Some(4));
    let (_server, dir, downloads, id) = start_with("lone.bin", file, &links, failing()).await;
    let history = record(&downloads, id);

    let end = ended(&downloads, id).await;
    assert_eq!(end.phase, Phase::Done, "{end:?}");
    assert_same(&dir.read("lone.bin"), 8 * MIB);
    let rested = history.await.unwrap().iter().any(|(_, s)| {
        s.links
            .iter()
            .any(|l| l.note.as_deref().is_some_and(|n| n.contains("rests")))
    });
    assert!(rested, "the Wi-Fi never rested");
}

#[tokio::test]
async fn a_link_leaving_the_table_loses_no_written_bytes() {
    let (links, live) = live_links(vec![wifi(), ethernet()]);
    let (_server, dir, downloads, id) = start_with("leaves.bin", busy(), &links, failing()).await;
    // It goes with its connections open, partway through their chunks.
    wait_for(&downloads, id, "30% with Ethernet 2 carrying", |s| {
        s.done >= LENGTH * 3 / 10
            && ethernet_in(s).is_some_and(|l| l.bytes > 0 && l.connections > 0)
    })
    .await;
    live.set(vec![wifi()]);

    let end = ended(&downloads, id).await;
    assert_eq!(end.phase, Phase::Done, "{end:?}");
    assert_same(&dir.read("leaves.bin"), LENGTH);
    // What it wrote stays written. Each share counts the bytes its own link
    // wrote, so shares adding up to exactly the file mean no byte was written
    // twice: its unfinished chunks were picked up after the bytes it had.
    assert!(share(&end, "Ethernet 2").bytes > 0);
    assert_eq!(end.links.iter().map(|l| l.bytes).sum::<u64>(), LENGTH);
}

#[tokio::test]
async fn a_link_that_comes_back_rejoins() {
    let (links, live) = live_links(vec![wifi(), ethernet()]);
    // Half the usual rate, so the Wi-Fi alone is slow to finish: finding a
    // link again takes a while on a loaded machine.
    let file = File::new(LENGTH).rate(2 * MIB);
    let (server, dir, downloads, id) = start_with("returns.bin", file, &links, failing()).await;
    wait_for(&downloads, id, "30%", |s| s.done >= LENGTH * 3 / 10).await;
    live.set(vec![wifi()]);
    // Back as soon as it is seen gone, while most of the file is still to
    // fetch: on a loaded machine finding it again takes a while, and a link
    // that comes back to an empty pile has nothing to carry.
    wait_for(&downloads, id, "Ethernet 2 gone", |s| {
        ethernet_in(s).is_some_and(|l| l.connections == 0)
    })
    .await;
    live.set(vec![wifi(), ethernet()]);
    let back = std::time::Instant::now();

    let end = ended(&downloads, id).await;
    assert_eq!(end.phase, Phase::Done, "{end:?}");
    assert_same(&dir.read("returns.bin"), LENGTH);
    // Back under a new id: two shares by its name, the second with bytes of
    // its own.
    let shares: Vec<&LinkShare> = end
        .links
        .iter()
        .filter(|l| l.name == "Ethernet 2")
        .collect();
    assert_eq!(shares.len(), 2, "{end:?}");
    assert_ne!(shares[0].link, shares[1].link);
    assert!(shares[1].bytes > 0, "{:?}", shares[1]);
    assert!(
        ethernet_pieces(&server.seen())
            .iter()
            .any(|s| s.started > back),
        "the server never heard from Ethernet 2 again"
    );
}

/// A link that stops being usable without leaving the table, such as a
/// sign-in page appearing, keeps its id; working again, it takes part again.
#[tokio::test]
async fn a_link_that_stops_working_and_works_again_rejoins_as_itself() {
    let (net, probe) = Fake::scripted(vec![vec![wifi(), ethernet()]])
        .loopback_links()
        .switched(ETHERNET);
    // Probed every 50 ms, so the change shows at once.
    let watch = WatchConfig {
        reprobe: ms(50),
        ..WatchConfig::every(ms(20))
    };
    let links = LinkHandle::start_with(Arc::new(net), watch).unwrap();
    let file = File::new(LENGTH).rate(2 * MIB);
    let (server, dir, downloads, id) = start_with("blips.bin", file, &links, failing()).await;
    wait_for(&downloads, id, "30% with Ethernet 2 carrying", |s| {
        s.done >= LENGTH * 3 / 10 && ethernet_in(s).is_some_and(|l| l.bytes > 0)
    })
    .await;
    probe.set(ProbeVerdict::Portal);
    wait_for(&downloads, id, "Ethernet 2 stopped", |s| {
        ethernet_in(s).is_some_and(|l| l.connections == 0)
    })
    .await;
    probe.set(ProbeVerdict::Working);
    let back = std::time::Instant::now();

    let end = ended(&downloads, id).await;
    assert_eq!(end.phase, Phase::Done, "{end:?}");
    assert_same(&dir.read("blips.bin"), LENGTH);
    // One share for it, the same link all along, carrying again.
    let shares = end.links.iter().filter(|l| l.name == "Ethernet 2").count();
    assert_eq!(shares, 1, "{end:?}");
    assert!(
        ethernet_pieces(&server.seen())
            .iter()
            .any(|s| s.started > back),
        "Ethernet 2 never took part again"
    );
    assert_eq!(end.links.iter().map(|l| l.bytes).sum::<u64>(), LENGTH);
}

// A server that limits connections (Task 10).

/// Ethernet 2's file requests the server refused with 429.
fn refused(seen: &[Seen]) -> Vec<&Seen> {
    ethernet_pieces(seen)
        .into_iter()
        .filter(|s| s.status == 429)
        .collect()
}

#[test]
fn retry_after_reads_seconds_and_dates() {
    // 1,700,000,000 s after 1970 is Tue, 14 Nov 2023 22:13:20 GMT.
    let now = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let secs = Duration::from_secs;
    assert_eq!(retry_after("120", now), Some(secs(120)));
    assert_eq!(retry_after(" 0 ", now), Some(secs(0)));
    // An HTTP date, in each of the three forms HTTP allows.
    assert_eq!(
        retry_after("Tue, 14 Nov 2023 22:14:50 GMT", now),
        Some(secs(90))
    );
    assert_eq!(
        retry_after("Tuesday, 14-Nov-23 22:14:50 GMT", now),
        Some(secs(90))
    );
    assert_eq!(retry_after("Tue Nov 14 22:14:50 2023", now), Some(secs(90)));
    // A date already past is no wait at all.
    assert_eq!(
        retry_after("Tue, 14 Nov 2023 22:12:00 GMT", now),
        Some(Duration::ZERO)
    );
    // More seconds than any clock holds is as long as can be.
    assert_eq!(
        retry_after("99999999999999999999999", now),
        Some(Duration::MAX)
    );
    for junk in ["", "soon", "-5", "1.5", "12 seconds"] {
        assert_eq!(retry_after(junk, now), None, "{junk:?}");
    }
}

#[tokio::test]
async fn a_capped_link_settles_at_what_the_server_allows() {
    let links = both();
    let file = busy().cap(ETHERNET, 2, None);
    let (server, dir, downloads, id) = start_with("capped.bin", file, &links, failing()).await;

    let end = ended(&downloads, id).await;
    assert_eq!(end.phase, Phase::Done, "{end:?}");
    assert_same(&dir.read("capped.bin"), LENGTH);
    // Ethernet 2 came down to what the server allows, and says so; the
    // Wi-Fi, under no cap, kept its four.
    let ethernet = share(&end, "Ethernet 2");
    assert_eq!(
        ethernet.note.as_deref(),
        Some("the server allows 2 connections on this cable")
    );
    assert!(ethernet.bytes > 0);
    let seen = server.seen();
    let answered = |s: &Seen| !is_probe(s) && s.status == 206;
    assert_eq!(most_at_once(&seen, ip(ETHERNET), answered), 2);
    assert_eq!(most_at_once(&seen, ip(WIFI), answered), 4);
    // One refusal for each connection too many, and no more after.
    assert_eq!(refused(&seen).len(), 2);
}

#[tokio::test]
async fn a_short_retry_after_is_waited_out() {
    let links = both();
    // Each of its first four connections refused, asked to wait a second:
    // exactly the longest wait it will sit through.
    let file = busy().refuse(ETHERNET, Some(4), Some("1"));
    let config = DownloadConfig {
        max_retry_after: ms(1000),
        ..failing()
    };
    let (server, dir, downloads, id) = start_with("short-wait.bin", file, &links, config).await;
    let history = record(&downloads, id);

    let end = ended(&downloads, id).await;
    assert_eq!(end.phase, Phase::Done, "{end:?}");
    assert_same(&dir.read("short-wait.bin"), LENGTH);
    // Its last connection refused, it waited as asked, then came back with
    // all four connections, what the server allows learned again, and
    // carried its share, its note gone.
    let wait = "the server asked this cable to wait 1 s before trying again";
    let history = history.await.unwrap();
    let waited = history
        .iter()
        .position(|(_, s)| ethernet_in(s).is_some_and(|l| l.note.as_deref() == Some(wait)));
    let notes = ethernet_notes(&history);
    let waited = waited.unwrap_or_else(|| panic!("{notes:?}"));
    assert!(
        history[waited..]
            .iter()
            .filter_map(|(_, s)| ethernet_in(s))
            .any(|l| l.connections == 4),
        "it never had its four connections back"
    );
    let ethernet = share(&end, "Ethernet 2");
    assert!(ethernet.bytes > 0, "{ethernet:?}");
    assert_eq!(ethernet.note, None);
    let seen = server.seen();
    let refusals = refused(&seen);
    assert_eq!(refusals.len(), 4);
    let last = refusals.iter().map(|s| s.started).max().unwrap();
    let answered: Vec<&Seen> = ethernet_pieces(&seen)
        .into_iter()
        .filter(|s| s.status == 206)
        .collect();
    assert!(!answered.is_empty());
    assert!(
        answered.iter().all(|s| s.started >= last + ms(950)),
        "it came back before the second was up"
    );
}

#[tokio::test]
async fn a_long_retry_after_leaves_the_link_out() {
    let links = both();
    let file = busy().refuse(ETHERNET, None, Some("3311"));
    let (server, dir, downloads, id) = start_with("long-wait.bin", file, &links, failing()).await;

    let end = ended(&downloads, id).await;
    assert_eq!(end.phase, Phase::Done, "{end:?}");
    assert_same(&dir.read("long-wait.bin"), LENGTH);
    // Asked to wait 55 minutes, it sits the download out, and says why.
    let ethernet = share(&end, "Ethernet 2");
    assert_eq!(ethernet.bytes, 0);
    assert_eq!(
        ethernet.note.as_deref(),
        Some("sits out: the server asked this cable to wait 55 min 11 s, longer than 1 min")
    );
    assert_eq!(share(&end, "Wi-Fi").bytes, LENGTH);
    // Its four refused connections, and none after.
    let seen = server.seen();
    assert_eq!(ethernet_pieces(&seen).len(), 4);
    assert_eq!(refused(&seen).len(), 4);
}

/// With no wait named, a refused last connection is a link failing.
#[tokio::test]
async fn a_last_refusal_without_a_wait_rests_like_a_failing_link() {
    let links = both();
    let file = busy().refuse(ETHERNET, Some(4), None);
    let (_server, dir, downloads, id) = start_with("no-wait.bin", file, &links, failing()).await;
    let history = record(&downloads, id);

    let end = ended(&downloads, id).await;
    assert_eq!(end.phase, Phase::Done, "{end:?}");
    assert_same(&dir.read("no-wait.bin"), LENGTH);
    let notes = ethernet_notes(&history.await.unwrap());
    assert!(
        notes.iter().any(|n| n
            == "the server refused this cable's connections (429 Too Many Requests); it rests 200 ms and tries again"),
        "{notes:?}"
    );
    assert!(share(&end, "Ethernet 2").bytes > 0);
}
