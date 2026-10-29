//! The file on disk: full size before any data arrives, written at offsets
//! by many workers at once, and named only when complete, never over another
//! file. And the folder it goes to unless told otherwise.
//!
//! NOT SPARSE
//!
//! The `.part` is sized with `set_len` and not marked sparse, so a full disk
//! or a folder that cannot be written fails here, before the first byte
//! (PLAN.md section 3.6, item 4). Chunks are claimed in ascending order, so
//! NTFS only fills zeros a few chunks ahead of the lowest unfinished byte.
//!
//! FROM THE INTERNET
//!
//! The `.part` carries the mark a browser gives a download, a
//! `Zone.Identifier` stream saying it came from the internet, so Windows
//! treats the finished file as one: SmartScreen checks a program before it
//! runs, Office opens a document in Protected View. The stream moves with
//! the file when it is named, and stays through a resume.

use std::fs::{File, OpenOptions};
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};

use reqwest::Url;

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

/// The `.part` an earlier engine left at `part`, opened to write the rest
/// of it: it must still be there, and still `length` bytes long.
pub(crate) fn reopen_part(part: &Path, length: u64) -> Result<File, String> {
    let file = OpenOptions::new()
        .write(true)
        .open(part)
        .map_err(|e| format!("cannot open {}: {e}", part.display()))?;
    match file.metadata() {
        Ok(meta) if meta.len() == length => Ok(file),
        Ok(meta) => Err(format!(
            "{} is {} bytes, not the file's {length}",
            part.display(),
            meta.len()
        )),
        Err(e) => Err(format!("cannot read {}: {e}", part.display())),
    }
}

/// Mark `part` as downloaded from `url`, as a browser marks a download: a
/// `Zone.Identifier` stream with the internet zone, 3. Any user name and
/// password in the URL are left out. A drive with no streams (FAT32, exFAT)
/// cannot hold the mark, and the file is downloaded without it, as a
/// browser's would be.
#[cfg(windows)]
pub(crate) fn mark_from_internet(part: &Path, url: &Url) {
    let mut host = url.clone();
    let _ = host.set_username("");
    let _ = host.set_password(None);
    let mark = format!("[ZoneTransfer]\r\nZoneId=3\r\nHostUrl={host}\r\n");
    let mut stream = part.as_os_str().to_owned();
    stream.push(":Zone.Identifier");
    if let Err(e) = std::fs::write(PathBuf::from(stream), mark) {
        tracing::debug!(error = %e, part = %part.display(), "could not mark a download as from the internet");
    }
}

/// Only Windows reads the mark.
#[cfg(not(windows))]
pub(crate) fn mark_from_internet(_part: &Path, _url: &Url) {}

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

/// The user's Downloads folder, wherever they have moved it: only the shell
/// knows, since `%USERPROFILE%\Downloads` is just where it starts out.
#[cfg(windows)]
pub fn downloads_folder() -> io::Result<PathBuf> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;

    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::{
        FOLDERID_Downloads, KF_FLAG_DEFAULT, SHGetKnownFolderPath,
    };

    let mut path = std::ptr::null_mut();
    // SAFETY: the folder id is a static the call only reads, a null token
    // means this user, and `path` is where the shell puts the string it
    // allocates.
    let result = unsafe {
        SHGetKnownFolderPath(
            &FOLDERID_Downloads,
            // The flags are declared signed, the parameter unsigned; it is 0.
            KF_FLAG_DEFAULT as u32,
            std::ptr::null_mut(),
            &mut path,
        )
    };
    let found = if result >= 0 && !path.is_null() {
        // SAFETY: on success `path` is a NUL-terminated wide string, read
        // up to its NUL and copied out before it is freed.
        let wide = unsafe {
            let mut len = 0;
            while *path.add(len) != 0 {
                len += 1;
            }
            std::slice::from_raw_parts(path, len)
        };
        Ok(PathBuf::from(OsString::from_wide(wide)))
    } else {
        Err(io::Error::from_raw_os_error(result))
    };
    // SAFETY: the shell's own string, freed once, whether or not the call
    // succeeded, as its documentation requires. Freeing null does nothing.
    unsafe { CoTaskMemFree(path.cast()) };
    found
}

/// Elsewhere, `Downloads` in the home folder.
#[cfg(not(windows))]
pub fn downloads_folder() -> io::Result<PathBuf> {
    std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join("Downloads"))
        .ok_or_else(|| io::Error::new(ErrorKind::NotFound, "HOME is not set"))
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

    #[cfg(windows)]
    #[test]
    fn the_part_is_marked_as_from_the_internet_and_keeps_the_mark_when_named() {
        let dir = Folder::new("mark");
        let (name, part, file) = create_part(&dir.0, "setup.exe", 3).unwrap();
        let url = Url::parse("https://user:secret@example.com/files/setup.exe?v=2").unwrap();
        mark_from_internet(&part, &url);
        drop(file);
        assert_eq!(name_finished(&part, &name).unwrap(), "setup.exe");

        let mark = std::fs::read_to_string(dir.0.join("setup.exe:Zone.Identifier")).unwrap();
        assert_eq!(
            mark,
            "[ZoneTransfer]\r\nZoneId=3\r\nHostUrl=https://example.com/files/setup.exe?v=2\r\n"
        );
        // The file itself is untouched by its mark.
        assert_eq!(std::fs::metadata(dir.0.join("setup.exe")).unwrap().len(), 3);
    }

    /// The default for `barrage --downloads`. Every Windows profile has one,
    /// moved or not.
    #[test]
    fn the_downloads_folder_is_found() {
        let dir = downloads_folder().unwrap();
        assert!(dir.is_absolute(), "{}", dir.display());
        assert!(dir.is_dir(), "{}", dir.display());
    }
}
