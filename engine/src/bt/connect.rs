//! BARRAGE's side of the fork's `PeerConnector`: every peer connection a
//! torrent opens leaves from the address of the cable `choose::next_cable`
//! picks, and every one is counted against its cable while it lasts, so the
//! next choice sees how many each cable holds.

use std::collections::HashMap;
use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use librqbit::dht::Id20;
use librqbit::{PeerConnector, PeerSocket};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpSocket;

use super::choose::next_cable;
use crate::links::watch::LinkTable;

/// Each cable's open peer connections, by name.
type Open = Arc<Mutex<HashMap<String, u32>>>;

pub(crate) struct Connector {
    table: LinkTable,
    open: Open,
}

impl Connector {
    pub fn new(table: LinkTable) -> Self {
        Self {
            table,
            open: Arc::default(),
        }
    }
}

/// One connection's place in its cable's count, given back when it ends.
struct Held {
    cable: String,
    open: Open,
}

impl Held {
    /// Counts one more connection through `cable`.
    fn take(open: &Open, cable: String) -> Self {
        *open.lock().unwrap().entry(cable.clone()).or_insert(0) += 1;
        Self {
            cable,
            open: Arc::clone(open),
        }
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        let mut open = self.open.lock().unwrap();
        if let Some(n) = open.get_mut(&self.cable) {
            *n = n.saturating_sub(1);
        }
    }
}

/// The read half of a connection, holding its place in its cable's count.
struct Tracked<R> {
    inner: R,
    _held: Held,
}

impl<R: AsyncRead + Unpin> AsyncRead for Tracked<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl PeerConnector for Connector {
    fn connect(
        &self,
        _info_hash: Id20,
        peer: SocketAddr,
    ) -> Pin<Box<dyn Future<Output = std::io::Result<PeerSocket>> + Send>> {
        let links = self.table.snapshot();
        // Chosen and counted under one lock, so two connections opening at
        // once both see each other.
        let chosen = {
            let open = self.open.lock().unwrap();
            next_cable(&links, &open, peer)
        };
        let (cable, local) = match chosen {
            Ok(chosen) => chosen,
            Err(why) => {
                return Box::pin(async move { Err(std::io::Error::other(why.to_string())) });
            }
        };
        let held = Held::take(&self.open, cable);
        Box::pin(async move {
            let socket = match local {
                IpAddr::V4(_) => TcpSocket::new_v4()?,
                IpAddr::V6(_) => TcpSocket::new_v6()?,
            };
            socket.bind(SocketAddr::new(local, 0))?;
            let (read, write) = socket.connect(peer).await?.into_split();
            Ok(PeerSocket {
                read: Box::new(Tracked {
                    inner: read,
                    _held: held,
                }),
                write: Box::new(write),
                may_upload: Arc::new(AtomicBool::new(true)),
            })
        })
    }

    fn accepted(
        &self,
        _info_hash: Id20,
        local: SocketAddr,
        _peer: SocketAddr,
        read: Box<dyn AsyncRead + Send + Unpin>,
        write: Box<dyn AsyncWrite + Send + Unpin>,
    ) -> PeerSocket {
        let links = self.table.snapshot();
        let cable = links
            .iter()
            .find(|l| l.addr.ip() == local.ip())
            .map_or_else(|| local.ip().to_string(), |l| l.name.clone());
        PeerSocket {
            read: Box::new(Tracked {
                inner: read,
                _held: Held::take(&self.open, cable),
            }),
            write,
            may_upload: Arc::new(AtomicBool::new(true)),
        }
    }
}
