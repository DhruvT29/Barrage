//! File names: from the server or the URL, made safe, and never one taken.
//!
//! A name from the server is untrusted, and so is one from a URL, which
//! anyone can write. Whatever arrives, the file lands in the downloads folder
//! under a name Windows accepts as an ordinary file:
//!
//! - **Only the last part of a path counts.** `../../evil.exe` and
//!   `C:\Windows\x.dll` name a file somewhere else; their last part is the
//!   name.
//! - **Characters Windows forbids become `_`**: `< > : " / \ | ? *` and
//!   control characters. So do the controls that reverse text direction,
//!   with which `report<RLO>fdp.exe` shows on screen as `reportexe.pdf`.
//! - **Trailing dots and spaces go**, because Windows drops them itself. Left
//!   in, the name checked for being free would not be the name created.
//! - **Device names get a `_` in front.** `CON`, `NUL`, `COM1` and the rest
//!   name devices, not files, with or without an extension: `con.txt` is
//!   the console.
//! - **A name over [`MAX_NAME`] is cut from the end of its stem**, keeping
//!   the extension, since that is what decides what opens the file.
//!
//! And a name already in the folder is never reused ([`free_name`]): the new
//! file gets ` (1)`, ` (2)` before its extension.

use std::io::ErrorKind;
use std::path::Path;

use reqwest::Url;

/// The name when neither the server nor the URL gives one.
const FALLBACK: &str = "download";

/// The longest name kept, in UTF-16 units, as Windows counts. NTFS allows
/// 255; the rest is room for ` (12)` and the `.part.json` beside the file.
const MAX_NAME: usize = 200;

// The longest name made from one, `<name> (99999).part.json`, still fits.
const _: () = assert!(MAX_NAME + " (99999)".len() + ".part.json".len() <= 255);

/// The longest extension [`shorten`] keeps whole when it cuts a name.
const MAX_EXTENSION: usize = 16;

/// Names Windows keeps for devices. `COM0`, `LPT0` and the superscript forms
/// are on Microsoft's list too, and `CONIN$` and `CONOUT$` open the console.
#[rustfmt::skip]
const RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$",
    "COM0", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9",
    "COM\u{b9}", "COM\u{b2}", "COM\u{b3}",
    "LPT0", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    "LPT\u{b9}", "LPT\u{b2}", "LPT\u{b3}",
];

/// The name the file gets: the server's `Content-Disposition` (its
/// `filename*`, then its `filename`), else the last segment of `url`, which
/// should be the URL after redirects, as browsers do. The first that leaves
/// a usable name once made safe wins; with none, [`FALLBACK`].
pub fn file_name(disposition: Option<&str>, url: &Url) -> String {
    let from_server = disposition.map(names_in).unwrap_or_default();
    from_server
        .into_iter()
        .chain(last_segment(url))
        .find_map(|raw| clean(&raw))
        .unwrap_or_else(|| FALLBACK.to_string())
}

/// `name` if `dir` holds neither it nor its `.part`, else the first free of
/// `stem (1).ext`, `stem (2).ext` and so on.
///
/// Only a check: the caller creates the `.part` with `create_new`, so a name
/// taken between this check and that creation is still never overwritten.
pub fn free_name(dir: &Path, name: &str) -> String {
    if !taken(dir, name) {
        return name.to_string();
    }
    let (stem, extension) = split_extension(name);
    (1u64..)
        .map(|n| format!("{stem} ({n}){extension}"))
        .find(|candidate| !taken(dir, candidate))
        .expect("a folder cannot hold a file for every number")
}

/// Whether `dir` holds `name` or its `.part`. Anything that cannot be checked
/// counts as taken: a file this cannot see is still one it must not
/// overwrite.
fn taken(dir: &Path, name: &str) -> bool {
    !nothing_at(&dir.join(name)) || !nothing_at(&dir.join(format!("{name}.part")))
}

/// Whether there is surely nothing at `path`: no file, no folder, no link.
fn nothing_at(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_err_and(|e| e.kind() == ErrorKind::NotFound)
}

/// `raw` made safe as one file name, or `None` when nothing usable is left.
fn clean(raw: &str) -> Option<String> {
    let last = raw.rsplit(['/', '\\']).next().unwrap_or(raw);
    let legal: String = last
        .chars()
        .map(|c| if forbidden(c) { '_' } else { c })
        .collect();
    let trimmed = legal.trim_end_matches(['.', ' ']);
    if trimmed.is_empty() {
        return None;
    }
    let short = shorten(trimmed);
    Some(if reserved(&short) {
        format!("_{short}")
    } else {
        short
    })
}

fn forbidden(c: char) -> bool {
    matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
        || c.is_control()
        || reorders_text(c)
}

/// The Unicode controls that change the direction text is shown in: the
/// marks, embeddings, overrides and isolates.
fn reorders_text(c: char) -> bool {
    matches!(c, '\u{61c}' | '\u{200e}' | '\u{200f}')
        || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

/// Whether Windows takes `name` for a device: the part before its first dot,
/// trailing spaces aside, is a reserved name in any case.
fn reserved(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name).trim_end_matches(' ');
    RESERVED.iter().any(|r| r.eq_ignore_ascii_case(stem))
}

/// `name` cut to [`MAX_NAME`] UTF-16 units from the end of its stem. An
/// extension over [`MAX_EXTENSION`] characters is not one worth keeping, and
/// is cut with the rest.
fn shorten(name: &str) -> String {
    if name.encode_utf16().count() <= MAX_NAME {
        return name.to_string();
    }
    let (stem, extension) = match split_extension(name) {
        (stem, extension) if extension.chars().count() <= MAX_EXTENSION => (stem, extension),
        _ => (name, ""),
    };
    let room = MAX_NAME - extension.encode_utf16().count();
    let mut used = 0;
    let cut: String = stem
        .chars()
        .take_while(|c| {
            used += c.len_utf16();
            used <= room
        })
        .collect();
    format!("{}{extension}", cut.trim_end_matches(['.', ' ']))
}

/// `name` as (stem, extension with its dot). A leading dot starts the name,
/// not an extension.
fn split_extension(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 => name.split_at(i),
        _ => (name, ""),
    }
}

/// The names a `Content-Disposition` value gives, best first: `filename*`
/// (RFC 8187, decoded) when its charset is readable, then `filename`.
fn names_in(disposition: &str) -> Vec<String> {
    let params = parameters(disposition);
    let value = |name: &str| params.iter().find(|(n, _)| n == name).map(|(_, v)| v);
    let star = value("filename*").and_then(|v| extended(v));
    star.into_iter().chain(value("filename").cloned()).collect()
}

/// The parameters after the first `;` of a header value such as
/// `attachment; filename="a;b.txt"`, names in lowercase, values unquoted. A
/// `;` inside quotes belongs to the value.
fn parameters(value: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    let mut chars = value.chars().peekable();
    // The disposition type, which says nothing about the name.
    for c in chars.by_ref() {
        if c == ';' {
            break;
        }
    }
    loop {
        let mut name = String::new();
        while let Some(&c) = chars.peek() {
            if c == '=' || c == ';' {
                break;
            }
            name.push(c);
            chars.next();
        }
        match chars.next() {
            Some('=') => {}
            Some(_) => continue,
            None => break,
        }
        while chars.next_if(|c| c.is_whitespace()).is_some() {}
        let mut text = String::new();
        if chars.next_if_eq(&'"').is_some() {
            while let Some(c) = chars.next() {
                match c {
                    '\\' => text.extend(chars.next()),
                    '"' => break,
                    _ => text.push(c),
                }
            }
            for c in chars.by_ref() {
                if c == ';' {
                    break;
                }
            }
        } else {
            for c in chars.by_ref() {
                if c == ';' {
                    break;
                }
                text.push(c);
            }
            text = text.trim().to_string();
        }
        found.push((name.trim().to_ascii_lowercase(), text));
    }
    found
}

/// An RFC 8187 value, `charset'language'percent-encoded`, decoded. `None` for
/// a charset other than the two the standard names.
fn extended(value: &str) -> Option<String> {
    let mut parts = value.splitn(3, '\'');
    let (charset, _language, encoded) = (parts.next()?, parts.next()?, parts.next()?);
    let bytes = percent_decode(encoded);
    if charset.eq_ignore_ascii_case("UTF-8") {
        Some(String::from_utf8_lossy(&bytes).into_owned())
    } else if charset.eq_ignore_ascii_case("ISO-8859-1") {
        Some(bytes.iter().map(|&b| char::from(b)).collect())
    } else {
        None
    }
}

/// The URL's last path segment, decoded. Query and fragment are not part of
/// it.
fn last_segment(url: &Url) -> Option<String> {
    let segment = url.path_segments()?.next_back()?;
    Some(String::from_utf8_lossy(&percent_decode(segment)).into_owned())
}

/// `%XX` escapes turned back into bytes. A `%` not followed by two hex digits
/// stays as it is.
fn percent_decode(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let hex = |b: u8| char::from(b).to_digit(16);
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let (Some(high), Some(low)) = (
                bytes.get(i + 1).and_then(|&b| hex(b)),
                bytes.get(i + 2).and_then(|&b| hex(b)),
            )
        {
            out.push((high * 16 + low) as u8);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::*;

    fn url(text: &str) -> Url {
        Url::parse(text).unwrap()
    }

    /// A folder of its own under the system temp folder, removed afterwards.
    struct Folder(PathBuf);

    impl Folder {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("barrage-name-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn touch(&self, name: &str) {
            fs::write(self.0.join(name), b"").unwrap();
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_name_never_leaves_the_folder() {
        let cases = [
            ("../../evil.exe", Some("evil.exe")),
            ("C:\\Windows\\x.dll", Some("x.dll")),
            ("..\\..\\x", Some("x")),
            ("/etc/passwd", Some("passwd")),
            // Relative to a drive's current folder: the colon cannot stay.
            ("C:x.dll", Some("C_x.dll")),
            ("..", None),
            ("files/", None),
            ("", None),
        ];
        for (raw, want) in cases {
            assert_eq!(clean(raw).as_deref(), want, "{raw:?}");
        }
        // A server's name with nothing left in it leaves the URL to name it.
        let plain = url("http://example.com/files/real.iso");
        assert_eq!(
            file_name(Some("attachment; filename=\"../\""), &plain),
            "real.iso"
        );
    }

    #[test]
    fn reserved_windows_names_and_characters_are_made_legal() {
        let cases = [
            ("CON", "_CON"),
            ("con.txt", "_con.txt"),
            ("Lpt1.tar.gz", "_Lpt1.tar.gz"),
            ("CON .txt", "_CON .txt"),
            ("COM\u{b9}.log", "_COM\u{b9}.log"),
            ("CONIN$", "_CONIN$"),
            // Only the whole stem counts.
            ("console.txt", "console.txt"),
            ("a:b?.txt", "a_b_.txt"),
            ("<x>|\"y\"*.txt", "_x___y__.txt"),
            ("tab\there.txt", "tab_here.txt"),
            // Windows drops trailing dots and spaces itself.
            ("name. ", "name"),
            ("name...", "name"),
            // Text direction controls disguise an extension: this shows as
            // reportexe.pdf.
            ("report\u{202e}fdp.exe", "report_fdp.exe"),
            (".hidden", ".hidden"),
        ];
        for (raw, want) in cases {
            assert_eq!(clean(raw).as_deref(), Some(want), "{raw:?}");
        }
    }

    #[test]
    fn filename_star_wins_and_is_decoded() {
        let plain = url("http://example.com/get?id=7");
        let name = |header: &str| file_name(Some(header), &plain);

        let both = "attachment; filename=\"x\"; filename*=UTF-8''r%C3%A9sum%C3%A9.pdf";
        assert_eq!(name(both), "résumé.pdf");
        // In either order, in any case.
        assert_eq!(
            name("attachment; FILENAME*=utf-8''a%20b.txt; filename=x"),
            "a b.txt"
        );
        // ISO-8859-1 is the other charset the standard names.
        assert_eq!(
            name("attachment; filename*=ISO-8859-1''caf%E9.txt"),
            "café.txt"
        );
        // A charset it cannot read, or a name with nothing left, falls back.
        let unknown = "attachment; filename*=KOI8-R''%C1.txt; filename=\"plain.txt\"";
        assert_eq!(name(unknown), "plain.txt");
        let empty = "attachment; filename*=UTF-8''..; filename=\"ok.txt\"";
        assert_eq!(name(empty), "ok.txt");
        // A quoted name may hold a ; and an escaped quote.
        assert_eq!(
            name("attachment; filename=\"a;b \\\"c\\\".txt\""),
            "a;b _c_.txt"
        );
        assert_eq!(name("inline; filename=view.png"), "view.png");
    }

    #[test]
    fn without_a_disposition_the_url_path_names_it() {
        let name = |text: &str| file_name(None, &url(text));
        assert_eq!(
            name("http://example.com/files/my%20file.iso?x=1"),
            "my file.iso"
        );
        assert_eq!(name("http://example.com/"), "download");
        assert_eq!(name("http://example.com"), "download");
        assert_eq!(name("http://example.com/files/"), "download");
        // An encoded ../ is still only a name.
        assert_eq!(name("http://example.com/a/%2E%2E%2Fevil.exe"), "evil.exe");
        // A disposition without a name leaves it to the URL.
        let plain = url("http://example.com/x.zip");
        assert_eq!(file_name(Some("attachment"), &plain), "x.zip");
    }

    #[test]
    fn an_overlong_name_is_cut_and_keeps_its_extension() {
        let long = format!("{}.iso", "a".repeat(300));
        let name = clean(&long).unwrap();
        assert_eq!(name.encode_utf16().count(), MAX_NAME);
        assert!(name.ends_with("a.iso"), "{name}");

        // Counted as Windows counts: each of these is two UTF-16 units.
        let wide = format!("{}.txt", "\u{1f600}".repeat(150));
        let name = clean(&wide).unwrap();
        assert!(name.encode_utf16().count() <= MAX_NAME, "{name}");
        assert!(name.ends_with("\u{1f600}.txt"), "{name}");

        // A cut that lands among spaces does not leave one before the dot.
        let spaced = format!(
            "{}{}{}.txt",
            "a".repeat(195),
            " ".repeat(10),
            "b".repeat(10)
        );
        let name = clean(&spaced).unwrap();
        assert!(name.ends_with("a.txt"), "{name}");
    }

    #[test]
    fn a_taken_name_gets_a_number() {
        let dir = Folder::new("taken");
        let free = |name: &str| free_name(&dir.0, name);
        assert_eq!(free("x.iso"), "x.iso");
        dir.touch("x.iso");
        assert_eq!(free("x.iso"), "x (1).iso");
        dir.touch("x (1).iso");
        assert_eq!(free("x.iso"), "x (2).iso");
        // A download still running holds its name too.
        dir.touch("x (2).iso.part");
        assert_eq!(free("x.iso"), "x (3).iso");
        dir.touch("y.iso.part");
        assert_eq!(free("y.iso"), "y (1).iso");
        // So does a folder.
        fs::create_dir(dir.0.join("z.iso")).unwrap();
        assert_eq!(free("z.iso"), "z (1).iso");
        // No extension, and a name that starts with its dot.
        dir.touch("x");
        assert_eq!(free("x"), "x (1)");
        dir.touch(".config");
        assert_eq!(free(".config"), ".config (1)");
        // Windows names ignore case.
        if cfg!(windows) {
            dir.touch("Report.PDF");
            assert_eq!(free("report.pdf"), "report (1).pdf");
        }
    }
}
