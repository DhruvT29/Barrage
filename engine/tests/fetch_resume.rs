//! Resuming after a restart (Task 11): a download's progress saved beside
//! its `.part` as `<name>.part.json`, and the next engine picking up where
//! the last one stopped; and one download stopped and resumed in the same
//! engine (Task 13). Against the scripted server and network as in
//! fetch_download.rs. Progress is saved every 100 ms here.

mod fake;
mod fetching;
mod serve;

use std::time::Duration;

use barrage::fetch::{ControlError, DownloadConfig, Downloads, Phase};
use fake::ms;
use fetching::*;
use serde_json::Value;
use serve::{Answer, File, Seen, Server};

/// Progress saved every 100 ms.
fn saving() -> DownloadConfig {
    DownloadConfig {
        save_every: ms(100),
        ..config()
    }
}

const LENGTH: u64 = 20 * MIB;

/// 2 MiB a second a connection: a 4 MiB chunk takes two seconds, and the
/// whole file most of two.
fn slow() -> File {
    File::new(LENGTH).rate(2 * MIB).etag("\"v1\"")
}

/// The file from a server that holds every answer after its first MiB. Of
/// its nine chunks, the five of 4 MiB (the last of them 3) stop a MiB in and
/// the four of a quarter MiB at the end finish, so the download never gets
/// past 6 MiB however slowly the test runs: an engine stopped there is
/// stopped mid-download.
fn held() -> File {
    File::new(LENGTH).stall_after(MIB)
}

/// What the finished chunks of [`held`] come to, the four at the end. Bytes
/// written past this are of chunks still under way.
const FINISHED: u64 = MIB;

/// The progress file for `name`, as the engine wrote it.
fn progress(dir: &Folder, name: &str) -> Option<Value> {
    let text = std::fs::read_to_string(dir.0.join(format!("{name}.part.json"))).ok()?;
    serde_json::from_str(&text).ok()
}

/// The bytes a progress file says are done.
fn done_in(saved: &Value) -> u64 {
    saved["done"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r[1].as_u64().unwrap() - r[0].as_u64().unwrap())
        .sum()
}

/// The file's bytes the server sent since request `from`, probes aside.
fn sent_since(server: &Server, from: usize) -> u64 {
    server.seen()[from..]
        .iter()
        .filter(|s| !is_probe(s))
        .map(Seen::sent)
        .sum()
}

/// What is in the folder, by name, in order.
fn listing(dir: &Folder) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(&dir.0)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

#[tokio::test]
async fn stopping_saves_progress_and_adding_again_resumes() {
    let links = both();
    let server = Server::start().await;
    server.at("/resume.bin", Answer::File(held().etag("\"v1\"")));
    let dir = Folder::new("resume");
    let url = server.url("/resume.bin");

    // The first engine, stopped where the server holds the download.
    let first = Downloads::with_config(links.table(), dir.0.clone(), saving());
    let id = first.add(request(&url)).await.unwrap();
    wait_for(&first, id, "chunks under way", |s| s.done > FINISHED).await;
    first.shutdown().await;
    // Stopped by the time shutdown returns, not on its way to it.
    let stopped = status_of(&first, id);
    assert_eq!(stopped.phase, Phase::Stopped, "{stopped:?}");
    assert_eq!(
        stopped.note.as_deref(),
        Some("stopped with barrage; adding it again carries on from here")
    );
    // Every byte written is saved, those of the chunks under way too.
    let saved = progress(&dir, "resume.bin").expect("no progress file");
    let had = done_in(&saved);
    assert_eq!(had, stopped.done, "{saved}");
    assert_eq!(saved["url"], url.as_str());
    assert_eq!(saved["length"], LENGTH);
    assert_eq!(saved["validator"]["etag"], "\"v1\"");

    // The next engine, the same URL, the server holding nothing back: only
    // what was missing is fetched.
    server.at(
        "/resume.bin",
        Answer::File(File::new(LENGTH).etag("\"v1\"")),
    );
    let before = server.seen().len();
    let next = Downloads::with_config(links.table(), dir.0.clone(), saving());
    let id = next.add(request(&url)).await.unwrap();
    let end = ended(&next, id).await;
    assert_eq!(end.phase, Phase::Done, "{end:?}");
    assert_eq!(end.name.as_deref(), Some("resume.bin"));
    assert_eq!(end.done, LENGTH);
    assert_same(&dir.read("resume.bin"), LENGTH);
    let note = end.note.unwrap_or_default();
    assert!(
        note.starts_with("resumed: ") && note.ends_with(" were already here"),
        "{note}"
    );
    let again = sent_since(&server, before);
    assert!(
        again <= LENGTH - had,
        "sent {again}, but only {} were missing",
        LENGTH - had
    );
    assert_eq!(listing(&dir), ["resume.bin"]);
}

/// Saved while it runs, without being asked, with what each connection has
/// written of the chunk it is on: here no big chunk ever finishes, so
/// progress on disk past the small ones can only be that, and an engine
/// killed mid-chunk loses no more than a save's worth.
#[tokio::test]
async fn progress_is_on_disk_while_it_runs() {
    let links = both();
    // Held connections are not given up on in the time this takes.
    let config = DownloadConfig {
        read_deadline: Duration::from_secs(60),
        ..saving()
    };
    let (_server, dir, downloads, id) = start_with("ongoing.bin", held(), &links, config).await;
    wait_for(&downloads, id, "chunks under way", |s| s.done > FINISHED).await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let on_disk = progress(&dir, "ongoing.bin").map(|saved| done_in(&saved));
        if on_disk.is_some_and(|done| done > FINISHED) {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{on_disk:?} bytes saved as done: the finished chunks at most"
        );
        tokio::time::sleep(ms(20)).await;
    }
}

#[tokio::test]
async fn a_changed_file_starts_over() {
    let links = both();
    let server = Server::start().await;
    server.at("/changes.bin", Answer::File(held().etag("\"v1\"")));
    let dir = Folder::new("changes");
    let url = server.url("/changes.bin");
    let first = Downloads::with_config(links.table(), dir.0.clone(), saving());
    let id = first.add(request(&url)).await.unwrap();
    wait_for(&first, id, "chunks under way", |s| s.done > FINISHED).await;
    first.shutdown().await;
    assert!(progress(&dir, "changes.bin").is_some());

    // Replaced on the server meanwhile: another version, the same length.
    let other = File::new(LENGTH).etag("\"v2\"");
    server.at("/changes.bin", Answer::File(other));
    let before = server.seen().len();
    let next = Downloads::with_config(links.table(), dir.0.clone(), saving());
    let id = next.add(request(&url)).await.unwrap();
    let end = ended(&next, id).await;
    assert_eq!(end.phase, Phase::Done, "{end:?}");
    assert_eq!(
        end.note.as_deref(),
        Some("the file changed on the server; started again")
    );
    assert_same(&dir.read("changes.bin"), LENGTH);
    // Fetched in full: nothing kept of the old version.
    assert_eq!(sent_since(&server, before), LENGTH);
    assert_eq!(listing(&dir), ["changes.bin"]);
}

/// A download that fails keeps what it had for a later attempt to decide
/// about, saved by the time it shows as failed. Here nothing is saved while
/// it runs, so the progress is the one saved as it failed.
#[tokio::test]
async fn a_failed_download_keeps_its_progress() {
    let links = both();
    let config = DownloadConfig {
        save_every: Duration::from_secs(3600),
        ..config()
    };
    let file = held().etag("\"v1\"");
    let (server, dir, downloads, id) = start_with("fails.bin", file, &links, config).await;
    wait_for(&downloads, id, "chunks under way", |s| s.done > FINISHED).await;
    assert_eq!(progress(&dir, "fails.bin"), None);

    // The link watcher gone, the download cannot go on.
    drop(links);
    let end = ended(&downloads, id).await;
    assert_eq!(end.phase, Phase::Failed, "{end:?}");
    assert_eq!(end.note.as_deref(), Some("the link watcher stopped"));
    // What was written as it failed, chunks under way too. Its connections,
    // stopping, may still write what they had.
    let saved = progress(&dir, "fails.bin").expect("no progress file");
    let had = done_in(&saved);
    assert!(
        had > FINISHED && had <= end.done,
        "{had} saved, {} written",
        end.done
    );
    assert_eq!(saved["url"], server.url("/fails.bin").as_str());
    assert_eq!(saved["validator"]["etag"], "\"v1\"");
}

#[tokio::test]
async fn a_finished_download_leaves_no_progress_file() {
    let links = both();
    let (_server, dir, downloads, id) = start_with("finished.bin", slow(), &links, saving()).await;
    let end = ended(&downloads, id).await;
    assert_eq!(end.phase, Phase::Done, "{end:?}");
    assert_eq!(listing(&dir), ["finished.bin"]);
}

/// A server that names no version of its file gives nothing to tell whether
/// the file changed while the engine was stopped, so what was saved is not
/// trusted: it starts over.
#[tokio::test]
async fn a_file_with_no_version_starts_over() {
    let links = both();
    let server = Server::start().await;
    server.at("/unnamed.bin", Answer::File(held()));
    let dir = Folder::new("no-version");
    let url = server.url("/unnamed.bin");
    let first = Downloads::with_config(links.table(), dir.0.clone(), saving());
    let id = first.add(request(&url)).await.unwrap();
    wait_for(&first, id, "chunks under way", |s| s.done > FINISHED).await;
    first.shutdown().await;
    let saved = progress(&dir, "unnamed.bin").expect("no progress file");
    assert_eq!(saved["validator"], Value::Null);

    server.at("/unnamed.bin", Answer::File(File::new(LENGTH)));
    let before = server.seen().len();
    let next = Downloads::with_config(links.table(), dir.0.clone(), saving());
    let id = next.add(request(&url)).await.unwrap();
    let end = ended(&next, id).await;
    assert_eq!(end.phase, Phase::Done, "{end:?}");
    assert_eq!(
        end.note.as_deref(),
        Some(
            "nothing from the server told whether the file was still the same, so it started \
             again"
        )
    );
    assert_same(&dir.read("unnamed.bin"), LENGTH);
    assert_eq!(sent_since(&server, before), LENGTH);
}

/// Another download's progress, under the name this one would take, is not
/// this one's: it is left as it is, and this one takes the next name.
#[tokio::test]
async fn progress_for_another_url_is_left_alone() {
    let links = both();
    let server = Server::start().await;
    server.at("/taken.bin", Answer::File(slow()));
    let dir = Folder::new("taken-progress");
    let theirs = r#"{"url":"http://example.com/other/taken.bin","length":100,"validator":{"etag":"\"v1\""},"done":[[0,50]]}"#;
    std::fs::write(dir.0.join("taken.bin.part"), vec![7u8; 100]).unwrap();
    std::fs::write(dir.0.join("taken.bin.part.json"), theirs).unwrap();

    let downloads = Downloads::with_config(links.table(), dir.0.clone(), saving());
    let id = downloads
        .add(request(&server.url("/taken.bin")))
        .await
        .unwrap();
    let end = ended(&downloads, id).await;
    assert_eq!(end.phase, Phase::Done, "{end:?}");
    assert_eq!(end.name.as_deref(), Some("taken (1).bin"));
    assert_eq!(end.note, None);
    assert_same(&dir.read("taken (1).bin"), LENGTH);
    assert_eq!(dir.read("taken.bin.part"), vec![7u8; 100]);
    assert_eq!(
        std::fs::read_to_string(dir.0.join("taken.bin.part.json")).unwrap(),
        theirs
    );
}

/// A file the server sends only whole cannot pick up where it stopped, so
/// no progress is kept for it, and adding it again starts over, beside what
/// the last attempt left.
#[tokio::test]
async fn a_whole_download_starts_over() {
    let links = both();
    let server = Server::start().await;
    let whole = File::new(LENGTH).no_ranges().stall_after(MIB);
    server.at("/whole.bin", Answer::File(whole));
    let dir = Folder::new("whole-again");
    let url = server.url("/whole.bin");
    let first = Downloads::with_config(links.table(), dir.0.clone(), saving());
    let id = first.add(request(&url)).await.unwrap();
    wait_for(&first, id, "data", |s| s.done > 0).await;
    first.shutdown().await;
    // Stopped, its one connection told to: not a cable gone.
    let stopped = status_of(&first, id);
    assert_eq!(stopped.phase, Phase::Stopped, "{stopped:?}");
    assert_eq!(
        stopped.note.as_deref(),
        Some("stopped with barrage; adding it again starts it over")
    );
    assert!(progress(&dir, "whole.bin").is_none());

    server.at("/whole.bin", Answer::File(File::new(LENGTH).no_ranges()));
    let next = Downloads::with_config(links.table(), dir.0.clone(), saving());
    let id = next.add(request(&url)).await.unwrap();
    let end = ended(&next, id).await;
    assert_eq!(end.phase, Phase::Done, "{end:?}");
    assert_eq!(end.name.as_deref(), Some("whole (1).bin"));
    assert_same(&dir.read("whole (1).bin"), LENGTH);
}

// One download stopped and resumed, the engine running on (Task 13).

#[tokio::test]
async fn stopping_one_download_saves_it_and_resuming_carries_on() {
    let links = both();
    let server = Server::start().await;
    server.at("/paused.bin", Answer::File(held().etag("\"v1\"")));
    let dir = Folder::new("paused");
    let url = server.url("/paused.bin");
    let downloads = Downloads::with_config(links.table(), dir.0.clone(), saving());
    let id = downloads.add(request(&url)).await.unwrap();
    wait_for(&downloads, id, "chunks under way", |s| s.done > FINISHED).await;

    downloads.stop(id).await.unwrap();
    // Stopped by the time stop returns, every byte written saved.
    let stopped = status_of(&downloads, id);
    assert_eq!(stopped.phase, Phase::Stopped, "{stopped:?}");
    assert_eq!(
        stopped.note.as_deref(),
        Some("stopped; resuming carries on from here")
    );
    let saved = progress(&dir, "paused.bin").expect("no progress file");
    let had = done_in(&saved);
    assert_eq!(had, stopped.done, "{saved}");

    // Resumed, the same download carries on, the server holding nothing
    // back now: only what was missing is fetched.
    server.at(
        "/paused.bin",
        Answer::File(File::new(LENGTH).etag("\"v1\"")),
    );
    let before = server.seen().len();
    downloads.resume(id).await.unwrap();
    assert_ne!(status_of(&downloads, id).phase, Phase::Stopped);
    let end = ended(&downloads, id).await;
    assert_eq!(end.phase, Phase::Done, "{end:?}");
    assert_eq!(end.name.as_deref(), Some("paused.bin"));
    assert_eq!(end.done, LENGTH);
    let note = end.note.unwrap_or_default();
    assert!(
        note.starts_with("resumed: ") && note.ends_with(" were already here"),
        "{note}"
    );
    let again = sent_since(&server, before);
    assert!(
        again <= LENGTH - had,
        "sent {again}, but only {} were missing",
        LENGTH - had
    );
    assert_same(&dir.read("paused.bin"), LENGTH);
    assert_eq!(listing(&dir), ["paused.bin"]);
    // One download all along, under its one id.
    assert_eq!(downloads.status().borrow().len(), 1);
}

#[tokio::test]
async fn stopping_one_download_leaves_the_others_running() {
    let links = both();
    let server = Server::start().await;
    server.at("/kept.bin", Answer::File(slow()));
    server.at("/stopped.bin", Answer::File(held().etag("\"v1\"")));
    let dir = Folder::new("one-of-two");
    let downloads = Downloads::with_config(links.table(), dir.0.clone(), saving());
    let kept = downloads
        .add(request(&server.url("/kept.bin")))
        .await
        .unwrap();
    let stopped = downloads
        .add(request(&server.url("/stopped.bin")))
        .await
        .unwrap();
    wait_for(&downloads, stopped, "chunks under way", |s| {
        s.done > FINISHED
    })
    .await;
    // Two seconds a chunk: far from done.
    assert_eq!(status_of(&downloads, kept).phase, Phase::Downloading);

    downloads.stop(stopped).await.unwrap();
    let end = ended(&downloads, kept).await;
    assert_eq!(end.phase, Phase::Done, "{end:?}");
    assert_eq!(end.note, None);
    assert_same(&dir.read("kept.bin"), LENGTH);
    assert_eq!(status_of(&downloads, stopped).phase, Phase::Stopped);
}

#[tokio::test]
async fn only_a_running_download_stops_and_only_a_stopped_one_resumes() {
    let links = both();
    let file = File::new(4 * MIB).rate(4 * MIB).etag("\"v1\"");
    let (_server, _dir, downloads, id) = start_with("asked.bin", file, &links, saving()).await;
    // Running: nothing to resume.
    assert_eq!(downloads.resume(id).await, Err(ControlError::NotStopped));
    let end = ended(&downloads, id).await;
    assert_eq!(end.phase, Phase::Done, "{end:?}");
    // Done: nothing to stop, nor to resume.
    assert_eq!(downloads.stop(id).await, Err(ControlError::NotRunning));
    assert_eq!(downloads.resume(id).await, Err(ControlError::NotStopped));
    assert_eq!(status_of(&downloads, id).phase, Phase::Done);
}

/// Added again while stopped, the file is the new download's; the stopped
/// one cannot take it back while that one runs.
#[tokio::test]
async fn a_stopped_download_does_not_resume_while_its_file_runs_as_another() {
    let links = both();
    let server = Server::start().await;
    server.at("/twice.bin", Answer::File(held().etag("\"v1\"")));
    let dir = Folder::new("twice");
    let url = server.url("/twice.bin");
    let downloads = Downloads::with_config(links.table(), dir.0.clone(), saving());
    let first = downloads.add(request(&url)).await.unwrap();
    wait_for(&downloads, first, "chunks under way", |s| s.done > FINISHED).await;
    downloads.stop(first).await.unwrap();

    let second = downloads.add(request(&url)).await.unwrap();
    assert_ne!(second, first);
    assert_eq!(
        downloads.resume(first).await,
        Err(ControlError::AlreadyRunning(second))
    );
    assert_eq!(status_of(&downloads, first).phase, Phase::Stopped);
}

/// A file the server sends only whole keeps no progress, so stopping it
/// says that resuming starts it over.
#[tokio::test]
async fn stopping_a_whole_download_says_it_starts_over() {
    let links = both();
    let whole = File::new(LENGTH).no_ranges().stall_after(MIB);
    let (_server, dir, downloads, id) = start_with("whole-stop.bin", whole, &links, saving()).await;
    wait_for(&downloads, id, "data", |s| s.done > 0).await;
    downloads.stop(id).await.unwrap();
    let stopped = status_of(&downloads, id);
    assert_eq!(stopped.phase, Phase::Stopped, "{stopped:?}");
    assert_eq!(
        stopped.note.as_deref(),
        Some("stopped; resuming starts it over")
    );
    assert!(progress(&dir, "whole-stop.bin").is_none());
}
