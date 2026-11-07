//! The folders tests write to, in one place for each run of a test program:
//! `%TEMP%\barrage-tests\<pid>\`, with `<pid>.alive` beside it.
//!
//! A guard per test is not enough. A torrent session lets go of its files
//! after its test ends, and Windows will not delete an open file, so such a
//! folder outlives its guard. So the run's whole folder goes when its program
//! exits, every session gone by then, and anything a killed or failed run
//! left goes at the start of the next: `.alive` stays locked while its
//! program runs, and Windows frees the lock however it ends.
//!
//! Included by `#[path]` from the unit tests and each test file that needs
//! it, so not all of it is used by each.
#![allow(dead_code)]

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// This run: its folder, and its lock, held for as long as the program runs.
struct Run {
    dir: PathBuf,
    alive: PathBuf,
    _held: File,
}

static RUN: OnceLock<Run> = OnceLock::new();

unsafe extern "C" {
    /// The C runtime's: `callback` runs as the program exits normally.
    /// A run that ends otherwise is swept by the next.
    fn atexit(callback: extern "C" fn()) -> i32;
}

/// Where every run keeps its folder.
pub fn root() -> PathBuf {
    std::env::temp_dir().join("barrage-tests")
}

/// A folder of its own for `name`, emptied first. Two tests of one program
/// give different names, as they always had to.
pub fn folder(name: &str) -> PathBuf {
    let dir = run().dir.join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn run() -> &'static Run {
    RUN.get_or_init(|| {
        let root = root();
        fs::create_dir_all(&root).unwrap();
        let pid = std::process::id();
        // The mark and its lock first: a folder with no mark is a dead run's,
        // so another program's sweep must never see this one without it.
        let alive = root.join(format!("{pid}.alive"));
        let held = File::create(&alive).unwrap();
        held.try_lock().unwrap();
        let dir = root.join(pid.to_string());
        fs::create_dir_all(&dir).unwrap();
        sweep(&root);
        // SAFETY: `remove_run` takes no argument and does not unwind.
        unsafe { atexit(remove_run) };
        Run {
            dir,
            alive,
            _held: held,
        }
    })
}

/// Remove this run's folder as the program exits. Its mark stays if anything
/// in it could not go, so the next run finishes the job.
extern "C" fn remove_run() {
    if let Some(run) = RUN.get()
        && fs::remove_dir_all(&run.dir).is_ok()
    {
        let _ = fs::remove_file(&run.alive);
    }
}

/// Remove every run under `root` whose program is no longer running: the
/// ones whose mark can be locked, and folders that have no mark at all.
pub fn sweep(root: &Path) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if !path.with_extension("alive").exists() {
                let _ = fs::remove_dir_all(&path);
            }
            continue;
        }
        if path.extension().is_none_or(|e| e != "alive") {
            continue;
        }
        let dead = File::open(&path).is_ok_and(|mark| mark.try_lock().is_ok());
        if dead
            && (!path.with_extension("").exists()
                || fs::remove_dir_all(path.with_extension("")).is_ok())
        {
            let _ = fs::remove_file(&path);
        }
    }
}
