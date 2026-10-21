//! A scripted file server on 127.0.0.1, for the downloader's tests.
//!
//! It serves a seeded pseudo-random file, so bytes written at the wrong
//! offset cannot match by chance, and it records every request: where it came
//! from, the method, the path, the range asked, the bytes sent, and when the
//! answer began and ended. The source address is how a test tells its links
//! apart: a socket bound to 127.0.0.2 reaches this server and shows up here
//! as 127.0.0.2.
//!
//! Every path answers as scripted with [`Server::at`], and anything not
//! scripted is 404. Phase 3 grows the scripts task by task, each adding the
//! misbehaviour its tests need.
#![allow(dead_code)]

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::ops::Range;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use barrage::fetch::PROBE_BYTES;
use tokio::net::TcpListener;

/// What one path answers.
#[derive(Clone, Debug)]
// A handful of scripts per test, so the size of a File in each is nothing.
#[allow(clippy::large_enum_variant)]
pub enum Answer {
    File(File),
    /// This status with this `Location`, written exactly as given.
    Redirect(u16, String),
    /// This status with these headers and no body, for answers no real file
    /// would get.
    Headers(u16, Vec<(&'static str, String)>),
    /// This status alone.
    Status(u16),
    /// Nothing, ever: the connection is held open with no answer.
    Silent,
}

/// A file of [`content`] and how its requests are answered.
#[derive(Clone, Debug)]
pub struct File {
    pub length: u64,
    /// Answer `Range` with 206. Off: every answer is the whole file, 200.
    pub ranges: bool,
    /// Send `Content-Length` with the whole file. Off: the body is chunked.
    pub send_length: bool,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub disposition: Option<String>,
    /// Stop after this many body bytes and hold the connection open.
    pub stall_after: Option<u64>,
    /// Send each answer at this many bytes a second. `None`: as fast as the
    /// machine goes.
    pub rate: Option<u64>,
    /// Hold each ranged answer to this many bytes, with a `Content-Range`
    /// shorter than the range asked, as some servers do.
    pub max_answer: Option<u64>,
    /// Run each ranged answer on to the end of the file whatever end was
    /// asked. The bytes past the asked end are not the file's, so writing
    /// them anywhere shows.
    pub past_the_range: bool,
    /// Answer each range from byte 0, whatever start was asked, with a
    /// `Content-Range` that says so.
    pub from_the_top: bool,
    pub cuts: Vec<Cut>,
    pub stalls: Vec<Stall>,
    pub caps: Vec<Cap>,
    pub refusals: Vec<Refusal>,
    /// The ETag one source is sent instead of `etag`: another version of the
    /// file, as one CDN node might hold.
    pub etag_for: Vec<(IpAddr, String)>,
    /// The file as it is once replaced, after some requests.
    pub replaced: Option<Replaced>,
    /// Requests for this file so far, from every source.
    requests: Arc<AtomicUsize>,
}

/// The file after it is replaced on the server: from the `after`th request
/// on, its ETag, its length, or both, are these.
#[derive(Clone, Debug)]
pub struct Replaced {
    after: usize,
    etag: Option<String>,
    length: Option<u64>,
}

/// Close the connection partway through some answers to one source.
#[derive(Clone, Debug)]
pub struct Cut {
    source: IpAddr,
    after: u64,
    left: Arc<AtomicUsize>,
}

impl Cut {
    /// Use up one cut, if any are left.
    fn take(&self) -> bool {
        self.left
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok()
    }
}

/// Stop sending partway through some answers to one source and hold the
/// connection open, as a connection that stops carrying data does. Never a
/// probe: the link still finds the file, then stops getting it.
#[derive(Clone, Debug)]
pub struct Stall {
    source: IpAddr,
    after: u64,
    /// How many answers are left to stall; `None`, every one.
    left: Option<Arc<AtomicUsize>>,
}

impl Stall {
    /// Use up one stall, if any are left.
    fn take(&self) -> bool {
        self.left.as_ref().is_none_or(|left| {
            left.try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                .is_ok()
        })
    }
}

/// Answer `429 Too Many Requests` to one source's pieces beyond a number open
/// at once, as a server that limits connections per address does.
#[derive(Clone, Debug)]
pub struct Cap {
    source: IpAddr,
    most: usize,
    retry_after: Option<String>,
    /// The source's pieces being answered now.
    open: Arc<AtomicUsize>,
}

impl Cap {
    /// Room for one more piece, held until the guard is dropped, or `None`.
    fn enter(&self) -> Option<Slot> {
        self.open
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                (n < self.most).then_some(n + 1)
            })
            .ok()
            .map(|_| Slot(self.open.clone()))
    }
}

/// One piece's place under a [`Cap`], given back when its answer ends.
pub struct Slot(Arc<AtomicUsize>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Answer `429 Too Many Requests` to some of one source's pieces, whatever
/// else is open.
#[derive(Clone, Debug)]
pub struct Refusal {
    source: IpAddr,
    /// How many are left to refuse; `None`, every one.
    left: Option<Arc<AtomicUsize>>,
    retry_after: Option<String>,
}

impl Refusal {
    /// Use up one refusal, if any are left.
    fn take(&self) -> bool {
        self.left.as_ref().is_none_or(|left| {
            left.try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                .is_ok()
        })
    }
}

impl File {
    pub fn new(length: u64) -> Self {
        Self {
            length,
            ranges: true,
            send_length: true,
            etag: None,
            last_modified: None,
            disposition: None,
            stall_after: None,
            rate: None,
            max_answer: None,
            past_the_range: false,
            from_the_top: false,
            cuts: Vec::new(),
            stalls: Vec::new(),
            caps: Vec::new(),
            refusals: Vec::new(),
            etag_for: Vec::new(),
            replaced: None,
            requests: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub fn etag_for(mut self, source: &str, tag: &str) -> Self {
        self.etag_for
            .push((source.parse().unwrap(), tag.to_string()));
        self
    }

    /// Replace the file with a new version, ETag `tag`, after `after`
    /// requests.
    pub fn replaced_after(mut self, after: usize, tag: &str) -> Self {
        self.replaced = Some(Replaced {
            after,
            etag: Some(tag.to_string()),
            length: None,
        });
        self
    }

    /// Make the file `length` bytes long after `after` requests, keeping its
    /// ETag: a change only the length shows.
    pub fn grows_after(mut self, after: usize, length: u64) -> Self {
        self.replaced = Some(Replaced {
            after,
            etag: None,
            length: Some(length),
        });
        self
    }

    pub fn no_ranges(mut self) -> Self {
        self.ranges = false;
        self
    }

    pub fn no_length(mut self) -> Self {
        self.send_length = false;
        self
    }

    pub fn etag(mut self, tag: &str) -> Self {
        self.etag = Some(tag.to_string());
        self
    }

    pub fn last_modified(mut self, date: &str) -> Self {
        self.last_modified = Some(date.to_string());
        self
    }

    pub fn disposition(mut self, value: &str) -> Self {
        self.disposition = Some(value.to_string());
        self
    }

    pub fn stall_after(mut self, bytes: u64) -> Self {
        self.stall_after = Some(bytes);
        self
    }

    pub fn rate(mut self, bytes_per_second: u64) -> Self {
        self.rate = Some(bytes_per_second);
        self
    }

    pub fn max_answer(mut self, bytes: u64) -> Self {
        self.max_answer = Some(bytes);
        self
    }

    pub fn past_the_range(mut self) -> Self {
        self.past_the_range = true;
        self
    }

    pub fn answers_from_the_top(mut self) -> Self {
        self.from_the_top = true;
        self
    }

    /// Cut the next `times` answers to `source` that are longer than `after`
    /// bytes, once `after` bytes are sent.
    pub fn cut(mut self, source: &str, after: u64, times: usize) -> Self {
        self.cuts.push(Cut {
            source: source.parse().unwrap(),
            after,
            left: Arc::new(AtomicUsize::new(times)),
        });
        self
    }

    /// Stall answers to `source` once `after` bytes are sent: the next
    /// `times` of them, or with `None` every one. A probe is never stalled.
    pub fn stalls(mut self, source: &str, after: u64, times: Option<usize>) -> Self {
        self.stalls.push(Stall {
            source: source.parse().unwrap(),
            after,
            left: times.map(|n| Arc::new(AtomicUsize::new(n))),
        });
        self
    }

    /// Refuse with 429 any piece for `source` while `most` of its pieces are
    /// being answered, sending this `Retry-After` if any. Probes pass.
    pub fn cap(mut self, source: &str, most: usize, retry_after: Option<&str>) -> Self {
        self.caps.push(Cap {
            source: source.parse().unwrap(),
            most,
            retry_after: retry_after.map(str::to_string),
            open: Arc::new(AtomicUsize::new(0)),
        });
        self
    }

    /// Refuse with 429 the next `times` pieces for `source`, or with `None`
    /// every one, sending this `Retry-After` if any. Probes pass.
    pub fn refuse(mut self, source: &str, times: Option<usize>, retry_after: Option<&str>) -> Self {
        self.refusals.push(Refusal {
            source: source.parse().unwrap(),
            left: times.map(|n| Arc::new(AtomicUsize::new(n))),
            retry_after: retry_after.map(str::to_string),
        });
        self
    }
}

/// One request, as the server saw it.
#[derive(Clone, Debug)]
pub struct Seen {
    pub source: IpAddr,
    pub method: Method,
    pub path: String,
    pub range: Option<String>,
    pub if_range: Option<String>,
    /// Body bytes handed to the connection so far.
    pub sent: Arc<AtomicU64>,
    pub started: Instant,
    /// When the answer ended, or was dropped because the client went away.
    pub finished: Arc<OnceLock<Instant>>,
    /// The file offset at which this answer was cut, if it was.
    pub cut: Option<u64>,
    /// The file offset at which this answer stalls, if it does.
    pub stall: Option<u64>,
    /// The answer's status; 0 for one never sent.
    pub status: u16,
}

impl Seen {
    /// The range asked, `bytes=START-END`, as (start, end) with the end
    /// included, as the header writes it.
    pub fn asked(&self) -> Option<(u64, u64)> {
        let (start, end) = parse_range(self.range.as_deref()?)?;
        Some((start, end?))
    }

    pub fn sent(&self) -> u64 {
        self.sent.load(Ordering::SeqCst)
    }
}

/// The most requests from `source` that `keep` picks which were open at the
/// same moment. One not finished yet counts as open until now.
pub fn most_at_once(seen: &[Seen], source: IpAddr, keep: impl Fn(&Seen) -> bool) -> usize {
    let now = Instant::now();
    let mut edges: Vec<(Instant, i64)> = Vec::new();
    for s in seen.iter().filter(|s| s.source == source && keep(s)) {
        edges.push((s.started, 1));
        edges.push((s.finished.get().copied().unwrap_or(now), -1));
    }
    // At the same instant, an end comes before a start.
    edges.sort();
    let (mut open, mut most) = (0i64, 0i64);
    for (_, step) in edges {
        open += step;
        most = most.max(open);
    }
    most as usize
}

pub struct Server {
    addr: SocketAddr,
    shared: Arc<Shared>,
}

#[derive(Default)]
struct Shared {
    routes: Mutex<HashMap<String, Answer>>,
    seen: Mutex<Vec<Seen>>,
}

impl Server {
    /// A server on a free port of 127.0.0.1, with nothing scripted yet.
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let shared = Arc::new(Shared::default());
        let app = Router::new().fallback(answer).with_state(shared.clone());
        tokio::spawn(async move {
            let app = app.into_make_service_with_connect_info::<SocketAddr>();
            axum::serve(listener, app).await.unwrap();
        });
        Self { addr, shared }
    }

    /// Script what `path` answers, replacing any earlier script for it. The
    /// path is matched without its query.
    pub fn at(&self, path: &str, answer: Answer) {
        self.shared
            .routes
            .lock()
            .unwrap()
            .insert(path.to_string(), answer);
    }

    /// `127.0.0.1:<port>`.
    pub fn authority(&self) -> String {
        self.addr.to_string()
    }

    /// `http://127.0.0.1:<port>`, for writing absolute redirects.
    pub fn base(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub fn url(&self, path_and_query: &str) -> reqwest::Url {
        reqwest::Url::parse(&format!("{}{path_and_query}", self.base())).unwrap()
    }

    /// Every request so far, in the order they arrived.
    pub fn seen(&self) -> Vec<Seen> {
        self.shared.seen.lock().unwrap().clone()
    }
}

/// The bytes of every served file in `range`: the same in every run, and
/// different at every offset.
pub fn content(range: Range<u64>) -> Vec<u8> {
    const SEED: u64 = 0x5EED_BA22_A6E0_0003;
    let mut out = Vec::with_capacity((range.end - range.start) as usize);
    let mut at = range.start;
    while at < range.end {
        let word = splitmix(SEED ^ (at / 8)).to_le_bytes();
        let from = (at % 8) as usize;
        let take = (8 - from).min((range.end - at) as usize);
        out.extend_from_slice(&word[from..from + take]);
        at += take as u64;
    }
    out
}

/// What a file's answer sends where the file's own bytes end: never a byte
/// of the file, so it cannot pass for one.
pub const NOT_THE_FILE: u8 = 0xEE;

/// SplitMix64: a well-mixed 64-bit value per input, so neighbouring words
/// share no pattern.
fn splitmix(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

async fn answer(
    State(shared): State<Arc<Shared>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request,
) -> Response {
    let path = request.uri().path().to_string();
    // In a block, so no borrow of the request lives on across an await: its
    // body is not Sync, which would make this handler's future not Send.
    let (range, if_range) = {
        let text = |name| {
            request
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };
        (text(header::RANGE), text(header::IF_RANGE))
    };
    let script = shared.routes.lock().unwrap().get(&path).cloned();
    let plan = match &script {
        Some(Answer::File(file)) => {
            Some(file.plan(range.as_deref(), if_range.as_deref(), peer.ip()))
        }
        _ => None,
    };
    let sent = Arc::new(AtomicU64::new(0));
    let finished = Arc::new(OnceLock::new());
    let code = match (&script, &plan) {
        (_, Some(plan)) => plan.status(),
        (Some(Answer::Silent), _) => 0,
        (Some(Answer::Status(code) | Answer::Redirect(code, _) | Answer::Headers(code, _)), _) => {
            *code
        }
        _ => 404,
    };
    shared.seen.lock().unwrap().push(Seen {
        source: peer.ip(),
        method: request.method().clone(),
        path,
        range,
        if_range,
        sent: sent.clone(),
        started: Instant::now(),
        finished: finished.clone(),
        cut: plan.as_ref().and_then(|p| p.cut),
        stall: plan.as_ref().and_then(|p| p.stall),
        status: code,
    });

    let finish_now = || {
        let _ = finished.set(Instant::now());
    };
    match (script, plan) {
        (
            Some(Answer::File(_)),
            Some(Plan {
                refused: Some(wait),
                ..
            }),
        ) => {
            finish_now();
            let mut response = StatusCode::TOO_MANY_REQUESTS.into_response();
            if let Some(wait) = wait {
                set(&mut response, header::RETRY_AFTER, wait);
            }
            response
        }
        (Some(Answer::File(file)), Some(plan)) => serve_file(&file, plan, sent, finished),
        (Some(Answer::Silent), _) => std::future::pending().await,
        (Some(Answer::Status(code)), _) => {
            finish_now();
            status(code).into_response()
        }
        (Some(Answer::Redirect(code, location)), _) => {
            finish_now();
            (status(code), [(header::LOCATION, location)]).into_response()
        }
        (Some(Answer::Headers(code, headers)), _) => {
            finish_now();
            let mut response = status(code).into_response();
            for (name, value) in headers {
                response
                    .headers_mut()
                    .insert(name, HeaderValue::from_str(&value).unwrap());
            }
            response
        }
        _ => {
            finish_now();
            StatusCode::NOT_FOUND.into_response()
        }
    }
}

fn status(code: u16) -> StatusCode {
    StatusCode::from_u16(code).unwrap()
}

/// How one request for a file is answered.
struct Plan {
    kind: Kind,
    /// The bytes sent.
    range: Range<u64>,
    /// Where the file's own bytes end; past it, [`NOT_THE_FILE`].
    real_until: u64,
    cut: Option<u64>,
    /// The file offset where the answer stops sending, if it stalls.
    stall: Option<u64>,
    /// Refused with 429, and the `Retry-After` sent with it, if any.
    refused: Option<Option<String>>,
    /// Its place under a [`Cap`], held for as long as it is answered.
    slot: Option<Slot>,
    /// The file as this request finds it.
    length: u64,
    etag: Option<String>,
}

impl Plan {
    fn status(&self) -> u16 {
        match (&self.refused, &self.kind) {
            (Some(_), _) => 429,
            (None, Kind::Whole) => 200,
            (None, Kind::Partial) => 206,
            (None, Kind::Unsatisfiable) => 416,
        }
    }
}

enum Kind {
    Whole,
    Partial,
    Unsatisfiable,
}

impl File {
    fn plan(&self, range: Option<&str>, if_range: Option<&str>, source: IpAddr) -> Plan {
        let (length, etag) = self.version(source);
        // If-Range: the range only for the version the client names, else
        // the whole file, so a client never gets a piece of another version.
        let current =
            |v: &str| etag.as_deref() == Some(v) || self.last_modified.as_deref() == Some(v);
        let asked = range
            .filter(|_| self.ranges && if_range.is_none_or(current))
            .and_then(parse_range);
        let (kind, range, real_until) = match asked {
            Some((start, _)) if start >= length => (Kind::Unsatisfiable, 0..0, 0),
            Some((start, last)) => {
                let asked_end = last.map_or(length, |last| (last + 1).min(length));
                let mut end = if self.past_the_range {
                    length
                } else {
                    asked_end
                };
                if let Some(most) = self.max_answer {
                    end = end.min(start + most);
                }
                if self.from_the_top {
                    (Kind::Partial, 0..end - start, end - start)
                } else {
                    (Kind::Partial, start..end, asked_end)
                }
            }
            None => (Kind::Whole, 0..length, length),
        };
        let piece = range.end - range.start > PROBE_BYTES && matches!(kind, Kind::Partial);
        // A piece is refused before anything else is decided about it.
        let (mut refused, mut slot) = (None, None);
        if piece {
            if let Some(r) = self
                .refusals
                .iter()
                .find(|r| r.source == source && r.take())
            {
                refused = Some(r.retry_after.clone());
            } else if let Some(cap) = self.caps.iter().find(|c| c.source == source) {
                match cap.enter() {
                    Some(held) => slot = Some(held),
                    None => refused = Some(cap.retry_after.clone()),
                }
            }
        }
        let answered = refused.is_none();
        let cut = self.cuts.iter().find_map(|c| {
            (answered && c.source == source && range.end - range.start > c.after && c.take())
                .then_some(range.start + c.after)
        });
        let stall = self.stalls.iter().find_map(|s| {
            (answered && s.source == source && piece && s.take()).then_some(range.start + s.after)
        });
        Plan {
            kind,
            range,
            real_until,
            cut,
            stall,
            refused,
            slot,
            length,
            etag,
        }
    }

    /// The file's length and ETag for this request from `source`, counting
    /// it.
    fn version(&self, source: IpAddr) -> (u64, Option<String>) {
        let n = self.requests.fetch_add(1, Ordering::SeqCst);
        let replaced = self.replaced.as_ref().filter(|r| n >= r.after);
        let length = replaced.and_then(|r| r.length).unwrap_or(self.length);
        let etag = replaced
            .and_then(|r| r.etag.clone())
            .or_else(|| {
                let own = self.etag_for.iter().find(|(s, _)| *s == source);
                own.map(|(_, tag)| tag.clone())
            })
            .or_else(|| self.etag.clone());
        (length, etag)
    }
}

fn serve_file(
    file: &File,
    plan: Plan,
    sent: Arc<AtomicU64>,
    finished: Arc<OnceLock<Instant>>,
) -> Response {
    let (length, etag) = (plan.length, plan.etag.clone());
    let mut response = match plan.kind {
        Kind::Unsatisfiable => {
            let _ = finished.set(Instant::now());
            let mut response = StatusCode::RANGE_NOT_SATISFIABLE.into_response();
            set(
                &mut response,
                header::CONTENT_RANGE,
                format!("bytes */{length}"),
            );
            response
        }
        Kind::Partial => {
            let Range { start, end } = plan.range.clone();
            let mut response = Response::new(body(file, plan, sent, finished));
            *response.status_mut() = StatusCode::PARTIAL_CONTENT;
            let content_range = format!("bytes {start}-{}/{length}", end - 1);
            set(&mut response, header::CONTENT_RANGE, content_range);
            set(
                &mut response,
                header::CONTENT_LENGTH,
                (end - start).to_string(),
            );
            response
        }
        Kind::Whole => {
            let mut response = Response::new(body(file, plan, sent, finished));
            if file.send_length {
                set(&mut response, header::CONTENT_LENGTH, length.to_string());
            }
            response
        }
    };
    if let Some(tag) = etag {
        set(&mut response, header::ETAG, tag);
    }
    if let Some(date) = &file.last_modified {
        set(&mut response, header::LAST_MODIFIED, date.clone());
    }
    if let Some(value) = &file.disposition {
        // From bytes, so a test can send a raw UTF-8 name as some servers do.
        let value = HeaderValue::from_bytes(value.as_bytes()).unwrap();
        response
            .headers_mut()
            .insert(header::CONTENT_DISPOSITION, value);
    }
    response
}

fn set(response: &mut Response, name: header::HeaderName, value: String) {
    response
        .headers_mut()
        .insert(name, HeaderValue::from_str(&value).unwrap());
}

/// `bytes=START-END` or `bytes=START-`, as (start, last byte). Anything else
/// is ignored, as a real server may, and the whole file is sent.
fn parse_range(value: &str) -> Option<(u64, Option<u64>)> {
    let (start, end) = value.strip_prefix("bytes=")?.split_once('-')?;
    let start: u64 = start.trim().parse().ok()?;
    let end = match end.trim() {
        "" => None,
        last => Some(last.parse().ok()?),
    };
    if end.is_some_and(|last| last < start) {
        return None;
    }
    Some((start, end))
}

/// Marks the answer finished when the body is dropped: at its end, or when
/// the client goes away. Its place under a [`Cap`], if it holds one, is given
/// back then too.
struct Finish(Arc<OnceLock<Instant>>, Option<Slot>);

impl Drop for Finish {
    fn drop(&mut self) {
        let _ = self.0.set(Instant::now());
    }
}

/// The planned bytes, made as they are sent and counted into `sent`, at the
/// file's rate, stalling or cutting where scripted.
fn body(file: &File, plan: Plan, sent: Arc<AtomicU64>, finished: Arc<OnceLock<Instant>>) -> Body {
    const PIECE: u64 = 64 << 10;
    let Range { start, end } = plan.range;
    let real_until = plan.real_until;
    let cut = plan.cut;
    let rate = file.rate;
    let stop = file
        .stall_after
        .map_or(end, |bytes| end.min(start.saturating_add(bytes)))
        .min(plan.stall.unwrap_or(end));
    let began = tokio::time::Instant::now();
    let mark = Finish(finished, plan.slot);
    let pieces = futures_util::stream::unfold((start, mark), move |(at, mark)| {
        let sent = sent.clone();
        async move {
            if at >= end {
                return None;
            }
            if cut == Some(at) {
                let error = std::io::Error::other("cut on purpose");
                return Some((Err(error), (end, mark)));
            }
            if at >= stop {
                std::future::pending::<()>().await;
            }
            let mut next = (at + PIECE).min(stop);
            if let Some(cut) = cut.filter(|&cut| cut > at) {
                next = next.min(cut);
            }
            if let Some(rate) = rate {
                let due = Duration::from_secs_f64((next - start) as f64 / rate as f64);
                tokio::time::sleep_until(began + due).await;
            }
            sent.fetch_add(next - at, Ordering::SeqCst);
            Some((Ok(Bytes::from(bytes(at..next, real_until))), (next, mark)))
        }
    });
    Body::from_stream(pieces)
}

/// The file's bytes in `range`, with [`NOT_THE_FILE`] from `real_until` on.
fn bytes(range: Range<u64>, real_until: u64) -> Vec<u8> {
    let real_end = range.end.min(real_until).max(range.start);
    let mut out = content(range.start..real_end);
    out.resize((range.end - range.start) as usize, NOT_THE_FILE);
    out
}
