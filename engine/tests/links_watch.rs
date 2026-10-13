//! The watcher, driven by a scripted network: no hardware, no internet.
//!
//! `Fake` stands in for the outside world. Its interface list follows a
//! script, one entry per poll, the last entry repeating. Its probe answers
//! `Working` unless told otherwise, and can be told to hang. Its speed test
//! answers at once and counts how often it was asked.

use std::collections::{HashMap, HashSet, VecDeque};
use std::net::IpAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use barrage::links::probe::{Probe, ProbeVerdict};
use barrage::links::watch::{Network, WatchConfig, spawn_watcher};
use barrage::links::{Addr, Candidate, LinkEvent, LinkState};
use tokio::sync::broadcast::Receiver;
use tokio::time::{Instant, timeout};

#[derive(Clone)]
enum Step {
    Links(Vec<Candidate>),
    Fail,
}

#[derive(Default)]
struct Fake {
    script: Mutex<VecDeque<Step>>,
    verdicts: Mutex<HashMap<IpAddr, VecDeque<ProbeVerdict>>>,
    hangs: HashSet<IpAddr>,
    samples: AtomicUsize,
    /// How long each speed test takes. Zero answers at once.
    sample_time: Duration,
    sampling_now: AtomicUsize,
    /// The most speed tests that were ever running at the same moment.
    most_at_once: AtomicUsize,
}

const SAMPLED_BPS: u64 = 40_000_000;

impl Fake {
    fn scripted(steps: Vec<Vec<Candidate>>) -> Self {
        Self::steps(steps.into_iter().map(Step::Links).collect())
    }

    fn steps(steps: Vec<Step>) -> Self {
        Self {
            script: Mutex::new(steps.into()),
            ..Self::default()
        }
    }

    /// Probe answers for `ip`, in order; the last one repeats.
    fn verdicts(self, ip: &str, answers: &[ProbeVerdict]) -> Self {
        self.verdicts
            .lock()
            .unwrap()
            .insert(ip.parse().unwrap(), answers.iter().copied().collect());
        self
    }

    fn hanging(mut self, ip: &str) -> Self {
        self.hangs.insert(ip.parse().unwrap());
        self
    }

    fn slow_samples(mut self, each: Duration) -> Self {
        self.sample_time = each;
        self
    }
}

impl Network for Fake {
    fn enumerate(&self) -> anyhow::Result<Vec<Candidate>> {
        let mut script = self.script.lock().unwrap();
        let step = if script.len() > 1 {
            script.pop_front()
        } else {
            script.front().cloned()
        };
        match step {
            Some(Step::Links(links)) => Ok(links),
            Some(Step::Fail) => anyhow::bail!("scripted enumeration failure"),
            None => Ok(Vec::new()),
        }
    }

    fn probe(
        &self,
        addr: IpAddr,
        _timeout: Duration,
    ) -> impl Future<Output = anyhow::Result<Probe>> + Send {
        let hang = self.hangs.contains(&addr);
        let verdict = match self.verdicts.lock().unwrap().get_mut(&addr) {
            Some(queue) if queue.len() > 1 => queue.pop_front().unwrap(),
            Some(queue) => *queue.front().unwrap(),
            None => ProbeVerdict::Working,
        };
        async move {
            if hang {
                std::future::pending::<()>().await;
            }
            Ok(Probe {
                verdict,
                rtt: Duration::from_millis(20),
            })
        }
    }

    fn sample(
        &self,
        _addr: IpAddr,
        _budget: Duration,
    ) -> impl Future<Output = anyhow::Result<u64>> + Send {
        self.samples.fetch_add(1, Ordering::SeqCst);
        async move {
            let now = self.sampling_now.fetch_add(1, Ordering::SeqCst) + 1;
            self.most_at_once.fetch_max(now, Ordering::SeqCst);
            tokio::time::sleep(self.sample_time).await;
            self.sampling_now.fetch_sub(1, Ordering::SeqCst);
            Ok(SAMPLED_BPS)
        }
    }
}

fn cand(name: &str, ip: &str, up: bool) -> Candidate {
    Candidate {
        os_index: 1,
        name: name.to_string(),
        addr: Addr::from(ip.parse::<IpAddr>().unwrap()),
        up,
    }
}

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

/// The next event, failing the test if none arrives within two seconds.
async fn next(rx: &mut Receiver<LinkEvent>) -> LinkEvent {
    timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("no event within 2 s")
        .expect("event channel closed")
}

/// Every event that arrives within `window`.
async fn drain_for(rx: &mut Receiver<LinkEvent>, window: Duration) -> Vec<LinkEvent> {
    let deadline = Instant::now() + window;
    let mut seen = Vec::new();
    while let Ok(Ok(event)) = tokio::time::timeout_at(deadline, rx.recv()).await {
        seen.push(event);
    }
    seen
}

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
    let (_h, mut rx, table) = spawn_watcher(Arc::new(net), WatchConfig::every(ms(50)));

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

/// A rotating IPv6 privacy address joins an interface whose other addresses
/// stay. Same connection, so the speed carries over and no data is spent.
#[tokio::test]
async fn an_address_joining_a_live_interface_inherits_its_speed() {
    let v4 = cand("Ethernet 2", "192.0.2.20", true);
    let net = Arc::new(Fake::scripted(vec![
        vec![v4.clone()],
        vec![v4.clone()],
        vec![v4.clone()],
        vec![v4, cand("Ethernet 2", "2001:db8::22", true)],
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
