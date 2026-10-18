//! A scripted file server on 127.0.0.1, for the downloader's tests.
//!
//! It serves a seeded pseudo-random file, so bytes written at the wrong
//! offset cannot match by chance, and it records every request: where it came
//! from, the method, the path and the range asked. The source address is how
//! a test tells its links apart: a socket bound to 127.0.0.2 reaches this
//! server and shows up here as 127.0.0.2.
//!
//! Every path answers as scripted with [`Server::at`], and anything not
//! scripted is 404. Phase 3 grows the scripts task by task, each adding the
//! misbehaviour its tests need.
#![allow(dead_code)]

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use tokio::net::TcpListener;

/// What one path answers.
#[derive(Clone, Debug)]
pub enum Answer {
    File(File),
    /// This status with this `Location`, written exactly as given.
    Redirect(u16, String),
    /// This status with these headers and no body, for answers no real file
    /// would get.
    Headers(u16, Vec<(&'static str, String)>),
    /// This status alone.
    Status(u16),
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
        }
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
}

/// One request, as the server saw it.
#[derive(Clone, Debug)]
pub struct Seen {
    pub source: IpAddr,
    pub method: Method,
    pub path: String,
    pub range: Option<String>,
    /// Body bytes handed to the connection so far.
    pub sent: Arc<AtomicU64>,
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
    let range = request
        .headers()
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let sent = Arc::new(AtomicU64::new(0));
    shared.seen.lock().unwrap().push(Seen {
        source: peer.ip(),
        method: request.method().clone(),
        path: path.clone(),
        range: range.clone(),
        sent: sent.clone(),
    });

    let script = shared.routes.lock().unwrap().get(&path).cloned();
    match script {
        None => StatusCode::NOT_FOUND.into_response(),
        Some(Answer::Status(code)) => status(code).into_response(),
        Some(Answer::Redirect(code, location)) => {
            (status(code), [(header::LOCATION, location)]).into_response()
        }
        Some(Answer::Headers(code, headers)) => {
            let mut response = status(code).into_response();
            for (name, value) in headers {
                response
                    .headers_mut()
                    .insert(name, HeaderValue::from_str(&value).unwrap());
            }
            response
        }
        Some(Answer::File(file)) => serve_file(&file, range.as_deref(), sent),
    }
}

fn status(code: u16) -> StatusCode {
    StatusCode::from_u16(code).unwrap()
}

fn serve_file(file: &File, range: Option<&str>, sent: Arc<AtomicU64>) -> Response {
    let asked = range.filter(|_| file.ranges).and_then(parse_range);
    let mut response = match asked {
        Some((start, _)) if start >= file.length => {
            let mut response = StatusCode::RANGE_NOT_SATISFIABLE.into_response();
            set(
                &mut response,
                header::CONTENT_RANGE,
                format!("bytes */{}", file.length),
            );
            response
        }
        Some((start, end)) => {
            let end = end.map_or(file.length, |last| (last + 1).min(file.length));
            let mut response = Response::new(body(file, start..end, sent));
            *response.status_mut() = StatusCode::PARTIAL_CONTENT;
            let content_range = format!("bytes {start}-{}/{}", end - 1, file.length);
            set(&mut response, header::CONTENT_RANGE, content_range);
            set(
                &mut response,
                header::CONTENT_LENGTH,
                (end - start).to_string(),
            );
            response
        }
        None => {
            let mut response = Response::new(body(file, 0..file.length, sent));
            if file.send_length {
                set(
                    &mut response,
                    header::CONTENT_LENGTH,
                    file.length.to_string(),
                );
            }
            response
        }
    };
    if let Some(tag) = &file.etag {
        set(&mut response, header::ETAG, tag.clone());
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

/// The file's bytes in `range`, made as they are sent, counted into `sent`.
fn body(file: &File, range: Range<u64>, sent: Arc<AtomicU64>) -> Body {
    const PIECE: u64 = 64 << 10;
    let end = range.end;
    let stop = file
        .stall_after
        .map_or(end, |bytes| end.min(range.start.saturating_add(bytes)));
    let pieces = futures_util::stream::unfold(range.start, move |at| {
        let sent = sent.clone();
        async move {
            if at >= end {
                return None;
            }
            if at >= stop {
                std::future::pending::<()>().await;
            }
            let next = (at + PIECE).min(stop);
            sent.fetch_add(next - at, Ordering::Relaxed);
            Some((
                Ok::<_, std::io::Error>(Bytes::from(content(at..next))),
                next,
            ))
        }
    });
    Body::from_stream(pieces)
}
