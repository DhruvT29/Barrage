//! One connection of one link: take a chunk from the pile, fetch it with a
//! `Range` request, write it at its place in the file, and say how it went.
//!
//! A worker keeps only its own client and the counter its link's workers
//! share. Everything else belongs to the download's coordinator, which hears
//! from it by message (PLAN.md section 3.5).
//!
//! ONLY THE BYTES ASKED FOR ARE WRITTEN
//!
//! An answer must be a 206 for the file's own length, starting where the
//! chunk starts. Past the chunk's end nothing is written, however long the
//! answer runs. An answer that ends short, cleanly or not, leaves its bytes
//! written and the rest of the chunk for the pile, so no byte is asked for
//! twice. A 200 is the whole file, which belongs at no chunk's offset, so it
//! writes nothing.
//!
//! For now a failed request ends its worker, and the link carries on with
//! the rest. Telling a bad connection from a failing link, and resting the
//! link, comes in Task 9.

use std::fs::File;
use std::net::IpAddr;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use reqwest::header::{CONTENT_RANGE, RANGE};
use reqwest::{Client, Response, StatusCode, Url};
use tokio::sync::{mpsc, watch};

use super::disk::write_all_at;
use super::remote::content_range;
use crate::links::{LinkId, bind};

/// How much a worker gathers before writing: fewer, larger writes.
const WRITE_PIECE: usize = 256 << 10;

pub(crate) struct Worker {
    pub link: LinkId,
    /// The link's address. The worker binds its own client to it.
    pub local: IpAddr,
    /// The file as this link found it, after its redirects.
    pub url: Url,
    pub length: u64,
    pub file: Arc<File>,
    pub pile: flume::Receiver<Range<u64>>,
    pub reports: mpsc::UnboundedSender<Report>,
    /// Bytes this link's workers have written to the file.
    pub written: Arc<AtomicU64>,
    /// Becomes true when the link must stop.
    pub stop: watch::Receiver<bool>,
    pub read_deadline: Duration,
}

/// What became of one chunk.
#[derive(Debug)]
pub(crate) struct Report {
    pub link: LinkId,
    pub range: Range<u64>,
    /// Bytes written from the chunk's start. The rest is still missing.
    pub written: u64,
    pub outcome: Outcome,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// Every byte of the chunk is written.
    Done,
    /// The answer ended cleanly with fewer bytes than asked. The worker
    /// carries on.
    Short,
    /// The link was told to stop.
    Stopped,
    /// The request or its answer failed. The worker ends.
    Failed(String),
    /// The file could not be written. The download ends.
    Disk(String),
}

/// Why a worker ended without a chunk to report.
pub(crate) type Exit = Option<String>;

enum Wait<T> {
    Got(T),
    TimedOut,
    Stopped,
}

impl Worker {
    pub(crate) async fn run(mut self) -> Exit {
        // Its own client: never shared, so no connection is reused by
        // another worker, let alone another link (PLAN.md item 9).
        let client = match bind::client(self.local, self.url.as_str()).await {
            Ok(client) => client,
            Err(e) => return Some(format!("{e:#}")),
        };
        loop {
            let range = tokio::select! {
                next = self.pile.recv_async() => match next {
                    Ok(range) => range,
                    // The pile is closed: the download is over.
                    Err(_) => return None,
                },
                () = stopped(&mut self.stop) => return None,
            };
            let (written, outcome) = self.fetch(&client, &range).await;
            let carry_on = matches!(outcome, Outcome::Done | Outcome::Short);
            let report = Report {
                link: self.link,
                range,
                written,
                outcome,
            };
            if self.reports.send(report).is_err() || !carry_on {
                return None;
            }
        }
    }

    /// Fetch `range` and write what arrives of it: how many bytes from its
    /// start were written, and how it ended.
    async fn fetch(&mut self, client: &Client, range: &Range<u64>) -> (u64, Outcome) {
        let asked = format!("bytes={}-{}", range.start, range.end - 1);
        let sent = client.get(self.url.clone()).header(RANGE, asked).send();
        let response = match self.within(sent).await {
            Wait::Got(Ok(response)) => response,
            Wait::Got(Err(e)) => return (0, Outcome::Failed(format!("asking for a piece: {e}"))),
            Wait::TimedOut => return (0, self.no_answer()),
            Wait::Stopped => return (0, Outcome::Stopped),
        };
        let answer_end = match self.check(&response, range) {
            Ok(end) => end,
            Err(why) => return (0, Outcome::Failed(why)),
        };
        self.copy(response, range, answer_end.min(range.end)).await
    }

    /// The end of the bytes the answer holds, if it is a piece of this file
    /// starting where `range` does.
    fn check(&self, response: &Response, range: &Range<u64>) -> Result<u64, String> {
        let status = response.status();
        if status == StatusCode::OK {
            return Err("the server sent the whole file instead of the piece asked for".into());
        }
        if status != StatusCode::PARTIAL_CONTENT {
            return Err(format!(
                "the server answered {status} to a request for a piece"
            ));
        }
        let text = response
            .headers()
            .get(CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
            .ok_or("a piece came without a Content-Range")?;
        match content_range(text) {
            Some((start, end, Some(total)))
                if start == range.start && end >= start && total == self.length =>
            {
                Ok(end + 1)
            }
            _ => Err(format!(
                "asked for bytes {}-{} of {}, the Content-Range says {text:?}",
                range.start,
                range.end - 1,
                self.length
            )),
        }
    }

    /// Write the answer's bytes from `range.start` up to `stop_at`.
    async fn copy(
        &mut self,
        mut response: Response,
        range: &Range<u64>,
        stop_at: u64,
    ) -> (u64, Outcome) {
        let mut at = range.start;
        let mut gathered: Vec<u8> = Vec::with_capacity(WRITE_PIECE);
        let ended = loop {
            let piece = match self.within(response.chunk()).await {
                Wait::Got(Ok(Some(piece))) => piece,
                Wait::Got(Ok(None)) => break None,
                Wait::Got(Err(e)) => {
                    break Some(Outcome::Failed(format!("the piece broke off: {e}")));
                }
                Wait::TimedOut => break Some(self.no_answer()),
                Wait::Stopped => break Some(Outcome::Stopped),
            };
            let room = (stop_at - at) as usize - gathered.len();
            gathered.extend_from_slice(&piece[..piece.len().min(room)]);
            let full = gathered.len() == (stop_at - at) as usize;
            if gathered.len() >= WRITE_PIECE || full {
                match self.write(at, &mut gathered).await {
                    Ok(n) => at += n,
                    Err(why) => return (at - range.start, Outcome::Disk(why)),
                }
            }
            if full {
                break None;
            }
        };
        // Whatever arrived is the file's, however the answer ended.
        if !gathered.is_empty() {
            match self.write(at, &mut gathered).await {
                Ok(n) => at += n,
                Err(why) => return (at - range.start, Outcome::Disk(why)),
            }
        }
        let written = at - range.start;
        let outcome = match ended {
            Some(outcome) => outcome,
            None if at == range.end => Outcome::Done,
            None if written > 0 => Outcome::Short,
            None => Outcome::Failed("the answer ended before any of the piece arrived".into()),
        };
        (written, outcome)
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
        Outcome::Failed(format!(
            "no data from the server for {:?}",
            self.read_deadline
        ))
    }
}

/// Resolves once the link is told to stop, or its coordinator is gone.
async fn stopped(stop: &mut watch::Receiver<bool>) {
    let _ = stop.wait_for(|&stop| stop).await;
}
