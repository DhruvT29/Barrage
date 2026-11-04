//! A peer connection's two halves, counted: every byte read is added to its
//! cable's data used as received and to the torrent's tally for that cable,
//! every byte written as sent. And ended at once when its cable stops
//! working: a connection through a cable that has gone would otherwise sit
//! until librqbit's timeouts noticed.

use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::task::{Context, Poll, Waker};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use crate::fetch::rate::Rate;
use crate::fetch::used::Counter;

/// One torrent's part through one cable.
#[derive(Default)]
pub(crate) struct Tally {
    pub received: AtomicU64,
    pub sent: AtomicU64,
    /// Its connections open now.
    pub open: AtomicU32,
    pub rate: Mutex<Rate>,
}

/// The end of every connection through one cable, once it stops working.
#[derive(Default)]
pub(crate) struct Cut {
    gone: AtomicBool,
    /// Each connection's reader waiting on a read, to wake when cut.
    waiting: Mutex<Vec<Weak<Mutex<Option<Waker>>>>>,
}

impl Cut {
    pub fn cut(&self) {
        self.gone.store(true, Ordering::SeqCst);
        for slot in self.waiting.lock().unwrap().drain(..) {
            if let Some(waker) = slot.upgrade().and_then(|s| s.lock().unwrap().take()) {
                waker.wake();
            }
        }
    }

    pub fn is_cut(&self) -> bool {
        self.gone.load(Ordering::SeqCst)
    }

    fn slot(&self) -> Arc<Mutex<Option<Waker>>> {
        let slot = Arc::new(Mutex::new(None));
        self.waiting.lock().unwrap().push(Arc::downgrade(&slot));
        slot
    }
}

fn ended() -> io::Error {
    io::Error::new(
        io::ErrorKind::ConnectionAborted,
        "its cable stopped working",
    )
}

/// What both halves of a connection count into.
#[derive(Clone)]
pub(crate) struct Counts {
    pub counter: Arc<Counter>,
    pub tally: Arc<Tally>,
    pub cut: Arc<Cut>,
}

pub(crate) struct CountedRead<R> {
    inner: R,
    counts: Counts,
    waker: Arc<Mutex<Option<Waker>>>,
    /// Kept for as long as the connection lasts (its place in its cable's count).
    _held: Box<dyn Send + Sync>,
}

impl<R> CountedRead<R> {
    pub fn new(inner: R, counts: Counts, held: Box<dyn Send + Sync>) -> Self {
        let waker = counts.cut.slot();
        Self {
            inner,
            counts,
            waker,
            _held: held,
        }
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for CountedRead<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.counts.cut.is_cut() {
            return Poll::Ready(Err(ended()));
        }
        *self.waker.lock().unwrap() = Some(cx.waker().clone());
        let before = buf.filled().len();
        let polled = Pin::new(&mut self.inner).poll_read(cx, buf);
        if let Poll::Ready(Ok(())) = polled {
            let n = (buf.filled().len() - before) as u64;
            self.counts.counter.add(n);
            self.counts.tally.received.fetch_add(n, Ordering::SeqCst);
        }
        polled
    }
}

pub(crate) struct CountedWrite<W> {
    inner: W,
    counts: Counts,
}

impl<W> CountedWrite<W> {
    pub fn new(inner: W, counts: Counts) -> Self {
        Self { inner, counts }
    }
}

impl<W: AsyncWrite + Unpin> AsyncWrite for CountedWrite<W> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        if self.counts.cut.is_cut() {
            return Poll::Ready(Err(ended()));
        }
        let polled = Pin::new(&mut self.inner).poll_write(cx, buf);
        if let Poll::Ready(Ok(n)) = polled {
            self.counts.counter.add_sent(n as u64);
            self.counts.tally.sent.fetch_add(n as u64, Ordering::SeqCst);
        }
        polled
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}
