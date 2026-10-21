//! A download's progress on disk, so the next engine picks it up where this
//! one stopped rather than start over (PLAN.md item 5).
//!
//! Beside each `<name>.part` is `<name>.part.json`:
//!
//! ```json
//! {"url": "https://example.com/files/ubuntu.iso", "length": 6114656256,
//!  "validator": {"etag": "\"a3f0\""}, "done": [[0, 4194304], [8388608, 9437184]]}
//! ```
//!
//! the URL, the file's length, what told this version of it apart, and the
//! byte ranges written, each start included and each end not. It is written
//! whole or not at all: to a file beside it, then renamed over it, so an
//! engine stopped mid-save leaves the last progress, never half of it.
//!
//! Its caller flushes the `.part` to disk first, so the progress never
//! claims bytes the disk has not kept. The progress itself is not forced to
//! disk: lost to a power cut, it is only the last progress, or none, and the
//! download goes back a little, or starts over.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::remote::Validator;

/// A download's progress, as saved beside its `.part`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Saved {
    /// The URL, parsed as the download used it.
    pub url: String,
    pub length: u64,
    /// The version the bytes are of; `None` if the server named none.
    pub validator: Option<Validator>,
    /// The bytes written, as `(start, end)`, the end not included.
    pub done: Vec<(u64, u64)>,
}

/// Where the progress of the download whose `.part` is `part` is saved.
pub(crate) fn progress_path(part: &Path) -> PathBuf {
    let mut path = part.as_os_str().to_owned();
    path.push(".json");
    PathBuf::from(path)
}

/// Save `saved` at `path`, replacing what was there in one step.
pub(crate) fn save(path: &Path, saved: &Saved) -> io::Result<()> {
    let text = serde_json::to_vec(saved).map_err(io::Error::other)?;
    let mut beside = path.as_os_str().to_owned();
    beside.push(".new");
    let beside = PathBuf::from(beside);
    let mut file = std::fs::File::create(&beside)?;
    file.write_all(&text)?;
    drop(file);
    // Windows can refuse the rename for a moment while something else, an
    // antivirus scan or the search indexer, holds the file just written.
    let mut tries = 0;
    loop {
        match std::fs::rename(&beside, path) {
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied && tries < 10 => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(10 * tries));
            }
            renamed => return renamed,
        }
    }
}

/// The progress saved at `path`, if there is any that can be read.
pub(crate) fn load(path: &Path) -> Option<Saved> {
    let text = std::fs::read(path).ok()?;
    serde_json::from_slice(&text).ok()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;

    /// A folder of its own for one test, emptied first.
    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("barrage-saved-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample() -> Saved {
        Saved {
            url: "https://example.com/files/ubuntu.iso".to_string(),
            length: 6_114_656_256,
            validator: Some(Validator::ETag("\"a3f0\"".to_string())),
            done: vec![(0, 4 << 20), (8 << 20, 9 << 20)],
        }
    }

    #[test]
    fn progress_round_trips_through_its_file() {
        let dir = folder("round");
        let path = progress_path(&dir.join("ubuntu.iso.part"));
        assert_eq!(path, dir.join("ubuntu.iso.part.json"));

        save(&path, &sample()).unwrap();
        assert_eq!(load(&path), Some(sample()));
        // As the plan writes it: url, length, validator, and the done ranges
        // as pairs, the end not included.
        let json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(json["url"], "https://example.com/files/ubuntu.iso");
        assert_eq!(json["length"], 6_114_656_256u64);
        assert_eq!(json["validator"]["etag"], "\"a3f0\"");
        assert_eq!(
            json["done"],
            serde_json::json!([[0, 4 << 20], [8 << 20, 9 << 20]])
        );

        // A date for a validator, and none at all.
        let dated = Saved {
            validator: Some(Validator::LastModified(
                "Tue, 14 Nov 2023 22:13:20 GMT".into(),
            )),
            ..sample()
        };
        save(&path, &dated).unwrap();
        assert_eq!(load(&path), Some(dated));
        let none = Saved {
            validator: None,
            ..sample()
        };
        save(&path, &none).unwrap();
        assert_eq!(load(&path), Some(none));
        // Saving leaves nothing else behind.
        let left: Vec<_> = std::fs::read_dir(&dir).unwrap().collect();
        assert_eq!(left.len(), 1);
    }

    /// Saved over again and again while another thread reads it, the file
    /// is always a whole one: the old progress or the new, never half.
    #[test]
    fn progress_is_never_seen_half_written() {
        let dir = folder("atomic");
        let path = progress_path(&dir.join("big.iso.part"));
        // Many ranges, so each save takes a while to write.
        let big = Saved {
            done: (0..20_000u64).map(|i| (i * 1000, i * 1000 + 500)).collect(),
            ..sample()
        };
        save(&path, &big).unwrap();

        let stop = Arc::new(AtomicBool::new(false));
        let reader = {
            let (path, big, stop) = (path.clone(), big.clone(), stop.clone());
            std::thread::spawn(move || {
                let mut reads = 0;
                while !stop.load(Ordering::SeqCst) {
                    // Windows can refuse to open a file at the instant it is
                    // replaced; that is no half-written file, so it is let
                    // go. Every file read must be a whole one.
                    let Ok(bytes) = std::fs::read(&path) else {
                        continue;
                    };
                    let read: Saved = serde_json::from_slice(&bytes)
                        .unwrap_or_else(|e| panic!("read {reads} was not a whole file: {e}"));
                    assert_eq!(read, big, "read {reads}");
                    reads += 1;
                }
                reads
            })
        };
        for _ in 0..100 {
            save(&path, &big).unwrap();
        }
        stop.store(true, Ordering::SeqCst);
        assert!(reader.join().unwrap() > 0);
    }

    #[test]
    fn a_missing_or_broken_progress_file_is_none() {
        let dir = folder("broken");
        let path = dir.join("x.part.json");
        assert_eq!(load(&path), None);
        std::fs::write(&path, "{\"url\": \"http://exa").unwrap();
        assert_eq!(load(&path), None);
        std::fs::write(&path, "[1, 2, 3]").unwrap();
        assert_eq!(load(&path), None);
    }
}
