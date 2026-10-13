//! `LinkHandle`: the one thing the rest of the engine holds to know which
//! links exist and which work.
//!
//! Starting a handle starts the watcher; dropping it stops the watcher. In
//! between, three questions, none of which ever waits on the network:
//!
//! - [`LinkHandle::snapshot`]: every link the watcher knows, whatever its state
//! - [`LinkHandle::usable`]: only the links that work right now
//! - [`LinkHandle::subscribe`]: be told as links come, go and change
//! - [`LinkHandle::shared_upstreams`]: which links are really one internet
//!
//! To follow the links without missing anything, subscribe first and then
//! take a snapshot. A change in between then shows up in the snapshot and as
//! an event, so at worst it is seen twice, never missed. The watcher updates
//! the table before it sends the event, so whoever hears `Up(id)` finds that
//! link `Up` in the snapshot.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

use super::model::{Link, LinkEvent, LinkId, LinkState};
use super::watch::{LinkTable, Network, SystemNetwork, WatchConfig, spawn_watcher};

/// A running watcher, and the way to ask it about links.
///
/// Not `Clone`: dropping it stops the watcher, so there is exactly one owner.
/// Share it behind an `Arc`.
#[derive(Debug)]
pub struct LinkHandle {
    watcher: JoinHandle<()>,
    table: LinkTable,
    /// Never read. Kept so `subscribe` has something to copy, and so the
    /// handle holds no sending side: when the watcher stops, every subscriber
    /// learns it, instead of waiting forever.
    events: broadcast::Receiver<LinkEvent>,
}

impl LinkHandle {
    /// Watch this machine's links, reading the interface list every `poll`.
    ///
    /// Reads the list once before starting, so a machine whose interfaces
    /// cannot be read fails here, with a reason, rather than showing an empty
    /// table forever.
    pub fn start(poll: Duration) -> anyhow::Result<Self> {
        super::enumerate::enumerate().context("cannot read this machine's network interfaces")?;
        Self::start_with(Arc::new(SystemNetwork), WatchConfig::every(poll))
    }

    /// Watch the links of any [`Network`]. Tests use a scripted one.
    pub fn start_with<N: Network>(net: Arc<N>, config: WatchConfig) -> anyhow::Result<Self> {
        // Without this check, starting outside a runtime panics inside tokio.
        tokio::runtime::Handle::try_current()
            .context("the link watcher needs a tokio runtime to run in")?;
        let (watcher, events, table) = spawn_watcher(net, config);
        Ok(Self {
            watcher,
            table,
            events,
        })
    }

    /// Every link the watcher knows, in the order they appeared, whatever
    /// their state. Copies the table and returns at once.
    pub fn snapshot(&self) -> Vec<Link> {
        self.table.snapshot()
    }

    /// The links that work right now: `Up` only. A portal, a dead link and
    /// a link still being probed are left out.
    ///
    /// Not the same question as [`super::filter::usable`], which picks the
    /// interface addresses worth probing at all.
    pub fn usable(&self) -> Vec<Link> {
        let mut links = self.snapshot();
        links.retain(|l| l.state == LinkState::Up);
        links
    }

    /// Groups of working links on different cables that reach the internet
    /// through one connection, so using them together gains nothing. See
    /// [`super::upstream::shared_upstreams`].
    pub fn shared_upstreams(&self) -> Vec<Vec<LinkId>> {
        super::upstream::shared_upstreams(&self.snapshot())
    }

    /// Events from now on. Each subscriber gets every event, and returns
    /// `Closed` once the watcher has stopped. One that falls more than
    /// [`EVENT_CAPACITY`](super::watch::EVENT_CAPACITY) events behind is told
    /// it lagged and should take a
    /// fresh snapshot.
    pub fn subscribe(&self) -> broadcast::Receiver<LinkEvent> {
        self.events.resubscribe()
    }
}

impl Drop for LinkHandle {
    fn drop(&mut self) {
        self.watcher.abort();
    }
}
