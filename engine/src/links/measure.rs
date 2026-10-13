//! How fast is a link, right now?
//!
//! Downloads through one link for a fixed budget, counts the bytes, and
//! reports bits per second.
//!
//! WHAT THE NUMBER MEANS, AND WHAT IT DOES NOT
//!
//! One connection, for about two seconds. That is a floor under the link's
//! capacity, not the capacity: phase 0 needed sixteen connections and about
//! ten seconds to reach full speed, because a single TCP connection starts
//! slow and ramps up. It is good enough to show a speed and to tell a fast
//! link from a slow one. It is not a number to divide work by.
//!
//! **One sample is noisy; never show or act on just one.** Measured on
//! 2026-10-13 over a steady Wi-Fi link, eight 2-second samples in a row ranged
//! from 22.7 to 47.1 Mbps with ordinary connection times, and two earlier ones
//! read 1.7 and 8.4 for reasons not reproduced since. Average several. The
//! meter itself is sound: given the same window as curl, it read 48.6, 51.4
//! and 47.5 Mbps against curl's 46.7, 36.7 and 45.0, counted the same way.
//!
//! **The budget includes connecting.** Reaching the first byte took 440 to
//! 540 ms over Wi-Fi and once 1108 ms, so a 2-second budget leaves about 1.5 s
//! of transfer at best. A link slow to answer, such as a tether whose probe
//! RTT reached 934 ms, needs a longer budget or it may not get to measure.
//!
//! HOW IT IS COUNTED
//!
//! - The clock starts when the response headers arrive, not when the request
//!   is sent. Connecting through the tether took up to 934 ms on 2026-10-13,
//!   and counting that inside a two-second window would roughly halve the
//!   rate reported for the link that is slowest to answer. The budget still
//!   covers everything, so a sample never runs longer than asked.
//! - The rate divides by the time the transfer actually took, not by the
//!   budget. A fast link can finish the capped range early, and dividing by
//!   the budget would report the fastest links as the slowest.
//! - Each sample asks for at most [`SAMPLE_CAP_BYTES`], so a very fast link
//!   cannot spend unlimited data, which matters on a metered tether. When the
//!   budget ends, the response is dropped and the connection closed, so the
//!   rest of the range is never fetched.

use std::net::IpAddr;
use std::time::Duration;

use anyhow::{Context, bail, ensure};
use tokio::time::{Instant, timeout_at};

use super::bind;

/// A large object served with range support: Hetzner's Singapore speed
/// server, close to this machine and the one phase 0 measured against. It
/// drops clients that send no User-Agent, which `bind::client` handles.
pub const SAMPLE_URL: &str = "https://sin-speed.hetzner.com/10GB.bin";

/// The most one sample asks for: 64 MiB, about two seconds at 270 Mbps.
pub const SAMPLE_CAP_BYTES: u64 = 64 * 1024 * 1024;

/// The download rate through the link that owns `addr`, in bits per second.
pub async fn sample(addr: IpAddr, budget: Duration) -> anyhow::Result<u64> {
    sample_url(addr, SAMPLE_URL, budget).await
}

/// [`sample`] against any URL. Public so tests can aim it at a server that
/// misbehaves on purpose.
pub async fn sample_url(addr: IpAddr, url: &str, budget: Duration) -> anyhow::Result<u64> {
    let asked = Instant::now();
    let deadline = asked + budget;
    let what = || format!("sampling {url} from {addr}");

    let request = async {
        let client = bind::client(addr, url).await?;
        let range = format!("bytes=0-{}", SAMPLE_CAP_BYTES - 1);
        let response = client
            .get(url)
            .header(reqwest::header::RANGE, range)
            .send()
            .await?;
        anyhow::Ok(response)
    };
    let mut response = match timeout_at(deadline, request).await {
        Ok(result) => result.with_context(what)?,
        Err(_) => bail!("{}: no answer within {budget:?}", what()),
    };

    // 206 is the range honoured. 200 is a server that ignored the range and
    // is sending the whole object, which the deadline cuts short anyway.
    // Anything else, a captive portal's redirect above all, is not data from
    // this link, and counting its bytes would invent a speed.
    let status = response.status().as_u16();
    ensure!(
        matches!(status, 200 | 206),
        "{}: answered {status}, which is not data",
        what()
    );

    let started = Instant::now();
    let mut bytes: u64 = 0;
    loop {
        match timeout_at(deadline, response.chunk()).await {
            Ok(Ok(Some(chunk))) => bytes += chunk.len() as u64,
            // The whole capped range arrived inside the budget.
            Ok(Ok(None)) => break,
            Ok(Err(e)) => return Err(anyhow::Error::new(e).context(what())),
            // The budget is spent. This is the normal way a sample ends.
            Err(_) => break,
        }
    }

    let secs = started.elapsed().as_secs_f64();
    tracing::debug!(
        %addr,
        to_headers_ms = (started - asked).as_millis() as u64,
        transfer_ms = (secs * 1000.0) as u64,
        bytes,
        "throughput sample"
    );
    ensure!(
        bytes > 0 && secs > 0.0,
        "{}: no data arrived within {budget:?}",
        what()
    );
    Ok((bytes as f64 * 8.0 / secs).round() as u64)
}
