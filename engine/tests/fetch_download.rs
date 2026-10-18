//! One download across every link, against the scripted server in
//! `serve/mod.rs` and the scripted network in `fake/mod.rs`. "Wi-Fi" is
//! 127.0.0.1 and "Ethernet 2" is 127.0.0.2, so real bytes flow over each and
//! the server tells the two apart by source address. The server sends each
//! answer at a few MiB a second, so that both links have time to take part.
//! No test here touches the internet.

mod fake;
mod serve;

use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use barrage::LinkHandle;
use barrage::fetch::{
    AddRequest, DownloadConfig, DownloadId, DownloadStatus, Downloads, PROBE_BYTES, Phase,
};
use barrage::links::Candidate;
use barrage::links::watch::WatchConfig;
use fake::{Fake, cand_idx, ms};
use serve::{Answer, File, Seen, Server, content, most_at_once};

const MIB: u64 = 1 << 20;
const WIFI: &str = "127.0.0.1";
const ETHERNET: &str = "127.0.0.2";

fn wifi() -> Candidate {
    cand_idx(1, "Wi-Fi", WIFI, true)
}

fn ethernet() -> Candidate {
    cand_idx(2, "Ethernet 2", ETHERNET, true)
}

fn ip(text: &str) -> IpAddr {
    text.parse().unwrap()
}

/// The links, one table per poll of the watcher, the last one repeating.
fn links(steps: Vec<Vec<Candidate>>) -> LinkHandle {
    let net = Fake::scripted(steps).loopback_links();
    LinkHandle::start_with(Arc::new(net), WatchConfig::every(ms(20))).unwrap()
}

fn both() -> LinkHandle {
    links(vec![vec![wifi(), ethernet()]])
}

/// The default worker count, with short times so a test takes a second.
fn config() -> DownloadConfig {
    DownloadConfig {
        read_deadline: Duration::from_secs(5),
        find_deadline: Duration::from_secs(5),
        publish_every: ms(20),
        speed_window: ms(200),
        ..DownloadConfig::default()
    }
}

/// A folder of its own for one test's downloads, removed afterwards.
struct Folder(PathBuf);

impl Folder {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("barrage-download-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn read(&self, name: &str) -> Vec<u8> {
        std::fs::read(self.0.join(name)).unwrap()
    }
}

impl Drop for Folder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn request(url: &reqwest::Url) -> AddRequest {
    AddRequest {
        url: url.to_string(),
        cables: None,
    }
}

/// The download's status once `wanted` holds, failing the test after 20 s.
async fn wait_for(
    downloads: &Downloads,
    id: DownloadId,
    what: &str,
    wanted: impl Fn(&DownloadStatus) -> bool,
) -> DownloadStatus {
    let mut status = downloads.status();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let now = status
            .borrow_and_update()
            .iter()
            .find(|s| s.id == id)
            .cloned();
        if let Some(now) = now.as_ref().filter(|s| wanted(s)) {
            return now.clone();
        }
        match tokio::time::timeout_at(deadline, status.changed()).await {
            Ok(Ok(())) => {}
            Ok(Err(_)) => panic!("the status channel closed while waiting for {what}"),
            Err(_) => panic!("no {what} within 20 s; last status: {now:?}"),
        }
    }
}

async fn ended(downloads: &Downloads, id: DownloadId) -> DownloadStatus {
    wait_for(downloads, id, "end", |s| {
        matches!(s.phase, Phase::Done | Phase::Failed)
    })
    .await
}

fn is_probe(seen: &Seen) -> bool {
    seen.asked() == Some((0, PROBE_BYTES - 1))
}

/// The file's bytes the server sent to `source`, its probes aside.
fn served(seen: &[Seen], source: &str) -> u64 {
    seen.iter()
        .filter(|s| s.source == ip(source) && !is_probe(s))
        .map(Seen::sent)
        .sum()
}

/// The file on disk is the server's, byte for byte.
fn assert_same(got: &[u8], length: u64) {
    let want = content(0..length);
    assert_eq!(got.len(), want.len(), "the file's length");
    if let Some(at) = got.iter().zip(&want).position(|(a, b)| a != b) {
        panic!("the file differs from the server's at byte {at}");
    }
}

/// A rate-limited file on a new server, downloaded into a new folder.
async fn start(
    name: &str,
    file: File,
    links: &LinkHandle,
) -> (Server, Folder, Downloads, DownloadId) {
    let server = Server::start().await;
    server.at(&format!("/{name}"), Answer::File(file));
    let dir = Folder::new(name);
    let downloads = Downloads::with_config(links.table(), dir.0.clone(), config());
    let id = downloads
        .add(request(&server.url(&format!("/{name}"))))
        .await
        .unwrap();
    (server, dir, downloads, id)
}

#[tokio::test]
async fn a_file_comes_down_through_every_link_byte_for_byte() {
    let links = both();
    let length = 20 * MIB;
    let file = File::new(length).rate(4 * MIB);
    let (server, dir, downloads, id) = start("every.bin", file, &links).await;

    let end = ended(&downloads, id).await;
    assert_eq!(end.phase, Phase::Done, "{end:?}");
    assert_eq!((end.length, end.done), (Some(length), length));
    assert_eq!(end.name.as_deref(), Some("every.bin"));
    assert_same(&dir.read("every.bin"), length);
    assert!(!dir.0.join("every.bin.part").exists());

    let seen = server.seen();
    let (wifi, ethernet) = (served(&seen, WIFI), served(&seen, ETHERNET));
    assert!(
        wifi > 0 && ethernet > 0,
        "Wi-Fi {wifi}, Ethernet 2 {ethernet}"
    );
    // No byte twice: beyond the probes, the server sent exactly the file.
    assert_eq!(wifi + ethernet, length);
}

#[tokio::test]
async fn each_link_gets_its_own_workers() {
    let links = both();
    let file = File::new(20 * MIB).rate(4 * MIB);
    let (server, _dir, downloads, id) = start("workers.bin", file, &links).await;
    assert_eq!(ended(&downloads, id).await.phase, Phase::Done);

    let seen = server.seen();
    for source in [WIFI, ETHERNET] {
        let most = most_at_once(&seen, ip(source), |s| !is_probe(s));
        assert_eq!(most, 4, "{source} held {most} connections at once");
    }
}

#[tokio::test]
async fn a_cut_connection_asks_only_for_the_bytes_still_missing() {
    let links = both();
    let length = 20 * MIB;
    let file = File::new(length).rate(4 * MIB).cut(ETHERNET, MIB, 1);
    let (server, dir, downloads, id) = start("cut.bin", file, &links).await;

    assert_eq!(ended(&downloads, id).await.phase, Phase::Done);
    assert_same(&dir.read("cut.bin"), length);

    let seen = server.seen();
    let cut = seen
        .iter()
        .find(|s| s.cut.is_some())
        .expect("nothing was cut");
    let (start, _) = cut.asked().unwrap();
    let starts: Vec<u64> = seen
        .iter()
        .filter(|s| !is_probe(s))
        .map(|s| s.asked().unwrap().0)
        .collect();
    // Asked for again from where the bytes it had ended, never from the top.
    assert_eq!(
        starts.iter().filter(|&&s| s == start).count(),
        1,
        "{starts:?}"
    );
    let again = starts
        .iter()
        .filter(|&&s| s > start && s <= start + MIB)
        .count();
    assert_eq!(again, 1, "{starts:?}");
}

/// A server that answers with less than asked, and one that runs on past the
/// range with bytes that are not the file's: only the bytes asked for are
/// written, and only the bytes still missing are asked for again.
#[tokio::test]
async fn a_short_or_overlong_answer_writes_only_what_was_asked() {
    let links = both();
    let length = 10 * MIB;
    // Dozens of short answers, many more than there are workers, so a worker
    // that gave up after one would leave none to finish.
    let short = File::new(length).max_answer(MIB / 4 + 13);
    let (server, dir, downloads, id) = start("short.bin", short, &links).await;
    let end = ended(&downloads, id).await;
    assert_eq!(end.phase, Phase::Done, "{end:?}");
    assert_same(&dir.read("short.bin"), length);
    let seen = server.seen();
    assert_eq!(served(&seen, WIFI) + served(&seen, ETHERNET), length);
    assert_eq!(end.links.iter().map(|l| l.bytes).sum::<u64>(), length);

    let long = File::new(length).past_the_range();
    let (_server, dir, downloads, id) = start("long.bin", long, &links).await;
    let end = ended(&downloads, id).await;
    assert_eq!(end.phase, Phase::Done, "{end:?}");
    assert_same(&dir.read("long.bin"), length);
    assert_eq!(end.links.iter().map(|l| l.bytes).sum::<u64>(), length);
}

/// A server that answers every piece from the top of the file, whatever was
/// asked: those bytes belong elsewhere, so none is written, and the download
/// fails saying why.
#[tokio::test]
async fn a_piece_from_the_wrong_place_is_never_written() {
    let links = both();
    let length = 12 * MIB;
    let file = File::new(length).answers_from_the_top();
    let (_server, dir, downloads, id) = start("top.bin", file, &links).await;

    let end = ended(&downloads, id).await;
    assert_eq!(end.phase, Phase::Failed, "{end:?}");
    let note = end.note.unwrap_or_default();
    assert!(note.contains("Content-Range"), "{note}");
    // The top of the file is the first chunk; past it nothing was written.
    let part = dir.read("top.bin.part");
    let past_the_first = &part[(4 * MIB) as usize..];
    assert!(
        past_the_first.iter().all(|&b| b == 0),
        "a piece landed in the wrong place"
    );
    assert!(!dir.0.join("top.bin").exists());
}

#[tokio::test]
async fn the_part_file_is_full_size_before_the_data_arrives() {
    let links = both();
    let length = 20 * MIB;
    let file = File::new(length).rate(4 * MIB);
    let (_server, dir, downloads, id) = start("sized.bin", file, &links).await;

    let first = wait_for(&downloads, id, "downloading", |s| {
        s.phase == Phase::Downloading
    })
    .await;
    let part = std::fs::metadata(dir.0.join("sized.bin.part")).unwrap();
    assert_eq!(part.len(), length);
    assert!(first.done < length);
    assert_eq!(ended(&downloads, id).await.phase, Phase::Done);
}

#[tokio::test]
async fn an_unwritable_folder_fails_at_once_and_says_so() {
    let links = both();
    let server = Server::start().await;
    server.at("/x.bin", Answer::File(File::new(10 * MIB)));
    let dir = Folder::new("unwritable");
    let not_a_folder = dir.0.join("a file");
    std::fs::write(&not_a_folder, b"in the way").unwrap();

    let downloads = Downloads::with_config(links.table(), not_a_folder.clone(), config());
    let id = downloads.add(request(&server.url("/x.bin"))).await.unwrap();
    let end = ended(&downloads, id).await;
    assert_eq!(end.phase, Phase::Failed);
    let note = end.note.unwrap_or_default();
    assert!(note.contains(&not_a_folder.display().to_string()), "{note}");
    // Nothing but the probes was asked for.
    assert!(server.seen().iter().all(is_probe));
}

#[tokio::test]
async fn a_taken_name_is_never_overwritten() {
    let links = both();
    let length = 4 * MIB;
    let server = Server::start().await;
    server.at("/file.bin", Answer::File(File::new(length)));
    let dir = Folder::new("taken");
    std::fs::write(dir.0.join("file.bin"), b"mine").unwrap();

    let downloads = Downloads::with_config(links.table(), dir.0.clone(), config());
    let id = downloads
        .add(request(&server.url("/file.bin")))
        .await
        .unwrap();
    let end = ended(&downloads, id).await;
    assert_eq!(end.phase, Phase::Done, "{end:?}");
    assert_eq!(end.name.as_deref(), Some("file (1).bin"));
    assert_eq!(dir.read("file.bin"), b"mine");
    assert_same(&dir.read("file (1).bin"), length);
}

#[tokio::test]
async fn cables_limits_the_download_to_those_cables() {
    let links = both();
    let length = 10 * MIB;
    let server = Server::start().await;
    server.at("/one.bin", Answer::File(File::new(length).rate(8 * MIB)));
    let dir = Folder::new("cables");
    let downloads = Downloads::with_config(links.table(), dir.0.clone(), config());
    let id = downloads
        .add(AddRequest {
            url: server.url("/one.bin").to_string(),
            cables: Some(vec!["Wi-Fi".to_string()]),
        })
        .await
        .unwrap();

    let end = ended(&downloads, id).await;
    assert_eq!(end.phase, Phase::Done, "{end:?}");
    assert_same(&dir.read("one.bin"), length);
    assert!(server.seen().iter().all(|s| s.source == ip(WIFI)));
    let names: Vec<&str> = end.links.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["Wi-Fi"]);
}

#[tokio::test]
async fn with_no_working_link_it_waits_then_starts() {
    // Half a second of nothing, then the Wi-Fi.
    let mut steps = vec![Vec::new(); 25];
    steps.push(vec![wifi()]);
    let links = links(steps);
    let length = 4 * MIB;
    let (_server, dir, downloads, id) = start("later.bin", File::new(length), &links).await;

    wait_for(&downloads, id, "waiting", |s| s.phase == Phase::Waiting).await;
    let end = ended(&downloads, id).await;
    assert_eq!(end.phase, Phase::Done, "{end:?}");
    assert_same(&dir.read("later.bin"), length);
}

#[tokio::test]
async fn a_link_that_comes_up_mid_download_joins_in() {
    // The Wi-Fi alone for a third of a second, then both.
    let mut steps = vec![vec![wifi()]; 15];
    steps.push(vec![wifi(), ethernet()]);
    let links = links(steps);
    let length = 20 * MIB;
    let file = File::new(length).rate(2 * MIB);
    let (server, dir, downloads, id) = start("joins.bin", file, &links).await;

    assert_eq!(ended(&downloads, id).await.phase, Phase::Done);
    assert_same(&dir.read("joins.bin"), length);
    assert!(served(&server.seen(), ETHERNET) > 0);
}

#[tokio::test]
async fn status_shows_bytes_and_speed_per_link() {
    let links = both();
    let length = 20 * MIB;
    let file = File::new(length).rate(4 * MIB);
    let (_server, _dir, downloads, id) = start("status.bin", file, &links).await;

    let mid = wait_for(&downloads, id, "speed on both links", |s| {
        s.phase == Phase::Downloading
            && s.links.len() == 2
            && s.links.iter().all(|l| l.bytes > 0 && l.bps > 0)
    })
    .await;
    assert_eq!(mid.links.iter().map(|l| l.bytes).sum::<u64>(), mid.done);
    assert_eq!(mid.bps, mid.links.iter().map(|l| l.bps).sum::<u64>());
    assert!(mid.done < length);
    for share in &mid.links {
        assert!((1..=4).contains(&share.connections), "{share:?}");
    }
    let names: Vec<&str> = mid.links.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["Wi-Fi", "Ethernet 2"]);

    let end = ended(&downloads, id).await;
    assert_eq!(end.phase, Phase::Done);
    assert_eq!(end.done, length);
    assert_eq!(end.links.iter().map(|l| l.bytes).sum::<u64>(), length);
    assert_eq!(end.bps, 0);
}

#[tokio::test]
async fn a_server_that_never_answers_fails_with_a_note() {
    let links = both();
    let server = Server::start().await;
    server.at("/silent", Answer::Silent);
    let dir = Folder::new("silent");
    let config = DownloadConfig {
        find_deadline: ms(300),
        ..config()
    };
    let downloads = Downloads::with_config(links.table(), dir.0.clone(), config);
    let id = downloads
        .add(request(&server.url("/silent")))
        .await
        .unwrap();

    let end = ended(&downloads, id).await;
    assert_eq!(end.phase, Phase::Failed);
    let note = end.note.unwrap_or_default();
    assert!(note.contains("no answer"), "{note}");
}

#[tokio::test]
async fn a_file_no_link_can_find_fails_with_the_reason() {
    let links = both();
    let server = Server::start().await;
    let dir = Folder::new("missing");
    let downloads = Downloads::with_config(links.table(), dir.0.clone(), config());
    let id = downloads
        .add(request(&server.url("/missing")))
        .await
        .unwrap();

    let end = ended(&downloads, id).await;
    assert_eq!(end.phase, Phase::Failed);
    let note = end.note.unwrap_or_default();
    assert!(note.contains("answered 404"), "{note}");
    assert!(
        std::fs::read_dir(&dir.0).unwrap().next().is_none(),
        "left a file"
    );
}

// When the server and the links disagree (Task 6).

/// The cable's share in `status`, by name.
fn share<'a>(status: &'a DownloadStatus, name: &str) -> &'a barrage::fetch::LinkShare {
    status
        .links
        .iter()
        .find(|l| l.name == name)
        .unwrap_or_else(|| panic!("no share for {name}: {status:?}"))
}

/// Review Focus 1: a CDN node holding another version. The Wi-Fi comes up
/// first, so its version is the one fixed; Ethernet 2's node then sends
/// another, and Ethernet 2 must take no part.
#[tokio::test]
async fn a_link_sent_a_different_file_sits_out_and_says_why() {
    let mut steps = vec![vec![wifi()]; 5];
    steps.push(vec![wifi(), ethernet()]);
    let links = links(steps);
    let length = 20 * MIB;
    let file = File::new(length)
        .rate(4 * MIB)
        .etag("\"a\"")
        .etag_for(ETHERNET, "\"b\"");
    let (server, dir, downloads, id) = start("versions.bin", file, &links).await;

    let end = ended(&downloads, id).await;
    assert_eq!(end.phase, Phase::Done, "{end:?}");
    assert_same(&dir.read("versions.bin"), length);
    let seen = server.seen();
    // Its probe, and nothing more.
    assert_eq!(seen.iter().filter(|s| s.source == ip(ETHERNET)).count(), 1);
    let note = share(&end, "Ethernet 2").note.clone().unwrap_or_default();
    assert!(note.contains("another version"), "{note}");
    // Every piece asked for the version the download was fixed to.
    for piece in seen.iter().filter(|s| !is_probe(s)) {
        assert_eq!(piece.if_range.as_deref(), Some("\"a\""), "{piece:?}");
    }
}

#[tokio::test]
async fn a_file_that_changes_mid_download_fails_rather_than_mix_versions() {
    let links = both();
    let file = File::new(20 * MIB)
        .rate(4 * MIB)
        .etag("\"v1\"")
        .replaced_after(5, "\"v2\"");
    let (_server, dir, downloads, id) = start("changes.bin", file, &links).await;

    let end = ended(&downloads, id).await;
    assert_eq!(end.phase, Phase::Failed, "{end:?}");
    let note = end.note.unwrap_or_default();
    assert!(note.contains("changed"), "{note}");
    assert!(!dir.0.join("changes.bin").exists());
    // Left for a later download to decide about.
    assert!(dir.0.join("changes.bin.part").exists());
}

/// A change only the length shows: the same ETag, a longer file.
#[tokio::test]
async fn a_file_whose_length_changes_mid_download_fails_too() {
    let links = both();
    let file = File::new(20 * MIB)
        .rate(4 * MIB)
        .etag("\"same\"")
        .grows_after(5, 21 * MIB);
    let (_server, dir, downloads, id) = start("grows.bin", file, &links).await;

    let end = ended(&downloads, id).await;
    assert_eq!(end.phase, Phase::Failed, "{end:?}");
    let note = end.note.unwrap_or_default();
    assert!(note.contains("length"), "{note}");
    assert!(!dir.0.join("grows.bin").exists());
}

#[tokio::test]
async fn a_link_with_no_address_for_the_host_sits_out() {
    let ipv6 = cand_idx(3, "Ethernet 3", "::1", true);
    let links = links(vec![vec![wifi(), ipv6]]);
    let length = 4 * MIB;
    let (_server, dir, downloads, id) = start("v4.bin", File::new(length), &links).await;

    let end = ended(&downloads, id).await;
    assert_eq!(end.phase, Phase::Done, "{end:?}");
    assert_same(&dir.read("v4.bin"), length);
    let share = share(&end, "Ethernet 3");
    assert_eq!(share.bytes, 0);
    let note = share.note.clone().unwrap_or_default();
    assert!(note.contains("only IPv4 addresses"), "{note}");
}

#[tokio::test]
async fn a_server_without_ranges_is_fetched_whole_through_one_link() {
    let links = both();
    let length = 8 * MIB;
    let file = File::new(length).no_ranges().rate(8 * MIB);
    let (server, dir, downloads, id) = start("whole.bin", file, &links).await;

    let end = ended(&downloads, id).await;
    assert_eq!(end.phase, Phase::Done, "{end:?}");
    assert_same(&dir.read("whole.bin"), length);
    let note = end.note.clone().unwrap_or_default();
    assert!(note.contains("only whole"), "{note}");
    // The probes asked for a range; the file itself came in one plain GET.
    let seen = server.seen();
    let plain: Vec<&Seen> = seen.iter().filter(|s| s.range.is_none()).collect();
    assert_eq!(plain.len(), 1, "{plain:?}");
    let carriers = end.links.iter().filter(|l| l.bytes > 0).count();
    assert_eq!(carriers, 1);
    // The other says why it took no part.
    for idle in end.links.iter().filter(|l| l.bytes == 0) {
        let note = idle.note.clone().unwrap_or_default();
        assert!(note.contains("only whole"), "{idle:?}");
    }
}

/// Nothing can pick up where a whole download broke off, so it fails, and
/// says so, rather than start again from nothing behind the user's back.
#[tokio::test]
async fn a_whole_download_that_breaks_off_cannot_resume() {
    let links = both();
    let server = Server::start().await;
    // Wi-Fi alone, so it carries the file; its probe uses up the first cut.
    let file = File::new(8 * MIB)
        .no_ranges()
        .rate(8 * MIB)
        .cut(WIFI, 2 * MIB, 2);
    server.at("/breaks.bin", Answer::File(file));
    let dir = Folder::new("breaks");
    let downloads = Downloads::with_config(links.table(), dir.0.clone(), config());
    let id = downloads
        .add(AddRequest {
            url: server.url("/breaks.bin").to_string(),
            cables: Some(vec!["Wi-Fi".to_string()]),
        })
        .await
        .unwrap();

    let end = ended(&downloads, id).await;
    assert_eq!(end.phase, Phase::Failed, "{end:?}");
    let note = end.note.unwrap_or_default();
    assert!(note.contains("cannot resume"), "{note}");
    assert!(!dir.0.join("breaks.bin").exists());
}

#[tokio::test]
async fn a_server_without_a_length_is_fetched_whole_too() {
    let links = both();
    let length = 4 * MIB;
    let file = File::new(length).no_ranges().no_length().rate(4 * MIB);
    let (_server, dir, downloads, id) = start("chunked.bin", file, &links).await;

    let running = wait_for(&downloads, id, "data", |s| {
        s.phase == Phase::Downloading && s.done > 0
    })
    .await;
    assert_eq!(running.length, None);
    let end = ended(&downloads, id).await;
    assert_eq!(end.phase, Phase::Done, "{end:?}");
    assert_eq!((end.done, end.length), (length, Some(length)));
    assert_same(&dir.read("chunked.bin"), length);
}

/// Nothing to fetch, through ranges (a 416 saying 0) or without them (a 200
/// with no body): done at once, an empty file.
#[tokio::test]
async fn an_empty_file_is_done_at_once() {
    let links = both();
    for (name, file) in [
        ("empty.bin", File::new(0)),
        ("empty-whole.bin", File::new(0).no_ranges()),
    ] {
        let (_server, dir, downloads, id) = start(name, file, &links).await;
        let end = ended(&downloads, id).await;
        assert_eq!(end.phase, Phase::Done, "{name}: {end:?}");
        assert_eq!(end.length, Some(0));
        assert!(dir.read(name).is_empty());
    }
}
