//! A restart: two engines, one after the other, on the same state and
//! download folders, against the scripted server and network as in
//! fetch_download.rs. "Killed" means every task of the first engine dropped
//! where it stood, with nothing saved on the way out.

mod fake;
mod fetching;
mod scratch;
mod serve;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use barrage::LinkHandle;
use barrage::bt::{TorrentConfig, Torrents};
use barrage::fetch::{CableUse, DownloadConfig, Downloads, Phase, Usage};
use barrage::keeper::{Engines, Keeper};
use barrage::links::watch::LinkTable;
use barrage::queue::Queue;
use barrage::record::{Kept, KeptState, Priority, Record, Store, What};
use fake::ms;
use fetching::*;
use serde_json::Value;
use serve::{Answer, File, Server};
use tokio::runtime::Runtime;

const LENGTH: u64 = 20 * MIB;

/// More than any one connection of [`held`] writes.
const FINISHED: u64 = MIB;

/// The file from a server that holds every answer after its first MiB, so a
/// download never gets past 8 MiB however slowly the test runs.
fn held() -> File {
    File::new(LENGTH).stall_after(MIB).etag("\"v1\"")
}

fn whole() -> File {
    File::new(LENGTH).etag("\"v1\"")
}

fn saving() -> DownloadConfig {
    DownloadConfig {
        save_every: ms(50),
        ..config()
    }
}

/// One engine: what `serve` builds, saving every 50 ms.
struct Engine {
    downloads: Downloads,
    keeper: Keeper,
}

/// Torrents with nothing to do.
async fn no_torrents(table: LinkTable, dir: PathBuf, usage: Arc<Usage>) -> Torrents {
    let config = TorrentConfig {
        network: false,
        listen: false,
        ..TorrentConfig::default()
    };
    Torrents::start(table, dir, usage, config).await.unwrap()
}

async fn open(links: &LinkHandle, state: &Folder, dir: &Folder) -> Engine {
    let (store, record, note) = Store::open(&state.0).unwrap();
    let usage = Usage::restored(links.table(), &record.used);
    let seq = Arc::new(AtomicU64::new(0));
    let downloads = Downloads::sharing(
        links.table(),
        dir.0.clone(),
        saving(),
        Arc::clone(&usage),
        seq,
    );
    downloads.restore(record.transfers.clone(), record.next_download);
    let torrents = no_torrents(links.table(), dir.0.clone(), Arc::clone(&usage)).await;
    let queue = Queue::start(downloads.clone(), torrents.clone(), record.limit);
    let keeper = Keeper::start(
        (store, record, note),
        Engines {
            downloads: downloads.clone(),
            torrents,
            queue,
            usage,
        },
        ms(50),
    );
    Engine { downloads, keeper }
}

impl Engine {
    /// A normal close: what was running goes on at the next start.
    async fn close(self) {
        let ran = self.keeper.running();
        self.downloads.shutdown().await;
        self.keeper.finish(&ran);
    }
}

/// The record in `state`, as the next engine would read it.
fn saved(state: &Folder) -> Value {
    serde_json::from_slice(&std::fs::read(state.0.join("state.json")).unwrap()).unwrap()
}

/// What state.json says of its first transfer, once it says anything.
async fn first_saved(state: &Folder, what: &str, ok: impl Fn(&Value) -> bool) -> Value {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        if let Ok(bytes) = std::fs::read(state.0.join("state.json"))
            && let Ok(all) = serde_json::from_slice::<Value>(&bytes)
            && ok(&all["transfers"][0])
        {
            return all["transfers"][0].clone();
        }
        assert!(tokio::time::Instant::now() < deadline, "never saved {what}");
        tokio::time::sleep(ms(20)).await;
    }
}

/// The bytes the progress file for `name` says are done.
fn progress_done(dir: &Folder, name: &str) -> u64 {
    let Ok(text) = std::fs::read_to_string(dir.0.join(format!("{name}.part.json"))) else {
        return 0;
    };
    let Ok(saved) = serde_json::from_str::<Value>(&text) else {
        return 0;
    };
    saved["done"].as_array().map_or(0, |all| {
        all.iter()
            .map(|r| r[1].as_u64().unwrap() - r[0].as_u64().unwrap())
            .sum()
    })
}

/// A web download's row, as a test writes it into a record.
fn web(id: u64, url: &str, dir: PathBuf, state: KeptState) -> Kept {
    Kept {
        seq: id,
        priority: Priority::Normal,
        state,
        dir,
        name: None,
        length: None,
        done: 0,
        note: None,
        what: What::Web {
            id,
            url: url.to_string(),
            cables: None,
        },
    }
}

/// Put `transfers` in `state`'s record, as an earlier engine left them.
fn left_behind(state: &Folder, transfers: Vec<Kept>) {
    let (store, _, _) = Store::open(&state.0).unwrap();
    let next_download = transfers.len() as u64;
    store
        .save(&Record {
            transfers,
            next_download,
            ..Record::default()
        })
        .unwrap();
}

#[tokio::test]
async fn a_download_cut_by_a_restart_finishes_with_the_right_hash() {
    let links = both();
    let server = Server::start().await;
    server.at("/cut.bin", Answer::File(held()));
    let (state, dir) = (Folder::new("cut-state"), Folder::new("cut"));
    let url = server.url("/cut.bin");

    let first = open(&links, &state, &dir).await;
    let id = first.downloads.add(request(&url)).await.unwrap();
    wait_for(&first.downloads, id, "parts under way", |s| {
        s.done > FINISHED
    })
    .await;
    first.close().await;
    let had = progress_done(&dir, "cut.bin");
    assert!(had > FINISHED, "{had}");
    assert_eq!(saved(&state)["transfers"][0]["state"], "run");

    server.at("/cut.bin", Answer::File(whole()));
    let before = server.seen().len();
    let next = open(&links, &state, &dir).await;
    let end = ended(&next.downloads, id).await;

    assert_eq!(end.phase, Phase::Done, "{end:?}");
    assert_eq!(end.id, id, "the same download, under the same id");
    assert_eq!(end.url, url.as_str());
    assert_same(&dir.read("cut.bin"), LENGTH);
    assert_eq!(asked(&server.seen()[before..]).covered(), LENGTH - had);
}

#[tokio::test]
async fn a_killed_engine_comes_back_from_its_last_save() {
    let links = both();
    let server = Server::start().await;
    server.at("/killed.bin", Answer::File(held()));
    let (state, dir) = (Folder::new("killed-state"), Folder::new("killed"));
    let url = server.url("/killed.bin");

    // Its own runtime, so every task of it can be dropped where it stands.
    let doomed = Runtime::new().unwrap();
    let (first, id) = {
        let (links, url) = (links.table(), url.clone());
        let (state, dir) = (state.0.clone(), dir.0.clone());
        doomed
            .spawn(async move {
                let (store, record, note) = Store::open(&state).unwrap();
                let usage = Usage::restored(links.clone(), &record.used);
                let seq = Arc::new(AtomicU64::new(0));
                let table = links.clone();
                let downloads =
                    Downloads::sharing(links, dir.clone(), saving(), Arc::clone(&usage), seq);
                let id = downloads.add(request(&url)).await.unwrap();
                let torrents = no_torrents(table, dir, Arc::clone(&usage)).await;
                let queue = Queue::start(downloads.clone(), torrents.clone(), record.limit);
                let keeper = Keeper::start(
                    (store, record, note),
                    Engines {
                        downloads: downloads.clone(),
                        torrents,
                        queue,
                        usage,
                    },
                    ms(50),
                );
                ((downloads, keeper), id)
            })
            .await
            .unwrap()
    };
    wait_for(&first.0, id, "parts under way", |s| s.done > FINISHED).await;
    first_saved(&state, "it running", |t| t["state"] == "run").await;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(20);
    while progress_done(&dir, "killed.bin") <= FINISHED {
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(ms(20)).await;
    }

    doomed.shutdown_background();
    drop(first);
    let had = progress_done(&dir, "killed.bin");

    server.at("/killed.bin", Answer::File(whole()));
    let before = server.seen().len();
    let next = open(&links, &state, &dir).await;
    let end = ended(&next.downloads, id).await;

    assert_eq!(end.phase, Phase::Done, "{end:?}");
    assert_same(&dir.read("killed.bin"), LENGTH);
    let fetched = asked(&server.seen()[before..]).covered();
    assert!(fetched <= LENGTH - had, "{fetched} of {LENGTH}, {had} kept");
}

#[tokio::test]
async fn a_stopped_download_stays_stopped() {
    let links = both();
    let server = Server::start().await;
    server.at("/stopped.bin", Answer::File(held()));
    let (state, dir) = (Folder::new("stopped-state"), Folder::new("stopped"));
    let url = server.url("/stopped.bin");

    let first = open(&links, &state, &dir).await;
    let id = first.downloads.add(request(&url)).await.unwrap();
    wait_for(&first.downloads, id, "parts under way", |s| {
        s.done > FINISHED
    })
    .await;
    first.downloads.stop(id).await.unwrap();
    let had = status_of(&first.downloads, id);
    first.close().await;
    assert_eq!(saved(&state)["transfers"][0]["state"], "stopped");

    server.at("/stopped.bin", Answer::File(whole()));
    let before = server.seen().len();
    let next = open(&links, &state, &dir).await;
    tokio::time::sleep(ms(200)).await;

    let back = status_of(&next.downloads, id);
    assert_eq!(back.phase, Phase::Stopped, "{back:?}");
    assert_eq!(back.done, had.done, "it shows what it has");
    assert_eq!(back.name.as_deref(), Some("stopped.bin"));
    assert_eq!(server.seen().len(), before, "and asks for nothing");

    next.downloads.resume(id).await.unwrap();
    let end = ended(&next.downloads, id).await;
    assert_eq!(end.phase, Phase::Done, "{end:?}");
    assert_same(&dir.read("stopped.bin"), LENGTH);
}

#[tokio::test]
async fn done_and_failed_stay_as_they_were_and_fetch_nothing() {
    let links = both();
    let server = Server::start().await;
    server.at("/done.bin", Answer::File(whole()));
    let (state, dir) = (Folder::new("done-state"), Folder::new("done"));
    let url = server.url("/done.bin").to_string();

    let done = Kept {
        name: Some("done.bin".into()),
        length: Some(LENGTH),
        done: LENGTH,
        ..web(1, &url, dir.0.clone(), KeptState::Done)
    };
    let failed = Kept {
        note: Some("the server refused it".into()),
        ..web(2, &url, dir.0.clone(), KeptState::Failed)
    };
    left_behind(&state, vec![done, failed]);

    let next = open(&links, &state, &dir).await;
    tokio::time::sleep(ms(200)).await;

    let all = next.downloads.status().borrow().clone();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].phase, Phase::Done);
    assert_eq!(all[0].name.as_deref(), Some("done.bin"));
    assert_eq!((all[0].done, all[0].length), (LENGTH, Some(LENGTH)));
    assert_eq!(all[1].phase, Phase::Failed);
    assert_eq!(all[1].note.as_deref(), Some("the server refused it"));
    assert!(server.seen().is_empty(), "{:?}", server.seen().len());
}

#[tokio::test]
async fn ids_carry_on_past_the_highest() {
    let links = both();
    let server = Server::start().await;
    server.at("/next.bin", Answer::File(whole()));
    let (state, dir) = (Folder::new("ids-state"), Folder::new("ids"));

    let (store, _, _) = Store::open(&state.0).unwrap();
    store
        .save(&Record {
            next_download: 7,
            transfers: vec![web(
                4,
                "https://example.com/a.bin",
                dir.0.clone(),
                KeptState::Done,
            )],
            ..Record::default()
        })
        .unwrap();
    drop(store);

    let next = open(&links, &state, &dir).await;
    let id = next
        .downloads
        .add(request(&server.url("/next.bin")))
        .await
        .unwrap();

    assert_eq!(id.as_u64(), 8);
}

/// Review Focus 1: the record still says running, the `.part` has gone, and
/// the file is there, so it finished in the moment before the kill.
#[tokio::test]
async fn a_download_finished_just_before_a_kill_is_done_not_fetched_again() {
    let links = both();
    let server = Server::start().await;
    server.at("/quick.bin", Answer::File(whole()));
    let (state, dir) = (Folder::new("quick-state"), Folder::new("quick"));
    std::fs::write(dir.0.join("quick.bin"), serve::content(0..LENGTH)).unwrap();
    let url = server.url("/quick.bin").to_string();
    left_behind(
        &state,
        vec![Kept {
            name: Some("quick.bin".into()),
            length: Some(LENGTH),
            done: LENGTH - MIB,
            ..web(1, &url, dir.0.clone(), KeptState::Run)
        }],
    );

    let next = open(&links, &state, &dir).await;
    tokio::time::sleep(ms(200)).await;

    let all = next.downloads.status().borrow().clone();
    assert_eq!(all[0].phase, Phase::Done, "{:?}", all[0]);
    assert_eq!(all[0].done, LENGTH);
    assert!(server.seen().is_empty(), "nothing fetched again");
    let names: Vec<_> = std::fs::read_dir(&dir.0).unwrap().collect();
    assert_eq!(names.len(), 1, "no second copy");
}

/// Review Focus 2.
#[tokio::test]
async fn a_missing_part_is_noted_and_starts_over() {
    let links = both();
    let server = Server::start().await;
    server.at("/gone.bin", Answer::File(whole()));
    let (state, dir) = (Folder::new("gone-state"), Folder::new("gone"));
    let url = server.url("/gone.bin").to_string();
    left_behind(
        &state,
        vec![Kept {
            name: Some("gone.bin".into()),
            length: Some(LENGTH),
            done: 5 * MIB,
            ..web(1, &url, dir.0.clone(), KeptState::Run)
        }],
    );

    let next = open(&links, &state, &dir).await;
    let id = next.downloads.status().borrow()[0].id;
    let end = ended(&next.downloads, id).await;

    assert_eq!(end.phase, Phase::Done, "{end:?}");
    assert_same(&dir.read("gone.bin"), LENGTH);
    assert_eq!(
        end.note.as_deref(),
        Some("what the last run saved has gone; started over")
    );
}

/// Review Focus 2.
#[tokio::test]
async fn a_missing_folder_fails_that_download_alone() {
    let links = both();
    let server = Server::start().await;
    server.at("/one.bin", Answer::File(whole()));
    server.at("/two.bin", Answer::File(whole()));
    let (state, dir) = (Folder::new("folder-state"), Folder::new("folder"));
    let gone = dir.0.join("no such folder");
    left_behind(
        &state,
        vec![
            web(1, server.url("/one.bin").as_str(), gone, KeptState::Run),
            web(
                2,
                server.url("/two.bin").as_str(),
                dir.0.clone(),
                KeptState::Run,
            ),
        ],
    );

    let next = open(&links, &state, &dir).await;
    let ids: Vec<_> = next
        .downloads
        .status()
        .borrow()
        .iter()
        .map(|s| s.id)
        .collect();
    let bad = ended(&next.downloads, ids[0]).await;
    let good = ended(&next.downloads, ids[1]).await;

    assert_eq!(bad.phase, Phase::Failed, "{bad:?}");
    assert!(bad.note.is_some());
    assert_eq!(good.phase, Phase::Done, "{good:?}");
    assert_same(&dir.read("two.bin"), LENGTH);
}

#[tokio::test]
async fn the_order_of_adding_carries_on_past_the_highest() {
    let links = both();
    let server = Server::start().await;
    server.at("/seq.bin", Answer::File(whole()));
    let (state, dir) = (Folder::new("seq-state"), Folder::new("seq"));
    let earlier = Kept {
        seq: 41,
        ..web(
            1,
            "https://example.com/a.bin",
            dir.0.clone(),
            KeptState::Done,
        )
    };
    left_behind(&state, vec![earlier]);

    let next = open(&links, &state, &dir).await;
    let id = next
        .downloads
        .add(request(&server.url("/seq.bin")))
        .await
        .unwrap();

    assert_eq!(status_of(&next.downloads, id).seq, 42);
}

#[tokio::test]
async fn what_each_cable_used_carries_over() {
    let links = both();
    let server = Server::start().await;
    server.at("/used.bin", Answer::File(whole()));
    let (state, dir) = (Folder::new("used-state"), Folder::new("used"));

    let first = open(&links, &state, &dir).await;
    let id = first
        .downloads
        .add(request(&server.url("/used.bin")))
        .await
        .unwrap();
    ended(&first.downloads, id).await;
    let used: Vec<CableUse> = first.downloads.used().borrow().clone();
    let total: u64 = used.iter().map(|u| u.bytes).sum();
    assert!(total >= LENGTH, "{used:?}");
    first.close().await;

    let next = open(&links, &state, &dir).await;

    assert_eq!(*next.downloads.used().borrow(), used);
    let id = status_of(&next.downloads, id).id;
    assert_eq!(status_of(&next.downloads, id).phase, Phase::Done);
    let since = saved(&state)["used_since"].as_u64().unwrap();
    assert!(since > 0);
}

#[tokio::test]
async fn a_save_that_fails_tells_the_page_and_the_engine_carries_on() {
    let links = both();
    let server = Server::start().await;
    server.at("/on.bin", Answer::File(whole()));
    let (state, dir) = (Folder::new("nosave-state"), Folder::new("nosave"));
    let engine = open(&links, &state, &dir).await;
    // Nothing can be renamed over a folder.
    std::fs::remove_file(state.0.join("state.json")).ok();
    std::fs::create_dir(state.0.join("state.json")).unwrap();

    let id = engine
        .downloads
        .add(request(&server.url("/on.bin")))
        .await
        .unwrap();
    let mut notice = engine.keeper.notice();
    let said = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        notice.wait_for(Option::is_some),
    )
    .await
    .expect("the page is told")
    .unwrap()
    .clone()
    .unwrap();

    assert!(said.contains("could not be saved"), "{said}");
    assert_eq!(ended(&engine.downloads, id).await.phase, Phase::Done);
    std::fs::remove_dir(state.0.join("state.json")).unwrap();
}
