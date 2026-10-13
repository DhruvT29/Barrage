//! Probe response classification.
//!
//! `classify` is pure, so every case is a plain function call and the
//! captive-portal acceptance criterion is pinned here rather than needing a
//! cafe with a login page.

use barrage::links::probe::{G_PROBE_URL, MS_PROBE_EXPECTED, MS_PROBE_URL, ProbeVerdict, classify};

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
