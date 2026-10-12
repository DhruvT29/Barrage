//! Task 1 test: the link model carries both address families.
//!
//! Phase 0 found the tether handing out an IPv4 address with its own IPv4
//! default route, so IPv4 is the path that matters today. IPv6 is in the model
//! from the start anyway, because a carrier that is IPv6-only is a live risk
//! (PLAN.md section 7) and retrofitting a second address family into a type
//! that every module already consumes is far more expensive than carrying it
//! from the first commit.

use barrage::links::model::Addr;

#[test]
fn addr_holds_both_families() {
    let v4 = Addr::V4("10.0.0.1".parse().unwrap());
    let v6 = Addr::V6("fe80::1".parse().unwrap());

    assert!(v4.ip().is_ipv4());
    assert!(v6.ip().is_ipv6());
}
