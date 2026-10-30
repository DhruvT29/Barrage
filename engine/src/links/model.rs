//! The vocabulary of the Link Manager.
//!
//! Pure data and pure functions only. Nothing here touches the OS or the
//! network, so every type in this file can be constructed in a test without a
//! real network adapter in sight. That separation is the point: four of phase
//! 1's seven acceptance criteria are about decisions rather than I/O, and a
//! decision expressed as a pure function is cheap to test exhaustively.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Duration;

/// A link's identity for as long as it exists.
///
/// Deliberately **not** the OS interface index. Windows reuses interface
/// indices after an adapter disappears, so an index that meant "the phone"
/// can later mean something else entirely. Anything holding an index across a
/// hotplug event is holding a stale reference that still looks valid, which is
/// the kind of bug that surfaces as chunks being written to the wrong link
/// rather than as an error.
///
/// Minted by the watcher, which owns the link table, and opaque to everyone
/// else so that no caller can invent one or do arithmetic on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LinkId(u64);

impl LinkId {
    /// Only the watcher mints ids, hence `pub(crate)`. The constructor's
    /// visibility is the mechanism that keeps id minting in one place.
    pub(crate) fn new(raw: u64) -> Self {
        Self(raw)
    }

    /// For display and log correlation. Not for reconstructing a `LinkId`.
    pub fn as_u64(self) -> u64 {
        self.0
    }
}

/// A link's local address, which is also the thing we bind a socket to.
///
/// Both families are carried from the first commit. Phase 0 measured the
/// tether handing out IPv4 with its own IPv4 default route, so IPv4 is what
/// matters today, but an IPv6-only carrier is an open risk in PLAN.md section
/// 7 and widening this type later would touch every module that consumes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Addr {
    V4(Ipv4Addr),
    V6(Ipv6Addr),
}

impl Addr {
    pub fn ip(&self) -> IpAddr {
        match *self {
            Addr::V4(a) => IpAddr::V4(a),
            Addr::V6(a) => IpAddr::V6(a),
        }
    }

    /// True for an IPv4 link-local address, which on Windows means DHCP failed
    /// and the OS invented an address. Phase 0 found four such interfaces on
    /// this machine, every one of them without internet access.
    ///
    /// This is a property of the address, not a hardcoded range: it reads the
    /// standard library's own classification rather than naming a prefix, so
    /// the subnet guard in `tests/no_hardcoded_subnets.rs` stays satisfied.
    pub fn is_link_local(&self) -> bool {
        match *self {
            Addr::V4(a) => a.is_link_local(),
            Addr::V6(a) => a.is_unicast_link_local(),
        }
    }
}

impl From<IpAddr> for Addr {
    fn from(ip: IpAddr) -> Self {
        match ip {
            IpAddr::V4(a) => Addr::V4(a),
            IpAddr::V6(a) => Addr::V6(a),
        }
    }
}

/// An interface the OS told us about, before any judgement is applied.
///
/// "Has an address" is not "has internet". Phase 0 demonstrated that on this
/// exact machine: five interfaces held addresses and one had connectivity. A
/// `Candidate` is therefore only a nomination, and `filter.rs` plus `probe.rs`
/// decide which nominations become links.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// The OS interface index. Kept for talking to the OS, never used as an
    /// identity, for the reason given on [`LinkId`].
    pub os_index: u32,
    pub name: String,
    pub addr: Addr,
    /// Whether the OS reports the interface as up. Necessary but nowhere near
    /// sufficient.
    pub up: bool,
    /// Whether Windows says the connection the interface is on now is
    /// metered: its connection profile's cost (`enumerate`).
    pub metered: bool,
}

/// What we currently believe about a link.
///
/// `Portal` is separate from `Degraded` on purpose. A captive portal answers
/// requests quickly and successfully, so by throughput and latency it looks
/// like a healthy link, while every byte it returns is a login page. Treating
/// it as merely degraded would keep feeding it chunks forever.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LinkState {
    /// Seen, not yet judged. Never assigned work.
    Probing,
    /// Usable.
    Up,
    /// Reachable but intercepted. Unusable, and not a transient fault.
    Portal,
    /// Usable but performing badly enough to deserve less work.
    Degraded,
    /// Gone or unusable.
    Down,
}

impl LinkState {
    /// Whether the scheduler may assign chunks to a link in this state.
    pub fn is_usable(self) -> bool {
        matches!(self, LinkState::Up | LinkState::Degraded)
    }
}

/// A link, as the rest of the engine sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub id: LinkId,
    pub name: String,
    pub addr: Addr,
    pub state: LinkState,
    /// `None` until a probe has produced a figure. Absent and zero are
    /// different facts and the scheduler must not confuse them.
    pub rtt: Option<Duration>,
    /// Measured download rate in bits per second, `None` until measured.
    ///
    /// Bits per second because that is the unit links are specified in. Phase
    /// 0 is a warning against assuming magnitudes here: the plan predicted a
    /// 25 Mbps tether and measured roughly 170, and the tether turned out to
    /// be faster than the Wi-Fi.
    pub down_bps: Option<u64>,
    /// The address the internet sees this link's traffic come from, `None`
    /// until looked up. Two links on different cables showing the same one
    /// share one internet connection: on 2026-10-14 the phone, joined to the
    /// laptop's Wi-Fi, showed exactly the Wi-Fi's own. See
    /// [`super::upstream::shared_upstreams`].
    pub public_ip: Option<IpAddr>,
    /// Whether the OS reports this link as metered. Phase 0 burned about
    /// 2.5 GB of mobile data in six test runs, which is the whole argument for
    /// carrying this flag from the start.
    pub metered: bool,
}

/// A change in the link table, emitted by the watcher.
///
/// Transitions, not snapshots. A consumer that only needs to know "something
/// changed" can ignore the payload, while the scheduler needs to know exactly
/// which link so it can reassign that link's outstanding chunks and nothing
/// else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkEvent {
    Up(LinkId),
    Down(LinkId),
    Degraded(LinkId),
    Portal(LinkId),
}

impl LinkEvent {
    pub fn link_id(self) -> LinkId {
        match self {
            LinkEvent::Up(id)
            | LinkEvent::Down(id)
            | LinkEvent::Degraded(id)
            | LinkEvent::Portal(id) => id,
        }
    }
}
