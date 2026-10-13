//! Link Manager: discover every usable network link, keep its state current,
//! and hand the rest of the engine a stable handle per link.
//!
//! The module split follows one rule, because it is what makes this phase
//! testable: **the OS edge is separated from the decisions.**
//!
//! | module | role | touches the OS |
//! |---|---|---|
//! | `model` | the vocabulary, pure data | no |
//! | `filter` | which candidates deserve a probe | no |
//! | `probe::classify` | what a probe result means | no |
//! | `enumerate` | ask the OS for interfaces | yes |
//! | `probe::probe` | perform the bound request | yes |
//! | `measure` | throughput sampling | yes |
//! | `watch` | poll, diff, emit events | yes |
//!
//! Plus `bind`, which every I/O module above uses to get an HTTP client whose
//! traffic really leaves through one chosen link.
//!
//! `model` (task 1), `filter` (task 2), `enumerate` (task 3), `probe` (tasks 4
//! and 5), `bind` and `measure` (task 6) and `watch` (task 7) exist so far.

pub(crate) mod bind;
pub mod enumerate;
pub mod filter;
pub mod measure;
pub mod model;
pub mod probe;
pub mod watch;

pub use enumerate::enumerate;
pub use filter::usable;
pub use measure::sample;
pub use model::{Addr, Candidate, Link, LinkEvent, LinkId, LinkState};
pub use probe::{Probe, ProbeVerdict, classify, probe};
