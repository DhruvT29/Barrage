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
//! `model` (task 1) and `filter` (task 2) exist so far.

pub mod filter;
pub mod model;

pub use filter::usable;
pub use model::{Addr, Candidate, Link, LinkEvent, LinkId, LinkState};
