//! A scripted network for driving the watcher: no hardware, no internet.
//!
//! `Fake` stands in for the outside world. Its interface list follows a
//! script, one entry per poll, the last entry repeating. Its probe answers
//! `Working` unless told otherwise, and can be told to fail or hang. Its speed
//! test answers at once. Both count how often they were asked.
//!
//! Shared by `links_watch.rs` and `links_handle.rs`, which each use a
//! different part of it.
#![allow(dead_code)]

use std::collections::{HashMap, HashSet, VecDeque};
use std::net::IpAddr;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
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
    /// How long each speed test takes. Zero answers at once.
    sample_time: Duration,
    sampling_now: AtomicUsize,
    /// The most speed tests that were ever running at the same moment.
    pub most_at_once: AtomicUsize,
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

    pub fn slow_samples(mut self, each: Duration) -> Self {
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
            None => ProbeVerdict::Working,
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

pub fn cand(name: &str, ip: &str, up: bool) -> Candidate {
    Candidate {
        os_index: 1,
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
