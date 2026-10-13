//! Is a link really on the internet, or is something in the way?
//!
//! Two halves, kept apart for the same reason as everywhere in this module:
//! `classify` decides what a response means and is pure, and the request
//! itself (task 5) is the I/O edge.
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
