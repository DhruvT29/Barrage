//! `brg-links`: every internet link on this machine, live. Phase 1's
//! deliverable.
//!
//! ```text
//! brg-links [--log FILE]
//! ```
//!
//! Redraws the table in place until Ctrl+C, then leaves the last one on
//! screen. With `--log`, every change goes to FILE with a UTC timestamp,
//! together with the watcher's own notes (why a probe failed, when a network
//! changed). That file, not the screen, is what a test is checked against.
//!
//! When the output is not a terminal (piped or redirected), there is no
//! redrawing: a fresh table is printed each time the table changes.

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::Context as _;
use barrage::LinkHandle;
use barrage::links::render::{changes, render};
use barrage::links::{Link, LinkState};
use tracing_subscriber::filter::Targets;
use tracing_subscriber::prelude::*;

const USAGE: &str = "usage: brg-links [--log FILE]";

/// How often the table is checked for changes: well under the watcher's 1 s
/// poll, so the screen adds little to "a new link shows within 2 s".
const REDRAW: Duration = Duration::from_millis(250);

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("brg-links: {e:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> anyhow::Result<()> {
    let log = match parse_args()? {
        Args::Help => {
            println!("{USAGE}");
            return Ok(());
        }
        Args::Run { log } => log,
    };
    if let Some(path) = &log {
        start_log(path)?;
    }
    let handle = LinkHandle::start(Duration::from_secs(1))?;
    tracing::info!("started");

    let mut screen = Screen::open();
    let started = tokio::time::Instant::now();
    let mut ticker = tokio::time::interval(REDRAW);
    let mut stop = std::pin::pin!(tokio::signal::ctrl_c());
    let mut shown: Vec<Link> = Vec::new();
    loop {
        tokio::select! {
            _ = &mut stop => break,
            _ = ticker.tick() => {}
        }
        let links = handle.snapshot();
        for line in changes(&shown, &links) {
            tracing::info!("{line}");
        }
        screen.show(&status(started.elapsed(), &links), &render(&links))?;
        shown = links;
    }
    tracing::info!("stopped");
    screen.close();
    Ok(())
}

enum Args {
    Help,
    Run { log: Option<PathBuf> },
}

fn parse_args() -> anyhow::Result<Args> {
    let mut log = None;
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--log") => {
                let path = args.next().context("--log needs a file name")?;
                log = Some(PathBuf::from(path));
            }
            Some("-h" | "--help") => return Ok(Args::Help),
            _ => anyhow::bail!("unexpected argument {arg:?}\n{USAGE}"),
        }
    }
    Ok(Args::Run { log })
}

/// Send the watcher's notes and this program's change lines to `path`,
/// appending, so a second run does not erase the first.
fn start_log(path: &Path) -> anyhow::Result<()> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("cannot open the log file {}", path.display()))?;
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(Mutex::new(file)),
        )
        .with(
            Targets::new()
                .with_target("barrage", tracing::Level::DEBUG)
                .with_target("brg_links", tracing::Level::INFO),
        )
        .init();
    Ok(())
}

/// The line above the table. The running clock shows the program is alive
/// even when nothing changes, and when nothing is connected at all.
fn status(running: Duration, links: &[Link]) -> String {
    let secs = running.as_secs();
    let clock = if secs < 60 {
        format!("{secs}s")
    } else {
        format!("{}m {:02}s", secs / 60, secs % 60)
    };
    let working = links.iter().filter(|l| l.state == LinkState::Up).count();
    let plural = if links.len() == 1 { "" } else { "s" };
    format!(
        "brg-links: watching for {clock}. {} link{plural}, {working} working. Ctrl+C to stop.",
        links.len()
    )
}

/// Where the table goes: redrawn in place on a terminal, printed afresh on
/// each change anywhere else.
struct Screen {
    live: bool,
    last: String,
    /// The console mode to put back on exit, if it was changed.
    #[cfg(windows)]
    restore: Option<u32>,
}

impl Screen {
    fn open() -> Self {
        let terminal = std::io::stdout().is_terminal();
        #[cfg(windows)]
        let (live, restore) = {
            let restore = if terminal {
                console::allow_redraw()
            } else {
                None
            };
            (restore.is_some(), restore)
        };
        #[cfg(not(windows))]
        let live = terminal;
        if live {
            // Clear once; later frames overwrite in place.
            print!("\x1b[2J");
        }
        Self {
            live,
            last: String::new(),
            #[cfg(windows)]
            restore,
        }
    }

    fn show(&mut self, status: &str, table: &str) -> std::io::Result<()> {
        let mut out = std::io::stdout().lock();
        if self.live {
            let frame = format!("{status}\n\n{table}");
            if frame == self.last {
                return Ok(());
            }
            // Cursor home, each line cleared to its end as it is rewritten,
            // then the rest of the screen: no blank flash between frames.
            write!(out, "\x1b[H{}\x1b[J", frame.replace('\n', "\x1b[K\n"))?;
            self.last = frame;
        } else {
            if table == self.last {
                return Ok(());
            }
            writeln!(out, "{status}\n\n{table}")?;
            self.last = table.to_string();
        }
        out.flush()
    }

    fn close(self) {
        println!("\nstopped.");
        #[cfg(windows)]
        if let Some(mode) = self.restore {
            console::set_mode(mode);
        }
    }
}

/// A Windows console moves the cursor on the escape codes the redraw uses
/// only once asked to, and that setting outlives this program, so it is put
/// back on exit.
#[cfg(windows)]
mod console {
    use windows_sys::Win32::System::Console::{
        CONSOLE_MODE, ENABLE_VIRTUAL_TERMINAL_PROCESSING, GetConsoleMode, GetStdHandle,
        STD_OUTPUT_HANDLE, SetConsoleMode,
    };

    /// Turn on escape code handling for standard output. Returns the mode to
    /// restore later, or `None` when the console refuses.
    pub fn allow_redraw() -> Option<CONSOLE_MODE> {
        // SAFETY: Win32 console calls on this process's own standard output
        // handle; `mode` is a live local for GetConsoleMode to write into.
        unsafe {
            let out = GetStdHandle(STD_OUTPUT_HANDLE);
            let mut mode: CONSOLE_MODE = 0;
            if GetConsoleMode(out, &mut mode) == 0 {
                return None;
            }
            if SetConsoleMode(out, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING) == 0 {
                return None;
            }
            Some(mode)
        }
    }

    pub fn set_mode(mode: CONSOLE_MODE) {
        // SAFETY: as above.
        unsafe {
            SetConsoleMode(GetStdHandle(STD_OUTPUT_HANDLE), mode);
        }
    }
}
