//! Is a link really on the internet, or is something in the way?
//!
//! Two halves, kept apart for the same reason as everywhere in this module:
//! `classify` decides what a response means and is pure, and `probe` performs
//! the request through one chosen link and is the I/O edge.
//!
//! A probe is only worth anything if it really leaves through the link it was
//! aimed at, which is `bind`'s job, including the silent IPv6 trap described
//! there.
//!
//! WHY THE PROBE HAS TO READ THE ANSWER, NOT JUST GET ONE
//!
//! A captive portal, the login page on a hotel, airport or cafe network,
//! answers every request quickly and successfully. By latency and status code
//! alone it looks like a healthy link, while every byte it returns is its own
//! login page. So the probe asks a question whose exact answer is known in
//! advance, and anything other than that exact answer means something between
//! us and the internet is answering in the real server's place.
//!
//! Known limit: a portal that recognises the probe hosts and answers them
//! correctly passes. Only measured throughput can expose a link that answers
//! the probe and then moves no data.

use std::net::IpAddr;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};

use super::bind;

/// What one probe response says about the link it came through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProbeVerdict {
    /// The exact expected answer came back. The link reaches the internet.
    Working,
    /// Something answered, but not with the expected answer: a login page, a
    /// redirect to one, or the status code reserved for portals. Reachable
    /// and unusable, and not a fault that retrying will fix.
    Portal,
    /// No usable answer: an error status, or one no portal would send.
    Broken,
}

/// Windows' own connectivity check for IPv4 links. Answers 200 with exactly
/// [`MS_PROBE_EXPECTED`] as the whole body.
///
/// **IPv4 only**: the host had no IPv6 address at all on 2026-10-13, so the
/// family check in `bind` refuses it for an IPv6 link. [`probe_target`]
/// picks [`MS_PROBE_URL_V6`] for those.
///
/// Plain HTTP on purpose, as are the other probe URLs. A portal can reveal
/// itself only on a request it can intercept and answer. Over HTTPS it cannot
/// present a valid certificate for this host, so the request would fail as a
/// TLS error and the portal would be reported as a broken link instead.
pub const MS_PROBE_URL: &str = "http://www.msftconnecttest.com/connecttest.txt";

/// Windows' connectivity check for IPv6 links: the same answer as
/// [`MS_PROBE_URL`], from a host with IPv6 addresses. Checked byte for byte
/// through the phone tether's two IPv6 addresses on 2026-10-13.
pub const MS_PROBE_URL_V6: &str = "http://ipv6.msftconnecttest.com/connecttest.txt";

/// The whole body both Microsoft probe URLs return: 22 bytes with no
/// trailing newline, checked byte for byte on 2026-10-13.
pub const MS_PROBE_EXPECTED: &str = "Microsoft Connect Test";

/// Android's connectivity check. Answers 204 with an empty body, also checked
/// on 2026-10-13.
pub const G_PROBE_URL: &str = "http://connectivitycheck.gstatic.com/generate_204";

/// RFC 6585 section 6, Network Authentication Required: the status code
/// defined specifically for captive portals.
const NETWORK_AUTHENTICATION_REQUIRED: u16 = 511;

/// What a probe response means.
///
/// `Working` only for the exact known answers: a 204 with an empty body, or a
/// 200 whose body is exactly [`MS_PROBE_EXPECTED`]. Exact rather than
/// "contains", because a login page quoting that text must not pass for the
/// real thing.
///
/// Any other 2xx or 3xx, and 511, is a `Portal`: something answered in the
/// real server's place. Everything else is `Broken`.
///
/// The rule does not ask which probe URL produced the response. A portal
/// exists to show its login page, so neither known answer is one it has
/// reason to send, and accepting either from either keeps the caller free to
/// probe whichever host is reachable.
pub fn classify(status: u16, body: &str) -> ProbeVerdict {
    match status {
        204 if body.is_empty() => ProbeVerdict::Working,
        200 if body == MS_PROBE_EXPECTED => ProbeVerdict::Working,
        200..=399 | NETWORK_AUTHENTICATION_REQUIRED => ProbeVerdict::Portal,
        _ => ProbeVerdict::Broken,
    }
}

/// One probe's outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Probe {
    pub verdict: ProbeVerdict,
    /// From sending the request to receiving the response headers. That
    /// includes the TCP handshake, so it is roughly two round trips rather
    /// than one. It excludes the DNS lookup, which need not have used this
    /// link at all.
    pub rtt: Duration,
}

/// Where [`public_ip_via`] asks, per address family. Each answers with the
/// caller's public address as plain text, and each host has addresses of
/// only its own family, so neither can answer through the wrong one.
const PUBLIC_IP_URL: &str = "https://api.ipify.org";
const PUBLIC_IP_URL_V6: &str = "https://api6.ipify.org";

/// The probe URL for a link with this address: a host that has addresses of
/// the same family, or the family check in `bind` would refuse it.
pub fn probe_target(addr: IpAddr) -> &'static str {
    if addr.is_ipv4() {
        MS_PROBE_URL
    } else {
        MS_PROBE_URL_V6
    }
}

/// Probe the internet through the link that owns `addr`.
pub async fn probe(addr: IpAddr, timeout: Duration) -> anyhow::Result<Probe> {
    probe_url(addr, probe_target(addr), timeout).await
}

/// [`probe`] against any URL. Public so tests can aim it at a server that
/// misbehaves on purpose.
///
/// `timeout` bounds everything: the lookup, the connection and the whole
/// answer. A link that accepts the connection and then goes quiet costs at
/// most this long, never a stalled poll loop (Review Focus 3).
pub async fn probe_url(addr: IpAddr, url: &str, timeout: Duration) -> anyhow::Result<Probe> {
    within(timeout, addr, url, async {
        let client = bind::client(addr, url).await?;
        let started = Instant::now();
        let response = client.get(url).send().await?;
        let rtt = started.elapsed();
        let status = response.status().as_u16();
        let body = response.bytes().await?;
        Ok(Probe {
            verdict: classify(status, &String::from_utf8_lossy(&body)),
            rtt,
        })
    })
    .await
}

/// The public address the internet sees for traffic leaving through the link
/// that owns `addr`.
///
/// This is what proves binding works. Two links that report the same public
/// address are not two paths, whatever the local addresses say. On IPv6 there
/// is normally no address sharing, so a link's public address is its own
/// address: both of the tether's IPv6 addresses reported themselves on
/// 2026-10-13.
pub async fn public_ip_via(addr: IpAddr, timeout: Duration) -> anyhow::Result<IpAddr> {
    let url = if addr.is_ipv4() {
        PUBLIC_IP_URL
    } else {
        PUBLIC_IP_URL_V6
    };
    within(timeout, addr, url, async {
        let client = bind::client(addr, url).await?;
        let text = client
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        let shown: String = text.chars().take(80).collect();
        text.trim()
            .parse()
            .with_context(|| format!("the answer {shown:?} is not an address"))
    })
    .await
}

/// Run `work` under one overall deadline, naming the link and URL in any
/// error so a failure in a table of links says which link failed.
async fn within<T>(
    timeout: Duration,
    addr: IpAddr,
    url: &str,
    work: impl Future<Output = anyhow::Result<T>>,
) -> anyhow::Result<T> {
    match tokio::time::timeout(timeout, work).await {
        Ok(result) => result.with_context(|| format!("request to {url} from {addr}")),
        // "Did not complete" rather than "no answer": the deadline also ends
        // a response that was arriving, just not fast enough.
        Err(_) => bail!("request to {url} from {addr} did not complete within {timeout:?}"),
    }
}
