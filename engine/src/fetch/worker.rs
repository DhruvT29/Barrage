//! One connection of one link: take a chunk from the pile, fetch it with a
//! `Range` request, write it at its place in the file, and say how it went.
//!
//! A worker keeps only its own client and the counter its link's workers
//! share. Everything else belongs to the download's coordinator, which hears
//! from it by message (PLAN.md section 3.5).
//!
//! ONLY THE BYTES ASKED FOR, OF THE VERSION ASKED FOR
//!
//! Every piece is asked for with `If-Range` and the link's validator, so a
//! server whose file has changed answers with the whole new file (200)
//! rather than a piece of it. A piece must be a 206 for the file's own
//! length, starting where the chunk starts. Past the chunk's end nothing is
//! written, however long the answer runs. An answer that ends short,
//! cleanly or not, leaves its bytes written and the rest of the chunk for
//! the pile, so no byte is asked for twice. A 200, or another length, means
//! the file is no longer the one being fetched: the download stops rather
//! than stitch two versions together.
//!
//! A server that sends no pieces gets one worker that fetches the whole
//! file with a plain `GET`, from the first byte to the last.
//!
//! WHEN A CONNECTION FAILS
//!
//! Every read has the read deadline on the read itself, not a watchdog
//! beside it (PLAN.md section 3.5). A connection that goes that long without
//! data, or whose request or answer breaks, is reported broken and its
//! worker ends: one worker cannot tell a bad connection from a failing link.
//! The coordinator can, from whether the link's other connections are still
//! receiving, so each worker notes when its own last received bytes. An
//! answer that arrives but cannot be used, an unexpected status or a piece
//! of the wrong bytes, is no fault of the connection: that worker ends, and
//! no other takes its place. A `429 Too Many Requests` is neither: the
//! server allows fewer connections through this address, and may say how
//! long to wait. That worker ends too, and its link holds one fewer
//! (PLAN.md item 8).

use std::fs::File;
use std::net::IpAddr;
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use reqwest::header::{CONTENT_RANGE, IF_RANGE, RANGE, RETRY_AFTER};
use reqwest::{Client, Response, StatusCode, Url};
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;

use super::disk::write_all_at;
use super::remote::{Validator, content_range, retry_after};
use super::wait_words;
use crate::links::{LinkId, bind};

/// How much a worker gathers before writing: fewer, larger writes.
const WRITE_PIECE: usize = 256 << 10;

pub(crate) struct Worker {
    pub link: LinkId,
    /// Which of the download's workers this is, for its reports.
    pub seat: u64,
    /// When this worker's connection last received bytes, as [`stamp`] made
    /// it; 0 until it has.
    pub heard: Arc<AtomicU64>,
    /// What `heard` is measured from.
    pub epoch: Instant,
    /// What this worker has written of the chunk it is on, from the chunk's
    /// start, so progress saved between its reports has it too.
    pub doing: Arc<Mutex<Option<Range<u64>>>>,
    /// The link's address. The worker binds its own client to it.
    pub local: IpAddr,
    /// The file as this link found it, after its redirects.
    pub url: Url,
    /// This link's validator for the file, sent as `If-Range`.
    pub validator: Option<Validator>,
    pub file: Arc<File>,
    pub reports: mpsc::UnboundedSender<Report>,
    /// Bytes this link's workers have written to the file.
    pub written: Arc<AtomicU64>,
    /// Becomes true when the link must stop.
    pub stop: watch::Receiver<bool>,
    pub read_deadline: Duration,
}

/// What became of one chunk, or of the whole file.
#[derive(Debug)]
pub(crate) struct Report {
    pub link: LinkId,
    pub seat: u64,
    pub range: Range<u64>,
    /// Bytes written from the range's start. The rest is still missing.
    pub written: u64,
    pub outcome: Outcome,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// Every byte asked for is written.
    Done,
    /// The answer ended cleanly with fewer bytes than asked. The worker
    /// carries on.
    Short,
    /// The link was told to stop.
    Stopped,
    /// The connection failed: no data within the read deadline, or the
    /// request or its answer broke. The worker ends, and the coordinator
    /// judges whether its link is failing.
    Broken(String),
    /// The server's answer cannot be used: an unexpected status, a piece of
    /// other bytes, an answer with nothing in it. The worker ends.
    Failed(String),
    /// The server refused the connection with `429 Too Many Requests`, and
    /// asked to wait this long if it said. The worker ends; its link holds
    /// one connection fewer (PLAN.md item 8).
    Capped(Option<Duration>),
    /// The file on the server is not the one being fetched any more. The
    /// download ends.
    Changed(String),
    /// The file could not be written. The download ends.
    Disk(String),
}

/// Why a worker ended without a chunk to report.
pub(crate) type Exit = Option<String>;

/// Now, as a worker's `heard` holds it: milliseconds since `epoch`, plus one,
/// so that 0 can mean never.
pub(crate) fn stamp(epoch: Instant) -> u64 {
    u64::try_from(epoch.elapsed().as_millis()).unwrap_or(u64::MAX - 1) + 1
}

enum Wait<T> {
    Got(T),
    TimedOut,
    Stopped,
}

impl Worker {
    /// Take chunks of a file `length` bytes long from `pile` until it closes.
    pub(crate) async fn run(mut self, pile: flume::Receiver<Range<u64>>, length: u64) -> Exit {
        // Its own client: never shared, so no connection is reused by
        // another worker, let alone another link (PLAN.md item 9).
        let client = match bind::client(self.local, self.url.as_str()).await {
            Ok(client) => client,
            Err(e) => return Some(format!("{e:#}")),
        };
        loop {
            let range = tokio::select! {
                next = pile.recv_async() => match next {
                    Ok(range) => range,
                    // The pile is closed: the download is over.
                    Err(_) => return None,
                },
                () = stopped(&mut self.stop) => return None,
            };
            let (written, outcome) = self.piece(&client, &range, length).await;
            let carry_on = matches!(outcome, Outcome::Done | Outcome::Short);
            if !self.report(range, written, outcome) || !carry_on {
                return None;
            }
        }
    }

    /// Fetch the whole file with one plain `GET`, for a server that sends no
    /// pieces. `length` is the file's, when the server said.
    pub(crate) async fn run_whole(mut self, length: Option<u64>) -> Exit {
        let client = match bind::client(self.local, self.url.as_str()).await {
            Ok(client) => client,
            Err(e) => return Some(format!("{e:#}")),
        };
        let (written, outcome) = self.whole(&client, length).await;
        self.report(0..length.unwrap_or(written), written, outcome);
        None
    }

    fn report(&self, range: Range<u64>, written: u64, outcome: Outcome) -> bool {
        let report = Report {
            link: self.link,
            seat: self.seat,
            range,
            written,
            outcome,
        };
        self.reports.send(report).is_ok()
    }

    /// Fetch `range` of a file `length` bytes long and write what arrives of
    /// it: how many bytes from its start were written, and how it ended.
    async fn piece(&mut self, client: &Client, range: &Range<u64>, length: u64) -> (u64, Outcome) {
        *self.doing.lock().unwrap() = Some(range.start..range.start);
        let asked = format!("bytes={}-{}", range.start, range.end - 1);
        let mut request = client.get(self.url.clone()).header(RANGE, asked);
        if let Some(validator) = &self.validator {
            request = request.header(IF_RANGE, validator.value());
        }
        let response = match self.within(request.send()).await {
            Wait::Got(Ok(response)) => response,
            Wait::Got(Err(e)) => return (0, Outcome::Broken(format!("asking for a piece: {e}"))),
            Wait::TimedOut => return (0, self.no_answer()),
            Wait::Stopped => return (0, Outcome::Stopped),
        };
        let answer_end = match self.check(&response, range, length) {
            Ok(end) => end,
            Err(outcome) => return (0, outcome),
        };
        let (written, ended) = self
            .copy(response, range.start, answer_end.min(range.end))
            .await;
        let outcome = match ended {
            Some(outcome) => outcome,
            None if written == range.end - range.start => Outcome::Done,
            None if written > 0 => Outcome::Short,
            None => Outcome::Failed("the answer ended before any of the piece arrived".into()),
        };
        (written, outcome)
    }

    /// The end of the bytes the answer holds, if it is a piece of this file
    /// starting where `range` does.
    fn check(&self, response: &Response, range: &Range<u64>, length: u64) -> Result<u64, Outcome> {
        let status = response.status();
        if status == StatusCode::OK {
            // With If-Range, the whole file is the server's way of saying
            // the version asked for is gone.
            return Err(Outcome::Changed(if self.validator.is_some() {
                "the file changed on the server during the download, so it stopped rather than \
                 mix two versions"
                    .into()
            } else {
                "the server stopped sending pieces partway through the download".into()
            }));
        }
        if status == StatusCode::TOO_MANY_REQUESTS {
            let wait = response
                .headers()
                .get(RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| retry_after(v, SystemTime::now()));
            return Err(Outcome::Capped(wait));
        }
        if status != StatusCode::PARTIAL_CONTENT {
            return Err(Outcome::Failed(format!(
                "the server answered {status} to a request for a piece"
            )));
        }
        let text = response
            .headers()
            .get(CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
            .ok_or(Outcome::Failed(
                "a piece came without a Content-Range".into(),
            ))?;
        match content_range(text) {
            Some((_, _, Some(total))) if total != length => Err(Outcome::Changed(format!(
                "the file's length on the server changed during the download ({total} bytes, \
                 not {length}), so it stopped rather than mix two versions"
            ))),
            Some((start, end, Some(_))) if start == range.start && end >= start => Ok(end + 1),
            _ => Err(Outcome::Failed(format!(
                "asked for bytes {}-{} of {length}, the Content-Range says {text:?}",
                range.start,
                range.end - 1
            ))),
        }
    }

    /// The whole file from byte 0: how many bytes were written, and how it
    /// ended.
    async fn whole(&mut self, client: &Client, length: Option<u64>) -> (u64, Outcome) {
        let response = match self.within(client.get(self.url.clone()).send()).await {
            Wait::Got(Ok(response)) => response,
            Wait::Got(Err(e)) => return (0, Outcome::Broken(format!("asking for the file: {e}"))),
            Wait::TimedOut => return (0, self.no_answer()),
            Wait::Stopped => return (0, Outcome::Stopped),
        };
        let status = response.status();
        if status != StatusCode::OK {
            let why = format!("the server answered {status} to a request for the whole file");
            return (0, Outcome::Failed(why));
        }
        let (written, ended) = self.copy(response, 0, length.unwrap_or(u64::MAX)).await;
        let outcome = match (ended, length) {
            (Some(outcome), _) => outcome,
            (None, Some(length)) if written != length => {
                Outcome::Failed(format!("the file ended after {written} of {length} bytes"))
            }
            (None, _) => Outcome::Done,
        };
        (written, outcome)
    }

    /// Write the answer's bytes at `from` onward, up to `stop_at`: how many
    /// were written, and the outcome if anything but the answer's end or
    /// `stop_at` ended it.
    async fn copy(
        &mut self,
        mut response: Response,
        from: u64,
        stop_at: u64,
    ) -> (u64, Option<Outcome>) {
        let mut at = from;
        let mut gathered: Vec<u8> = Vec::with_capacity(WRITE_PIECE);
        let ended = loop {
            let piece = match self.within(response.chunk()).await {
                Wait::Got(Ok(Some(piece))) => {
                    self.heard.store(stamp(self.epoch), Ordering::SeqCst);
                    piece
                }
                Wait::Got(Ok(None)) => break None,
                Wait::Got(Err(e)) => {
                    break Some(Outcome::Broken(format!("the answer broke off: {e}")));
                }
                Wait::TimedOut => break Some(self.no_answer()),
                Wait::Stopped => break Some(Outcome::Stopped),
            };
            let left = (stop_at - at) as usize - gathered.len();
            gathered.extend_from_slice(&piece[..piece.len().min(left)]);
            let full = gathered.len() == (stop_at - at) as usize;
            if gathered.len() >= WRITE_PIECE || full {
                match self.write(at, &mut gathered).await {
                    Ok(n) => {
                        at += n;
                        *self.doing.lock().unwrap() = Some(from..at);
                    }
                    Err(why) => return (at - from, Some(Outcome::Disk(why))),
                }
            }
            if full {
                break None;
            }
        };
        // Whatever arrived is the file's, however the answer ended.
        if !gathered.is_empty() {
            match self.write(at, &mut gathered).await {
                Ok(n) => {
                    at += n;
                    *self.doing.lock().unwrap() = Some(from..at);
                }
                Err(why) => return (at - from, Some(Outcome::Disk(why))),
            }
        }
        (at - from, ended)
    }

    /// Write `gathered` at `at` and count it for the link: the bytes written.
    async fn write(&self, at: u64, gathered: &mut Vec<u8>) -> Result<u64, String> {
        let data = std::mem::replace(gathered, Vec::with_capacity(WRITE_PIECE));
        let n = data.len() as u64;
        let file = self.file.clone();
        let wrote = tokio::task::spawn_blocking(move || write_all_at(&file, &data, at)).await;
        match wrote {
            Ok(Ok(())) => {
                self.written.fetch_add(n, Ordering::SeqCst);
                Ok(n)
            }
            Ok(Err(e)) => Err(e.to_string()),
            Err(e) => Err(e.to_string()),
        }
    }

    /// `work`, unless the read deadline passes or the link is told to stop
    /// first.
    async fn within<T>(&mut self, work: impl Future<Output = T>) -> Wait<T> {
        tokio::select! {
            got = tokio::time::timeout(self.read_deadline, work) => match got {
                Ok(value) => Wait::Got(value),
                Err(_) => Wait::TimedOut,
            },
            () = stopped(&mut self.stop) => Wait::Stopped,
        }
    }

    fn no_answer(&self) -> Outcome {
        Outcome::Broken(format!(
            "no data from the server for {}",
            wait_words(self.read_deadline)
        ))
    }
}

/// Resolves once the link is told to stop, or its coordinator is gone.
async fn stopped(stop: &mut watch::Receiver<bool>) {
    let _ = stop.wait_for(|&stop| stop).await;
}
