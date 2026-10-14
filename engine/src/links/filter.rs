//! Which candidates deserve a probe.
//!
//! Pure. No OS calls, no network, no clock. Everything here is a decision
//! about data that was already gathered, which is what makes it exhaustively
//! testable without a real adapter in sight.
//!
//! This is a cheap pre-filter, not a verdict. It removes addresses that cannot
//! possibly work, so the expensive step that follows does not waste a probe
//! and a timeout on them. **Everything it keeps is still unproven.** Having an
//! address is not having internet, which phase 0 demonstrated on the test
//! machine: five interfaces held addresses and exactly one reached the
//! internet. Only `probe` decides whether a link is real.
//!
//! Every rule below asks the standard library to classify the address rather
//! than comparing it against a written-out range. That is both more correct
//! than a hand-written prefix and what keeps `tests/no_hardcoded_subnets.rs`
//! satisfied, which is not a coincidence: the guard exists to push code
//! towards exactly this.

use std::net::IpAddr;

use super::model::Candidate;

/// The candidates worth probing, in the order they were given.
///
/// Order is preserved deliberately. Task 10 renders a table that refreshes
/// every second, and rows that reshuffle between polls are hard to read and
/// harder to trust.
///
/// Duplicates are not merged. A filter removes; it does not rewrite its input.
/// If the OS ever reports one address twice, that is a fact worth surfacing
/// upstream rather than quietly papering over here.
pub fn usable(candidates: &[Candidate]) -> Vec<Candidate> {
    candidates
        .iter()
        .filter(|c| is_usable(c))
        .cloned()
        .collect()
}

fn is_usable(candidate: &Candidate) -> bool {
    // The OS reporting the interface as down is necessary to respect and
    // nowhere near sufficient on its own.
    if !candidate.up {
        return false;
    }

    // Link-local means DHCP failed and the OS invented an address to fill the
    // gap. It looks like a working interface and leads nowhere, which is the
    // most expensive kind of candidate to keep: it accepts work and then times
    // out instead of failing fast.
    if candidate.addr.is_link_local() {
        return false;
    }

    match candidate.addr.ip() {
        IpAddr::V4(a) => {
            !(a.is_loopback() || a.is_unspecified() || a.is_broadcast() || a.is_multicast())
        }
        IpAddr::V6(a) => !(a.is_loopback() || a.is_unspecified() || a.is_multicast()),
    }
}
