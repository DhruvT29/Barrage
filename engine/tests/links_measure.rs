//! Throughput sampling.
//!
//! The first two tests go through a real link to the real internet, so they
//! need a working connection and spend a couple of seconds of data each. The
//! rest talk to a tiny HTTP server on this machine, scripted to misbehave in
//! one specific way, so their outcome does not depend on the network.

use std::net::IpAddr;
use std::time::{Duration, Instant};

use barrage::links::measure::{SAMPLE_CAP_BYTES, SAMPLE_URL, sample, sample_url};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

mod common;
use common::first_usable_local_addr;

#[tokio::test]
async fn sample_reports_a_positive_rate_on_a_working_link() {
    let addr = first_usable_local_addr();
    let got = sample(addr, Duration::from_secs(2)).await.unwrap();
    assert!(got.bps > 0);
    assert!(got.bytes > 0, "what it pulled, counted as data used");
}

#[tokio::test]
async fn sample_stops_inside_its_budget() {
    let addr = first_usable_local_addr();
    let budget = Duration::from_secs(2);
    let started = Instant::now();
    let _ = sample(addr, budget).await;
    assert!(started.elapsed() < budget + Duration::from_secs(1));
}

/// If the cap and the URL drift apart, the URL can ask Cloudflare for more
/// than the cap allows. The cap itself is held under Cloudflare's 100 MB
/// limit by a compile-time check in `measure.rs`.
#[test]
fn the_sample_url_asks_for_exactly_the_cap() {
    assert!(
        SAMPLE_URL.ends_with(&format!("bytes={SAMPLE_CAP_BYTES}")),
        "{SAMPLE_URL} does not ask for {SAMPLE_CAP_BYTES} bytes"
    );
}

// Scripted local servers from here on.

/// How a scripted server behaves after reading the request.
enum Script {
    /// Send these bytes, then close.
    Reply(Vec<u8>),
    /// Send these bytes, then hold the connection open and say nothing more.
    ReplyThenStall(Vec<u8>),
    /// Accept, read the request, and never answer.
    Silent,
}

/// A one-connection HTTP server on `addr` following `script`. Returns the URL.
async fn serve(addr: IpAddr, script: Script) -> String {
    let listener = TcpListener::bind((addr, 0)).await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        // Read the whole request first. Closing a socket with unread input
        // makes Windows send a reset, which can destroy the reply in flight.
        let mut request = Vec::new();
        let mut buf = [0u8; 1024];
        while !request.windows(4).any(|w| w == b"\r\n\r\n") {
            match socket.read(&mut buf).await {
                Ok(0) | Err(_) => return,
                Ok(n) => request.extend_from_slice(&buf[..n]),
            }
        }
        match script {
            Script::Reply(bytes) => {
                let _ = socket.write_all(&bytes).await;
                let _ = socket.shutdown().await;
            }
            Script::ReplyThenStall(bytes) => {
                let _ = socket.write_all(&bytes).await;
                tokio::time::sleep(Duration::from_secs(30)).await;
            }
            Script::Silent => tokio::time::sleep(Duration::from_secs(30)).await,
        }
    });
    url
}

fn partial_content(body_len: usize, declared_len: usize) -> Vec<u8> {
    let mut reply =
        format!("HTTP/1.1 206 Partial Content\r\nContent-Length: {declared_len}\r\n\r\n")
            .into_bytes();
    reply.resize(reply.len() + body_len, b'x');
    reply
}

/// A fast link can finish the capped range before the budget runs out. The
/// rate must come from the time it actually took, or the fastest links would
/// be reported as the slowest.
#[tokio::test]
async fn a_short_fast_answer_is_rated_by_the_time_it_took() {
    let addr = first_usable_local_addr();
    let body = 2 * 1024 * 1024;
    let url = serve(addr, Script::Reply(partial_content(body, body))).await;

    let budget = Duration::from_secs(2);
    let started = Instant::now();
    let bps = sample_url(addr, &url, budget).await.unwrap().bps;

    let if_divided_by_budget = (body as u64 * 8) / budget.as_secs();
    assert!(
        bps > if_divided_by_budget * 5,
        "{bps} bps looks divided by the budget"
    );
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "waited out the budget"
    );
}

/// A link that delivers some data and then stalls is a slow link, not a
/// failed one. The budget ends the sample and the bytes so far are the rate.
#[tokio::test]
async fn a_transfer_that_stalls_is_ended_by_the_budget_and_still_rated() {
    let addr = first_usable_local_addr();
    let sent = 256 * 1024;
    let url = serve(
        addr,
        Script::ReplyThenStall(partial_content(sent, 64 * 1024 * 1024)),
    )
    .await;

    let budget = Duration::from_millis(800);
    let started = Instant::now();
    let bps = sample_url(addr, &url, budget).await.unwrap().bps;
    let took = started.elapsed();

    assert!(bps > 0);
    assert!(took >= budget, "ended before the budget: {took:?}");
    assert!(
        took < budget + Duration::from_millis(500),
        "overran: {took:?}"
    );
}

#[tokio::test]
async fn a_server_that_never_answers_is_an_error_within_the_budget() {
    let addr = first_usable_local_addr();
    let url = serve(addr, Script::Silent).await;

    let budget = Duration::from_millis(800);
    let started = Instant::now();
    let result = sample_url(addr, &url, budget).await;

    assert!(result.is_err(), "no data cannot be a rate");
    assert!(started.elapsed() < budget + Duration::from_millis(500));
}

/// A captive portal answers with a redirect to its login page. Counting the
/// bytes of that answer would report a speed for a link that has none.
#[tokio::test]
async fn a_redirect_is_not_a_measurement() {
    let addr = first_usable_local_addr();
    let reply =
        b"HTTP/1.1 302 Found\r\nLocation: http://login.invalid/\r\nContent-Length: 0\r\n\r\n";
    let url = serve(addr, Script::Reply(reply.to_vec())).await;

    let err = sample_url(addr, &url, Duration::from_secs(2))
        .await
        .expect_err("a redirect is not data");
    assert!(
        format!("{err:#}").contains("302"),
        "unhelpful error: {err:#}"
    );
}
