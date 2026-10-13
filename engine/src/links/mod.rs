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
//! | `upstream` | which links share one internet connection | no |
//! | `handle` | [`LinkHandle`], what the rest of the engine holds | through `watch` |
//!
//! Plus `bind`, which every I/O module above uses to get an HTTP client whose
//! traffic really leaves through one chosen link.
//!
//! `model` (task 1), `filter` (task 2), `enumerate` (task 3), `probe` (tasks 4
//! and 5), `bind` and `measure` (task 6), `watch` (task 7), `handle`
//! (task 8) and `upstream` (task 9) exist so far.

pub(crate) mod bind;
pub mod enumerate;
pub mod filter;
pub mod handle;
pub mod measure;
pub mod model;
pub mod probe;
pub mod upstream;
pub mod watch;

pub use enumerate::enumerate;
pub use filter::usable;
pub use handle::LinkHandle;
pub use measure::sample;
pub use model::{Addr, Candidate, Link, LinkEvent, LinkId, LinkState};
pub use probe::{Probe, ProbeVerdict, classify, probe};
pub use upstream::shared_upstreams;
