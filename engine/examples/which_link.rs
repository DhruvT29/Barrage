//! Does binding to a link's address really send traffic out of that link?
//!
//! Task 5's real acceptance test, and the core mechanic of the whole project.
//! For every usable IPv4 link: probe the internet through it, and ask what
//! public address the far end sees. Separate links must report separate
//! public addresses. If two report the same one, either the bind is being
//! ignored and the OS route table is choosing the link, or both links reach
//! the internet through one shared connection. Either way they are not two
//! paths.
//!
//! Run: cargo run --example which_link

use std::collections::HashSet;
use std::time::Duration;

use barrage::links::probe::{probe, public_ip_via};
use barrage::links::{enumerate, usable};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let links: Vec<_> = usable(&enumerate()?)
        .into_iter()
        .filter(|c| c.addr.ip().is_ipv4())
        .collect();

    println!(
        "{:<14} {:<16} {:<8} {:>7}  public address",
        "link", "local address", "probe", "rtt"
    );
    let mut publics = Vec::new();
    for link in &links {
        let local = link.addr.ip();
        let (verdict, rtt) = match probe(local, Duration::from_secs(5)).await {
            Ok(p) => (
                format!("{:?}", p.verdict),
                format!("{} ms", p.rtt.as_millis()),
            ),
            Err(e) => (format!("error: {e:#}"), "-".to_string()),
        };
        let public = match public_ip_via(local, Duration::from_secs(10)).await {
            Ok(ip) => {
                publics.push(ip);
                ip.to_string()
            }
            Err(e) => format!("error: {e:#}"),
        };
        println!(
            "{:<14} {:<16} {:<8} {:>7}  {public}",
            link.name,
            local.to_string(),
            verdict,
            rtt
        );
    }

    let distinct: HashSet<_> = publics.iter().collect();
    println!();
    if publics.len() < 2 {
        println!(
            "INCONCLUSIVE: {} link(s) reported a public address. Two are needed to prove anything.",
            publics.len()
        );
    } else if distinct.len() == publics.len() {
        println!(
            "PASS: {} links, {} different public addresses. Binding picks the link.",
            publics.len(),
            distinct.len()
        );
    } else {
        println!(
            "FAIL: links share a public address, so they are not separate paths. \
             Either binding is ignored, or they share one upstream connection."
        );
    }
    Ok(())
}
