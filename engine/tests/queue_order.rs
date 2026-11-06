//! One line for web downloads and torrents, and one limit on what runs at
//! once. Against the scripted server and local seeds: nothing leaves this
//! machine.

mod fake;
mod fetching;
mod scratch;
mod serve;
mod torrent;

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::Duration;

use barrage::LinkHandle;
use barrage::bt::{Source, TorrentConfig, TorrentId, TorrentPhase, Torrents};
use barrage::fetch::{AddError, ControlError, DownloadConfig, DownloadId, Downloads, Phase, Usage};
use barrage::keeper::{Engines, Keeper};
use barrage::queue::{MOST_LIMIT, Queue, QueueError, QueueView};
use barrage::record::{Priority, Store};
use barrage::transfer::{Key, TransferEngine};
use fake::ms;
use fetching::{MIB, both, config, request, status_of};
use serve::{Answer, File, Server};
use torrent::{Seeds, fresh_dir, listening_on, pattern, seeds_with};

/// A file that never finishes: it holds its place until it is stopped.
fn held() -> File {
    File::new(32 * MIB).stall_after(MIB).etag("\"v1\"")
}

fn quick() -> File {
    File::new(MIB).etag("\"v1\"")
}

/// A server with `/one.bin` to `/six.bin`, each held.
async fn server() -> Server {
    let server = Server::start().await;
    for name in ["one", "two", "three", "four", "five", "six"] {
        server.at(&format!("/{name}.bin"), Answer::File(held()));
    }
    server
}

fn patient() -> DownloadConfig {
    DownloadConfig {
        read_deadline: Duration::from_secs(60),
        ..config()
    }
}

struct Engine {
    queue: Queue,
    downloads: Downloads,
    torrents: Torrents,
}

async fn engine(links: &LinkHandle, name: &str, limit: u32, peers: Vec<SocketAddr>) -> Engine {
    let dir = fresh_dir(name);
    let usage = Usage::counting_speed_tests(links.table());
    let seq = Arc::new(AtomicU64::new(0));
    let downloads = Downloads::sharing(
        links.table(),
        dir.clone(),
        patient(),
        Arc::clone(&usage),
        Arc::clone(&seq),
    );
    let torrents = Torrents::start(
        links.table(),
        dir,
        Arc::clone(&usage),
        TorrentConfig {
            network: false,
            listen: false,
            tick: ms(50),
            peers,
            seq,
            ..TorrentConfig::default()
        },
    )
    .await
    .unwrap();
    let queue = Queue::start(downloads.clone(), torrents.clone(), limit);
    Engine {
        queue,
        downloads,
        torrents,
    }
}

async fn slow_seeds(name: &str) -> Seeds {
    seeds_with(name, 2, &pattern(4 << 20), 64 << 10, || {
        let mut options = listening_on("127.0.0.1");
        options.ratelimits.upload_bps = std::num::NonZeroU32::new(64 << 10);
        options
    })
    .await
}

fn phase(e: &Engine, id: DownloadId) -> Phase {
    status_of(&e.downloads, id).phase
}

fn torrent_phase(e: &Engine, id: TorrentId) -> TorrentPhase {
    let all = e.torrents.status();
    let all = all.borrow();
    all.iter().find(|t| t.id == id).unwrap().phase
}

/// Waits for `what` to hold, for at most 20 s.
async fn until(what: &str, ok: impl Fn() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while !ok() {
        assert!(tokio::time::Instant::now() < deadline, "never {what}");
        tokio::time::sleep(ms(20)).await;
    }
}

fn places(view: &QueueView) -> Vec<(Key, u32)> {
    view.places.clone()
}

async fn add(e: &Engine, server: &Server, path: &str) -> DownloadId {
    e.queue
        .add_download(request(&server.url(path)))
        .await
        .unwrap()
}

fn running(p: Phase) -> bool {
    matches!(p, Phase::Starting | Phase::Waiting | Phase::Downloading)
}

#[tokio::test]
async fn with_a_limit_of_2_and_4_transfers_2_run_and_the_rest_follow_in_order() {
    let links = both();
    let files = server().await;
    let seeds = slow_seeds("q-mixed-seed").await;
    let e = engine(&links, "q-mixed", 2, seeds.addrs.clone()).await;

    let first = add(&e, &files, "/one.bin").await;
    let second = add(&e, &files, "/two.bin").await;
    let third = e
        .queue
        .add_torrent(Source::File(seeds.torrent.clone()))
        .await
        .unwrap();
    let fourth = add(&e, &files, "/three.bin").await;

    until("two running", || {
        running(phase(&e, first)) && running(phase(&e, second))
    })
    .await;
    assert_eq!(torrent_phase(&e, third), TorrentPhase::Queued);
    assert_eq!(phase(&e, fourth), Phase::Queued);
    let view = e.queue.view();
    until("two holding", || view.borrow().holding == 2).await;
    assert_eq!(
        places(&view.borrow()),
        [(Key::Torrent(third), 1), (Key::Web(fourth), 2)],
        "they wait in the order they were added, web and torrent alike"
    );

    e.downloads.stop(first).await.unwrap();
    until("the torrent starting", || {
        torrent_phase(&e, third) != TorrentPhase::Queued
    })
    .await;
    assert_eq!(phase(&e, fourth), Phase::Queued);

    e.torrents.stop(third).await.unwrap();
    until("the last one starting", || running(phase(&e, fourth))).await;
    assert!(running(phase(&e, second)));
}

#[tokio::test]
async fn a_finished_or_failed_one_frees_its_place() {
    let links = both();
    let files = server().await;
    files.at("/quick.bin", Answer::File(quick()));
    files.at("/missing.bin", Answer::Status(404));
    let e = engine(&links, "q-frees", 1, Vec::new()).await;

    let finishes = add(&e, &files, "/quick.bin").await;
    let fails = add(&e, &files, "/missing.bin").await;
    let last = add(&e, &files, "/one.bin").await;

    until("the first done", || phase(&e, finishes) == Phase::Done).await;
    until("the second failed", || phase(&e, fails) == Phase::Failed).await;
    until("the last running", || running(phase(&e, last))).await;
}

#[tokio::test]
async fn a_stopped_queued_one_never_started() {
    let links = both();
    let files = server().await;
    let e = engine(&links, "q-never", 1, Vec::new()).await;
    let first = add(&e, &files, "/one.bin").await;
    until("the first running", || running(phase(&e, first))).await;
    let waiting = add(&e, &files, "/two.bin").await;

    e.downloads.stop(waiting).await.unwrap();
    assert_eq!(phase(&e, waiting), Phase::Stopped);
    e.downloads.stop(first).await.unwrap();
    tokio::time::sleep(ms(200)).await;

    assert_eq!(phase(&e, waiting), Phase::Stopped, "it held no place");
    let asked = files.url("/two.bin");
    assert!(
        !files.seen().iter().any(|s| s.path == asked.path()),
        "no request reached the server"
    );
}

#[tokio::test]
async fn resume_sends_it_to_the_back_of_the_line() {
    let links = both();
    let files = server().await;
    let e = engine(&links, "q-back", 1, Vec::new()).await;
    let first = add(&e, &files, "/one.bin").await;
    until("the first running", || running(phase(&e, first))).await;
    let second = add(&e, &files, "/two.bin").await;
    e.downloads.stop(second).await.unwrap();
    let third = add(&e, &files, "/three.bin").await;

    e.queue.resume_download(second).await.unwrap();

    assert_eq!(phase(&e, second), Phase::Queued);
    let view = e.queue.view();
    until("both in line", || view.borrow().places.len() == 2).await;
    assert_eq!(
        places(&view.borrow()),
        [(Key::Web(third), 1), (Key::Web(second), 2)]
    );
}

#[tokio::test]
async fn a_raised_limit_starts_the_next_at_once_and_a_lower_one_stops_nothing() {
    let links = both();
    let files = server().await;
    let e = engine(&links, "q-raised", 1, Vec::new()).await;
    let first = add(&e, &files, "/one.bin").await;
    let second = add(&e, &files, "/two.bin").await;
    until("the first running", || running(phase(&e, first))).await;

    e.queue.set_limit(2).unwrap();
    until("the second running", || running(phase(&e, second))).await;

    e.queue.set_limit(1).unwrap();
    tokio::time::sleep(ms(200)).await;
    assert!(running(phase(&e, first)) && running(phase(&e, second)));
    assert_eq!(e.queue.view().borrow().limit, 1);
}

#[tokio::test]
async fn a_limit_of_0_or_over_20_is_refused() {
    let links = both();
    let e = engine(&links, "q-refused", 3, Vec::new()).await;

    assert_eq!(e.queue.set_limit(0), Err(QueueError::BadLimit));
    assert_eq!(e.queue.set_limit(MOST_LIMIT + 1), Err(QueueError::BadLimit));
    assert_eq!(e.queue.set_limit(MOST_LIMIT), Ok(()));
    assert_eq!(MOST_LIMIT, 20);
}

#[tokio::test]
async fn the_same_url_queued_is_refused_as_running() {
    let links = both();
    let files = server().await;
    let e = engine(&links, "q-same", 1, Vec::new()).await;
    let first = add(&e, &files, "/one.bin").await;
    until("the first running", || running(phase(&e, first))).await;
    let waiting = add(&e, &files, "/two.bin").await;

    let again = e.queue.add_download(request(&files.url("/two.bin"))).await;

    assert_eq!(again, Err(AddError::AlreadyRunning(waiting)));
}

/// What `serve` builds, on `state` and `dir`, saving every 50 ms.
async fn keeping(links: &LinkHandle, state: &Path, dir: &Path) -> (Engine, Keeper) {
    let (store, record, note) = Store::open(state).unwrap();
    let usage = Usage::restored(links.table(), &record.used);
    let seq = Arc::new(AtomicU64::new(0));
    let downloads = Downloads::sharing(
        links.table(),
        dir.to_path_buf(),
        patient(),
        Arc::clone(&usage),
        Arc::clone(&seq),
    );
    downloads.restore(record.transfers.clone(), record.next_download);
    let torrents = Torrents::start(
        links.table(),
        dir.to_path_buf(),
        Arc::clone(&usage),
        TorrentConfig {
            network: false,
            listen: false,
            tick: ms(50),
            seq,
            state: Some(state.to_path_buf()),
            ..TorrentConfig::default()
        },
    )
    .await
    .unwrap();
    torrents
        .restore(record.transfers.clone(), record.next_torrent)
        .await;
    let queue = Queue::start(downloads.clone(), torrents.clone(), record.limit);
    let keeper = Keeper::start(
        (store, record, note),
        Engines {
            downloads: downloads.clone(),
            torrents: torrents.clone(),
            queue: queue.clone(),
            usage,
        },
        ms(50),
    );
    (
        Engine {
            queue,
            downloads,
            torrents,
        },
        keeper,
    )
}

async fn close(e: Engine, keeper: Keeper) {
    let ran = keeper.running();
    e.queue.freeze();
    e.downloads.shutdown().await;
    e.torrents.shutdown().await;
    keeper.finish(&ran);
}

#[tokio::test]
async fn the_order_and_the_limit_survive_a_restart() {
    let links = both();
    let files = server().await;
    let (state, dir) = (fresh_dir("q-order-state"), fresh_dir("q-order"));

    let (e, keeper) = keeping(&links, &state, &dir).await;
    e.queue.set_limit(1).unwrap();
    let first = add(&e, &files, "/one.bin").await;
    until("the first running", || running(phase(&e, first))).await;
    let second = add(&e, &files, "/two.bin").await;
    let third = add(&e, &files, "/three.bin").await;
    close(e, keeper).await;

    let (e, _keeper) = keeping(&links, &state, &dir).await;

    assert_eq!(e.queue.view().borrow().limit, 1);
    until("the first running again", || running(phase(&e, first))).await;
    let view = e.queue.view();
    until("the rest in line", || view.borrow().places.len() == 2).await;
    assert_eq!(
        places(&view.borrow()),
        [(Key::Web(second), 1), (Key::Web(third), 2)]
    );
}

#[tokio::test]
async fn nothing_starts_during_shutdown() {
    let links = both();
    let files = server().await;
    let e = engine(&links, "q-shutdown", 1, Vec::new()).await;
    let first = add(&e, &files, "/one.bin").await;
    until("the first running", || running(phase(&e, first))).await;
    let waiting = add(&e, &files, "/two.bin").await;

    e.queue.freeze();
    e.downloads.stop(first).await.unwrap();
    tokio::time::sleep(ms(300)).await;

    assert_eq!(phase(&e, waiting), Phase::Queued);
}

/// Review Focus 3: nobody serves it, so it would be `finding` for good. It
/// keeps its place, and says so.
#[tokio::test]
async fn a_dead_magnet_keeps_its_place_and_says_so() {
    let links = both();
    let files = server().await;
    let dir = fresh_dir("q-dead");
    let usage = Usage::counting_speed_tests(links.table());
    let seq = Arc::new(AtomicU64::new(0));
    let downloads = Downloads::sharing(
        links.table(),
        dir.clone(),
        patient(),
        Arc::clone(&usage),
        Arc::clone(&seq),
    );
    let torrents = Torrents::start(
        links.table(),
        dir,
        usage,
        TorrentConfig {
            network: false,
            listen: false,
            tick: ms(50),
            seq,
            finding_note_after: ms(200),
            ..TorrentConfig::default()
        },
    )
    .await
    .unwrap();
    let queue = Queue::start(downloads.clone(), torrents.clone(), 1);
    let dead = format!("magnet:?xt=urn:btih:{}&dn=dead.iso", "cd".repeat(20));
    let id = queue.add_torrent(Source::Magnet(dead)).await.unwrap();
    let waiting = queue
        .add_download(request(&files.url("/one.bin")))
        .await
        .unwrap();

    let note = || {
        let all = torrents.status();
        let all = all.borrow();
        all.iter().find(|t| t.id == id).unwrap().note.clone()
    };
    until("the note", || note().is_some()).await;
    assert_eq!(
        note().as_deref(),
        Some(
            "no peer has given this torrent's details yet. It is holding a place in the queue; \
             stop it to let the next start"
        )
    );
    assert_eq!(status_of(&downloads, waiting).phase, Phase::Queued);

    torrents.stop(id).await.unwrap();
    until("the next starting", || {
        status_of(&downloads, waiting).phase != Phase::Queued
    })
    .await;
}

#[tokio::test]
async fn a_change_moves_a_waiting_transfer() {
    let links = both();
    let files = server().await;
    let e = engine(&links, "q-moves", 1, Vec::new()).await;
    let first = add(&e, &files, "/one.bin").await;
    until("the first running", || running(phase(&e, first))).await;
    let second = add(&e, &files, "/two.bin").await;
    let third = add(&e, &files, "/three.bin").await;
    let view = e.queue.view();
    until("both in line", || view.borrow().places.len() == 2).await;

    TransferEngine::set_priority(&e.downloads, third, Priority::High)
        .await
        .unwrap();

    until("the high one first", || {
        view.borrow().places.first() == Some(&(Key::Web(third), 1))
    })
    .await;
    assert_eq!(
        places(&view.borrow()),
        [(Key::Web(third), 1), (Key::Web(second), 2)]
    );

    e.downloads.stop(first).await.unwrap();
    until("the high one running", || running(phase(&e, third))).await;
    assert_eq!(phase(&e, second), Phase::Queued);
}

#[tokio::test]
async fn a_change_never_touches_a_running_one() {
    let links = both();
    let files = server().await;
    let e = engine(&links, "q-touches", 1, Vec::new()).await;
    let first = add(&e, &files, "/one.bin").await;
    until("the first running", || running(phase(&e, first))).await;
    let second = add(&e, &files, "/two.bin").await;

    TransferEngine::set_priority(&e.downloads, first, Priority::Low)
        .await
        .unwrap();
    TransferEngine::set_priority(&e.downloads, second, Priority::High)
        .await
        .unwrap();
    tokio::time::sleep(ms(300)).await;

    assert!(
        running(phase(&e, first)),
        "a high one waits for a free place"
    );
    assert_eq!(phase(&e, second), Phase::Queued);
    assert_eq!(status_of(&e.downloads, first).priority, Priority::Low);
}

#[tokio::test]
async fn a_torrent_takes_a_priority_too() {
    let links = both();
    let files = server().await;
    let seeds = slow_seeds("q-torrent-seed").await;
    let e = engine(&links, "q-torrent", 1, seeds.addrs.clone()).await;
    let first = add(&e, &files, "/one.bin").await;
    until("the first running", || running(phase(&e, first))).await;
    let second = add(&e, &files, "/two.bin").await;
    let third = e
        .queue
        .add_torrent(Source::File(seeds.torrent.clone()))
        .await
        .unwrap();

    TransferEngine::set_priority(&e.torrents, third, Priority::High)
        .await
        .unwrap();
    e.downloads.stop(first).await.unwrap();

    until("the torrent running", || {
        torrent_phase(&e, third) != TorrentPhase::Queued
    })
    .await;
    assert_eq!(phase(&e, second), Phase::Queued);
}

#[tokio::test]
async fn priorities_survive_a_restart() {
    let links = both();
    let files = server().await;
    let (state, dir) = (fresh_dir("q-keeps-state"), fresh_dir("q-keeps"));

    let (e, keeper) = keeping(&links, &state, &dir).await;
    e.queue.set_limit(1).unwrap();
    let first = add(&e, &files, "/one.bin").await;
    until("the first running", || running(phase(&e, first))).await;
    let second = add(&e, &files, "/two.bin").await;
    let third = add(&e, &files, "/three.bin").await;
    TransferEngine::set_priority(&e.downloads, third, Priority::High)
        .await
        .unwrap();
    close(e, keeper).await;

    let (e, _keeper) = keeping(&links, &state, &dir).await;

    assert_eq!(status_of(&e.downloads, third).priority, Priority::High);
    // The high one takes the only place, ahead of what was running.
    until("the high one running", || running(phase(&e, third))).await;
    let view = e.queue.view();
    until("the rest in line", || view.borrow().places.len() == 2).await;
    assert_eq!(
        places(&view.borrow()),
        [(Key::Web(first), 1), (Key::Web(second), 2)]
    );
}

#[tokio::test]
async fn a_done_transfer_has_no_priority_to_change() {
    let links = both();
    let files = server().await;
    files.at("/quick.bin", Answer::File(quick()));
    let e = engine(&links, "q-done", 1, Vec::new()).await;
    let id = add(&e, &files, "/quick.bin").await;
    until("done", || phase(&e, id) == Phase::Done).await;

    let refused = TransferEngine::set_priority(&e.downloads, id, Priority::High).await;

    assert_eq!(refused, Err(ControlError::Finished));
}
