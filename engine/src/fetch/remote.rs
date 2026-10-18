//! Finding the file: what one link sees at a URL.
//!
//! One ranged `GET`, for the first [`PROBE_BYTES`] bytes, answers everything a
//! download needs to know before it starts: whether the server sends pieces
//! (206) or only the whole file (200), how long the file is, what identifies
//! this version of it, and its name. Never `HEAD`: servers and CDNs answer
//! `HEAD` differently from `GET`, or not at all, and the ranged `GET` is the
//! request the download itself will make.
//!
//! REDIRECTS ARE FOLLOWED HERE, BY HAND
//!
//! Download links commonly bounce to a CDN. Each hop gets its own
//! `bind::client`, so it too leaves through this link and looks its host up
//! in this link's address family. A client following redirects itself would
//! resolve the new host on its own, which is the trap `bind` describes.
//!
//! THE BODY IS NEVER READ
//!
//! Every question is answered by the status and the headers, so the answer is
//! dropped as soon as they arrive. A server that ignores the range and starts
//! sending the whole file costs only what was already in flight.
//!
//! NO DEADLINE HERE
//!
//! A server that never answers keeps `find` waiting. The caller, one
//! download's coordinator, bounds it with its own deadline, which tests can
//! shrink.

use std::net::IpAddr;

use anyhow::{Context, bail, ensure};
use reqwest::header::{
    CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_RANGE, ETAG, HeaderMap, HeaderName, LAST_MODIFIED,
    LOCATION, RANGE,
};
use reqwest::{Response, StatusCode, Url};

use super::name::file_name;
use crate::links::bind;

/// How many bytes the probe asks for.
pub const PROBE_BYTES: u64 = 1024;

/// How many redirects one link follows before giving up.
pub const MAX_REDIRECTS: usize = 10;

/// The file at a URL, as one link sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remote {
    /// Where the file is, after this link's redirects. Each link follows its
    /// own, so two links can end at two CDN nodes.
    pub url: Url,
    /// The whole file's length, when the server says.
    pub length: Option<u64>,
    /// Whether the server answered the range asked for, so that the file can
    /// be fetched in pieces.
    pub ranges: bool,
    pub validator: Option<Validator>,
    /// A safe name for the file, from the server or the URL. Whether it is
    /// free in the download folder is for `free_name` to say.
    pub name: String,
}

/// What identifies this version of the file. Sent back as `If-Range`, it
/// makes a server whose file has changed answer with the whole new file
/// rather than a piece of it, so no file is ever stitched from two versions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Validator {
    /// A strong entity tag, quotes included, as the server sent it.
    ETag(String),
    /// The `Last-Modified` date, as the server sent it.
    LastModified(String),
}

/// The file at `url` as the link that owns `local` sees it, after that
/// link's redirects.
pub async fn find(local: IpAddr, url: &Url) -> anyhow::Result<Remote> {
    follow(local, url)
        .await
        .with_context(|| format!("finding {url} through {local}"))
}

async fn follow(local: IpAddr, start: &Url) -> anyhow::Result<Remote> {
    let mut url = start.clone();
    for hop in 0..=MAX_REDIRECTS {
        if !matches!(url.scheme(), "http" | "https") {
            if hop == 0 {
                bail!("{url} is not an http or https address");
            }
            bail!("redirected to {url}, which is not an http or https address");
        }
        let response = probe(local, &url).await?;
        if is_redirect(response.status()) {
            url = next_hop(&url, &response)?;
            continue;
        }
        return describe(url, &response);
    }
    bail!("more than {MAX_REDIRECTS} redirects")
}

/// The probe: a `GET` for the first [`PROBE_BYTES`] bytes, through its own
/// client for this hop's host.
async fn probe(local: IpAddr, url: &Url) -> anyhow::Result<Response> {
    let client = bind::client(local, url.as_str()).await?;
    client
        .get(url.clone())
        .header(RANGE, format!("bytes=0-{}", PROBE_BYTES - 1))
        .send()
        .await
        .with_context(|| format!("asking {url}"))
}

fn is_redirect(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::MOVED_PERMANENTLY
            | StatusCode::FOUND
            | StatusCode::SEE_OTHER
            | StatusCode::TEMPORARY_REDIRECT
            | StatusCode::PERMANENT_REDIRECT
    )
}

/// Where a redirect points: its `Location`, which may be relative to `from`.
fn next_hop(from: &Url, response: &Response) -> anyhow::Result<Url> {
    let status = response.status();
    let location = response
        .headers()
        .get(LOCATION)
        .with_context(|| format!("{from} answered {status} without a Location"))?;
    let text = std::str::from_utf8(location.as_bytes())
        .with_context(|| format!("{from} answered {status} with an unreadable Location"))?;
    from.join(text)
        .with_context(|| format!("{from} redirected to {text:?}, which is not an address"))
}

/// The file as the final answer describes it.
fn describe(url: Url, response: &Response) -> anyhow::Result<Remote> {
    let headers = response.headers();
    let status = response.status();
    let (length, ranges) = match status {
        StatusCode::PARTIAL_CONTENT => (probed_length(headers)?, true),
        StatusCode::OK => (number(headers, CONTENT_LENGTH), false),
        // An empty file has no byte 0 to send. The standard answer says so:
        // 416, with `Content-Range: bytes */0`.
        StatusCode::RANGE_NOT_SATISFIABLE if unsatisfied_length(headers) == Some(0) => {
            (Some(0), true)
        }
        _ => bail!("{url} answered {status}"),
    };
    // Bytes, not `to_str`: many servers send the name as raw UTF-8, which
    // `to_str` refuses.
    let disposition = headers
        .get(CONTENT_DISPOSITION)
        .map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned());
    Ok(Remote {
        name: file_name(disposition.as_deref(), &url),
        validator: validator(headers),
        url,
        length,
        ranges,
    })
}

/// The whole file's length from a 206 answer to the probe, `None` if the
/// server does not know it. The answer must hold exactly the bytes asked for,
/// or less when the file is shorter than the probe: a server that answers a
/// range with other bytes would put every later piece in the wrong place.
fn probed_length(headers: &HeaderMap) -> anyhow::Result<Option<u64>> {
    let text =
        header_text(headers, CONTENT_RANGE).context("a 206 answer without a Content-Range")?;
    let (start, end, total) = content_range(text)
        .with_context(|| format!("a 206 answer with an unreadable Content-Range {text:?}"))?;
    let last = PROBE_BYTES - 1;
    let asked = match total {
        Some(total) => total > 0 && end == last.min(total - 1),
        None => end <= last,
    };
    ensure!(
        start == 0 && asked,
        "asked for bytes 0-{last}, the Content-Range says {text:?}"
    );
    Ok(total)
}

/// `bytes START-END/TOTAL` as (start, end, total), the total `None` when it
/// is `*`, unknown.
fn content_range(text: &str) -> Option<(u64, u64, Option<u64>)> {
    let (unit, rest) = text.trim().split_once(' ')?;
    if !unit.eq_ignore_ascii_case("bytes") {
        return None;
    }
    let (range, total) = rest.split_once('/')?;
    let (start, end) = range.split_once('-')?;
    let total = match total.trim() {
        "*" => None,
        total => Some(total.parse().ok()?),
    };
    Some((start.trim().parse().ok()?, end.trim().parse().ok()?, total))
}

/// The length a 416 answer gives, `bytes */LENGTH`.
fn unsatisfied_length(headers: &HeaderMap) -> Option<u64> {
    let (unit, length) = header_text(headers, CONTENT_RANGE)?
        .trim()
        .split_once(" */")?;
    if !unit.eq_ignore_ascii_case("bytes") {
        return None;
    }
    length.trim().parse().ok()
}

/// A strong `ETag` if the server sent one, else `Last-Modified`. A weak tag,
/// `W/"x"`, cannot go in `If-Range`, and a tag without its quotes is not a
/// tag at all; with either, the date is what is left.
fn validator(headers: &HeaderMap) -> Option<Validator> {
    let strong = header_text(headers, ETAG)
        .map(str::trim)
        .filter(|tag| tag.len() >= 2 && tag.starts_with('"') && tag.ends_with('"'));
    if let Some(tag) = strong {
        return Some(Validator::ETag(tag.to_string()));
    }
    header_text(headers, LAST_MODIFIED)
        .map(str::trim)
        .filter(|date| !date.is_empty())
        .map(|date| Validator::LastModified(date.to_string()))
}

fn number(headers: &HeaderMap, name: HeaderName) -> Option<u64> {
    header_text(headers, name)?.trim().parse().ok()
}

fn header_text(headers: &HeaderMap, name: HeaderName) -> Option<&str> {
    headers.get(name)?.to_str().ok()
}
