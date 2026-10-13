//! Is a link really on the internet, or is something in the way?
//!
//! Two halves, kept apart for the same reason as everywhere in this module:
//! `classify` decides what a response means and is pure, and `probe` performs
//! the request through one chosen link and is the I/O edge.
//!
//! BINDING, AND THE TRAP UNDERNEATH IT
//!
//! A probe is only worth anything if it really leaves through the link it was
//! aimed at. Telling the HTTP client to use a link's address as the source
//! does that, with one exception that fails silently. The connection layer
//! under `reqwest` (hyper-util's `bind_local_address`) applies an IPv4 source
//! only to IPv4 destinations. Given an IPv6 destination it binds to "any
//! address" instead, and the OS sends the request out of whichever link it
//! prefers. The request succeeds, through the wrong link, with no error.
//!
//! So the probe resolves the host itself, keeps only addresses of the same
//! family as the link, and pins the client to those. A host with no address
//! of that family is refused outright. Nothing on this machine had global
//! IPv6 on 2026-10-13, so the trap was not live, but mobile carriers hand out
//! IPv6 freely and a tether could start passing it through at any time.
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

use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

use anyhow::{Context, bail, ensure};

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

/// Windows' own connectivity check. Answers 200 with exactly
/// [`MS_PROBE_EXPECTED`] as the whole body.
///
/// Plain HTTP on purpose, as is [`G_PROBE_URL`]. A portal can reveal itself
/// only on a request it can intercept and answer. Over HTTPS it cannot present
/// a valid certificate for this host, so the request would fail as a TLS error
/// and the portal would be reported as a broken link instead.
pub const MS_PROBE_URL: &str = "http://www.msftconnecttest.com/connecttest.txt";

/// The whole body [`MS_PROBE_URL`] returns: 22 bytes with no trailing
/// newline, checked byte for byte on 2026-10-13.
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

/// Where [`public_ip_via`] asks. Answers with the caller's public address as
/// plain text. The host has IPv4 addresses only, which suits IPv4 links; an
/// IPv6 link is refused loudly by the family check rather than misreported.
const PUBLIC_IP_URL: &str = "https://api.ipify.org";

/// Probe the internet through the link that owns `addr`.
pub async fn probe(addr: IpAddr, timeout: Duration) -> anyhow::Result<Probe> {
    probe_url(addr, MS_PROBE_URL, timeout).await
}

/// [`probe`] against any URL. Public so tests can aim it at a server that
/// misbehaves on purpose.
///
/// `timeout` bounds everything: the lookup, the connection and the whole
/// answer. A link that accepts the connection and then goes quiet costs at
/// most this long, never a stalled poll loop (Review Focus 3).
pub async fn probe_url(addr: IpAddr, url: &str, timeout: Duration) -> anyhow::Result<Probe> {
    within(timeout, addr, url, async {
        let client = bound_client(addr, url).await?;
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
/// address are not two paths, whatever the local addresses say.
pub async fn public_ip_via(addr: IpAddr, timeout: Duration) -> anyhow::Result<IpAddr> {
    within(timeout, addr, PUBLIC_IP_URL, async {
        let client = bound_client(addr, PUBLIC_IP_URL).await?;
        let text = client
            .get(PUBLIC_IP_URL)
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

/// A client whose every connection leaves through the link that owns `local`.
///
/// Built per call, so a pooled connection opened through one link can never
/// be reused for another.
async fn bound_client(local: IpAddr, url: &str) -> anyhow::Result<reqwest::Client> {
    let parsed = reqwest::Url::parse(url).with_context(|| format!("unusable URL {url}"))?;
    let host = parsed.host_str().context("the URL has no host")?;
    let port = parsed
        .port_or_known_default()
        .context("the URL has no port")?;

    let literal = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .parse::<IpAddr>()
        .ok();
    let found: Vec<SocketAddr> = match literal {
        Some(ip) => vec![SocketAddr::new(ip, port)],
        None => tokio::net::lookup_host((host, port))
            .await
            .with_context(|| format!("looking up {host}"))?
            .collect(),
    };
    ensure!(!found.is_empty(), "{host} resolved to no addresses");

    let targets = same_family(local, found);
    ensure!(
        !targets.is_empty(),
        "{host} has only {} addresses, which an {} link cannot reach. Refused rather than let the \
         OS send it out of whichever link it prefers",
        family_name(!local.is_ipv4()),
        family_name(local.is_ipv4()),
    );

    let mut builder = reqwest::Client::builder()
        .local_address(local)
        // reqwest sends no User-Agent unless told to, and some servers drop
        // anonymous clients. Measured 2026-10-13: Hetzner's speed server
        // served curl with its user-agent and closed the connection on the
        // same request without one, and Cloudflare refused it outright. A
        // server refusing the client would make a working link look broken.
        .user_agent(concat!("barrage/", env!("CARGO_PKG_VERSION")))
        // A proxy would carry every request over the route to the proxy,
        // whichever link was asked for.
        .no_proxy()
        // A redirect is evidence of a portal, so it is returned, not followed.
        .redirect(reqwest::redirect::Policy::none());
    if literal.is_none() {
        // Pin the hostname to the addresses checked above, so the client
        // cannot resolve it again and pick one of the other family.
        builder = builder.resolve_to_addrs(host, &targets);
    }
    builder.build().context("building the HTTP client")
}

/// The destinations a socket bound to `local` really reaches through its own
/// link: those of the same address family. See the module docs for what
/// happens to the others.
fn same_family(local: IpAddr, found: impl IntoIterator<Item = SocketAddr>) -> Vec<SocketAddr> {
    found
        .into_iter()
        .filter(|a| a.is_ipv4() == local.is_ipv4())
        .collect()
}

fn family_name(ipv4: bool) -> &'static str {
    if ipv4 { "IPv4" } else { "IPv6" }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addrs(list: &[&str]) -> Vec<SocketAddr> {
        list.iter().map(|s| s.parse().unwrap()).collect()
    }

    const MIXED: &[&str] = &["192.0.2.10:80", "[2001:db8::10]:80", "198.51.100.10:80"];

    #[test]
    fn an_ipv4_link_keeps_only_ipv4_destinations() {
        let local: IpAddr = "192.0.2.1".parse().unwrap();
        let kept = same_family(local, addrs(MIXED));
        assert_eq!(kept, addrs(&["192.0.2.10:80", "198.51.100.10:80"]));
    }

    #[test]
    fn an_ipv6_link_keeps_only_ipv6_destinations() {
        let local: IpAddr = "2001:db8::1".parse().unwrap();
        let kept = same_family(local, addrs(MIXED));
        assert_eq!(kept, addrs(&["[2001:db8::10]:80"]));
    }

    #[test]
    fn a_host_with_no_address_of_the_links_family_leaves_nothing() {
        let local: IpAddr = "192.0.2.1".parse().unwrap();
        assert!(same_family(local, addrs(&["[2001:db8::10]:80"])).is_empty());
    }
}
