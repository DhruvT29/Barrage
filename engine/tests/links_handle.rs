//! `LinkHandle`, the surface phase 2 consumes. All but the last test drive it
//! with the scripted network in `fake/mod.rs`; the last uses this machine.

mod fake;

use std::sync::Arc;
use std::time::Duration;

use barrage::LinkHandle;
use barrage::links::probe::ProbeVerdict;
use barrage::links::watch::WatchConfig;
use barrage::links::{Candidate, LinkEvent, LinkState};
use fake::{Fake, cand, ms, next};
use tokio::sync::broadcast::error::{RecvError, TryRecvError};
use tokio::time::timeout;
use tracing_subscriber::filter::Targets;
use tracing_subscriber::prelude::*;

#[tokio::test]
async fn snapshot_reflects_an_emitted_up_event() {
    let net = Fake::scripted(vec![vec![], vec![cand("Wi-Fi", "192.0.2.10", true)]]);
    let h = LinkHandle::start_with(Arc::new(net), WatchConfig::every(ms(50))).unwrap();
    let mut rx = h.subscribe();

    let LinkEvent::Up(id) = next(&mut rx).await else {
        panic!("expected Up");
    };
    let snapshot = h.snapshot();
    assert_eq!(snapshot.len(), 1);
    assert_eq!((snapshot[0].id, snapshot[0].state), (id, LinkState::Up));
}

/// Only `Up` is usable. A portal, a dead link and a link whose probe has not
/// answered yet all stay in the table but are left out.
#[tokio::test]
async fn usable_excludes_portal_down_and_unprobed_links() {
    let net = Fake::scripted(vec![vec![
        cand("Wi-Fi", "192.0.2.10", true),
        cand("Hotel", "192.0.2.30", true),
        cand("Dead", "192.0.2.40", true),
        cand("Slow", "192.0.2.50", true),
    ]])
    .verdicts("192.0.2.30", &[ProbeVerdict::Portal])
    .verdicts("192.0.2.40", &[ProbeVerdict::Broken])
    .hanging("192.0.2.50");
    let h = LinkHandle::start_with(Arc::new(net), WatchConfig::every(ms(50))).unwrap();
    let mut rx = h.subscribe();

    // Up, Portal and Down, in whatever order; "Slow" stays Probing.
    for _ in 0..3 {
        next(&mut rx).await;
    }
    let names: Vec<_> = h.usable().into_iter().map(|l| l.name).collect();
    assert_eq!(names, ["Wi-Fi"]);
    assert_eq!(h.snapshot().len(), 4);
}

/// Phase 2 will have more than one listener: the scheduler, and later the UI.
#[tokio::test]
async fn every_subscriber_hears_every_event() {
    let net = Fake::scripted(vec![vec![], vec![cand("Wi-Fi", "192.0.2.10", true)]]);
    let h = LinkHandle::start_with(Arc::new(net), WatchConfig::every(ms(50))).unwrap();
    let mut a = h.subscribe();
    let mut b = h.subscribe();

    let heard = next(&mut a).await;
    assert!(matches!(heard, LinkEvent::Up(_)));
    assert_eq!(next(&mut b).await, heard);
}

/// Phase 2 will hear `Up(id)` and at once look the link up. If the event went
/// out before the table changed, it would find the link still `Probing`.
///
/// The listener is a plain thread checking constantly, as a UI thread might
/// be, so it runs truly alongside the watcher instead of waiting its turn on
/// the same thread. One link appears per poll, so each `Up` arrives alone.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_event_goes_out_only_after_the_table_shows_it() {
    const LINKS: usize = 60;
    let all: Vec<Candidate> = (1..=LINKS)
        .map(|i| cand("Ethernet 2", &format!("2001:db8::{i:x}"), true))
        .collect();
    let steps = (1..=LINKS).map(|n| all[..n].to_vec()).collect();
    let h = Arc::new(
        LinkHandle::start_with(Arc::new(Fake::scripted(steps)), WatchConfig::every(ms(5))).unwrap(),
    );
    let mut rx = h.subscribe();
    let reader = Arc::clone(&h);

    let (heard, stale) = tokio::task::spawn_blocking(move || {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let (mut heard, mut stale) = (0, Vec::new());
        while heard < LINKS && std::time::Instant::now() < deadline {
            match rx.try_recv() {
                Ok(LinkEvent::Up(id)) => {
                    heard += 1;
                    let state = reader
                        .snapshot()
                        .iter()
                        .find(|l| l.id == id)
                        .map(|l| l.state);
                    if state != Some(LinkState::Up) {
                        stale.push((id, state));
                    }
                }
                Err(TryRecvError::Empty) => std::hint::spin_loop(),
                Err(TryRecvError::Closed) => break,
                Ok(_) | Err(TryRecvError::Lagged(_)) => {}
            }
        }
        (heard, stale)
    })
    .await
    .unwrap();

    assert_eq!(heard, LINKS, "missed some Up events");
    assert!(
        stale.is_empty(),
        "{} of {LINKS} Up events arrived before the table showed them: {stale:?}",
        stale.len()
    );
}

/// Seen 2026-10-14: with the phone on the laptop's Wi-Fi, the tether's public
/// address was the Wi-Fi's own, two rows over one internet. The phone's IPv6
/// address reports itself and shares with nothing.
#[tokio::test]
async fn links_on_different_cables_behind_one_public_address_share_an_upstream() {
    let net = Fake::scripted(vec![vec![
        cand("Wi-Fi", "192.0.2.10", true),
        cand("Ethernet 2", "192.0.2.20", true),
        cand("Ethernet 2", "2001:db8::20", true),
    ]])
    .public("192.0.2.10", &[Some("198.51.100.99")])
    .public("192.0.2.20", &[Some("198.51.100.99")]);
    let h = LinkHandle::start_with(Arc::new(net), WatchConfig::every(ms(50))).unwrap();

    tokio::time::sleep(ms(300)).await;
    let groups = h.shared_upstreams();
    assert_eq!(groups.len(), 1, "{groups:?}");
    let table = h.snapshot();
    let mut names: Vec<_> = groups[0]
        .iter()
        .map(|id| table.iter().find(|l| l.id == *id).unwrap().name.clone())
        .collect();
    names.sort();
    assert_eq!(names, ["Ethernet 2", "Wi-Fi"]);
}

/// Otherwise the watcher would go on probing the internet every few seconds,
/// for nobody.
#[tokio::test]
async fn dropping_the_handle_stops_the_watcher() {
    let net = Fake::scripted(vec![vec![cand("Wi-Fi", "192.0.2.10", true)]]);
    let h = LinkHandle::start_with(Arc::new(net), WatchConfig::every(ms(50))).unwrap();
    let mut rx = h.subscribe();
    assert!(matches!(next(&mut rx).await, LinkEvent::Up(_)));

    drop(h);
    // Closed means the watcher, which owns the sending side, is gone.
    let end = timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("the watcher was still running 2 s after the handle was dropped");
    assert!(matches!(end, Err(RecvError::Closed)), "{end:?}");
}

#[test]
fn starting_outside_a_runtime_is_an_error_not_a_panic() {
    let started = LinkHandle::start_with(Arc::new(Fake::default()), WatchConfig::every(ms(50)));
    let err = started.expect_err("started without a runtime");
    assert!(format!("{err:#}").contains("runtime"), "{err:#}");
}

/// This machine's interfaces and the real internet; needs one working
/// connection. The test ends as soon as a link is up, which also cancels the
/// speed test that coming up starts, so it spends next to no data.
#[tokio::test]
async fn on_this_machine_a_working_link_comes_up() {
    // The watcher logs why a probe failed. Shown only if this test fails.
    let _ = tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_test_writer())
        .with(Targets::new().with_target("barrage", tracing::Level::DEBUG))
        .try_init();

    let h = LinkHandle::start(Duration::from_secs(1)).expect("could not start");
    let mut rx = h.subscribe();

    let mut heard = Vec::new();
    let up = timeout(Duration::from_secs(10), async {
        loop {
            match rx.recv().await {
                Ok(LinkEvent::Up(id)) => return id,
                Ok(other) => heard.push(other),
                Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => panic!("the watcher stopped"),
            }
        }
    })
    .await;
    let Ok(up) = up else {
        panic!(
            "no link came up within 10 s. Heard {heard:?}; table {:#?}",
            h.snapshot()
        );
    };

    let usable = h.usable();
    assert!(usable.iter().any(|l| l.id == up), "{usable:?}");
}
