//! A log file for the programs: the engine's own notes (why a probe failed,
//! when a network changed) and each program's lines, with UTC timestamps.
//! That file, not a screen, is what a test run is checked against.

use std::path::Path;
use std::sync::Mutex;

use anyhow::Context as _;
use tracing::Level;
use tracing_subscriber::filter::Targets;
use tracing_subscriber::prelude::*;

/// Send the engine's notes, at debug level, and the lines of each target in
/// `programs`, at info level, to `path`. Appends, so a second run does not
/// erase the first. Once per process.
pub fn to_file(path: &Path, programs: &[&str]) -> anyhow::Result<()> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("cannot open the log file {}", path.display()))?;
    let targets = programs.iter().fold(
        Targets::new().with_target("barrage", Level::DEBUG),
        |targets, program| targets.with_target(*program, Level::INFO),
    );
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(Mutex::new(file)),
        )
        .with(targets)
        .try_init()
        .context("a log is already set up for this program")
}
