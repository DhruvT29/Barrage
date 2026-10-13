//! `enumerate` against the real OS.
//!
//! These assert invariants, not contents. The interface list on this machine
//! changes from run to run (a phone plugged in or not, Wi-Fi on or off), so a
//! test that named a particular adapter or address would pass or fail
//! depending on what happened to be connected. Only facts that hold on every
//! Windows machine in every network state are checked here.
//!
//! Run with `-- --nocapture` to print the list, for comparing by hand against
//! `Get-NetIPAddress`.

use barrage::links::enumerate;

#[test]
fn enumerate_returns_well_formed_candidates() {
    let got = enumerate().expect("enumeration failed");
    assert!(!got.is_empty(), "no interfaces at all is implausible");
    for c in &got {
        assert!(!c.name.is_empty(), "nameless candidate: {c:?}");
        assert!(c.os_index > 0, "candidate without a real index: {c:?}");
    }

    for c in &got {
        println!("{}\t{}\t{}\t{}", c.os_index, c.name, c.addr.ip(), c.up);
    }
}

/// Every Windows machine has a loopback interface in every network state,
/// including airplane mode, and the OS reports it as up. Seeing it proves the
/// call reaches real data and that the up flag can come back true, which the
/// test above cannot tell apart from a list of entries that are all down.
#[test]
fn the_loopback_interface_is_reported_and_up() {
    let got = enumerate().expect("enumeration failed");
    assert!(
        got.iter().any(|c| c.addr.ip().is_loopback() && c.up),
        "no loopback candidate reported up: {got:#?}"
    );
}
