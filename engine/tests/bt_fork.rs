//! The librqbit fork's hooks (`DhruvT29/rqbit`, branch `barrage`), tested
//! through librqbit itself: the only tests outside `src/bt` that name its
//! types. Every peer is a local seed on 127.0.0.1; the leecher's connections
//! leave from 127.0.0.2 and 127.0.0.3, two addresses standing for two cables.

mod torrent;

use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::future::BoxFuture;
use librqbit::dht::Id20;
use librqbit::{
    AddTorrent, AddTorrentOptions, ConnectionOptions, PeerConnector, PeerSocket, Session,
};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpSocket;

use torrent::{downloaded, fresh_dir, listening_on, pattern, quiet_options, seeds_with};

/// Opens each connection from the next of `from` in turn, and remembers
/// which torrent and peer each was for.
struct Alternating {
    from: Vec<IpAddr>,
    next: AtomicUsize,
    opened: Mutex<Vec<(Id20, SocketAddr)>>,
}

impl PeerConnector for Alternating {
    fn connect(
        &self,
        info_hash: Id20,
        peer: SocketAddr,
    ) -> BoxFuture<'static, std::io::Result<PeerSocket>> {
        self.opened.lock().unwrap().push((info_hash, peer));
        let local = self.from[self.next.fetch_add(1, Ordering::Relaxed) % self.from.len()];
        Box::pin(async move {
            let socket = TcpSocket::new_v4()?;
            socket.bind(SocketAddr::new(local, 0))?;
            let (read, write) = socket.connect(peer).await?.into_split();
            Ok(PeerSocket {
                read: Box::new(read),
                write: Box::new(write),
                may_upload: Arc::new(AtomicBool::new(true)),
            })
        })
    }

    fn accepted(
        &self,
        _info_hash: Id20,
        _local: SocketAddr,
        _peer: SocketAddr,
        read: Box<dyn AsyncRead + Send + Unpin>,
        write: Box<dyn AsyncWrite + Send + Unpin>,
    ) -> PeerSocket {
        PeerSocket {
            read,
            write,
            may_upload: Arc::new(AtomicBool::new(true)),
        }
    }
}

/// Lets every connection in as it is, and remembers where each came from.
#[derive(Default)]
struct Watching {
    from: Mutex<Vec<SocketAddr>>,
}

impl PeerConnector for Watching {
    fn connect(
        &self,
        _info_hash: Id20,
        peer: SocketAddr,
    ) -> BoxFuture<'static, std::io::Result<PeerSocket>> {
        Box::pin(async move {
            let (read, write) = tokio::net::TcpStream::connect(peer).await?.into_split();
            Ok(PeerSocket {
                read: Box::new(read),
                write: Box::new(write),
                may_upload: Arc::new(AtomicBool::new(true)),
            })
        })
    }

    fn accepted(
        &self,
        _info_hash: Id20,
        _local: SocketAddr,
        peer: SocketAddr,
        read: Box<dyn AsyncRead + Send + Unpin>,
        write: Box<dyn AsyncWrite + Send + Unpin>,
    ) -> PeerSocket {
        self.from.lock().unwrap().push(peer);
        PeerSocket {
            read,
            write,
            may_upload: Arc::new(AtomicBool::new(true)),
        }
    }
}

fn with_connector(
    mut options: librqbit::SessionOptions,
    connector: Arc<dyn PeerConnector>,
) -> librqbit::SessionOptions {
    options.connect = Some(ConnectionOptions {
        peer_connector: Some(connector),
        ..Default::default()
    });
    options
}

#[tokio::test]
async fn each_peer_connection_leaves_from_the_address_the_connector_chose() {
    let watching = Arc::new(Watching::default());
    let seen = Arc::clone(&watching);
    let seeds = seeds_with("chose", 4, &pattern(3 << 20), 64 << 10, move || {
        with_connector(listening_on("127.0.0.1"), seen.clone())
    })
    .await;

    let alternating = Arc::new(Alternating {
        from: vec!["127.0.0.2".parse().unwrap(), "127.0.0.3".parse().unwrap()],
        next: AtomicUsize::new(0),
        opened: Mutex::new(Vec::new()),
    });
    let dir = fresh_dir("chose-leech");
    let leecher = Session::new_with_opts(
        dir.clone(),
        with_connector(quiet_options(), alternating.clone()),
    )
    .await
    .unwrap();
    let added = leecher
        .add_torrent(
            AddTorrent::from_bytes(seeds.torrent.clone()),
            Some(AddTorrentOptions {
                output_folder: Some(dir.to_string_lossy().into_owned()),
                initial_peers: Some(seeds.addrs.clone()),
                ..Default::default()
            }),
        )
        .await
        .unwrap();
    let handle = added.into_handle().unwrap();
    tokio::time::timeout(Duration::from_secs(60), handle.wait_until_completed())
        .await
        .expect("the download finishes")
        .unwrap();

    assert_eq!(downloaded(&dir), seeds.bytes, "the file arrives whole");
    let from: Vec<IpAddr> = watching
        .from
        .lock()
        .unwrap()
        .iter()
        .map(|a| a.ip())
        .collect();
    assert!(!from.is_empty(), "the seeds saw the leecher connect");
    for ip in &from {
        assert!(
            ["127.0.0.2", "127.0.0.3"].contains(&ip.to_string().as_str()),
            "a connection came from {ip}, not an address the connector chose"
        );
    }
    assert!(
        from.iter().any(|ip| ip.to_string() == "127.0.0.2"),
        "127.0.0.2 was used"
    );
    assert!(
        from.iter().any(|ip| ip.to_string() == "127.0.0.3"),
        "127.0.0.3 was used"
    );
    for (info_hash, _) in alternating.opened.lock().unwrap().iter() {
        assert_eq!(
            *info_hash, seeds.info_hash,
            "the connector was told the torrent"
        );
    }
}
