//! `barrage`: the engine and its web page. Phase 2's deliverable.
//!
//! ```text
//! barrage [--port N] [--no-open] [--log FILE] [--static DIR]
//! ```
//!
//! Starts the link watcher, serves the page and its API at this machine's
//! loopback address only (port 7700 unless `--port` says otherwise), and
//! opens the browser on it. Runs until Ctrl+C. With `--log`, every change to
//! the links goes to FILE with a UTC timestamp, as `brg-links --log` writes
//! it, together with the engine's own notes.
//!
//! The page is read from `engine/static`, where `pnpm --dir ui build` puts
//! it, or from `--static DIR`. Until it is built, the browser gets a note
//! saying how.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use anyhow::Context as _;
use barrage::LinkHandle;
use barrage::api::{DEFAULT_PORT, bind, serve};
use barrage::links::render::changes;
use barrage::links::watch::LinkTable;

const USAGE: &str = "usage: barrage [--port N] [--no-open] [--log FILE] [--static DIR]";

/// Where the built page is unless `--static` says otherwise: `engine/static`,
/// fixed when this program is compiled.
const STATIC_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/static");

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("barrage: {e:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> anyhow::Result<()> {
    let options = match parse_args()? {
        Args::Help => {
            println!("{USAGE}");
            return Ok(());
        }
        Args::Run(options) => options,
    };
    if let Some(path) = &options.log {
        barrage::logging::to_file(path, &[])?;
    }
    // The port first: a taken one stops here, before any probe or speed test
    // has spent data.
    let listener = bind(options.port).await?;
    let handle = LinkHandle::start(Duration::from_secs(1))?;
    tracing::info!("started");
    tokio::spawn(log_changes(handle.table()));

    let url = format!("http://{}", listener.local_addr()?);
    println!("BARRAGE on {url}. Ctrl+C to stop.");
    if !options.static_dir.join("index.html").is_file() {
        println!("The page is not built yet: run `pnpm --dir ui build`, then reload it.");
    }
    if options.open
        && let Err(e) = open::that(&url)
    {
        println!("Could not open the browser ({e}). Open {url} yourself.");
    }

    serve(listener, handle, options.static_dir, async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await?;
    tracing::info!("stopped");
    println!("stopped.");
    Ok(())
}

/// Log each change to the links, as `brg-links --log` does. Nothing is
/// written without `--log`.
async fn log_changes(mut table: LinkTable) {
    let mut shown = table.latest();
    while table.changed().await {
        let now = table.latest();
        for line in changes(&shown, &now) {
            tracing::info!("{line}");
        }
        shown = now;
    }
}

enum Args {
    Help,
    Run(Options),
}

struct Options {
    port: u16,
    open: bool,
    log: Option<PathBuf>,
    static_dir: PathBuf,
}

fn parse_args() -> anyhow::Result<Args> {
    let mut options = Options {
        port: DEFAULT_PORT,
        open: true,
        log: None,
        static_dir: PathBuf::from(STATIC_DIR),
    };
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--port") => {
                let port = args.next().context("--port needs a number")?;
                options.port = port
                    .to_str()
                    .and_then(|p| p.parse().ok())
                    .with_context(|| format!("--port needs a number up to 65535, not {port:?}"))?;
            }
            Some("--no-open") => options.open = false,
            Some("--log") => {
                let path = args.next().context("--log needs a file name")?;
                options.log = Some(PathBuf::from(path));
            }
            Some("--static") => {
                let dir = args.next().context("--static needs a folder")?;
                options.static_dir = PathBuf::from(dir);
            }
            Some("-h" | "--help") => return Ok(Args::Help),
            _ => anyhow::bail!("unexpected argument {arg:?}\n{USAGE}"),
        }
    }
    Ok(Args::Run(options))
}
