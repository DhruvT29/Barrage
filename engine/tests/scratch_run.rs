//! The folders tests write to: each run has one, and none outlives its run.

mod scratch;

use std::fs::{self, File};
use std::path::PathBuf;

/// A place of its own to play a whole set of runs in, away from the real one.
fn playground(name: &str) -> PathBuf {
    let root = scratch::folder(name);
    fs::create_dir_all(&root).unwrap();
    root
}

/// A run that once lived in `root`: its folder, and its `.alive` beside it.
fn run_in(root: &std::path::Path, pid: u32) -> (PathBuf, PathBuf) {
    let dir = root.join(pid.to_string());
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("left-behind.bin"), b"data").unwrap();
    let alive = root.join(format!("{pid}.alive"));
    File::create(&alive).unwrap();
    (dir, alive)
}

#[test]
fn a_folder_is_empty_and_under_this_run() {
    let dir = scratch::folder("under-this-run");
    fs::write(dir.join("old.bin"), b"data").unwrap();

    let again = scratch::folder("under-this-run");

    assert_eq!(dir, again);
    assert_eq!(fs::read_dir(&again).unwrap().count(), 0, "emptied first");
    assert!(again.starts_with(scratch::root()));
    assert!(
        again
            .components()
            .any(|c| c.as_os_str() == std::process::id().to_string().as_str()),
        "{again:?} is under this run's own folder"
    );
}

#[test]
fn the_run_of_a_dead_process_is_swept() {
    let root = playground("dead");
    let (dir, alive) = run_in(&root, 4242);

    scratch::sweep(&root);

    assert!(!dir.exists(), "its folder has gone");
    assert!(!alive.exists(), "and so has its mark");
}

#[test]
fn the_run_of_a_live_one_is_not() {
    let root = playground("live");
    let (dir, alive) = run_in(&root, 4243);
    let held = File::open(&alive).unwrap();
    held.try_lock().unwrap();

    scratch::sweep(&root);

    assert!(dir.join("left-behind.bin").exists());
    assert!(alive.exists());
    drop(held);
    scratch::sweep(&root);
    assert!(!dir.exists(), "once it has gone, the next sweep takes it");
}

#[test]
fn a_folder_with_no_mark_has_lost_its_owner() {
    let root = playground("no-mark");
    let dir = root.join("4244");
    fs::create_dir_all(&dir).unwrap();

    scratch::sweep(&root);

    assert!(!dir.exists());
}
