//! Fewer, longer requests (phase 4, Task 1): each connection asks for a long
//! span of the file, and one is cut in half for a connection with nothing to
//! do, so a download sends tens of requests, not one per 4 MiB. Hetzner's and
//! Vultr's speed servers limit requests per address (phase 3, Task 12), and
//! each request too many is a 429. Against the scripted server and network,
//! as in fetch_download.rs.

mod fake;
mod fetching;
mod serve;

use barrage::fetch::{DownloadConfig, Downloads, Phase};
use fake::ms;
use fetching::*;
use serve::{Answer, File, Seen, Server};

/// Short rests, so a link a server refused tries again within the test.
fn quick() -> DownloadConfig {
    DownloadConfig {
        retry_connection: ms(100),
        rest_first: ms(200),
        rest_max: ms(1000),
        ..config()
    }
}

/// `source`'s requests for the file's bytes, its probes aside, refused ones
/// included: what a server that limits requests counts.
fn pieces<'a>(seen: &'a [Seen], source: &str) -> Vec<&'a Seen> {
    seen.iter()
        .filter(|s| s.source == ip(source) && s.range.is_some() && !is_probe(s))
        .collect()
}

/// Every note a cable showed while the download ran.
fn notes_of(
    history: &[(tokio::time::Instant, barrage::fetch::DownloadStatus)],
    name: &str,
) -> Vec<String> {
    history
        .iter()
        .filter_map(|(_, s)| s.links.iter().find(|l| l.name == name)?.note.clone())
        .collect()
}

#[tokio::test]
async fn a_request_limited_server_is_asked_rarely() {
    // 128 MiB at 32 MiB a second per connection, from a server that allows
    // each address two requests at once and one more every 300 ms. In 4 MiB
    // chunks this file is 57 requests, about 28 an address.
    const LENGTH: u64 = 128 * MIB;
    let links = both();
    let every = ms(300);
    let file = File::new(LENGTH)
        .rate(32 * MIB)
        .limit(WIFI, 2, every)
        .limit(ETHERNET, 2, every);
    let (server, dir, downloads, id) = start_with("limited.bin", file, &links, quick()).await;

    let end = ended(&downloads, id).await;
    assert_eq!(end.phase, Phase::Done, "{end:?}");
    assert_same(&dir.read("limited.bin"), LENGTH);
    let seen = server.seen();
    for cable in [WIFI, ETHERNET] {
        let asked = pieces(&seen, cable).len();
        assert!(asked <= 16, "{cable} asked {asked} times");
    }
    // Both carried a real part of it.
    for name in ["Wi-Fi", "Ethernet 2"] {
        let bytes = share(&end, name).bytes;
        assert!(bytes >= LENGTH / 4, "{name} carried only {bytes}");
    }
}

#[tokio::test]
async fn the_end_of_a_file_splits_so_no_link_waits() {
    // The Wi-Fi sixteen times as fast as the Ethernet: halving the file
    // between them would leave the Wi-Fi idle while the Ethernet crawls.
    const LENGTH: u64 = 16 * MIB;
    let links = both();
    let file = File::new(LENGTH)
        .rate_for(WIFI, 16 * MIB)
        .rate_for(ETHERNET, MIB);
    let (_server, dir, downloads, id) = start_with("uneven.bin", file, &links, config()).await;

    let end = ended(&downloads, id).await;
    assert_eq!(end.phase, Phase::Done, "{end:?}");
    assert_same(&dir.read("uneven.bin"), LENGTH);
    // Halved between them, the Ethernet would carry 8 MiB; cut by bytes
    // left alone, about 4 (phase 4, Task 1), each of its connections keeping
    // a last part under 1 MiB, a second's work, while the Wi-Fi waited. By
    // their speeds, it keeps a seventeenth of what is left at each cut.
    let slow = share(&end, "Ethernet 2").bytes;
    assert!(
        slow < LENGTH / 8,
        "the slow link carried {slow} of {LENGTH}"
    );
}

#[tokio::test]
async fn a_link_joining_late_takes_part_of_what_is_left() {
    const LENGTH: u64 = 32 * MIB;
    let (links, live) = live_links(vec![wifi()]);
    let file = File::new(LENGTH).rate(4 * MIB);
    let (server, dir, downloads, id) = start_with("late.bin", file, &links, config()).await;

    wait_for(&downloads, id, "2 MiB through the Wi-Fi", |s| {
        s.done >= 2 * MIB
    })
    .await;
    live.set(vec![wifi(), ethernet()]);
    let end = ended(&downloads, id).await;
    assert_eq!(end.phase, Phase::Done, "{end:?}");
    assert_same(&dir.read("late.bin"), LENGTH);
    assert!(share(&end, "Ethernet 2").bytes > 0, "{end:?}");
    // Each share counts the bytes its own link wrote: adding up to exactly
    // the file, no byte was written twice, the cut halves taken over where
    // the owners stopped. What the server sent is not judged here: on a
    // quiet machine it is exactly the file, each owner stopping at its cut
    // as the paced answer reaches it; on a loaded one the connections fall
    // seconds behind, a whole answer can be waiting in the socket when its
    // claim is cut, and all of it past the cut is thrown away (the price of
    // a cut, PLAN.md 3.6). Task 6 counts the data each cable carries.
    assert_eq!(end.links.iter().map(|l| l.bytes).sum::<u64>(), LENGTH);
    let seen = server.seen();
    let pieces = pieces(&seen, WIFI).len() + pieces(&seen, ETHERNET).len();
    assert!(pieces <= 24, "{pieces} requests for parts of the file");
}

/// Review Focus 1: files smaller than a cut, and one resumed with many holes.
#[tokio::test]
async fn every_byte_once_for_odd_sizes_and_holes() {
    let links = both();
    for length in [1, 1000, 512 * KIB + 1] {
        let name = format!("odd-{length}.bin");
        let (server, dir, downloads, id) =
            start_with(&name, File::new(length), &links, config()).await;
        let end = ended(&downloads, id).await;
        assert_eq!(end.phase, Phase::Done, "{length}: {end:?}");
        assert_same(&dir.read(&name), length);
        // Never a range ending before it starts, nor one past the file.
        for s in server.seen() {
            if s.range.is_some() {
                let (start, last) = s
                    .asked()
                    .unwrap_or_else(|| panic!("{length}: {:?}", s.range));
                assert!(start <= last && last < length.max(PROBE), "{length}: {s:?}");
            }
        }
    }

    // A file an earlier engine left with sixteen holes: only the holes are
    // fetched.
    const LENGTH: u64 = 8 * MIB;
    let server = Server::start().await;
    server.at("/holes.bin", Answer::File(File::new(LENGTH).etag("\"v1\"")));
    let dir = Folder::new("holes");
    let step = LENGTH / 32;
    let done: Vec<(u64, u64)> = (0..16)
        .map(|i| (2 * i * step, (2 * i + 1) * step))
        .collect();
    let mut part = vec![0u8; LENGTH as usize];
    for &(start, end) in &done {
        part[start as usize..end as usize].copy_from_slice(&serve::content(start..end));
    }
    std::fs::write(dir.0.join("holes.bin.part"), part).unwrap();
    let saved = serde_json::json!({
        "url": server.url("/holes.bin").to_string(),
        "length": LENGTH,
        "validator": {"etag": "\"v1\""},
        "done": done,
    });
    std::fs::write(dir.0.join("holes.bin.part.json"), saved.to_string()).unwrap();
    let downloads = Downloads::with_config(links.table(), dir.0.clone(), config());
    let id = downloads
        .add(request(&server.url("/holes.bin")))
        .await
        .unwrap();
    let end = ended(&downloads, id).await;
    assert_eq!(end.phase, Phase::Done, "{end:?}");
    assert_same(&dir.read("holes.bin"), LENGTH);
    // Asked for the holes and nothing else.
    assert_eq!(asked(&server.seen()).covered(), LENGTH / 2);
}

/// Where a probe's range ends, for the check above: a probe asks for its
/// 1 KiB whatever the file's size.
const PROBE: u64 = barrage::fetch::PROBE_BYTES;

/// Review Focus 2: stopped inside one long span, exactly what was written is
/// saved, and resuming fetches only the rest.
#[tokio::test]
async fn stopping_mid_span_saves_exactly_what_was_written() {
    const LENGTH: u64 = 32 * MIB;
    let links = links(vec![vec![wifi()]]);
    let file = File::new(LENGTH).rate(2 * MIB).etag("\"v1\"");
    let (server, dir, downloads, id) = start_with("mid-span.bin", file, &links, config()).await;

    wait_for(&downloads, id, "2 MiB", |s| s.done >= 2 * MIB).await;
    downloads.stop(id).await.unwrap();
    let stopped = status_of(&downloads, id);
    let saved: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.0.join("mid-span.bin.part.json")).unwrap(),
    )
    .unwrap();
    let covered: u64 = saved["done"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r[1].as_u64().unwrap() - r[0].as_u64().unwrap())
        .sum();
    assert_eq!(covered, stopped.done, "saved {saved} for {stopped:?}");

    let before = server.seen().len();
    downloads.resume(id).await.unwrap();
    let end = ended(&downloads, id).await;
    assert_eq!(end.phase, Phase::Done, "{end:?}");
    assert_same(&dir.read("mid-span.bin"), LENGTH);
    // Asked for exactly the rest.
    assert_eq!(asked(&server.seen()[before..]).covered(), LENGTH - covered);
}

/// BRG-64: a 429 to a link's probe, with a short Retry-After, is waited out.
#[tokio::test]
async fn a_refused_probe_waits_and_finds_the_file_again() {
    const LENGTH: u64 = 16 * MIB;
    let links = both();
    let file = File::new(LENGTH)
        .rate(MIB)
        .refuse_probe(ETHERNET, 1, 429, Some("1"));
    let (_server, dir, downloads, id) = start_with("probe-429.bin", file, &links, quick()).await;
    let history = record(&downloads, id);

    let end = ended(&downloads, id).await;
    assert_eq!(end.phase, Phase::Done, "{end:?}");
    assert_same(&dir.read("probe-429.bin"), LENGTH);
    assert!(share(&end, "Ethernet 2").bytes > 0, "{end:?}");
    let notes = notes_of(&history.await.unwrap(), "Ethernet 2");
    let wait = "the server asked this cable to wait 1 s before trying again";
    assert!(notes.iter().any(|n| n == wait), "{notes:?}");
}

/// BRG-64: a 503 to a link's probe with no Retry-After rests the link, as a
/// refused piece does, and it finds the file again.
#[tokio::test]
async fn a_refused_probe_without_a_wait_rests_and_tries_again() {
    const LENGTH: u64 = 16 * MIB;
    let links = both();
    let file = File::new(LENGTH)
        .rate(MIB)
        .refuse_probe(ETHERNET, 1, 503, None);
    let (_server, dir, downloads, id) = start_with("probe-503.bin", file, &links, quick()).await;
    let history = record(&downloads, id);

    let end = ended(&downloads, id).await;
    assert_eq!(end.phase, Phase::Done, "{end:?}");
    assert_same(&dir.read("probe-503.bin"), LENGTH);
    assert!(share(&end, "Ethernet 2").bytes > 0, "{end:?}");
    let notes = notes_of(&history.await.unwrap(), "Ethernet 2");
    let rest = "the server refused this cable's request for the file (503 Service Unavailable); \
                it rests 200 ms and tries again";
    assert!(notes.iter().any(|n| n == rest), "{notes:?}");
}
