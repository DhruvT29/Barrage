//! BARRAGE engine: BitTorrent And Range Retrieval Across Grouped Egress.
//!
//! One download is split across every usable network link at once, so the
//! achievable rate is the sum of the links rather than the best one. Phase 0
//! measured 291.9 Mbps across a Wi-Fi link and a USB-tethered phone where the
//! better single link managed 173.7, with both links holding most of their
//! solo rate simultaneously.
//!
//! Module layout follows one rule: **code that touches the OS or the network
//! is kept apart from code that only decides things.** Decisions live in pure
//! functions that can be tested without summoning a fake network adapter,
//! which is most of what this phase needs to get right.

pub mod links;

pub use links::LinkHandle;
