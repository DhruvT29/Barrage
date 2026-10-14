//! The watcher: one task that owns the link table.
//!
//! Every poll it reads the interface list, keeps the usable entries, and
//! compares them with the table. A newcomer gets a fresh `LinkId` and starts
//! as `Probing`; a link no longer listed is removed. Probes, speed tests and
//! lookups run as separate tasks that the watcher owns, each ending in a
//! report, so the loop itself never waits on the network (Review Focus 3),
//! and when the watcher stops they stop with it. Nothing else ever writes
//! the table. Readers get a published copy through [`LinkTable`].
//!
//! THE OUTSIDE WORLD IS INJECTED
//!
//! The interface list, the probe and the speed test all come through the
//! [`Network`] trait. [`SystemNetwork`] is the real machine and the real
//! internet; tests supply a scripted one. The plan injected only the
//! interface list, but its own tests expected made-up links to come up, which
//! needs a probe that can be scripted too.
//!
//! IDENTITY
//!
//! A link is its (name, address) pair. When either changes it is a different
//! link: a new `LinkId`, a fresh `Probing` state, nothing carried over. The OS
//! interface index never takes part, because Windows reuses indices.
//!
//! EVENTS
//!
//! An event is sent when a link's state changes to a judged state (`Up`,
//! `Portal`, `Down`, and later `Degraded`). A link that disappears is reported
//! `Down`, unless it was already `Down` or never judged, since then nobody was
//! relying on it.
//!
//! An event goes out only after the table showing it has been published, so
//! whoever hears `Up(id)` and looks the link up finds it `Up`. The other order
//! was measured on 2026-10-13: a listener on its own thread found all 60 of 60
//! links still `Probing` when their `Up` arrived.
//!
//! PROBING: EVERY ADDRESS, AGAIN AND AGAIN
//!
//! Every link is probed when it appears and again every
//! [`WatchConfig::reprobe`]. That is how a portal the user has logged into
//! becomes `Up`, and how a link that loses the internet while staying
//! connected becomes `Down`. A probe is about 1 KB, so this costs next to
//! nothing, and each address is probed on its own because IPv4 and IPv6 can
//! fail independently.
//!
//! One failed probe changes nothing. The link is probed again after
//! [`WatchConfig::retry`] and called `Down` only after
//! [`FAILURES_FOR_DOWN`] failures in a row. On 2026-10-13 about one probe in
//! 15 over a healthy Wi-Fi ran past 3 s, the first after a pause, while the
//! system looked the probe host up again (one lookup took 2,026 ms against a
//! usual 25). Calling the link `Down` on that would hide a working link for
//! 10 s, or make phase 2 drop one mid-download. Once a link is `Down` it is
//! probed at the normal pace again, so a dead link costs one probe per
//! `reprobe`, not one a second.
//!
//! SPEED: ONCE PER INTERFACE, SHARED BY ITS ADDRESSES
//!
//! Speed belongs to the connection, not to an address on it. On 2026-10-13
//! the phone tether held one IPv4 and two IPv6 addresses over one cable.
//! Tested one per address, back to back, they read 160.5, 16.4 and 10.3 Mbps,
//! and curl showed the same pattern (188.9 then 8.6): the carrier allows a
//! burst, then slows the line, so every test after the first measures the
//! slowdown. The user chose one test per interface, its result shown on all
//! of that interface's addresses.
//!
//! - The test runs once, when the first address on an interface comes up:
//!   the user's choice on 2026-10-13, because each test is seconds of real
//!   download, and through a phone that is real mobile data. Phase 2 will
//!   measure speed for free from actual download traffic.
//! - An interface keeps its measured speed only while it stays on the same
//!   network. Moving Wi-Fi to a new network removes every old address, so
//!   the old speed is forgotten and the new network is tested afresh. A
//!   rotating IPv6 privacy address joins while the others stay, so it
//!   inherits the speed and costs no new test.
//! - **A change in an interface's IPv6 networks is a new network too**, the
//!   user's choice on 2026-10-14. That day the phone, cable in, switched from
//!   mobile data to Wi-Fi and back: the tether's IPv4 address never changed,
//!   while its IPv6 addresses went Deprecated and returned, and its public
//!   address moved from the carrier's to the laptop Wi-Fi's own. So IPv6
//!   leaving, arriving or changing prefix forgets the speed and the public
//!   addresses and tests afresh. Rotation keeps the prefix, so it does not
//!   count, and neither does IPv6 arriving within
//!   [`WatchConfig::settle`] of the interface appearing. Blind spot: a phone
//!   whose networks have no IPv6 shows no sign.
//! - A test still running when that happens is thrown away when it ends, and
//!   a fresh one follows; two at once would split the cable. Every test and
//!   lookup is numbered, so an answer that no longer matters, including one
//!   from a network that has entirely gone, is recognised and dropped.
//! - A failed test is retried after [`WatchConfig::retry`], through an
//!   address not tried yet if one is up, otherwise through one already
//!   tried, at most [`MAX_SAMPLE_ATTEMPTS`] times while the interface stays
//!   on one network.
//! - A link shows a speed only while it is `Up`. The speed stays with the
//!   cable, so a link that recovers shows it again without a new test.
//! - Separate interfaces test at the same time, since phase 0 showed separate
//!   links keep most of their speed when used together.
//!
//! PUBLIC ADDRESS: ONCE PER LINK, AGAIN ON A NEW NETWORK
//!
//! Each `Up` link's public address is looked up once (a few KB over HTTPS),
//! at most [`MAX_LOOKUP_ATTEMPTS`] times if it fails, and again when its
//! interface's network changes or the link recovers from being down. Links on
//! different cables showing the same one share an internet connection; see
//! [`super::upstream`].

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{broadcast, watch};
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::{Instant, MissedTickBehavior};

use super::filter::usable;
use super::model::{Candidate, Link, LinkEvent, LinkId, LinkState};
use super::probe::{Probe, ProbeVerdict};

/// Everything the watcher asks of the outside world.
pub trait Network: Send + Sync + 'static {
    /// Every address on every interface, unjudged.
    fn enumerate(&self) -> anyhow::Result<Vec<Candidate>>;

    /// Probe the internet through the link that owns `addr`.
    fn probe(
        &self,
        addr: IpAddr,
        timeout: Duration,
    ) -> impl Future<Output = anyhow::Result<Probe>> + Send;

    /// Measure the download rate through the link that owns `addr`, in bits
    /// per second.
    fn sample(
        &self,
        addr: IpAddr,
        budget: Duration,
    ) -> impl Future<Output = anyhow::Result<u64>> + Send;

    /// The address the internet sees traffic from `addr` arrive from.
    fn public_ip(
        &self,
        addr: IpAddr,
        timeout: Duration,
    ) -> impl Future<Output = anyhow::Result<IpAddr>> + Send;
}

/// This machine's interfaces and the real internet.
pub struct SystemNetwork;

impl Network for SystemNetwork {
    fn enumerate(&self) -> anyhow::Result<Vec<Candidate>> {
        super::enumerate::enumerate()
    }

    fn probe(
        &self,
        addr: IpAddr,
        timeout: Duration,
    ) -> impl Future<Output = anyhow::Result<Probe>> + Send {
        super::probe::probe(addr, timeout)
    }

    fn sample(
        &self,
        addr: IpAddr,
        budget: Duration,
    ) -> impl Future<Output = anyhow::Result<u64>> + Send {
        super::measure::sample(addr, budget)
    }

    fn public_ip(
        &self,
        addr: IpAddr,
        timeout: Duration,
    ) -> impl Future<Output = anyhow::Result<IpAddr>> + Send {
        super::probe::public_ip_via(addr, timeout)
    }
}

/// How often the watcher looks, and how long it lets each check take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WatchConfig {
    /// How often the interface list is read. The acceptance criteria want a
    /// plugged-in phone to appear within 2 s, hence a 1 s default.
    pub poll: Duration,
    /// How long one probe may take before the link is judged `Down`. The
    /// tether's probe took up to 934 ms on 2026-10-13, and a probe makes
    /// several round trips.
    pub probe_timeout: Duration,
    /// How often every link is probed again.
    pub reprobe: Duration,
    /// How soon a failed probe is tried again, before the link is called
    /// `Down`. Long enough for a stalled lookup to have finished, so the
    /// second try finds the answer remembered. A failed speed test waits as
    /// long before its retry.
    pub retry: Duration,
    /// How long the one speed test per interface runs. Five seconds, because
    /// a single 2 s sample ranged 22.7 to 47.1 Mbps on steady Wi-Fi, and
    /// about half a second of any budget goes on connecting.
    pub sample_budget: Duration,
    /// How long after an interface first appears a change in its IPv6
    /// networks is taken for its addresses still arriving rather than a new
    /// network. A plugged-in phone's IPv6 addresses follow its IPv4 one.
    pub settle: Duration,
}

impl WatchConfig {
    /// Defaults, polling every `poll`.
    pub fn every(poll: Duration) -> Self {
        Self {
            poll,
            probe_timeout: Duration::from_secs(3),
            reprobe: Duration::from_secs(10),
            retry: Duration::from_secs(1),
            sample_budget: Duration::from_secs(5),
            settle: Duration::from_secs(10),
        }
    }
}

/// How many times an interface's speed test is tried before giving up.
/// Bounded because a test that keeps failing part-way through can keep
/// spending mobile data.
pub const MAX_SAMPLE_ATTEMPTS: u8 = 3;

/// How many times a link's public address is looked up before giving up,
/// until its network changes.
pub const MAX_LOOKUP_ATTEMPTS: u8 = 3;

/// How long one public address lookup may take. It is an HTTPS request, so a
/// TLS handshake at the tether's worst round trip (934 ms on 2026-10-13)
/// alone takes about 3 s.
pub const LOOKUP_TIMEOUT: Duration = Duration::from_secs(5);

/// How many probes in a row must fail before a link is called `Down`.
pub const FAILURES_FOR_DOWN: u8 = 2;

/// Room for bursts of events before a slow subscriber starts missing some.
pub const EVENT_CAPACITY: usize = 256;

/// A read-only view of the watcher's table.
#[derive(Debug, Clone)]
pub struct LinkTable(watch::Receiver<Vec<Link>>);

impl LinkTable {
    /// The links as of the watcher's last change, in the order they appeared.
    /// Copies out and returns at once; it never waits on the watcher.
    pub fn snapshot(&self) -> Vec<Link> {
        self.0.borrow().clone()
    }
}

/// Start the watcher.
///
/// Returns the task, a receiver for link events, and a view of the table.
/// Events go out on a broadcast channel so later subscribers can join
/// (task 8), each only after the table showing it. A subscriber that falls
/// more than [`EVENT_CAPACITY`] events behind misses the oldest; the table is
/// always current regardless.
pub fn spawn_watcher<N: Network>(
    net: Arc<N>,
    config: WatchConfig,
) -> (JoinHandle<()>, broadcast::Receiver<LinkEvent>, LinkTable) {
    let (events, events_rx) = broadcast::channel(EVENT_CAPACITY);
    let (table, table_rx) = watch::channel(Vec::new());
    let watcher = Watcher {
        net,
        config,
        links: BTreeMap::new(),
        speeds: HashMap::new(),
        networks: HashMap::new(),
        next_id: 1,
        next_job: 1,
        events,
        table,
        tasks: JoinSet::new(),
        changed: false,
        pending: Vec::new(),
    };
    (tokio::spawn(watcher.run()), events_rx, LinkTable(table_rx))
}

/// What a probe, speed test or lookup reports back to the loop.
enum Report {
    Probed(LinkId, anyhow::Result<Probe>),
    /// Keyed by interface, since the speed belongs to the interface and the
    /// address the test ran through may have vanished in the meantime.
    Sampled {
        interface: String,
        job: u64,
        result: anyhow::Result<u64>,
    },
    LookedUp {
        id: LinkId,
        job: u64,
        result: anyhow::Result<IpAddr>,
    },
}

struct Entry {
    link: Link,
    probing: bool,
    next_probe: Instant,
    /// Probes failed in a row since the last one that answered.
    failures: u8,
    /// The public address lookup whose answer is wanted, if one is running.
    lookup: Option<u64>,
    lookup_attempts: u8,
}

/// One interface's speed test, shared by all its addresses.
#[derive(Default)]
struct Speed {
    bps: Option<u64>,
    /// The test running on this interface, if any. Only its result counts.
    running: Option<u64>,
    /// The network changed while `running` ran, so its result describes
    /// neither network: throw it away and test again.
    stale: bool,
    attempts: u8,
    /// The links tests have gone through, so a retry can go another way.
    tried: BTreeSet<LinkId>,
}

impl Speed {
    /// Forget the old network. A test still running measured it, so it is
    /// marked stale rather than dropped: the next test waits for it to end,
    /// since two at once would split the cable.
    fn forget(&mut self) {
        *self = Speed {
            running: self.running,
            stale: self.running.is_some(),
            ..Speed::default()
        };
    }
}

struct Watcher<N> {
    net: Arc<N>,
    config: WatchConfig,
    /// Keyed by `LinkId`, which is minted in increasing order, so iterating
    /// gives the order links appeared in.
    links: BTreeMap<LinkId, Entry>,
    /// Per interface name, for as long as the interface has a link.
    speeds: HashMap<String, Speed>,
    /// Per interface name, when it was first seen and its IPv6 networks
    /// (/64 prefixes) as last seen.
    networks: HashMap<String, (Instant, BTreeSet<u64>)>,
    next_id: u64,
    /// Numbers every speed test and lookup, so a late answer to one that no
    /// longer matters can be recognised and dropped.
    next_job: u64,
    events: broadcast::Sender<LinkEvent>,
    table: watch::Sender<Vec<Link>>,
    /// Every probe, speed test and lookup still running, each ending in its
    /// report. Owned here so that when the watcher stops they all stop: a
    /// speed test left running would go on spending mobile data for nobody.
    tasks: JoinSet<Report>,
    /// Whether the table differs from the copy last published.
    changed: bool,
    /// Events waiting for the table that shows them to be published.
    pending: Vec<LinkEvent>,
}

impl<N: Network> Watcher<N> {
    async fn run(mut self) {
        let mut ticker = tokio::time::interval(self.config.poll);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = ticker.tick() => self.poll(),
                Some(done) = self.tasks.join_next() => match done {
                    Ok(report) => self.receive(report),
                    Err(e) => tracing::error!(error = %e, "a probe, speed test or lookup panicked"),
                },
            }
            if self.changed {
                let copy = self.links.values().map(|e| e.link.clone()).collect();
                self.table.send_replace(copy);
                self.changed = false;
            }
            for event in self.pending.drain(..) {
                // An error only means nobody is listening right now.
                let _ = self.events.send(event);
            }
        }
    }

    fn poll(&mut self) {
        let candidates = match self.net.enumerate() {
            Ok(all) => usable(&all),
            Err(e) => {
                // Says nothing about the links themselves; keep what we know.
                tracing::warn!(error = %format!("{e:#}"), "interface list unavailable this poll");
                return;
            }
        };

        let gone: Vec<LinkId> = self
            .links
            .iter()
            .filter(|(_, e)| !candidates.iter().any(|c| is_same_link(c, &e.link)))
            .map(|(id, _)| *id)
            .collect();
        for id in gone {
            if let Some(entry) = self.links.remove(&id) {
                if matches!(
                    entry.link.state,
                    LinkState::Up | LinkState::Degraded | LinkState::Portal
                ) {
                    self.emit(LinkEvent::Down(id));
                }
                self.changed = true;
            }
        }
        // An interface whose every address has gone forgets its speed, so a
        // new network on the same interface is tested afresh. This runs
        // before newcomers are added, so a network swap seen within a single
        // poll still counts. The record stays while a test is still running,
        // so the new network's test waits for it.
        let links = &self.links;
        self.speeds.retain(|name, speed| {
            if links.values().any(|e| &e.link.name == name) {
                return true;
            }
            speed.forget();
            speed.running.is_some()
        });

        // An interface whose IPv6 networks changed is on a new network even
        // if an address stayed, as the tether's IPv4 did when the phone
        // switched between mobile data and Wi-Fi.
        // Changes in an interface's first moments are its addresses still
        // arriving: a plugged-in phone's IPv6 follows its IPv4 by a second or
        // two, and counting that would run every first speed test twice.
        let at = Instant::now();
        let mut switched = Vec::new();
        let mut networks = HashMap::new();
        for (name, prefixes) in ipv6_networks(&candidates) {
            let since = match self.networks.get(&name) {
                Some((since, before)) => {
                    if *before != prefixes && at.duration_since(*since) >= self.config.settle {
                        switched.push(name.clone());
                    }
                    *since
                }
                None => at,
            };
            networks.insert(name, (since, prefixes));
        }
        self.networks = networks;
        for interface in &switched {
            self.network_changed(interface);
        }

        for c in &candidates {
            if self.links.values().any(|e| is_same_link(c, &e.link)) {
                continue;
            }
            let id = LinkId::new(self.next_id);
            self.next_id += 1;
            let link = Link {
                id,
                name: c.name.clone(),
                addr: c.addr,
                state: LinkState::Probing,
                rtt: None,
                down_bps: None,
                public_ip: None,
                // No task reads the Windows network cost type yet.
                metered: false,
            };
            self.links.insert(
                id,
                Entry {
                    link,
                    probing: false,
                    next_probe: Instant::now(),
                    failures: 0,
                    lookup: None,
                    lookup_attempts: 0,
                },
            );
            self.changed = true;
        }

        let now = Instant::now();
        let due: Vec<(LinkId, IpAddr)> = self
            .links
            .iter_mut()
            .filter(|(_, e)| !e.probing && e.next_probe <= now)
            .map(|(id, e)| {
                e.probing = true;
                (*id, e.link.addr.ip())
            })
            .collect();
        for (id, addr) in due {
            self.start_probe(id, addr);
        }
    }

    /// Forget what was measured about `interface`'s old network, and measure
    /// the new one.
    fn network_changed(&mut self, interface: &str) {
        tracing::info!(%interface, "IPv6 networks changed: a new network, testing afresh");
        if let Some(speed) = self.speeds.get_mut(interface) {
            speed.forget();
        }
        let ids: Vec<LinkId> = self
            .links
            .iter_mut()
            .filter(|(_, e)| e.link.name == interface)
            .map(|(id, e)| {
                e.link.down_bps = None;
                e.link.public_ip = None;
                e.lookup = None;
                e.lookup_attempts = 0;
                *id
            })
            .collect();
        if !ids.is_empty() {
            self.changed = true;
        }
        self.measure_interface(interface, Duration::ZERO);
        for id in ids {
            self.look_up_if_wanted(id);
        }
    }

    fn receive(&mut self, report: Report) {
        match report {
            Report::Probed(id, result) => self.probed(id, result),
            Report::Sampled {
                interface,
                job,
                result,
            } => self.sampled(interface, job, result),
            Report::LookedUp { id, job, result } => self.looked_up(id, job, result),
        }
    }

    fn probed(&mut self, id: LinkId, result: anyhow::Result<Probe>) {
        // Gone since the probe started: a stale answer about a link that no
        // longer exists, possibly about an address now held by another.
        let Some(e) = self.links.get_mut(&id) else {
            return;
        };
        e.probing = false;
        let now = Instant::now();

        let answered = match result {
            Ok(p) => match p.verdict {
                ProbeVerdict::Working => Some((LinkState::Up, p.rtt)),
                ProbeVerdict::Portal => Some((LinkState::Portal, p.rtt)),
                ProbeVerdict::Broken => {
                    tracing::debug!(link = %e.link.name, "probe answered, but not as the internet does");
                    None
                }
            },
            Err(err) => {
                tracing::debug!(link = %e.link.name, error = %format!("{err:#}"), "probe failed");
                None
            }
        };
        let (state, rtt) = match answered {
            Some((state, rtt)) => {
                e.failures = 0;
                (state, Some(rtt))
            }
            None => {
                e.failures = e.failures.saturating_add(1);
                if e.failures < FAILURES_FOR_DOWN {
                    // Not yet: try again soon and change nothing meanwhile.
                    e.next_probe = now + self.config.retry;
                    return;
                }
                (LinkState::Down, None)
            }
        };
        e.next_probe = now + self.config.reprobe;
        e.link.rtt = rtt;
        self.changed = true;

        let changed_state = e.link.state != state;
        e.link.state = state;
        if state != LinkState::Up {
            // A speed and a public address describe a working link, and a
            // dead one showing them would count capacity that is not there.
            // The speed stays with the cable and comes back with the link;
            // the public address is looked up again.
            e.link.down_bps = None;
            e.link.public_ip = None;
        }

        if changed_state && let Some(event) = event_for(state, id) {
            self.emit(event);
        }
        self.measure_if_wanted(id, Duration::ZERO);
        self.look_up_if_wanted(id);
    }

    fn sampled(&mut self, interface: String, job: u64, result: anyhow::Result<u64>) {
        // A test of a network that has since gone, every address with it:
        // its result describes a connection that no longer exists.
        let Some(speed) = self.speeds.get_mut(&interface) else {
            return;
        };
        if speed.running != Some(job) {
            return;
        }
        speed.running = None;
        if std::mem::take(&mut speed.stale) {
            // Ran across a network change, so it measured part of each.
            self.measure_interface(&interface, Duration::ZERO);
            return;
        }
        match result {
            Ok(bps) => {
                speed.bps = Some(bps);
                for e in self.links.values_mut() {
                    if e.link.name == interface && e.link.state == LinkState::Up {
                        e.link.down_bps = Some(bps);
                    }
                }
                self.changed = true;
            }
            Err(err) => {
                tracing::warn!(%interface, error = %format!("{err:#}"), "speed test failed");
                // Retry after a pause, through an address not tried yet if
                // one is up: an IPv6 route can fail where IPv4 works.
                self.measure_interface(&interface, self.config.retry);
            }
        }
    }

    /// Start `interface`'s speed test after `wait`, through the first of its
    /// addresses that can take it, preferring one not tried yet.
    fn measure_interface(&mut self, interface: &str, wait: Duration) {
        let tried = self
            .speeds
            .get(interface)
            .map(|s| s.tried.clone())
            .unwrap_or_default();
        let (fresh, again): (Vec<LinkId>, Vec<LinkId>) = self
            .links
            .iter()
            .filter(|(_, e)| e.link.name == interface)
            .map(|(id, _)| *id)
            .partition(|id| !tried.contains(id));
        for id in fresh.into_iter().chain(again) {
            if self.measure_if_wanted(id, wait) {
                break;
            }
        }
    }

    /// Give an up link its interface's speed, or start the interface's test
    /// through it, after `wait`, if none is known and none is running.
    /// Returns whether a test started.
    fn measure_if_wanted(&mut self, id: LinkId, wait: Duration) -> bool {
        let Some(e) = self.links.get_mut(&id) else {
            return false;
        };
        if e.link.state != LinkState::Up {
            return false;
        }
        let speed = self.speeds.entry(e.link.name.clone()).or_default();
        if let Some(bps) = speed.bps {
            if e.link.down_bps != Some(bps) {
                e.link.down_bps = Some(bps);
                self.changed = true;
            }
            return false;
        }
        if speed.running.is_some() || speed.attempts >= MAX_SAMPLE_ATTEMPTS {
            return false;
        }
        let job = self.next_job;
        self.next_job += 1;
        speed.running = Some(job);
        speed.attempts += 1;
        speed.tried.insert(id);
        let interface = e.link.name.clone();
        let addr = e.link.addr.ip();
        self.start_sample(interface, job, addr, wait);
        true
    }

    /// Look up an up link's public address, unless it is known, being looked
    /// up, or has failed too often.
    fn look_up_if_wanted(&mut self, id: LinkId) {
        let Some(e) = self.links.get_mut(&id) else {
            return;
        };
        if e.link.state != LinkState::Up
            || e.link.public_ip.is_some()
            || e.lookup.is_some()
            || e.lookup_attempts >= MAX_LOOKUP_ATTEMPTS
        {
            return;
        }
        let job = self.next_job;
        self.next_job += 1;
        e.lookup = Some(job);
        e.lookup_attempts += 1;
        let addr = e.link.addr.ip();
        self.start_lookup(id, job, addr);
    }

    fn looked_up(&mut self, id: LinkId, job: u64, result: anyhow::Result<IpAddr>) {
        let Some(e) = self.links.get_mut(&id) else {
            return;
        };
        // Started before the network changed: an answer about the old one.
        if e.lookup != Some(job) {
            return;
        }
        e.lookup = None;
        match result {
            // Asked while the link worked, answered after it stopped: kept
            // off a dead link, and looked up again if it recovers.
            Ok(_) if e.link.state != LinkState::Up => {}
            Ok(public) => {
                e.link.public_ip = Some(public);
                // The tries are for a lookup that keeps failing; a link that
                // goes down and up again gets them afresh.
                e.lookup_attempts = 0;
                self.changed = true;
            }
            Err(err) => {
                tracing::debug!(link = %e.link.name, error = %format!("{err:#}"), "public address lookup failed");
            }
        }
    }

    fn start_probe(&mut self, id: LinkId, addr: IpAddr) {
        let net = Arc::clone(&self.net);
        let limit = self.config.probe_timeout;
        self.tasks.spawn(async move {
            // The watcher enforces the limit itself rather than trusting
            // every `Network` to honour the timeout it is given.
            let result = match tokio::time::timeout(limit, net.probe(addr, limit)).await {
                Ok(result) => result,
                Err(_) => Err(anyhow::anyhow!("no probe answer within {limit:?}")),
            };
            Report::Probed(id, result)
        });
    }

    /// Start a speed test through `addr` after `wait`. It counts as running
    /// while it waits, so nothing else starts meanwhile.
    fn start_sample(&mut self, interface: String, job: u64, addr: IpAddr, wait: Duration) {
        let net = Arc::clone(&self.net);
        let budget = self.config.sample_budget;
        self.tasks.spawn(async move {
            tokio::time::sleep(wait).await;
            let limit = budget + Duration::from_secs(1);
            let result = match tokio::time::timeout(limit, net.sample(addr, budget)).await {
                Ok(result) => result,
                Err(_) => Err(anyhow::anyhow!("no speed test result within {limit:?}")),
            };
            Report::Sampled {
                interface,
                job,
                result,
            }
        });
    }

    fn start_lookup(&mut self, id: LinkId, job: u64, addr: IpAddr) {
        let net = Arc::clone(&self.net);
        self.tasks.spawn(async move {
            let limit = LOOKUP_TIMEOUT;
            let result = match tokio::time::timeout(limit, net.public_ip(addr, limit)).await {
                Ok(result) => result,
                Err(_) => Err(anyhow::anyhow!("no public address within {limit:?}")),
            };
            Report::LookedUp { id, job, result }
        });
    }

    /// Queue an event. `run` sends it once the table showing it is published.
    fn emit(&mut self, event: LinkEvent) {
        self.pending.push(event);
    }
}

/// Each interface's IPv6 networks, as the set of its addresses' /64
/// prefixes. An interface with no IPv6 address maps to an empty set, so IPv6
/// arriving or leaving shows as a change. A rotating privacy address keeps
/// its prefix, so it does not.
fn ipv6_networks(candidates: &[Candidate]) -> HashMap<String, BTreeSet<u64>> {
    let mut networks: HashMap<String, BTreeSet<u64>> = HashMap::new();
    for c in candidates {
        let prefixes = networks.entry(c.name.clone()).or_default();
        if let IpAddr::V6(v6) = c.addr.ip() {
            prefixes.insert((v6.to_bits() >> 64) as u64);
        }
    }
    networks
}

fn is_same_link(candidate: &Candidate, link: &Link) -> bool {
    candidate.name == link.name && candidate.addr == link.addr
}

fn event_for(state: LinkState, id: LinkId) -> Option<LinkEvent> {
    match state {
        LinkState::Up => Some(LinkEvent::Up(id)),
        LinkState::Degraded => Some(LinkEvent::Degraded(id)),
        LinkState::Portal => Some(LinkEvent::Portal(id)),
        LinkState::Down => Some(LinkEvent::Down(id)),
        LinkState::Probing => None,
    }
}
