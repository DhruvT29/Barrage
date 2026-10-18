//! Downloads: one file fetched through every working link at once.
//!
//! Each link's workers pull the next chunk from a shared pile, fetch it with
//! a `Range` request through their own address, and write it at its place in
//! the file, so a faster link simply fetches more. This module grows through
//! phase 3. So far it holds what decides which bytes to fetch: [`Ranges`],
//! the bytes done, and [`plan`], the chunks still to fetch.

pub mod chunk;
pub mod ranges;

pub use chunk::plan;
pub use ranges::Ranges;
