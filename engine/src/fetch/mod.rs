//! Downloads: one file fetched through every working link at once.
//!
//! Each link's workers pull the next chunk from a shared pile, fetch it with
//! a `Range` request through their own address, and write it at its place in
//! the file, so a faster link simply fetches more. This module grows through
//! phase 3. So far it holds what decides which bytes to fetch: [`Ranges`],
//! the bytes done, and [`plan`], the chunks still to fetch; and what one link
//! sees at the URL: [`find`], with the file's name made safe by [`file_name`]
//! and kept from any name already taken by [`free_name`].

pub mod chunk;
pub mod name;
pub mod ranges;
pub mod remote;

pub use chunk::plan;
pub use name::{file_name, free_name};
pub use ranges::Ranges;
pub use remote::{MAX_REDIRECTS, PROBE_BYTES, Remote, Validator, find};
