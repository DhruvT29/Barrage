//! A link failing mid-download, against the scripted server and network as
//! in fetch_download.rs. The times are short: a connection is given up after
//! 300 ms without data, a fresh one takes its place 100 ms later, and a link
//! whose every connection failed rests 200 ms, doubling up to 1 s.

mod fake;
mod fetching;
mod serve;

use barrage::fetch::{DownloadConfig, DownloadStatus, LinkShare, Phase};
use fake::ms;
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
    // fetched a whole 4 MiB chunk.
    assert!(back.bytes < 2 * MIB, "{back:?}");
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
    let (server, dir, downloads, id) = start_with("returns.bin", busy(), &links, failing()).await;
    wait_for(&downloads, id, "30%", |s| s.done >= LENGTH * 3 / 10).await;
    live.set(vec![wifi()]);
    wait_for(&downloads, id, "45% without Ethernet 2", |s| {
        s.done >= LENGTH * 45 / 100 && ethernet_in(s).is_some_and(|l| l.connections == 0)
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
