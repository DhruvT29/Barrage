//! Local torrents for the tests: seeds are librqbit sessions on 127.0.0.1,
//! reached by `initial_peers`, with DHT, trackers, local service discovery
//! and UPnP off, so nothing leaves this machine.
#![allow(dead_code)]

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use librqbit::dht::Id20;
use librqbit::spawn_utils::BlockingSpawner;
use librqbit::{
    AddTorrent, AddTorrentOptions, CreateTorrentOptions, ListenerMode, ListenerOptions, Session,
    SessionOptions, create_torrent,
};

/// The file every seed shares, by this name.
pub const FILE: &str = "shared.bin";

/// A session that talks to nobody it is not told about.
pub fn quiet_options() -> SessionOptions {
    SessionOptions {
        dht: None,
        disable_trackers: true,
        disable_local_service_discovery: true,
        fastresume: false,
        persistence: None,
        listen: None,
        ..Default::default()
    }
}

/// `quiet_options`, listening for peers on `ip` at a free port.
pub fn listening_on(ip: &str) -> SessionOptions {
    SessionOptions {
        listen: Some(ListenerOptions {
            mode: ListenerMode::TcpOnly,
            listen_addr: SocketAddr::new(ip.parse().unwrap(), 0),
            enable_upnp_port_forwarding: false,
            ..Default::default()
        }),
        ..quiet_options()
    }
}

/// Seeds of one file: each a session of its own on 127.0.0.1.
pub struct Seeds {
    pub addrs: Vec<SocketAddr>,
    /// The .torrent file's bytes.
    pub torrent: Vec<u8>,
    pub info_hash: Id20,
    pub magnet: String,
    /// The file's bytes, to compare a download with.
    pub bytes: Vec<u8>,
    sessions: Vec<Arc<Session>>,
}

/// A folder of its own for one test, emptied first.
pub fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("barrage-bt-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// `bytes`, made a torrent with pieces of `piece_len` bytes, seeded by `n`
/// sessions, each made with `options` (so a test can give the seeds a
/// connector of its own).
pub async fn seeds_with(
    name: &str,
    n: usize,
    bytes: &[u8],
    piece_len: u32,
    options: impl Fn() -> SessionOptions,
) -> Seeds {
    let dir = fresh_dir(&format!("{name}-seed"));
    let path = dir.join(FILE);
    std::fs::write(&path, bytes).unwrap();
    let made = create_torrent(
        &path,
        CreateTorrentOptions {
            name: Some(FILE),
            trackers: vec![],
            piece_length: Some(piece_len),
        },
        &BlockingSpawner::new(1),
    )
    .await
    .unwrap();
    let torrent = made.as_bytes().unwrap().to_vec();
    let info_hash = made.info_hash();

    let mut sessions = Vec::new();
    let mut addrs = Vec::new();
    for _ in 0..n {
        let session = Session::new_with_opts(dir.clone(), options())
            .await
            .unwrap();
        let added = session
            .add_torrent(
                AddTorrent::from_bytes(torrent.clone()),
                Some(AddTorrentOptions {
                    output_folder: Some(dir.to_string_lossy().into_owned()),
                    overwrite: true,
                    ..Default::default()
                }),
            )
            .await
            .unwrap();
        added
            .into_handle()
            .unwrap()
            .wait_until_completed()
            .await
            .unwrap();
        addrs.push(session.listen_addr().expect("a seed listens"));
        sessions.push(session);
    }
    Seeds {
        addrs,
        torrent,
        magnet: format!("magnet:?xt=urn:btih:{}", info_hash.as_string()),
        info_hash,
        bytes: bytes.to_vec(),
        sessions,
    }
}

/// `n` seeds of `bytes`, listening on 127.0.0.1.
pub async fn seeds(name: &str, n: usize, bytes: &[u8], piece_len: u32) -> Seeds {
    seeds_with(name, n, bytes, piece_len, || listening_on("127.0.0.1")).await
}

/// Some bytes that are not all the same, so a piece put in the wrong place
/// would fail its hash.
pub fn pattern(len: usize) -> Vec<u8> {
    (0..len)
        .map(|i| (i.wrapping_mul(31) ^ (i >> 7)) as u8)
        .collect()
}

/// The downloaded file in `dir`.
pub fn downloaded(dir: &Path) -> Vec<u8> {
    std::fs::read(dir.join(FILE)).unwrap()
}
