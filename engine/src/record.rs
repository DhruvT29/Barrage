//! The engine's saved record of every transfer, so a restart loses nothing.
//!
//! One file, `state.json`, in the engine's own folder, apart from the
//! downloads: what each transfer is, its state and where it saves, the limit
//! on what runs at once, and the data each cable has used. A web download's
//! progress stays in its `.part.json` and a torrent's in its pieces: this
//! says what there is, and they say how far it got.
//!
//! It is replaced whole (`fetch::replace`), so an engine killed while saving
//! leaves the last good record. One that cannot be read never stops the
//! engine and is never lost: it is moved aside, the engine starts with an
//! empty list, and the note says so.
//!
//! `state.lock` stays locked for as long as a [`Store`] lives. Windows frees
//! the lock however a program ends, so a killed engine never blocks the next,
//! and a second engine on one folder stops at the start rather than the two
//! writing over each other.
//!
//! `running` is put down at the start and taken up by [`Store::close`], so
//! finding it at the start means the last engine did not end cleanly. The
//! fork flushes a torrent's saved bitfield every two seconds but never syncs
//! the pieces, so after a power cut or a crash the bitfield can name pieces
//! that never reached the disk. Then the bitfield of each torrent still to
//! run is deleted, and its session checks every piece. A killed engine
//! counts too: the marker cannot tell it from a power cut.
//!
//! The engine syncs a torrent's files itself, twice: when it finishes, and
//! it is saved done only after that, and when the torrents shut down, and
//! the marker is taken up only after that (`Keeper::finish`). So a done row
//! is on the disk, and a clean end leaves no piece only in Windows' cache.

use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context as _;
use serde::{Deserialize, Serialize};

use crate::bt::fastresume_file;
use crate::fetch::replace;

/// The record this engine writes, and the newest it can read.
pub const VERSION: u32 = 1;

/// How many transfers run at once until the user says otherwise.
pub const DEFAULT_LIMIT: u32 = 3;

const FILE: &str = "state.json";
const LOCK: &str = "state.lock";
const RUNNING: &str = "running";

/// Who goes next: high first, then the earliest added.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    High,
    #[default]
    Normal,
    Low,
}

/// What a transfer should do when the engine comes back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeptState {
    /// Running or queued when saved: it goes on.
    Run,
    Stopped,
    Done,
    Failed,
}

/// What a transfer is, and what it takes to start it again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum What {
    Web {
        id: u64,
        url: String,
        cables: Option<Vec<String>>,
    },
    Torrent {
        id: u64,
        info_hash: String,
        /// The link it was added from, until its metadata is known. After
        /// that its `.torrent` is kept in the engine's folder.
        magnet: Option<String>,
        /// The folder librqbit chose for it, once known.
        output: Option<PathBuf>,
    },
}

/// One transfer as saved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Kept {
    /// Its place in the order of adding, across web downloads and torrents.
    pub seq: u64,
    pub priority: Priority,
    pub state: KeptState,
    /// The folder it saves to: a later `--downloads` is for new ones.
    pub dir: PathBuf,
    pub name: Option<String>,
    pub length: Option<u64>,
    pub done: u64,
    pub note: Option<String>,
    pub what: What,
}

/// The data one cable has moved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeptUse {
    pub cable: String,
    pub bytes: u64,
    pub sent: u64,
}

/// Everything saved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub version: u32,
    pub limit: u32,
    /// When the data used was last zeroed, in Unix seconds.
    pub used_since: u64,
    pub used: Vec<KeptUse>,
    /// The highest ids ever given, so none is used twice.
    pub next_download: u64,
    pub next_torrent: u64,
    pub transfers: Vec<Kept>,
}

impl Default for Record {
    fn default() -> Self {
        Self {
            version: VERSION,
            limit: DEFAULT_LIMIT,
            used_since: 0,
            used: Vec::new(),
            next_download: 0,
            next_torrent: 0,
            transfers: Vec::new(),
        }
    }
}

/// Seconds since the Unix epoch.
pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// What [`Store::open`] gives: the hold on the folder, what it held, and why
/// that was set aside if it was.
pub type Opened = (Store, Record, Option<String>);

/// The engine's hold on its folder. Only one lives per folder at a time.
#[derive(Debug)]
pub struct Store {
    path: PathBuf,
    /// Keeps `state.lock` locked; dropping it lets the next engine in.
    _lock: File,
    /// Whether `running` was there at the start.
    unclean: bool,
}

impl Store {
    /// Take `dir`, made if it is missing, and read what it holds: the record,
    /// and what the user should know if one had to be set aside.
    ///
    /// An error means this engine cannot go on: the folder cannot be used, or
    /// another engine is using it. A record that cannot be read is no error.
    pub fn open(dir: &Path) -> anyhow::Result<Opened> {
        fs::create_dir_all(dir)
            .with_context(|| format!("cannot make the state folder {}", dir.display()))?;
        let lock = File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join(LOCK))
            .with_context(|| format!("cannot use the state folder {}", dir.display()))?;
        lock.try_lock().map_err(|_| {
            anyhow::anyhow!(
                "another BARRAGE is using the state folder {}. Stop it, or give this one a \
                 folder of its own with --state DIR",
                dir.display()
            )
        })?;
        let path = dir.join(FILE);
        let (record, note) = read(&path)?;
        let unclean = dir.join(RUNNING).exists();
        if unclean {
            forget_saved_bitfields(dir, &record);
        }
        mark_running(&dir.join(RUNNING))
            .with_context(|| format!("cannot use the state folder {}", dir.display()))?;
        Ok((
            Store {
                path,
                _lock: lock,
                unclean,
            },
            record,
            note,
        ))
    }

    /// Whether the engine before this one did not end cleanly. The saved
    /// bitfields of the torrents still to run have been deleted if so.
    pub fn after_unclean_end(&self) -> bool {
        self.unclean
    }

    /// The engine has stopped: the next one finds no sign of a bad end. Call
    /// it after the torrents have shut down.
    pub fn close(self) {
        if let Err(e) = fs::remove_file(self.folder().join(RUNNING))
            && e.kind() != io::ErrorKind::NotFound
        {
            tracing::warn!("cannot take up the running marker: {e}");
        }
    }

    /// Replace the record with `record`, whole.
    pub fn save(&self, record: &Record) -> io::Result<()> {
        let text = serde_json::to_vec_pretty(record).map_err(io::Error::other)?;
        replace(&self.path, &text)
    }

    /// The folder this store holds.
    pub fn folder(&self) -> &Path {
        self.path.parent().expect("the record is in a folder")
    }
}

/// Put `running` down and sync it, so it is on the disk before any download
/// starts.
fn mark_running(path: &Path) -> io::Result<()> {
    let file = File::create(path)?;
    file.sync_all()
}

/// Delete the saved bitfield of each torrent in `record` that will be given
/// to a session again: one to run, or stopped. A done torrent is never given
/// to one, so its pieces are not checked either way, and a failed one is
/// removed or added again by the user, which clears its bitfield.
fn forget_saved_bitfields(dir: &Path, record: &Record) {
    for kept in &record.transfers {
        if let What::Torrent { info_hash, .. } = &kept.what
            && matches!(kept.state, KeptState::Run | KeptState::Stopped)
            && let Some(bitfield) = fastresume_file(dir, info_hash)
        {
            let _ = fs::remove_file(bitfield);
        }
    }
}

/// What is at `path`, or a new record and why the old one was set aside.
fn read(path: &Path) -> anyhow::Result<(Record, Option<String>)> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok((fresh(), None)),
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
    };
    let why = match serde_json::from_slice::<serde_json::Value>(&bytes) {
        Ok(value)
            if value["version"]
                .as_u64()
                .is_some_and(|v| v > VERSION.into()) =>
        {
            "was saved by a newer BARRAGE"
        }
        Ok(value) => match serde_json::from_value::<Record>(value) {
            Ok(record) => return Ok((record, None)),
            Err(_) => "could not be read",
        },
        Err(_) => "could not be read",
    };
    let aside = set_aside(path)?;
    let name = aside.file_name().unwrap_or_default().display();
    Ok((
        fresh(),
        Some(format!(
            "The saved list {why}, so the engine started with an empty one. It was kept as {name}."
        )),
    ))
}

/// A record for a folder with none.
fn fresh() -> Record {
    Record {
        used_since: unix_now(),
        ..Record::default()
    }
}

/// Move the record at `path` out of the way, under a name nothing else has.
fn set_aside(path: &Path) -> anyhow::Result<PathBuf> {
    let now = unix_now();
    let dir = path.parent().expect("the record is in a folder");
    let aside = (0..)
        .map(|n| match n {
            0 => dir.join(format!("state.broken-{now}.json")),
            n => dir.join(format!("state.broken-{now}-{n}.json")),
        })
        .find(|name| !name.exists())
        .expect("the names never run out");
    fs::rename(path, &aside).with_context(|| format!("cannot move {} aside", path.display()))?;
    Ok(aside)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> PathBuf {
        crate::scratch::folder(&format!("record-{name}"))
    }

    fn sample() -> Record {
        Record {
            limit: 5,
            used_since: 1_700_000_000,
            used: vec![KeptUse {
                cable: "Wi-Fi".into(),
                bytes: 10,
                sent: 3,
            }],
            next_download: 7,
            next_torrent: 2,
            transfers: vec![
                Kept {
                    seq: 1,
                    priority: Priority::High,
                    state: KeptState::Run,
                    dir: PathBuf::from("D:/Downloads"),
                    name: Some("ubuntu.iso".into()),
                    length: Some(4 << 30),
                    done: 1 << 30,
                    note: None,
                    what: What::Web {
                        id: 7,
                        url: "https://example.com/ubuntu.iso".into(),
                        cables: Some(vec!["Wi-Fi".into()]),
                    },
                },
                Kept {
                    seq: 2,
                    priority: Priority::Low,
                    state: KeptState::Stopped,
                    dir: PathBuf::from("D:/Downloads"),
                    name: None,
                    length: None,
                    done: 0,
                    note: Some("stopped".into()),
                    what: What::Torrent {
                        id: 2,
                        info_hash: "ab".repeat(20),
                        magnet: Some("magnet:?xt=urn:btih:abab".into()),
                        output: None,
                    },
                },
            ],
            ..Record::default()
        }
    }

    #[test]
    fn a_record_round_trips() {
        let dir = folder("round");
        let (store, _, _) = Store::open(&dir).unwrap();
        store.save(&sample()).unwrap();
        drop(store);

        let (_store, record, note) = Store::open(&dir).unwrap();

        assert_eq!(record, sample());
        assert_eq!(note, None);
        let names: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(
            names.len(),
            3,
            "the record, its lock and the marker: {names:?}"
        );
    }

    #[test]
    fn a_priority_is_a_word_on_disk() {
        assert_eq!(serde_json::to_string(&Priority::High).unwrap(), "\"high\"");
        assert!(Priority::High < Priority::Normal && Priority::Normal < Priority::Low);
        assert_eq!(Priority::default(), Priority::Normal);
    }

    #[test]
    fn a_folder_with_no_record_opens_empty() {
        let dir = folder("empty").join("not yet there");

        let (_store, record, note) = Store::open(&dir).unwrap();

        assert_eq!(record.limit, 3);
        assert_eq!(record.version, VERSION);
        assert!(record.transfers.is_empty());
        assert!(record.used_since > 0, "the count starts now");
        assert_eq!(note, None);
    }

    /// The one file in `dir` that was set aside, and its name.
    fn aside_in(dir: &Path) -> (PathBuf, String) {
        let found: Vec<PathBuf> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.to_string_lossy().contains("state.broken-"))
            .collect();
        assert_eq!(found.len(), 1, "{found:?}");
        let name = found[0].file_name().unwrap().to_str().unwrap().to_string();
        (found[0].clone(), name)
    }

    #[test]
    fn a_damaged_record_is_set_aside() {
        let dir = folder("damaged");
        let broken = b"{\"version\": 1, \"transfers\": [{\"seq\"";
        fs::write(dir.join(FILE), broken).unwrap();

        let (_store, record, note) = Store::open(&dir).unwrap();

        assert!(record.transfers.is_empty());
        assert_eq!(record.limit, 3);
        let (aside, name) = aside_in(&dir);
        assert_eq!(fs::read(aside).unwrap(), broken, "its bytes are kept");
        let note = note.expect("the user is told");
        assert!(note.contains(&name), "{note}");
        assert!(note.contains("could not be read"), "{note}");
        assert!(!dir.join(FILE).exists());
    }

    #[test]
    fn valid_json_of_the_wrong_shape_is_damaged_too() {
        let dir = folder("shape");
        fs::write(dir.join(FILE), "[1, 2, 3]").unwrap();

        let (_store, record, note) = Store::open(&dir).unwrap();

        assert!(record.transfers.is_empty());
        assert!(note.is_some());
    }

    #[test]
    fn a_record_from_a_newer_version_is_set_aside_not_lost() {
        let dir = folder("newer");
        let newer = format!("{{\"version\": {}, \"more\": true}}", VERSION + 1);
        fs::write(dir.join(FILE), &newer).unwrap();

        let (_store, record, note) = Store::open(&dir).unwrap();

        assert!(record.transfers.is_empty());
        let (aside, _) = aside_in(&dir);
        assert_eq!(fs::read_to_string(aside).unwrap(), newer);
        assert!(note.unwrap().contains("newer BARRAGE"));
    }

    #[test]
    fn two_set_aside_in_one_second_keep_both() {
        let dir = folder("twice");
        fs::write(dir.join(FILE), "one").unwrap();
        let first = set_aside(&dir.join(FILE)).unwrap();
        fs::write(dir.join(FILE), "two").unwrap();
        let second = set_aside(&dir.join(FILE)).unwrap();
        assert_ne!(first, second);
        assert_eq!(fs::read_to_string(first).unwrap(), "one");
        assert_eq!(fs::read_to_string(second).unwrap(), "two");
    }

    #[test]
    fn an_engine_killed_while_saving_leaves_the_last_good_one() {
        let dir = folder("killed");
        let (store, _, _) = Store::open(&dir).unwrap();
        store.save(&sample()).unwrap();
        drop(store);
        // What a kill between writing and renaming leaves.
        fs::write(dir.join("state.json.new"), "{\"version\": 1, \"lim").unwrap();

        let (store, record, note) = Store::open(&dir).unwrap();

        assert_eq!(record, sample());
        assert_eq!(note, None);
        store.save(&Record::default()).unwrap();
        assert!(
            !dir.join("state.json.new").exists(),
            "the next save tidies it"
        );
    }

    /// Review Focus 5: `--port` lets a second engine run.
    #[test]
    fn a_second_engine_on_the_same_folder_is_refused() {
        let dir = folder("second");
        let (first, _, _) = Store::open(&dir).unwrap();

        let refused = Store::open(&dir).unwrap_err().to_string();

        assert!(refused.contains("another BARRAGE"), "{refused}");
        assert!(refused.contains(&dir.display().to_string()), "{refused}");
        drop(first);
        Store::open(&dir).expect("free again once the first has gone");
    }

    /// A torrent's row, with the fast resume file it would have on disk.
    fn with_resume(dir: &Path, id: u64, state: KeptState) -> (Kept, PathBuf) {
        let hash = format!("{id:02x}").repeat(20);
        let file = dir.join("resume").join(format!("{hash}.bitv"));
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, [0xff]).unwrap();
        let kept = Kept {
            seq: id,
            priority: Priority::Normal,
            state,
            dir: PathBuf::from("D:/Downloads"),
            name: None,
            length: None,
            done: 0,
            note: None,
            what: What::Torrent {
                id,
                info_hash: hash,
                magnet: None,
                output: None,
            },
        };
        (kept, file)
    }

    #[test]
    fn after_an_unclean_end_the_fast_resume_data_of_torrents_still_to_run_goes() {
        let dir = folder("unclean");
        let (store, _, _) = Store::open(&dir).unwrap();
        let (run, run_file) = with_resume(&dir, 1, KeptState::Run);
        let (stopped, stopped_file) = with_resume(&dir, 2, KeptState::Stopped);
        let (done, done_file) = with_resume(&dir, 3, KeptState::Done);
        store
            .save(&Record {
                transfers: vec![run, stopped, done],
                ..Record::default()
            })
            .unwrap();
        // Not closed: the engine was killed, or the machine lost power.
        drop(store);

        let (store, _, _) = Store::open(&dir).unwrap();

        assert!(store.after_unclean_end());
        assert!(!run_file.exists(), "a running torrent is checked in full");
        assert!(!stopped_file.exists(), "and so is a stopped one");
        assert!(
            done_file.exists(),
            "a done torrent is never given to a session"
        );
    }

    #[test]
    fn after_a_clean_end_the_fast_resume_data_stays() {
        let dir = folder("clean");
        let (store, _, _) = Store::open(&dir).unwrap();
        let (run, run_file) = with_resume(&dir, 1, KeptState::Run);
        store
            .save(&Record {
                transfers: vec![run],
                ..Record::default()
            })
            .unwrap();
        store.close();

        let (store, _, _) = Store::open(&dir).unwrap();

        assert!(!store.after_unclean_end());
        assert!(run_file.exists());
    }

    #[test]
    fn a_new_folder_is_not_an_unclean_end() {
        let dir = folder("new-folder");

        let (store, _, _) = Store::open(&dir).unwrap();

        assert!(!store.after_unclean_end());
        assert!(dir.join("running").exists(), "the marker is down at once");
    }

    #[test]
    fn a_hash_that_is_not_a_hash_deletes_nothing_outside_the_resume_folder() {
        let dir = folder("bad-hash");
        let (store, _, _) = Store::open(&dir).unwrap();
        let (mut run, _) = with_resume(&dir, 1, KeptState::Run);
        if let What::Torrent { info_hash, .. } = &mut run.what {
            *info_hash = "../outside".into();
        }
        let outside = dir.join("outside.bitv");
        fs::write(&outside, [0xff]).unwrap();
        store
            .save(&Record {
                transfers: vec![run],
                ..Record::default()
            })
            .unwrap();
        drop(store);

        let (store, _, _) = Store::open(&dir).unwrap();

        assert!(store.after_unclean_end());
        assert!(outside.exists(), "a file outside resume was deleted");
    }
}
