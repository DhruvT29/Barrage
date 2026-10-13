//! The probe: classification (pure) and the source-bound request (real OS,
//! real internet).
//!
//! `classify` is pure, so every case is a plain function call and the
//! captive-portal acceptance criterion is pinned here rather than needing a
//! cafe with a login page.
//!
//! The tests further down send real requests through a real link, so they
//! need at least one working IPv4 connection. They pick whichever usable link
//! enumerates first and assume nothing about which one that is.

use std::net::IpAddr;
use std::time::{Duration, Instant};

use barrage::links::probe::{
    G_PROBE_URL, MS_PROBE_EXPECTED, MS_PROBE_URL, ProbeVerdict, classify, probe, probe_url,
    public_ip_via,
};
use barrage::links::{enumerate, usable};
use tokio::net::TcpListener;

#[test]
fn empty_204_is_working() {
    assert_eq!(classify(204, ""), ProbeVerdict::Working);
}

#[test]
fn exact_expected_body_is_working() {
    assert_eq!(classify(200, MS_PROBE_EXPECTED), ProbeVerdict::Working);
}

#[test]
fn html_login_page_is_a_portal() {
    let body = "<html><body>Please sign in to continue</body></html>";
    assert_eq!(classify(200, body), ProbeVerdict::Portal);
}

#[test]
fn redirect_to_a_login_host_is_a_portal() {
    assert_eq!(classify(302, ""), ProbeVerdict::Portal);
}

#[test]
fn server_error_is_broken() {
    assert_eq!(classify(500, ""), ProbeVerdict::Broken);
}

#[test]
fn a_204_with_a_body_is_a_portal() {
    assert_eq!(classify(204, "<html>"), ProbeVerdict::Portal);
}

/// RFC 6585 section 6 defines 511 for exactly this situation. Reading it as
/// broken would hide the one fact the user needs, which is that logging in
/// would fix the link.
#[test]
fn network_authentication_required_is_a_portal() {
    assert_eq!(classify(511, ""), ProbeVerdict::Portal);
}

/// Exact means exact. A login page that happens to quote the expected text
/// must not pass for the real answer.
#[test]
fn the_expected_text_inside_a_page_is_a_portal() {
    let body = format!("<html><body>{MS_PROBE_EXPECTED}</body></html>");
    assert_eq!(classify(200, &body), ProbeVerdict::Portal);
}

#[test]
fn every_status_range_lands_where_the_rule_says() {
    use ProbeVerdict::{Broken, Portal};
    let cases = [
        (100, Broken),
        (199, Broken),
        (200, Portal), // a 200 without the expected body
        (206, Portal),
        (299, Portal),
        (300, Portal),
        (304, Portal),
        (399, Portal),
        (400, Broken),
        (404, Broken),
        (510, Broken),
        (512, Broken),
        (599, Broken),
    ];
    for (status, want) in cases {
        assert_eq!(classify(status, ""), want, "status {status}");
    }
}

/// A portal reveals itself only on a request it can intercept and answer.
/// Over HTTPS it cannot present a valid certificate for the probe host, so
/// the request would fail as a TLS error, `classify` would never see it, and
/// a portal would be reported as a broken link.
#[test]
fn probe_urls_are_plain_http() {
    for url in [MS_PROBE_URL, G_PROBE_URL] {
        assert!(url.starts_with("http://"), "{url} must be plain HTTP");
    }
}

// The source-bound probe. Everything below goes through a real link.

fn first_usable_local_addr() -> IpAddr {
    usable(&enumerate().expect("enumeration failed"))
        .into_iter()
        .map(|c| c.addr.ip())
        .find(IpAddr::is_ipv4)
        .expect("these tests need at least one working IPv4 network link")
}

#[tokio::test]
async fn probe_from_a_real_local_address_reports_working() {
    let addr = first_usable_local_addr();
    let p = probe(addr, Duration::from_secs(5)).await.unwrap();
    assert_eq!(p.verdict, ProbeVerdict::Working);
    assert!(p.rtt > Duration::ZERO);
}

#[tokio::test]
async fn probe_bound_to_an_unroutable_address_fails() {
    let addr: IpAddr = "169.254.1.1".parse().unwrap();
    assert!(probe(addr, Duration::from_secs(3)).await.is_err());
}

/// Review Focus 3. A server that accepts the connection and then says nothing
/// is the hardest stall to bound: the connection succeeds, so only an overall
/// deadline stops the wait. Served from this machine, so the stall is the same
/// on every run instead of depending on how a router treats a dead address.
#[tokio::test]
async fn probe_respects_its_timeout() {
    let addr = first_usable_local_addr();
    let silent = TcpListener::bind((addr, 0)).await.unwrap();
    let url = format!("http://{}/", silent.local_addr().unwrap());

    let budget = Duration::from_millis(400);
    let started = Instant::now();
    let err = probe_url(addr, &url, budget)
        .await
        .expect_err("a server that never answers cannot be working");
    let took = started.elapsed();

    // Ended by the deadline, not by some faster unrelated failure, which
    // would pass the timing check below while proving nothing about it.
    assert!(
        format!("{err:#}").contains("did not complete within"),
        "{err:#}"
    );
    assert!(took >= budget, "gave up before the deadline: {took:?}");
    assert!(took < budget * 3, "overran the deadline: {took:?}");
}

/// The guarantee that makes binding mean anything. A destination of the other
/// address family cannot leave through this link, and the connection layer
/// underneath would otherwise send it out of whichever link the OS prefers,
/// with no error. It must fail instead, and fail fast.
#[tokio::test]
async fn probe_refuses_a_destination_of_the_other_family() {
    let addr = first_usable_local_addr();
    let started = Instant::now();
    let err = probe_url(addr, "http://[2001:db8::1]/", Duration::from_secs(5))
        .await
        .expect_err("an IPv6 destination from an IPv4 link must be refused");
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "refused, but slowly"
    );
    assert!(
        format!("{err:#}").contains("IPv6"),
        "unhelpful error: {err:#}"
    );
}

#[tokio::test]
async fn public_ip_via_a_link_is_a_public_address() {
    let addr = first_usable_local_addr();
    let ip = public_ip_via(addr, Duration::from_secs(10)).await.unwrap();
    let IpAddr::V4(v4) = ip else {
        panic!("asked through an IPv4 link, answered with {ip}");
    };
    assert!(
        !(v4.is_private() || v4.is_loopback() || v4.is_link_local() || v4.is_unspecified()),
        "{v4} is not a public address"
    );
}
