//! One connection of one link: wait for the coordinator to give it a part of
//! the file, fetch all of it with one `Range` request, write it at its place
//! in the file, and say how it went.
//!
//! A worker keeps only its own client and the counter its link's workers
//! share. Its claim on the file (`spans.rs`) is the one thing it shares with
//! the download's coordinator, which hands claims out and may cut one short
//! for another connection; everything else the coordinator hears by message
//! (PLAN.md section 3.5).
//!
//! ONLY THE BYTES ASKED FOR, OF THE VERSION ASKED FOR
//!
//! Every piece is asked for with `If-Range` and the link's validator, so a
//! server whose file has changed answers with the whole new file (200)
//! rather than a piece of it. A piece must be a 206 for the file's own
//! length, starting where the claim's unwritten part starts. Past the
//! claim's end nothing is written, however long the answer runs, and the end
//! is read again before each write, since a cut moves it: the worker then
//! stops there and drops the answer. Each write is reserved on the claim
//! first, so a cut never takes bytes on their way to the file. An answer
//! that ends cleanly but short leaves its bytes written and asks again for
//! the rest; one that breaks leaves the rest for the coordinator to hand
//! out, so no byte is asked for twice. A 200, or another length, means the
//! file is no longer the one being fetched: the download stops rather than
//! stitch two versions together.
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
//! receiving, so each worker notes when its own last received bytes. A
//! server error (5xx) is reported broken too: the server failing this
//! request, which a later one may not. An answer that arrives but cannot be
//! used, an unexpected status or a piece of the wrong bytes, is no fault of
//! the connection: that worker ends, and no other takes its place. A
//! `429 Too Many Requests` or a `503 Service Unavailable` is neither: the
//! server allows fewer connections or requests through this address, and may
//! say how long to wait. That worker ends too, and its link holds one fewer
//! (PLAN.md item 8). A `401`, `403`, `404` or `410` to the address a
//! redirect led to says the address is gone, as a signed one is once it
//! expires, and the coordinator may find the file again.

use std::fs::File;
use std::net::IpAddr;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use reqwest::header::{CONTENT_RANGE, IF_RANGE, RANGE, RETRY_AFTER};
use reqwest::{Client, Response, StatusCode, Url};
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;

use super::disk::write_all_at;
use super::remote::{Validator, content_range, retry_after};
use super::spans::Hand;
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
    /// This worker's claim on the file, which the coordinator hands out and
    /// may cut; what it has written of it is there too, so progress saved
    /// between its reports has it.
    pub hand: Arc<Hand>,
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
    /// The server refused the connection with this status, `429 Too Many
    /// Requests` or `503 Service Unavailable`, and asked to wait this long
    /// if it said. The worker ends; its link holds one connection fewer
    /// (PLAN.md item 8).
    Capped(String, Option<Duration>),
    /// The address answered `401`, `403`, `404` or `410`: not this file, or
    /// not any more. The worker ends.
    Gone(String),
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
    /// Fetch the parts of a file `length` bytes long the coordinator gives
    /// this worker, until it is told to stop.
    pub(crate) async fn run(mut self, length: u64) -> Exit {
        // Its own client: never shared, so no connection is reused by
        // another worker, let alone another link (PLAN.md item 9).
        let client = match bind::client(self.local, self.url.as_str()).await {
            Ok(client) => client,
            Err(e) => return Some(format!("{e:#}")),
        };
        loop {
            // Told to stop while it waited: nothing to report.
            let range = self.next().await?;
            let (written, outcome) = self.piece(&client, &range, length).await;
            let carry_on = matches!(outcome, Outcome::Done | Outcome::Short);
            if !self.report(range, written, outcome) || !carry_on {
                return None;
            }
        }
    }

    /// What to ask for next: what its claim has not written yet, once the
    /// coordinator has given it one. `None` once the link is told to stop,
    /// or this connection to retire.
    async fn next(&mut self) -> Option<Range<u64>> {
        let hand = self.hand.clone();
        loop {
            if hand.retire.load(Ordering::SeqCst) {
                return None;
            }
            let left = {
                let claim = hand.claim.lock().unwrap();
                (!claim.is_done()).then_some(claim.reserved..claim.end)
            };
            if left.is_some() {
                return left;
            }
            tokio::select! {
                () = hand.ready.notified() => {}
                () = stopped(&mut self.stop) => return None,
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

    /// Fetch `range` of a file `length` bytes long, the rest of its claim,
    /// and write what arrives of it up to the claim's end, wherever a cut
    /// has moved it: how many bytes from its start were written, and how it
    /// ended.
    async fn piece(&mut self, client: &Client, range: &Range<u64>, length: u64) -> (u64, Outcome) {
        let asked = format!("bytes={}-{}", range.start, range.end - 1);
        let mut request = client.get(self.url.clone()).header(RANGE, &asked);
        if let Some(validator) = &self.validator {
            request = request.header(IF_RANGE, validator.value());
        }
        let response = match self.within(request.send()).await {
            Wait::Got(Ok(response)) => response,
            Wait::Got(Err(e)) => return (0, Outcome::Broken(format!("asking for a piece: {e}"))),
            Wait::TimedOut => return (0, self.no_answer()),
            Wait::Stopped => return (0, Outcome::Stopped),
        };
        // One line a request, so a log shows how often a server was asked.
        tracing::debug!(
            link = self.link.as_u64(),
            seat = self.seat,
            range = %asked,
            status = %response.status(),
            "asked for a part of the file"
        );
        let validated = self.validator.is_some();
        let answer_end = match answer_end(&response, range, length, validated) {
            Ok(end) => end,
            Err(outcome) => return (0, outcome),
        };
        let (written, ended) = self.copy(response, range.start, answer_end).await;
        let outcome = match ended {
            Some(outcome) => outcome,
            None if self.hand.claim.lock().unwrap().is_done() => Outcome::Done,
            None if written > 0 => Outcome::Short,
            None => Outcome::Failed("the answer ended before any of the piece arrived".into()),
        };
        (written, outcome)
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

    /// Write the answer's bytes at `from` onward, up to `answer_end` or the
    /// claim's end, whichever comes first, read again as each piece arrives
    /// since a cut moves it: how many were written, and the outcome if
    /// anything but reaching one of those ends, or the answer's own, ended
    /// it.
    async fn copy(
        &mut self,
        mut response: Response,
        from: u64,
        answer_end: u64,
    ) -> (u64, Option<Outcome>) {
        let mut at = from;
        let mut gathered: Vec<u8> = Vec::with_capacity(WRITE_PIECE);
        let ended = loop {
            let piece = match self.within(response.chunk()).await {
                Wait::Got(Ok(Some(piece))) => {
                    self.heard.store(stamp(self.epoch), Ordering::SeqCst);
                    self.hand
                        .got
                        .fetch_add(piece.len() as u64, Ordering::SeqCst);
                    piece
                }
                Wait::Got(Ok(None)) => break None,
                Wait::Got(Err(e)) => {
                    break Some(Outcome::Broken(format!("the answer broke off: {e}")));
                }
                Wait::TimedOut => break Some(self.no_answer()),
                Wait::Stopped => break Some(Outcome::Stopped),
            };
            let stop_at = self.hand.claim.lock().unwrap().end.min(answer_end);
            let left = stop_at.saturating_sub(at + gathered.len() as u64);
            gathered.extend_from_slice(&piece[..piece.len().min(left as usize)]);
            let full = at + gathered.len() as u64 >= stop_at;
            if gathered.len() >= WRITE_PIECE || full {
                match self.write(&mut gathered).await {
                    Ok(n) => at += n,
                    Err(why) => return (at - from, Some(Outcome::Disk(why))),
                }
            }
            // Its claim's end, or the answer's: whatever the server sends
            // past it is dropped with the answer.
            if full || self.hand.claim.lock().unwrap().is_done() {
                break None;
            }
        };
        // Whatever arrived is the file's, however the answer ended.
        if !gathered.is_empty() {
            match self.write(&mut gathered).await {
                Ok(n) => at += n,
                Err(why) => return (at - from, Some(Outcome::Disk(why))),
            }
        }
        (at - from, ended)
    }

    /// Write `gathered` where the claim's unwritten part begins, and count it
    /// for the claim and the link: the bytes written. Reserved on the claim
    /// first, so no cut takes them; a cut that came first leaves fewer to
    /// write, and the rest is dropped.
    async fn write(&self, gathered: &mut Vec<u8>) -> Result<u64, String> {
        let mut data = std::mem::replace(gathered, Vec::with_capacity(WRITE_PIECE));
        let range = self.hand.claim.lock().unwrap().reserve(data.len() as u64);
        let n = range.end - range.start;
        data.truncate(n as usize);
        let file = self.file.clone();
        let wrote =
            tokio::task::spawn_blocking(move || write_all_at(&file, &data, range.start)).await;
        match wrote {
            Ok(Ok(())) => {
                self.hand.claim.lock().unwrap().wrote(n);
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

/// The end of the bytes `response` holds, if it is a piece of a file
/// `length` bytes long starting where `range` does; else what became of the
/// request. `validated`: it was asked with `If-Range`.
fn answer_end(
    response: &Response,
    range: &Range<u64>,
    length: u64,
    validated: bool,
) -> Result<u64, Outcome> {
    let status = response.status();
    if status == StatusCode::OK {
        // With If-Range, the whole file is the server's way of saying the
        // version asked for is gone.
        return Err(Outcome::Changed(if validated {
            "the file changed on the server during the download, so it stopped rather than mix \
             two versions"
                .into()
        } else {
            "the server stopped sending pieces partway through the download".into()
        }));
    }
    if status == StatusCode::TOO_MANY_REQUESTS || status == StatusCode::SERVICE_UNAVAILABLE {
        let wait = response
            .headers()
            .get(RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| retry_after(v, SystemTime::now()));
        return Err(Outcome::Capped(status.to_string(), wait));
    }
    let why = format!("the server answered {status} to a request for a piece");
    if status.is_server_error() {
        return Err(Outcome::Broken(why));
    }
    if matches!(status.as_u16(), 401 | 403 | 404 | 410) {
        return Err(Outcome::Gone(why));
    }
    if status != StatusCode::PARTIAL_CONTENT {
        return Err(Outcome::Failed(why));
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
            "the file's length on the server changed during the download ({total} bytes, not \
             {length}), so it stopped rather than mix two versions"
        ))),
        // Ending inside the file, so the end past it is a byte that exists.
        Some((start, end, Some(_))) if start == range.start && start <= end && end < length => {
            Ok(end + 1)
        }
        _ => Err(Outcome::Failed(format!(
            "asked for bytes {}-{} of {length}, the Content-Range says {text:?}",
            range.start,
            range.end - 1
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An answer with this status and these headers.
    fn answer(status: u16, headers: &[(&str, &str)]) -> Response {
        let mut built = axum::http::Response::builder().status(status);
        for (name, value) in headers {
            built = built.header(*name, *value);
        }
        Response::from(built.body(Vec::<u8>::new()).unwrap())
    }

    const LENGTH: u64 = 10_000;

    fn end_of(status: u16, headers: &[(&str, &str)]) -> Result<u64, Outcome> {
        answer_end(&answer(status, headers), &(4000..8000), LENGTH, true)
    }

    #[test]
    fn a_piece_is_its_bytes_of_this_file() {
        let piece = |range: &str| end_of(206, &[("content-range", range)]);
        assert_eq!(piece("bytes 4000-7999/10000"), Ok(8000));
        // Shorter than asked, or running on past the chunk, to the file's
        // last byte: the worker writes only what the chunk needs.
        assert_eq!(piece("bytes 4000-4999/10000"), Ok(5000));
        assert_eq!(piece("bytes 4000-9999/10000"), Ok(10000));
        // Starting elsewhere is other bytes.
        assert!(matches!(
            piece("bytes 0-3999/10000"),
            Err(Outcome::Failed(_))
        ));
        // Another total is another file.
        assert!(matches!(
            piece("bytes 4000-7999/20000"),
            Err(Outcome::Changed(_))
        ));
        assert!(matches!(end_of(206, &[]), Err(Outcome::Failed(_))));
    }

    /// An end past the file is a lie the bytes cannot keep, and the largest
    /// one would wrap the worker's counting round to 0 (PR #4's review).
    #[test]
    fn a_piece_ending_past_the_file_is_refused() {
        let piece = |range: &str| end_of(206, &[("content-range", range)]);
        for range in [
            "bytes 4000-10000/10000",
            "bytes 4000-18446744073709551615/10000",
            "bytes 4000-3999/10000",
        ] {
            assert!(matches!(piece(range), Err(Outcome::Failed(_))), "{range}");
        }
    }

    #[test]
    fn a_429_or_a_503_is_a_ceiling_with_its_wait() {
        assert_eq!(
            end_of(429, &[("retry-after", "7")]),
            Err(Outcome::Capped(
                "429 Too Many Requests".into(),
                Some(Duration::from_secs(7))
            ))
        );
        assert_eq!(
            end_of(503, &[]),
            Err(Outcome::Capped("503 Service Unavailable".into(), None))
        );
    }

    #[test]
    fn a_server_error_is_the_connection_failing() {
        for status in [500, 502, 504] {
            let ended = end_of(status, &[]);
            assert!(
                matches!(ended, Err(Outcome::Broken(_))),
                "{status}: {ended:?}"
            );
        }
    }

    #[test]
    fn an_address_that_refuses_or_has_nothing_is_gone() {
        for status in [401, 403, 404, 410] {
            let ended = end_of(status, &[]);
            assert!(
                matches!(ended, Err(Outcome::Gone(_))),
                "{status}: {ended:?}"
            );
        }
        // Other statuses are an answer that cannot be used.
        for status in [301, 400, 416] {
            let ended = end_of(status, &[]);
            assert!(
                matches!(ended, Err(Outcome::Failed(_))),
                "{status}: {ended:?}"
            );
        }
    }

    #[test]
    fn the_whole_file_means_it_changed_or_stopped_coming_in_pieces() {
        let changed = answer_end(&answer(200, &[]), &(4000..8000), LENGTH, true);
        assert!(matches!(changed, Err(Outcome::Changed(w)) if w.contains("changed")));
        let unasked = answer_end(&answer(200, &[]), &(4000..8000), LENGTH, false);
        assert!(matches!(unasked, Err(Outcome::Changed(w)) if w.contains("stopped sending")));
    }
}
