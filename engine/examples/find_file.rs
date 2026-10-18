//! Does every link see the same file at one URL?
//!
//! Phase 3 Task 4's real check. For every usable link, IPv4 and IPv6: find
//! the file through that link, following its redirects by hand, and show
//! where it ended up and what the server said about the file. Every link
//! that finds it should report the same length. A link whose host has no
//! address of the link's family is refused, which is `bind` doing its job.
//!
//! Each link costs one ranged request for 1 KB per hop, plus the TLS
//! handshakes: a few KB, on mobile data for a tethered phone.
//!
//! Run: cargo run --example find_file -- <url>

use std::collections::HashSet;
use std::time::Duration;

use anyhow::Context;
use barrage::fetch::{Validator, find};
use barrage::links::{enumerate, usable};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let url: reqwest::Url = std::env::args()
        .nth(1)
        .context("usage: cargo run --example find_file -- <url>")?
        .parse()?;

    println!(
        "{:<14} {:<5} {:<30} {:>12} {:<6} {:<28} name",
        "link", "", "ended at", "length", "ranges", "validator"
    );
    let mut lengths = Vec::new();
    for link in usable(&enumerate()?) {
        let local = link.addr.ip();
        let family = if local.is_ipv4() { "IPv4" } else { "IPv6" };
        let found = tokio::time::timeout(Duration::from_secs(20), find(local, &url)).await;
        match found {
            Ok(Ok(remote)) => {
                lengths.push(remote.length);
                let validator = match &remote.validator {
                    Some(Validator::ETag(tag)) => format!("ETag {tag}"),
                    Some(Validator::LastModified(date)) => format!("date {date}"),
                    None => "none".to_string(),
                };
                let length = remote
                    .length
                    .map_or("unknown".to_string(), |l| l.to_string());
                println!(
                    "{:<14} {family:<5} {:<30} {length:>12} {:<6} {validator:<28} {}",
                    link.name,
                    remote.url.host_str().unwrap_or("?"),
                    remote.ranges,
                    remote.name
                );
            }
            Ok(Err(e)) => println!("{:<14} {family:<5} error: {e:#}", link.name),
            Err(_) => println!("{:<14} {family:<5} no answer within 20 s", link.name),
        }
    }

    let distinct: HashSet<_> = lengths.iter().collect();
    println!();
    match (lengths.len(), distinct.len()) {
        (0 | 1, _) => println!(
            "INCONCLUSIVE: {} link(s) found the file. Two are needed to compare.",
            lengths.len()
        ),
        (n, 1) => println!("SAME: all {n} links that found the file report the same length."),
        (n, _) => println!(
            "DIFFERENT: {n} links found the file, with {} lengths.",
            distinct.len()
        ),
    }
    Ok(())
}
