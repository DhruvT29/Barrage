//! A scripted network for driving the watcher: no hardware, no internet.
//!
//! `Fake` stands in for the outside world. Its interface list follows a
//! script, one entry per poll, the last entry repeating. Its probe answers
//! `Working` unless told otherwise, and can be told to fail or hang. Its speed
//! test answers at once unless told to be slow or to fail. Both count how
//! often they were asked.
//!
//! Shared by `links_watch.rs` and `links_handle.rs`, which each use a
//! different part of it.
#![allow(dead_code)]

use std::collections::{HashMap, HashSet, VecDeque};
use std::net::IpAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use barrage::links::probe::{Probe, ProbeVerdict};
use barrage::links::watch::Network;
use barrage::links::{Addr, Candidate, LinkEvent};
use tokio::sync::broadcast::Receiver;
use tokio::time::{Instant, timeout};

#[derive(Clone)]
pub enum Step {
    Links(Vec<Candidate>),
    Fail,
}

#[derive(Default)]
pub struct Fake {
    script: Mutex<VecDeque<Step>>,
    verdicts: Mutex<HashMap<IpAddr, VecDeque<ProbeVerdict>>>,
    /// How many of each address's next probes fail with an error.
    failures: Mutex<HashMap<IpAddr, usize>>,
    hangs: HashSet<IpAddr>,
    pub probes: AtomicUsize,
    pub samples: AtomicUsize,
    /// Speed tests that ran to the end, rather than being stopped.
    pub samples_finished: AtomicUsize,
    /// The address each speed test went through, in order.
    sampled_through: Mutex<Vec<IpAddr>>,
    /// Addresses whose speed tests fail, after taking their usual time.
    failing_samples: HashSet<IpAddr>,
    /// How long each speed test takes. Zero answers at once.
    sample_time: Duration,
    sampling_now: AtomicUsize,
    /// The most speed tests that were ever running at the same moment.
    pub most_at_once: AtomicUsize,
    /// Scripted speed test results and how long each takes, in order; the
    /// last repeats. Empty means `SAMPLED_BPS` after `sample_time`.
    sample_answers: Mutex<VecDeque<(u64, Duration)>>,
    /// Public addresses per local address, in order; the last repeats. An
    /// address with none scripted reports itself, so no two links share.
    publics: Mutex<HashMap<IpAddr, VecDeque<Option<IpAddr>>>>,
    pub lookups: AtomicUsize,
    /// How long each public address lookup takes. Zero answers at once.
    lookup_time: Duration,
    /// Keep loopback addresses as links, which the real filter drops.
    loopback: bool,
    /// When set, the interface list is whatever this holds at each poll, and
    /// the script is not used.
    live: Option<Live>,
    /// Probe answers a test switches while the watcher runs, per address.
    switches: HashMap<IpAddr, Switch>,
}

/// A probe answer a test changes while the watcher runs: a link that stops
/// working, and works again, under the same id.
#[derive(Clone)]
pub struct Switch(Arc<Mutex<ProbeVerdict>>);

impl Switch {
    pub fn set(&self, verdict: ProbeVerdict) {
        *self.0.lock().unwrap() = verdict;
    }
}

/// An interface list a test changes while the watcher runs, so a link comes
/// or goes at a moment the test picks, such as 30% into a download, rather
/// than at a poll it has to count.
#[derive(Clone, Default)]
pub struct Live(Arc<Mutex<Vec<Candidate>>>);

impl Live {
    pub fn set(&self, links: Vec<Candidate>) {
        *self.0.lock().unwrap() = links;
    }
}

pub const SAMPLED_BPS: u64 = 40_000_000;

impl Fake {
    pub fn scripted(steps: Vec<Vec<Candidate>>) -> Self {
        Self::steps(steps.into_iter().map(Step::Links).collect())
    }

    pub fn steps(steps: Vec<Step>) -> Self {
        Self {
            script: Mutex::new(steps.into()),
            ..Self::default()
        }
    }

    /// The interfaces in `links` until the test changes them through the
    /// returned [`Live`].
    pub fn live(links: Vec<Candidate>) -> (Self, Live) {
        let live = Live(Arc::new(Mutex::new(links)));
        let fake = Self {
            live: Some(live.clone()),
            ..Self::default()
        };
        (fake, live)
    }

    /// Probes of `ip` answer whatever the returned [`Switch`] holds, at first
    /// `Working`.
    pub fn switched(mut self, ip: &str) -> (Self, Switch) {
        let switch = Switch(Arc::new(Mutex::new(ProbeVerdict::Working)));
        self.switches.insert(ip.parse().unwrap(), switch.clone());
        (self, switch)
    }

    /// Probe answers for `ip`, in order; the last one repeats.
    pub fn verdicts(self, ip: &str, answers: &[ProbeVerdict]) -> Self {
        self.verdicts
            .lock()
            .unwrap()
            .insert(ip.parse().unwrap(), answers.iter().copied().collect());
        self
    }

    /// The first `n` probes of `ip` fail with an error, as a probe that ran
    /// out of time does; after that its verdicts apply.
    pub fn failing_first(self, ip: &str, n: usize) -> Self {
        self.failures.lock().unwrap().insert(ip.parse().unwrap(), n);
        self
    }

    pub fn hanging(mut self, ip: &str) -> Self {
        self.hangs.insert(ip.parse().unwrap());
        self
    }

    /// Every speed test through `ip` fails, as one over a route that cannot
    /// carry a download does.
    pub fn failing_samples(mut self, ip: &str) -> Self {
        self.failing_samples.insert(ip.parse().unwrap());
        self
    }

    pub fn sampled_through(&self) -> Vec<IpAddr> {
        self.sampled_through.lock().unwrap().clone()
    }

    pub fn slow_samples(mut self, each: Duration) -> Self {
        self.sample_time = each;
        self
    }

    /// Speed test results and durations, in order; the last repeats.
    pub fn sample_answers(self, answers: &[(u64, Duration)]) -> Self {
        *self.sample_answers.lock().unwrap() = answers.iter().copied().collect();
        self
    }

    pub fn slow_lookups(mut self, each: Duration) -> Self {
        self.lookup_time = each;
        self
    }

    /// Let 127.0.0.x addresses stand in for real links, so a download over
    /// them carries real bytes to a server on this machine. Everything else
    /// is still judged by the real filter.
    pub fn loopback_links(mut self) -> Self {
        self.loopback = true;
        self
    }

    /// The public address seen through `ip`, lookup after lookup; the last
    /// repeats. `None` makes that lookup fail.
    pub fn public(self, ip: &str, answers: &[Option<&str>]) -> Self {
        let answers = answers
            .iter()
            .map(|a| a.map(|s| s.parse().unwrap()))
            .collect();
        self.publics
            .lock()
            .unwrap()
            .insert(ip.parse().unwrap(), answers);
        self
    }
}

impl Network for Fake {
    fn enumerate(&self) -> anyhow::Result<Vec<Candidate>> {
        if let Some(live) = &self.live {
            return Ok(live.0.lock().unwrap().clone());
        }
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

    fn usable(&self, all: &[Candidate]) -> Vec<Candidate> {
        let real = barrage::links::usable(all);
        all.iter()
            .filter(|c| (self.loopback && c.up && c.addr.ip().is_loopback()) || real.contains(c))
            .cloned()
            .collect()
    }

    fn probe(
        &self,
        addr: IpAddr,
        _timeout: Duration,
    ) -> impl Future<Output = anyhow::Result<Probe>> + Send {
        self.probes.fetch_add(1, Ordering::SeqCst);
        let hang = self.hangs.contains(&addr);
        let fail = match self.failures.lock().unwrap().get_mut(&addr) {
            Some(left) if *left > 0 => {
                *left -= 1;
                true
            }
            _ => false,
        };
        // A failed probe does not use up a scripted verdict.
        let verdict = match self.verdicts.lock().unwrap().get_mut(&addr) {
            Some(queue) if queue.len() > 1 && !fail => queue.pop_front().unwrap(),
            Some(queue) => *queue.front().unwrap(),
            None => self
                .switches
                .get(&addr)
                .map_or(ProbeVerdict::Working, |s| *s.0.lock().unwrap()),
        };
        async move {
            if hang {
                std::future::pending::<()>().await;
            }
            if fail {
                anyhow::bail!("scripted probe failure");
            }
            Ok(Probe {
                verdict,
                rtt: Duration::from_millis(20),
            })
        }
    }

    fn sample(
        &self,
        addr: IpAddr,
        _budget: Duration,
    ) -> impl Future<Output = anyhow::Result<u64>> + Send {
        self.samples.fetch_add(1, Ordering::SeqCst);
        self.sampled_through.lock().unwrap().push(addr);
        let fail = self.failing_samples.contains(&addr);
        let (bps, takes) = {
            let mut answers = self.sample_answers.lock().unwrap();
            match answers.len() {
                0 => (SAMPLED_BPS, self.sample_time),
                1 => *answers.front().unwrap(),
                _ => answers.pop_front().unwrap(),
            }
        };
        async move {
            let now = self.sampling_now.fetch_add(1, Ordering::SeqCst) + 1;
            self.most_at_once.fetch_max(now, Ordering::SeqCst);
            tokio::time::sleep(takes).await;
            self.sampling_now.fetch_sub(1, Ordering::SeqCst);
            self.samples_finished.fetch_add(1, Ordering::SeqCst);
            if fail {
                anyhow::bail!("scripted speed test failure");
            }
            Ok(bps)
        }
    }

    fn public_ip(
        &self,
        addr: IpAddr,
        _timeout: Duration,
    ) -> impl Future<Output = anyhow::Result<IpAddr>> + Send {
        self.lookups.fetch_add(1, Ordering::SeqCst);
        let answer = match self.publics.lock().unwrap().get_mut(&addr) {
            Some(queue) if queue.len() > 1 => queue.pop_front().unwrap(),
            Some(queue) => *queue.front().unwrap(),
            None => Some(addr),
        };
        let takes = self.lookup_time;
        async move {
            tokio::time::sleep(takes).await;
            answer.ok_or_else(|| anyhow::anyhow!("scripted lookup failure"))
        }
    }
}

pub fn cand(name: &str, ip: &str, up: bool) -> Candidate {
    cand_idx(1, name, ip, up)
}

/// A candidate with a chosen OS interface index.
pub fn cand_idx(os_index: u32, name: &str, ip: &str, up: bool) -> Candidate {
    Candidate {
        os_index,
        name: name.to_string(),
        addr: Addr::from(ip.parse::<IpAddr>().unwrap()),
        up,
    }
}

pub fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

/// The next event, failing the test if none arrives within two seconds.
pub async fn next(rx: &mut Receiver<LinkEvent>) -> LinkEvent {
    timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("no event within 2 s")
        .expect("event channel closed")
}

/// Every event that arrives within `window`.
pub async fn drain_for(rx: &mut Receiver<LinkEvent>, window: Duration) -> Vec<LinkEvent> {
    let deadline = Instant::now() + window;
    let mut seen = Vec::new();
    while let Ok(Ok(event)) = tokio::time::timeout_at(deadline, rx.recv()).await {
        seen.push(event);
    }
    seen
}
