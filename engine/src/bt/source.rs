//! What a torrent is added from, and its info hash, which names it: a
//! magnet link and the .torrent file of the same torrent are one torrent.

use std::fmt;

use super::TorrentId;

/// A torrent to add.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// `magnet:?xt=urn:btih:<info hash>&...`
    Magnet(String),
    /// A .torrent file's bytes.
    File(Vec<u8>),
}

/// Why a torrent was not added, in the engine's words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TorrentAddError {
    BadMagnet(String),
    NotATorrent(String),
    /// The same torrent (same info hash) is already listed, as this one.
    AlreadyAdded(TorrentId),
    /// The engine is shutting down.
    Stopped,
}

impl fmt::Display for TorrentAddError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadMagnet(why) => write!(f, "not a magnet link this engine can use: {why}"),
            Self::NotATorrent(why) => write!(f, "not a torrent file: {why}"),
            Self::AlreadyAdded(id) => write!(f, "already added, as torrent {}", id.as_u64()),
            Self::Stopped => f.write_str("the engine is stopping"),
        }
    }
}

impl std::error::Error for TorrentAddError {}

/// The torrent's info hash, 40 lowercase hex characters, or why `source`
/// names no torrent.
pub fn info_hash(source: &Source) -> Result<String, TorrentAddError> {
    match source {
        Source::Magnet(link) => {
            if !link.starts_with("magnet:") {
                return Err(TorrentAddError::BadMagnet(
                    "it does not start with magnet:".into(),
                ));
            }
            let magnet = librqbit::Magnet::parse(link)
                .map_err(|e| TorrentAddError::BadMagnet(format!("{e:#}")))?;
            magnet
                .as_id20()
                .map(|id| id.as_string())
                .ok_or_else(|| TorrentAddError::BadMagnet("it has no v1 (btih) info hash".into()))
        }
        Source::File(bytes) => librqbit::torrent_from_bytes(bytes)
            .map(|torrent| torrent.info_hash.as_string())
            .map_err(|e| TorrentAddError::NotATorrent(e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_magnet_names_its_info_hash() {
        let hash = "ab".repeat(20);
        let got = info_hash(&Source::Magnet(format!(
            "magnet:?xt=urn:btih:{hash}&dn=file.iso"
        )));
        assert_eq!(got, Ok(hash));
    }

    #[test]
    fn a_magnet_without_an_info_hash_is_refused() {
        let got = info_hash(&Source::Magnet("magnet:?dn=file.iso".into()));
        assert!(matches!(got, Err(TorrentAddError::BadMagnet(_))), "{got:?}");
        let got = info_hash(&Source::Magnet("https://example.com/file.torrent".into()));
        assert_eq!(
            got,
            Err(TorrentAddError::BadMagnet(
                "it does not start with magnet:".into()
            ))
        );
    }

    #[test]
    fn random_bytes_are_not_a_torrent() {
        let got = info_hash(&Source::File(b"this is not bencode at all".to_vec()));
        assert!(
            matches!(got, Err(TorrentAddError::NotATorrent(_))),
            "{got:?}"
        );
        assert!(
            got.unwrap_err()
                .to_string()
                .starts_with("not a torrent file: ")
        );
    }
}
