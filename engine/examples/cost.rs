//! What Windows says each connection costs, and which adapter it is on.
//!
//! Task 6's real check: whether "Set as metered connection" reaches the
//! engine as `Fixed`, and what the engine makes of it. Each connection is
//! shown by its adapter's name, matched by GUID as the engine matches it; no
//! network name or address is printed, so the output can be pasted anywhere.
//!
//! Run: cargo run --example cost

use barrage::links::enumerate::{enumerate, profiles};

fn main() -> anyhow::Result<()> {
    let interfaces = if_addrs::get_if_addrs()?;
    println!(
        "{:<28} {:<13} {:<8} over limit",
        "adapter", "cost", "roaming"
    );
    for p in profiles()? {
        let id = p.adapter.trim_matches(['{', '}']);
        let name = interfaces
            .iter()
            .find(|i| {
                adapter_name(i)
                    .trim_matches(['{', '}'])
                    .eq_ignore_ascii_case(id)
            })
            .map_or("(no address)", |i| i.name.as_str());
        println!(
            "{:<28} {:<13} {:<8} {}",
            name,
            format!("{:?}", p.cost),
            p.roaming,
            p.over_limit
        );
    }

    println!("\nlinks as the engine sees them:");
    let mut seen = Vec::new();
    for c in enumerate()? {
        if !seen.contains(&c.name) {
            println!("{:<28} metered: {}", c.name, c.metered);
            seen.push(c.name);
        }
    }
    Ok(())
}

#[cfg(windows)]
fn adapter_name(i: &if_addrs::Interface) -> &str {
    &i.adapter_name
}

#[cfg(not(windows))]
fn adapter_name(_: &if_addrs::Interface) -> &str {
    ""
}
