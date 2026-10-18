//! The file on disk: full size before any data arrives, written at offsets
//! by many workers at once, and named only when complete, never over another
//! file.
//!
//! NOT SPARSE
//!
//! The `.part` is sized with `set_len` and not marked sparse, so a full disk
//! or a folder that cannot be written fails here, before the first byte
//! (PLAN.md section 3.6, item 4). Chunks are claimed in ascending order, so
//! NTFS only fills zeros a few chunks ahead of the lowest unfinished byte.

use std::fs::{File, OpenOptions};
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};

use super::name::free_name;

/// How often a new name is chosen when another program takes it first.
const NAME_TRIES: usize = 10;

/// A new, full-size `<name>.part` in `dir`, for the first free name like
/// `name`: that name, the part's path, and the open file.
pub(crate) fn create_part(
    dir: &Path,
    name: &str,
    length: u64,
) -> Result<(String, PathBuf, File), String> {
    for _ in 0..NAME_TRIES {
        let free = free_name(dir, name);
        let part = dir.join(format!("{free}.part"));
        let opened = OpenOptions::new().write(true).create_new(true).open(&part);
        let file = match opened {
            Ok(file) => file,
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return Err(format!(
                    "cannot write to the download folder {}: {e}",
                    dir.display()
                ));
            }
        };
        if let Err(e) = file.set_len(length) {
            drop(file);
            let _ = std::fs::remove_file(&part);
            return Err(format!(
                "no room for the file ({length} bytes) in {}: {e}",
                dir.display()
            ));
        }
        return Ok((free, part, file));
    }
    Err(format!(
        "every name like {name} in {} was taken while choosing one",
        dir.display()
    ))
}

/// Write all of `data` at `offset`, without moving any shared position, so
/// many threads can write one file at once.
pub(crate) fn write_all_at(file: &File, mut data: &[u8], mut offset: u64) -> io::Result<()> {
    while !data.is_empty() {
        #[cfg(windows)]
        let n = std::os::windows::fs::FileExt::seek_write(file, data, offset)?;
        #[cfg(not(windows))]
        let n = std::os::unix::fs::FileExt::write_at(file, data, offset)?;
        if n == 0 {
            return Err(io::Error::new(
                ErrorKind::WriteZero,
                "the disk took no bytes",
            ));
        }
        data = &data[n..];
        offset += n as u64;
    }
    Ok(())
}

/// Give the finished `part` its name: `name` in the same folder, or the next
/// free one if something took `name` meanwhile. Returns the name used.
pub(crate) fn name_finished(part: &Path, name: &str) -> Result<String, String> {
    let dir = part.parent().unwrap_or(Path::new("."));
    let mut chosen = name.to_string();
    for _ in 0..NAME_TRIES {
        match rename_new(part, &chosen) {
            Ok(()) => return Ok(chosen),
            Err(e) if e.kind() == ErrorKind::AlreadyExists => chosen = free_name(dir, name),
            Err(e) => {
                return Err(format!(
                    "cannot name the finished file {}: {e}",
                    dir.join(&chosen).display()
                ));
            }
        }
    }
    Err(format!(
        "every name like {name} in {} was taken while naming the file",
        dir.display()
    ))
}

/// Rename `part` to `name` in its own folder, failing if `name` exists.
/// std's `rename` would replace it.
#[cfg(windows)]
fn rename_new(part: &Path, name: &str) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_WRITE_THROUGH, MoveFileExW};

    // Canonical, so the `\\?\` form: a long folder path plus a long name can
    // pass the 260 characters older Windows calls stop at.
    let from = std::fs::canonicalize(part)?;
    let to = from.with_file_name(name);
    let wide = |p: &Path| -> Vec<u16> { p.as_os_str().encode_wide().chain([0]).collect() };
    let (from, to) = (wide(&from), wide(&to));
    // SAFETY: both are NUL-terminated wide strings that outlive the call.
    // Without MOVEFILE_REPLACE_EXISTING the move fails rather than replace
    // a file already there.
    let moved = unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), MOVEFILE_WRITE_THROUGH) };
    if moved == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Elsewhere a hard link fails if `name` exists, where a rename would not.
#[cfg(not(windows))]
fn rename_new(part: &Path, name: &str) -> io::Result<()> {
    let to = part.with_file_name(name);
    std::fs::hard_link(part, &to)?;
    std::fs::remove_file(part)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Folder(PathBuf);

    impl Folder {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("barrage-disk-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_finished_file_never_replaces_one_that_appeared_meanwhile() {
        let dir = Folder::new("rename");
        let (name, part, file) = create_part(&dir.0, "x.bin", 3).unwrap();
        assert_eq!(name, "x.bin");
        write_all_at(&file, b"new", 0).unwrap();
        drop(file);
        // Another program takes the name while the download runs.
        std::fs::write(dir.0.join("x.bin"), b"theirs").unwrap();

        assert_eq!(name_finished(&part, &name).unwrap(), "x (1).bin");
        assert_eq!(std::fs::read(dir.0.join("x.bin")).unwrap(), b"theirs");
        assert_eq!(std::fs::read(dir.0.join("x (1).bin")).unwrap(), b"new");
        assert!(!part.exists());
    }

    #[test]
    fn the_part_is_full_size_at_once() {
        let dir = Folder::new("size");
        let (_, part, _file) = create_part(&dir.0, "big.bin", 5 << 20).unwrap();
        assert_eq!(std::fs::metadata(part).unwrap().len(), 5 << 20);
    }
}
