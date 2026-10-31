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

/// Counts what is written through it.
struct CountingWrite {
    inner: Box<dyn AsyncWrite + Send + Unpin>,
    written: Arc<std::sync::atomic::AtomicU64>,
}

impl AsyncWrite for CountingWrite {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        let polled = std::pin::Pin::new(&mut self.inner).poll_write(cx, buf);
        if let std::task::Poll::Ready(Ok(n)) = &polled {
            self.written.fetch_add(*n as u64, Ordering::Relaxed);
        }
        polled
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// A seed's side of every incoming connection: what it may upload, from one
/// switch, and how much it has written.
struct Switched {
    may_upload: Arc<AtomicBool>,
    written: Arc<std::sync::atomic::AtomicU64>,
}

impl PeerConnector for Switched {
    fn connect(
        &self,
        _info_hash: Id20,
        _peer: SocketAddr,
    ) -> BoxFuture<'static, std::io::Result<PeerSocket>> {
        Box::pin(async { Err(std::io::Error::other("a seed here only takes connections")) })
    }

    fn accepted(
        &self,
        _info_hash: Id20,
        _local: SocketAddr,
        _peer: SocketAddr,
        read: Box<dyn AsyncRead + Send + Unpin>,
        write: Box<dyn AsyncWrite + Send + Unpin>,
    ) -> PeerSocket {
        let write = Box::new(CountingWrite {
            inner: write,
            written: Arc::clone(&self.written),
        });
        PeerSocket {
            read,
            write,
            may_upload: Arc::clone(&self.may_upload),
        }
    }
}

/// One seed whose uploads follow `switched`, and a leecher fetching from it
/// through 127.0.0.2. Gives the leecher's torrent and the folder it writes to.
async fn leech_from_switched(
    name: &str,
    switched: Arc<Switched>,
) -> (Arc<librqbit::ManagedTorrent>, Arc<Session>) {
    let seeds = seeds_with(name, 1, &pattern(1 << 20), 64 << 10, move || {
        with_connector(listening_on("127.0.0.1"), switched.clone())
    })
    .await;
    let through = Arc::new(Alternating {
        from: vec!["127.0.0.2".parse().unwrap()],
        next: AtomicUsize::new(0),
        opened: Mutex::new(Vec::new()),
    });
    let dir = fresh_dir(&format!("{name}-leech"));
    let leecher = Session::new_with_opts(dir.clone(), with_connector(quiet_options(), through))
        .await
        .unwrap();
    let handle = leecher
        .add_torrent(
            AddTorrent::from_bytes(seeds.torrent.clone()),
            Some(AddTorrentOptions {
                output_folder: Some(dir.to_string_lossy().into_owned()),
                initial_peers: Some(seeds.addrs.clone()),
                ..Default::default()
            }),
        )
        .await
        .unwrap()
        .into_handle()
        .unwrap();
    // The seeds' sessions live as long as the test: leaked on purpose.
    std::mem::forget(seeds);
    (handle, leecher)
}

#[tokio::test]
async fn no_piece_is_sent_through_a_connection_that_may_not_upload() {
    let switched = Arc::new(Switched {
        may_upload: Arc::new(AtomicBool::new(false)),
        written: Default::default(),
    });
    let (handle, _leecher) = leech_from_switched("no-upload", switched.clone()).await;

    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(
        handle.stats().progress_bytes,
        0,
        "the leecher got nothing from a seed that may not upload"
    );
    let written = switched.written.load(Ordering::Relaxed);
    assert!(written > 0, "the seed spoke to the leecher at all");
    assert!(
        written < 1024,
        "the seed sent only the protocol's messages, not pieces: {written} bytes"
    );
}

#[tokio::test]
async fn upload_starts_when_the_switch_turns_on() {
    let switched = Arc::new(Switched {
        may_upload: Arc::new(AtomicBool::new(false)),
        written: Default::default(),
    });
    let (handle, _leecher) = leech_from_switched("upload-on", switched.clone()).await;

    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(handle.stats().progress_bytes, 0);
    switched.may_upload.store(true, Ordering::Relaxed);
    // The switch is looked at every second, on the open connection: well
    // before a reconnect (which would start unchoked anyway) could pass it.
    tokio::time::timeout(Duration::from_secs(5), handle.wait_until_completed())
        .await
        .expect("the file arrives within seconds of the seed being allowed to upload")
        .unwrap();
}

#[tokio::test]
async fn a_choked_peer_that_asks_anyway_gets_no_piece() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let switched = Arc::new(Switched {
        may_upload: Arc::new(AtomicBool::new(false)),
        written: Default::default(),
    });
    let seen = switched.clone();
    let seeds = seeds_with("asks-anyway", 1, &pattern(1 << 20), 64 << 10, move || {
        with_connector(listening_on("127.0.0.1"), seen.clone())
    })
    .await;

    // A peer that ignores being choked: the handshake, then Interested and
    // a Request for the first 16 KiB, without waiting for an Unchoke.
    let mut peer = tokio::net::TcpStream::connect(seeds.addrs[0])
        .await
        .unwrap();
    let mut hello = vec![19u8];
    hello.extend_from_slice(b"BitTorrent protocol");
    hello.extend_from_slice(&[0; 8]);
    hello.extend_from_slice(&seeds.info_hash.0);
    hello.extend_from_slice(b"-XX0000-asks-anyway!");
    peer.write_all(&hello).await.unwrap();
    peer.write_all(&[0, 0, 0, 1, 2]).await.unwrap();
    let mut request = vec![0, 0, 0, 13, 6];
    request.extend_from_slice(&0u32.to_be_bytes());
    request.extend_from_slice(&0u32.to_be_bytes());
    request.extend_from_slice(&(16u32 << 10).to_be_bytes());
    peer.write_all(&request).await.unwrap();

    let mut got = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(2), peer.read_to_end(&mut got)).await;
    let written = switched.written.load(Ordering::Relaxed);
    assert!(
        written < 1024,
        "the seed sent no piece to a peer it keeps choked: {written} bytes"
    );
    std::mem::forget(seeds);
}
