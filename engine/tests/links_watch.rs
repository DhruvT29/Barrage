//! The watcher, driven by a scripted network: no hardware, no internet.
//! The scripted network itself lives in `fake/mod.rs`.

mod fake;

use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use barrage::links::probe::ProbeVerdict;
use barrage::links::watch::{WatchConfig, spawn_watcher};
use barrage::links::{Candidate, Link, LinkEvent, LinkState};
use fake::{Fake, SAMPLED_BPS, Step, cand, cand_idx, drain_for, ms, next};
use tokio::time::{Instant, timeout};

#[tokio::test]
async fn appearance_emits_up_and_disappearance_emits_down() {
    let net = Fake::scripted(vec![
        vec![],
        vec![cand("Wi-Fi", "192.0.2.10", true)],
        vec![],
    ]);
    let (_h, mut rx, _t) = spawn_watcher(Arc::new(net), WatchConfig::every(ms(50)));

    let LinkEvent::Up(id) = next(&mut rx).await else {
        panic!("expected Up first");
    };
    assert_eq!(next(&mut rx).await, LinkEvent::Down(id));
}

#[tokio::test]
async fn an_unchanged_link_set_emits_nothing() {
    let one = vec![cand("Wi-Fi", "192.0.2.10", true)];
    let net = Fake::scripted(vec![one.clone(), one.clone(), one]);
    let (_h, mut rx, _t) = spawn_watcher(Arc::new(net), WatchConfig::every(ms(50)));

    assert!(matches!(next(&mut rx).await, LinkEvent::Up(_)));
    assert!(timeout(ms(300), rx.recv()).await.is_err());
}

// Review Focus 1
#[tokio::test]
async fn a_flapping_link_emits_at_most_one_event_per_poll() {
    let on = vec![cand("Tether", "192.0.2.20", true)];
    let net = Fake::scripted(vec![on.clone(), vec![], on.clone(), vec![], on]);
    let (_h, mut rx, _t) = spawn_watcher(Arc::new(net), WatchConfig::every(ms(50)));

    let seen = drain_for(&mut rx, ms(400)).await;
    assert!(seen.len() <= 5, "event storm: {seen:?}");
}

// Acceptance: a link appears within 2 s
#[tokio::test]
async fn a_new_link_is_reported_within_two_seconds() {
    let net = Fake::scripted(vec![vec![], vec![cand("Tether", "192.0.2.20", true)]]);
    let (_h, mut rx, _t) = spawn_watcher(Arc::new(net), WatchConfig::every(Duration::from_secs(1)));

    assert!(timeout(Duration::from_secs(2), rx.recv()).await.is_ok());
}

// Acceptance: a captive portal is shown as "portal", not "up"
#[tokio::test]
async fn a_portal_is_reported_as_portal_never_up() {
    let net = Fake::scripted(vec![vec![cand("Hotel", "192.0.2.30", true)]])
        .verdicts("192.0.2.30", &[ProbeVerdict::Portal]);
    let (_h, mut rx, table) = spawn_watcher(Arc::new(net), WatchConfig::every(ms(50)));

    assert!(matches!(next(&mut rx).await, LinkEvent::Portal(_)));
    assert_eq!(table.snapshot()[0].state, LinkState::Portal);
    assert!(drain_for(&mut rx, ms(200)).await.is_empty());
}

/// Without probing again, a portal would stay a portal forever, even after
/// the user logs in.
#[tokio::test]
async fn a_portal_that_is_logged_into_becomes_up_on_a_later_probe() {
    let net = Fake::scripted(vec![vec![cand("Hotel", "192.0.2.30", true)]])
        .verdicts("192.0.2.30", &[ProbeVerdict::Portal, ProbeVerdict::Working]);
    let mut config = WatchConfig::every(ms(50));
    config.reprobe = ms(100);
    let (_h, mut rx, _t) = spawn_watcher(Arc::new(net), config);

    let LinkEvent::Portal(id) = next(&mut rx).await else {
        panic!("expected Portal first");
    };
    assert_eq!(next(&mut rx).await, LinkEvent::Up(id));
}

#[tokio::test]
async fn a_link_that_fails_its_probe_is_down_and_never_up() {
    let net = Fake::scripted(vec![vec![cand("Dead", "192.0.2.40", true)]])
        .verdicts("192.0.2.40", &[ProbeVerdict::Broken]);
    let mut config = WatchConfig::every(ms(50));
    config.retry = ms(100);
    let (_h, mut rx, table) = spawn_watcher(Arc::new(net), config);

    assert!(matches!(next(&mut rx).await, LinkEvent::Down(_)));
    assert_eq!(table.snapshot()[0].state, LinkState::Down);
}

/// Review Focus 3, at the level of the loop. A probe that never answers must
/// not delay any other link, and the watcher must give up on it by itself
/// rather than trust the probe to honour its own timeout.
#[tokio::test]
async fn a_hanging_probe_holds_up_nothing_and_is_given_up_on() {
    let a = cand("Stuck", "192.0.2.50", true);
    let b = cand("Fine", "192.0.2.51", true);
    let net = Fake::scripted(vec![vec![a.clone()], vec![a, b]]).hanging("192.0.2.50");
    let mut config = WatchConfig::every(ms(50));
    config.probe_timeout = ms(300);
    config.retry = ms(100);
    let (_h, mut rx, table) = spawn_watcher(Arc::new(net), config);

    assert!(
        matches!(next(&mut rx).await, LinkEvent::Up(_)),
        "B was held up"
    );
    assert!(
        matches!(next(&mut rx).await, LinkEvent::Down(_)),
        "A never timed out"
    );
    let states: Vec<_> = table
        .snapshot()
        .iter()
        .map(|l| (l.name.clone(), l.state))
        .collect();
    assert!(
        states.contains(&("Stuck".to_string(), LinkState::Down)),
        "{states:?}"
    );
    assert!(
        states.contains(&("Fine".to_string(), LinkState::Up)),
        "{states:?}"
    );
}

/// Found by task 8's real-machine test, 2026-10-13: about one probe in 15 over
/// this Wi-Fi ran past 3 s, the first after a pause, while the system looked
/// the probe host up again. One slow probe must not make a working link look
/// dead, here for 10 s until the next probe.
#[tokio::test]
async fn a_new_link_whose_first_probe_fails_is_tried_again_before_it_is_called_down() {
    let net = Fake::scripted(vec![vec![cand("Wi-Fi", "192.0.2.10", true)]])
        .failing_first("192.0.2.10", 1);
    let (_h, mut rx, table) = spawn_watcher(Arc::new(net), WatchConfig::every(ms(50)));

    let first = next(&mut rx).await;
    assert!(matches!(first, LinkEvent::Up(_)), "{first:?}");
    assert_eq!(table.snapshot()[0].state, LinkState::Up);
}

/// The same for a link already in use: phase 3 would drop it, and the
/// download through it, over one slow probe. Two failures with a success in
/// between are not "in a row" either.
#[tokio::test]
async fn an_up_link_stays_up_through_failed_probes_that_are_not_in_a_row() {
    use ProbeVerdict::{Broken, Working};
    let net = Fake::scripted(vec![vec![cand("Wi-Fi", "192.0.2.10", true)]])
        .verdicts("192.0.2.10", &[Working, Broken, Working, Broken, Working]);
    let mut config = WatchConfig::every(ms(50));
    config.reprobe = ms(100);
    config.retry = ms(200);
    let (_h, mut rx, _t) = spawn_watcher(Arc::new(net), config);

    assert!(matches!(next(&mut rx).await, LinkEvent::Up(_)));
    let after = drain_for(&mut rx, ms(1500)).await;
    assert!(after.is_empty(), "flapped: {after:?}");
}

/// A link that really dies must still be reported, just not on one failure.
#[tokio::test]
async fn an_up_link_that_fails_two_probes_in_a_row_is_down() {
    let net = Fake::scripted(vec![vec![cand("Wi-Fi", "192.0.2.10", true)]])
        .verdicts("192.0.2.10", &[ProbeVerdict::Working, ProbeVerdict::Broken]);
    let mut config = WatchConfig::every(ms(50));
    config.reprobe = ms(100);
    let (_h, mut rx, table) = spawn_watcher(Arc::new(net), config);

    let LinkEvent::Up(id) = next(&mut rx).await else {
        panic!("expected Up first");
    };
    assert_eq!(next(&mut rx).await, LinkEvent::Down(id));
    assert_eq!(table.snapshot()[0].state, LinkState::Down);
}

/// A dead link is probed quickly only to confirm it is dead, then at the
/// normal pace, so one left plugged in does not cost a probe every second.
#[tokio::test]
async fn a_dead_link_is_probed_at_the_normal_pace_once_confirmed() {
    let net = Arc::new(
        Fake::scripted(vec![vec![cand("Dead", "192.0.2.40", true)]])
            .verdicts("192.0.2.40", &[ProbeVerdict::Broken]),
    );
    let mut config = WatchConfig::every(ms(50));
    config.retry = ms(50);
    config.reprobe = ms(500);
    let (_h, mut rx, _t) = spawn_watcher(net.clone(), config);

    assert!(matches!(next(&mut rx).await, LinkEvent::Down(_)));
    tokio::time::sleep(ms(1200)).await;
    // Two to confirm, then one per 500 ms. Retrying every 50 ms would be ~26.
    let probes = net.probes.load(Ordering::SeqCst);
    assert!(probes <= 5, "{probes} probes in about 1.3 s");
}

/// The user's choice, 2026-10-13: one speed test per link, when it first comes
/// up. Probing again must not test again, since each test costs real data.
#[tokio::test]
async fn a_link_is_speed_tested_once_after_it_comes_up() {
    let net = Arc::new(Fake::scripted(vec![vec![cand(
        "Wi-Fi",
        "192.0.2.10",
        true,
    )]]));
    let mut config = WatchConfig::every(ms(50));
    config.reprobe = ms(50);
    let (_h, mut rx, table) = spawn_watcher(net.clone(), config);

    assert!(matches!(next(&mut rx).await, LinkEvent::Up(_)));
    tokio::time::sleep(ms(500)).await;
    assert_eq!(net.samples.load(Ordering::SeqCst), 1);
    assert_eq!(table.snapshot()[0].down_bps, Some(SAMPLED_BPS));
}

/// The user's choice, 2026-10-13. The phone held one IPv4 and two IPv6
/// addresses over one cable, and back-to-back tests read 160.5, 16.4 and
/// 10.3 Mbps because the carrier slows the line after a burst. Speed belongs
/// to the interface: one test, its result on every address.
#[tokio::test]
async fn one_speed_test_per_interface_shared_by_its_addresses() {
    let net = Arc::new(
        Fake::scripted(vec![vec![
            cand("Ethernet 2", "192.0.2.20", true),
            cand("Ethernet 2", "2001:db8::20", true),
            cand("Ethernet 2", "2001:db8::21", true),
        ]])
        .slow_samples(ms(100)),
    );
    let (_h, _rx, table) = spawn_watcher(net.clone(), WatchConfig::every(ms(50)));

    tokio::time::sleep(ms(600)).await;
    assert_eq!(
        net.samples.load(Ordering::SeqCst),
        1,
        "tested more than once"
    );
    let speeds: Vec<_> = table.snapshot().iter().map(|l| l.down_bps).collect();
    assert_eq!(speeds, [Some(SAMPLED_BPS); 3]);
}

/// A rotating IPv6 privacy address: a new one joins, later the old one
/// leaves, all within the same IPv6 network. Same connection, so the speed
/// carries over and no data is spent.
#[tokio::test]
async fn a_rotating_ipv6_address_inherits_the_speed() {
    let v4 = cand("Ethernet 2", "192.0.2.20", true);
    let old = cand("Ethernet 2", "2001:db8::21", true);
    let new = cand("Ethernet 2", "2001:db8::22", true);
    let net = Arc::new(Fake::scripted(vec![
        vec![v4.clone(), old.clone()],
        vec![v4.clone(), old.clone()],
        vec![v4.clone(), old.clone()],
        vec![v4.clone(), old, new.clone()],
        vec![v4.clone(), new.clone()],
        vec![v4, new],
    ]));
    let (_h, _rx, table) = spawn_watcher(net.clone(), WatchConfig::every(ms(50)));

    tokio::time::sleep(ms(500)).await;
    assert_eq!(net.samples.load(Ordering::SeqCst), 1);
    let snapshot = table.snapshot();
    assert_eq!(snapshot.len(), 2);
    assert!(snapshot.iter().all(|l| l.down_bps == Some(SAMPLED_BPS)));
}

/// Acceptance, "new network invalidates cached measurements", at interface
/// level. Every old address leaving means a different network, even when the
/// swap happens within a single poll, so the old speed must not carry over.
#[tokio::test]
async fn a_new_network_on_the_same_interface_is_tested_afresh() {
    let net = Arc::new(Fake::scripted(vec![
        vec![cand("Wi-Fi", "192.0.2.10", true)],
        vec![cand("Wi-Fi", "192.0.2.10", true)],
        vec![cand("Wi-Fi", "192.0.2.10", true)],
        vec![cand("Wi-Fi", "198.51.100.10", true)],
    ]));
    let (_h, _rx, table) = spawn_watcher(net.clone(), WatchConfig::every(ms(50)));

    tokio::time::sleep(ms(500)).await;
    assert_eq!(
        net.samples.load(Ordering::SeqCst),
        2,
        "the new network was not tested"
    );
    assert_eq!(table.snapshot().len(), 1);
}

/// Taking turns is per interface. Separate links keep testing in parallel,
/// or plugging in a phone would wait behind the Wi-Fi's test.
#[tokio::test]
async fn separate_interfaces_test_at_the_same_time() {
    let net = Arc::new(
        Fake::scripted(vec![vec![
            cand("Wi-Fi", "192.0.2.10", true),
            cand("Ethernet 2", "192.0.2.20", true),
        ]])
        .slow_samples(ms(200)),
    );
    let (_h, _rx, _t) = spawn_watcher(net.clone(), WatchConfig::every(ms(50)));

    tokio::time::sleep(ms(500)).await;
    assert_eq!(net.samples.load(Ordering::SeqCst), 2);
    assert_eq!(
        net.most_at_once.load(Ordering::SeqCst),
        2,
        "tests were serialised"
    );
}

/// A failed read of the interface list says nothing about the links. Treating
/// it as "no links" would report every link down and back up again.
#[tokio::test]
async fn a_failed_enumeration_keeps_the_last_known_links() {
    let wifi = cand("Wi-Fi", "192.0.2.10", true);
    let net = Fake::steps(vec![
        Step::Links(vec![wifi.clone()]),
        Step::Fail,
        Step::Fail,
        Step::Links(vec![wifi]),
    ]);
    let (_h, mut rx, table) = spawn_watcher(Arc::new(net), WatchConfig::every(ms(50)));

    assert!(matches!(next(&mut rx).await, LinkEvent::Up(_)));
    assert!(drain_for(&mut rx, ms(300)).await.is_empty());
    assert_eq!(table.snapshot().len(), 1);
}

/// Task 10 redraws its table every second. Rows that reshuffle between
/// redraws are hard to read, so the table keeps the order links appeared in.
#[tokio::test]
async fn the_table_keeps_links_in_the_order_they_appeared() {
    let first = cand("Tether", "192.0.2.20", true);
    let second = cand("Wi-Fi", "192.0.2.10", true);
    let net = Fake::scripted(vec![vec![first.clone()], vec![second, first]]);
    let (_h, mut rx, table) = spawn_watcher(Arc::new(net), WatchConfig::every(ms(50)));

    let _ = next(&mut rx).await;
    let _ = next(&mut rx).await;
    let names: Vec<_> = table.snapshot().into_iter().map(|l| l.name).collect();
    assert_eq!(names, ["Tether", "Wi-Fi"]);
}

/// Poll `check` every 10 ms until it finds something, failing after 3 s.
///
/// The tests below wait on conditions, not fixed sleeps. On Windows a 50 ms
/// tick can fire late under load, and on 2026-10-14 a full-suite run caught a
/// fixed 400 ms sleep short of the network switch it was waiting for.
async fn wait_for<T>(what: &str, mut check: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(found) = check() {
            return found;
        }
        assert!(Instant::now() < deadline, "waited 3 s for {what}");
        tokio::time::sleep(ms(10)).await;
    }
}

fn samples(net: &Fake) -> usize {
    net.samples.load(Ordering::SeqCst)
}

/// Watch a link get measured, swap it for `replacement` (after `gap` empty
/// polls), and check the replacement starts with no speed and is tested
/// afresh. Shared by the two invalidation tests below.
async fn replacement_starts_unmeasured(
    original: Candidate,
    gap: usize,
    replacement: Candidate,
) -> Link {
    let mut steps = vec![vec![original]; 10];
    steps.extend(std::iter::repeat_n(vec![], gap));
    steps.push(vec![replacement]);
    let net = Arc::new(Fake::scripted(steps).slow_samples(ms(200)));
    let (_h, mut rx, table) = spawn_watcher(net.clone(), WatchConfig::every(ms(50)));

    let LinkEvent::Up(old) = next(&mut rx).await else {
        panic!("expected the original link Up");
    };
    wait_for("the original link to be measured", || {
        (table.snapshot()[0].down_bps == Some(SAMPLED_BPS)).then_some(())
    })
    .await;

    assert_eq!(next(&mut rx).await, LinkEvent::Down(old));
    let LinkEvent::Up(new) = next(&mut rx).await else {
        panic!("expected the replacement Up");
    };
    let links = table.snapshot();
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].id, new);
    assert_ne!(new, old, "the replacement kept the old link's id");
    assert_eq!(links[0].down_bps, None, "the old speed carried over");

    let measured = wait_for("the replacement to be tested", || {
        let link = table.snapshot().remove(0);
        link.down_bps.is_some().then_some(link)
    })
    .await;
    assert_eq!(samples(&net), 2);
    measured
}

// Acceptance: a new network invalidates cached measurements. Moving Wi-Fi to
// another network gives it a new address, so it is a new link: the old speed
// must not show on it even for a moment, and it is tested afresh.
#[tokio::test]
async fn a_changed_address_on_the_same_interface_clears_measurements() {
    let after = replacement_starts_unmeasured(
        cand("Wi-Fi", "192.0.2.10", true),
        0,
        cand("Wi-Fi", "198.51.100.10", true),
    )
    .await;
    assert_eq!(after.addr.ip().to_string(), "198.51.100.10");
    assert_eq!(after.down_bps, Some(SAMPLED_BPS));
}

// Review Focus 5. Windows hands a freed interface index to the next adapter
// plugged in. Identity is (name, address) and never the index, so the
// newcomer is a new link with nothing carried over.
#[tokio::test]
async fn a_reused_os_index_does_not_inherit_the_old_links_measurements() {
    let after = replacement_starts_unmeasured(
        cand_idx(20, "Tether", "192.0.2.20", true),
        1,
        cand_idx(20, "Dock Ethernet", "198.51.100.20", true),
    )
    .await;
    assert_eq!(after.name, "Dock Ethernet");
}

fn ip(s: &str) -> IpAddr {
    s.parse().unwrap()
}

/// These tests switch networks well inside the default settle time.
fn switching_config() -> WatchConfig {
    let mut config = WatchConfig::every(ms(50));
    config.settle = ms(0);
    config
}

/// A phone being plugged in: its IPv6 address follows its IPv4 one by a
/// moment. That is one network arriving, not a switch, so one speed test.
#[tokio::test]
async fn ipv6_arriving_just_after_a_cable_appears_is_not_a_new_network() {
    let v4 = cand("Ethernet 2", "192.0.2.20", true);
    let net = Arc::new(Fake::scripted(vec![
        vec![v4.clone()],
        vec![v4.clone()],
        vec![v4, cand("Ethernet 2", "2001:db8::20", true)],
    ]));
    let (_h, _rx, table) = spawn_watcher(net.clone(), WatchConfig::every(ms(50)));

    wait_for("the IPv6 address to arrive", || {
        (table.snapshot().len() == 2).then_some(())
    })
    .await;
    // Room for a wrong second test to start and finish.
    tokio::time::sleep(ms(300)).await;
    assert_eq!(samples(&net), 1, "a plug-in tested twice");
    assert!(
        table
            .snapshot()
            .iter()
            .all(|l| l.down_bps == Some(SAMPLED_BPS))
    );
}

/// Seen 2026-10-14 with the real phone. Switched from mobile data to Wi-Fi
/// with the cable in, the tether kept its IPv4 address throughout while its
/// IPv6 addresses went Deprecated and its public address became the laptop
/// Wi-Fi's own. The IPv6 network leaving was the only sign of a new network.
/// The user's choice: treat it as one, and test the cable afresh.
#[tokio::test]
async fn ipv6_leaving_a_cable_is_a_new_network() {
    let v4 = cand("Ethernet 2", "192.0.2.20", true);
    let mut steps = vec![
        vec![
            v4.clone(),
            cand("Ethernet 2", "2001:db8::20", true),
            cand("Ethernet 2", "2001:db8::21", true),
        ];
        6
    ];
    steps.push(vec![v4]);
    let net = Arc::new(
        Fake::scripted(steps)
            .sample_answers(&[(SAMPLED_BPS, ms(0)), (50_000_000, ms(300))])
            .public("192.0.2.20", &[Some("203.0.113.1"), Some("198.51.100.99")]),
    );
    let (_h, _rx, table) = spawn_watcher(net.clone(), switching_config());

    wait_for("the cable to be measured", || {
        let links = table.snapshot();
        (links.len() == 3 && links.iter().all(|l| l.down_bps == Some(SAMPLED_BPS))).then_some(())
    })
    .await;
    let at_switch = wait_for("the IPv6 addresses to leave", || {
        let links = table.snapshot();
        (links.len() == 1).then_some(links)
    })
    .await;
    assert_eq!(
        at_switch[0].down_bps, None,
        "the old network's speed still shows"
    );

    let after = wait_for("the fresh test and lookup", || {
        let link = table.snapshot().remove(0);
        (link.down_bps.is_some() && link.public_ip.is_some()).then_some(link)
    })
    .await;
    assert_eq!(after.down_bps, Some(50_000_000));
    assert_eq!(
        after.public_ip,
        Some(ip("198.51.100.99")),
        "public address not looked up again"
    );
    assert_eq!(samples(&net), 2, "not tested afresh");
}

/// The way back, also seen 2026-10-14: the phone returned to mobile data and
/// its IPv6 addresses came back while the IPv4 address stayed the same.
#[tokio::test]
async fn ipv6_arriving_on_a_cable_is_a_new_network() {
    let v4 = cand("Ethernet 2", "192.0.2.20", true);
    let mut steps = vec![vec![v4.clone()]; 6];
    steps.push(vec![v4, cand("Ethernet 2", "2001:db8::20", true)]);
    let net = Arc::new(Fake::scripted(steps));
    let (_h, _rx, _t) = spawn_watcher(net.clone(), switching_config());

    wait_for("a fresh test", || (samples(&net) == 2).then_some(())).await;
    tokio::time::sleep(ms(200)).await;
    assert_eq!(samples(&net), 2, "tested more than once more");
}

/// A phone moving between two networks that both have IPv6 keeps IPv6 on
/// the cable, but under a different network prefix.
#[tokio::test]
async fn a_new_ipv6_prefix_on_a_cable_is_a_new_network() {
    let v4 = cand("Ethernet 2", "192.0.2.20", true);
    let mut steps = vec![vec![v4.clone(), cand("Ethernet 2", "2001:db8:0:1::20", true)]; 6];
    steps.push(vec![v4, cand("Ethernet 2", "2001:db8:0:2::20", true)]);
    let net = Arc::new(Fake::scripted(steps));
    let (_h, _rx, _t) = spawn_watcher(net.clone(), switching_config());

    wait_for("a fresh test", || (samples(&net) == 2).then_some(())).await;
    tokio::time::sleep(ms(200)).await;
    assert_eq!(samples(&net), 2, "tested more than once more");
}

/// A test still running when the network changes measured some of each
/// network, so its result is thrown away and a fresh test follows. The fresh
/// one waits for the old to finish: two at once would split the cable. The
/// first test runs 600 ms so the switch, due at about 200 ms, lands inside it
/// even when ticks run late.
#[tokio::test]
async fn a_speed_test_running_across_a_network_change_is_thrown_away() {
    let v4 = cand("Ethernet 2", "192.0.2.20", true);
    let mut steps = vec![vec![v4.clone(), cand("Ethernet 2", "2001:db8::20", true)]; 4];
    steps.push(vec![v4]);
    let net =
        Arc::new(Fake::scripted(steps).sample_answers(&[(111, ms(600)), (222_000_000, ms(100))]));
    let (_h, _rx, table) = spawn_watcher(net.clone(), switching_config());

    wait_for("the fresh test's result", || {
        let links = table.snapshot();
        (links.first()?.down_bps == Some(222_000_000)).then_some(())
    })
    .await;
    assert_eq!(samples(&net), 2);
    assert_eq!(
        net.most_at_once.load(Ordering::SeqCst),
        1,
        "two tests at once"
    );
}

/// Wi-Fi moves to another network while its test is still running. The old
/// test's result describes a network that has gone and must not land on the
/// new one, which runs its own test. The old test runs 800 ms so it ends
/// well after the switch, due at about 200 ms.
#[tokio::test]
async fn a_speed_test_of_a_network_that_left_is_not_applied_to_the_next() {
    let mut steps = vec![vec![cand("Wi-Fi", "192.0.2.10", true)]; 4];
    steps.push(vec![cand("Wi-Fi", "198.51.100.10", true)]);
    let net =
        Arc::new(Fake::scripted(steps).sample_answers(&[(111, ms(800)), (222_000_000, ms(50))]));
    let started = Instant::now();
    let (_h, _rx, table) = spawn_watcher(net.clone(), WatchConfig::every(ms(50)));

    wait_for("the new network's result", || {
        let links = table.snapshot();
        let link = links.first()?;
        (link.addr.ip() == ip("198.51.100.10") && link.down_bps == Some(222_000_000)).then_some(())
    })
    .await;
    // Past the end of the old network's test, so it has had its chance to land.
    tokio::time::sleep_until(started + ms(1100)).await;
    assert_eq!(
        table.snapshot()[0].down_bps,
        Some(222_000_000),
        "the old network's result landed"
    );
    // The new network's test waits for the old one, as on an IPv6 change:
    // two at once would split the cable (code review of PR #2).
    assert_eq!(
        net.most_at_once.load(Ordering::SeqCst),
        1,
        "two tests at once"
    );
}

/// The phone's IPv6 address arrives first, so it is first in the table. When
/// its speed test fails, the retry must go through the IPv4 address, not the
/// IPv6 one again (code review of PR #2). The IPv6 test takes 200 ms to fail,
/// so the IPv4 address is up by then.
#[tokio::test]
async fn a_failed_speed_test_is_retried_through_another_address() {
    let v6 = cand("Ethernet 2", "2001:db8::20", true);
    let v4 = cand("Ethernet 2", "192.0.2.20", true);
    let net = Arc::new(
        Fake::scripted(vec![vec![v6.clone()], vec![v6, v4]])
            .slow_samples(ms(200))
            .failing_samples("2001:db8::20"),
    );
    let mut config = WatchConfig::every(ms(50));
    config.retry = ms(50);
    let (_h, _rx, table) = spawn_watcher(net.clone(), config);

    wait_for("the speed, through IPv4", || {
        let links = table.snapshot();
        (links.len() == 2 && links.iter().all(|l| l.down_bps == Some(SAMPLED_BPS))).then_some(())
    })
    .await;
    assert_eq!(
        net.sampled_through(),
        [ip("2001:db8::20"), ip("192.0.2.20")]
    );
}

/// A test that fails at once is not retried at once: three back-to-back
/// tries would all meet the same passing trouble.
#[tokio::test]
async fn a_failed_speed_test_waits_before_it_is_tried_again() {
    let net = Arc::new(
        Fake::scripted(vec![vec![cand("Wi-Fi", "192.0.2.10", true)]]).failing_samples("192.0.2.10"),
    );
    let mut config = WatchConfig::every(ms(50));
    config.retry = ms(300);
    let (_h, _rx, _t) = spawn_watcher(net.clone(), config);

    wait_for("the first test", || (samples(&net) >= 1).then_some(())).await;
    tokio::time::sleep(ms(150)).await;
    assert_eq!(samples(&net), 1, "retried without waiting");
    wait_for("all three tries", || (samples(&net) == 3).then_some(())).await;
}

/// Code review of PR #2: a dead link kept its speed and public address, so
/// anything reading the table could count capacity that was not there. The
/// figures go when the link stops being `Up`. The speed comes back without a
/// new test when the link does, since it belongs to the cable, and the
/// public address is looked up again.
#[tokio::test]
async fn a_link_that_stops_working_shows_no_speed_or_public_address() {
    let net = Arc::new(
        Fake::scripted(vec![vec![cand("Wi-Fi", "192.0.2.10", true)]])
            .verdicts(
                "192.0.2.10",
                &[
                    ProbeVerdict::Working,
                    ProbeVerdict::Broken,
                    ProbeVerdict::Broken,
                    ProbeVerdict::Working,
                ],
            )
            .public("192.0.2.10", &[Some("203.0.113.7")]),
    );
    let mut config = WatchConfig::every(ms(50));
    config.reprobe = ms(300);
    config.retry = ms(50);
    let (_h, mut rx, table) = spawn_watcher(net.clone(), config);

    let LinkEvent::Up(id) = next(&mut rx).await else {
        panic!("expected Up first");
    };
    wait_for("the speed and public address", || {
        let link = table.snapshot().remove(0);
        (link.down_bps.is_some() && link.public_ip.is_some()).then_some(())
    })
    .await;

    assert_eq!(next(&mut rx).await, LinkEvent::Down(id));
    let dead = table.snapshot().remove(0);
    assert_eq!(
        (dead.state, dead.down_bps, dead.public_ip),
        (LinkState::Down, None, None)
    );

    assert_eq!(next(&mut rx).await, LinkEvent::Up(id));
    let back = wait_for("the figures to return", || {
        let link = table.snapshot().remove(0);
        (link.down_bps.is_some() && link.public_ip.is_some()).then_some(link)
    })
    .await;
    assert_eq!(back.down_bps, Some(SAMPLED_BPS));
    assert_eq!(samples(&net), 1, "the cable was tested again");
    assert_eq!(net.lookups.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn an_up_link_learns_its_public_address() {
    let net = Fake::scripted(vec![vec![cand("Wi-Fi", "192.0.2.10", true)]])
        .public("192.0.2.10", &[Some("203.0.113.7")]);
    let (_h, _rx, table) = spawn_watcher(Arc::new(net), WatchConfig::every(ms(50)));

    let public = wait_for("the public address", || {
        table.snapshot().first().and_then(|l| l.public_ip)
    })
    .await;
    assert_eq!(public, ip("203.0.113.7"));
}

/// A portal would answer with its own address, or not at all.
#[tokio::test]
async fn a_link_that_is_not_up_is_not_looked_up() {
    let net = Arc::new(
        Fake::scripted(vec![vec![cand("Hotel", "192.0.2.30", true)]])
            .verdicts("192.0.2.30", &[ProbeVerdict::Portal]),
    );
    let (_h, mut rx, table) = spawn_watcher(net.clone(), WatchConfig::every(ms(50)));

    assert!(matches!(next(&mut rx).await, LinkEvent::Portal(_)));
    tokio::time::sleep(ms(200)).await;
    assert_eq!(net.lookups.load(Ordering::SeqCst), 0);
    assert_eq!(table.snapshot()[0].public_ip, None);
}

#[tokio::test]
async fn a_failing_public_lookup_is_tried_at_most_three_times() {
    let net = Arc::new(
        Fake::scripted(vec![vec![cand("Wi-Fi", "192.0.2.10", true)]]).public("192.0.2.10", &[None]),
    );
    let mut config = WatchConfig::every(ms(50));
    config.reprobe = ms(50);
    let (_h, _rx, table) = spawn_watcher(net.clone(), config);

    wait_for("three lookups", || {
        (net.lookups.load(Ordering::SeqCst) >= 3).then_some(())
    })
    .await;
    // Several more probes' worth of time for a fourth to happen, if it would.
    tokio::time::sleep(ms(400)).await;
    assert_eq!(net.lookups.load(Ordering::SeqCst), 3);
    assert_eq!(table.snapshot()[0].public_ip, None);
}

/// A lookup still running when the network changes answers for the old
/// network, so its answer is dropped and the new lookup's is kept. Lookups
/// take 600 ms, so the first is still running at the switch, due at about
/// 200 ms.
#[tokio::test]
async fn a_public_lookup_from_before_a_switch_is_not_applied() {
    let v4 = cand("Ethernet 2", "192.0.2.20", true);
    let mut steps = vec![vec![v4.clone(), cand("Ethernet 2", "2001:db8::20", true)]; 4];
    steps.push(vec![v4]);
    let net = Arc::new(
        Fake::scripted(steps)
            .slow_lookups(ms(600))
            .public("192.0.2.20", &[Some("203.0.113.1"), Some("198.51.100.99")]),
    );
    let (_h, _rx, table) = spawn_watcher(net.clone(), switching_config());

    wait_for("the switch", || (table.snapshot().len() == 1).then_some(())).await;
    // From the switch until the new lookup answers, the old answer must
    // never show.
    let public = wait_for("the new network's answer", || {
        let public = table.snapshot()[0].public_ip;
        assert_ne!(
            public,
            Some(ip("203.0.113.1")),
            "the old network's answer landed"
        );
        public
    })
    .await;
    assert_eq!(public, ip("198.51.100.99"));
}
