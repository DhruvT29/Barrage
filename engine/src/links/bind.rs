//! An HTTP client bound to one link.
//!
//! Every request the engine makes through a chosen link gets its client here,
//! so that "send it through this link" means the same thing everywhere: the
//! probe, throughput sampling, and the phase 2 downloader.
//!
//! THE TRAP UNDERNEATH BINDING
//!
//! Telling the HTTP client to use a link's address as the source sends the
//! request out of that link, with one exception that fails silently. The
//! connection layer under `reqwest` (hyper-util's `bind_local_address`)
//! applies an IPv4 source only to IPv4 destinations. Given an IPv6 destination
//! it binds to "any address" instead, and the OS sends the request out of
//! whichever link it prefers. The request succeeds, through the wrong link,
//! with no error.
//!
//! So `client` resolves the host itself, keeps only addresses of the same
//! family as the link, and pins the client to those. A host with no address of
//! that family is refused outright. Nothing on this machine had global IPv6 on
//! 2026-10-13, so the trap was not live, but mobile carriers hand out IPv6
//! freely and a tether could start passing it through at any time.
//!
//! REDIRECTS ARE NOT FOLLOWED
//!
//! Both callers so far treat a redirect as evidence: of a captive portal for
//! the probe, of no measurement for sampling. A followed redirect would also
//! slip past the family check, because `reqwest` would resolve the new host
//! itself. Phase 2 will need redirects, since download links commonly bounce
//! to a CDN, and must follow them by hand, calling `client` again for each new
//! host.

use std::net::{IpAddr, SocketAddr};

use anyhow::{Context, ensure};

/// A client whose every connection leaves through the link that owns `local`,
/// ready to request `url`.
///
/// Built per call, so a pooled connection opened through one link can never
/// be reused for another. The client is tied to `url`'s host: asking it for a
/// different host would bypass the family check.
pub(crate) async fn client(local: IpAddr, url: &str) -> anyhow::Result<reqwest::Client> {
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
        // identical request without one. A server refusing the client would
        // make a working link look broken.
        .user_agent(concat!("barrage/", env!("CARGO_PKG_VERSION")))
        // A proxy would carry every request over the route to the proxy,
        // whichever link was asked for.
        .no_proxy()
        // See the module docs.
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
