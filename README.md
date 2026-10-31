# BARRAGE - **B**itTorrent **A**nd **R**ange **R**etrieval **A**cross **G**rouped **E**gress

A Multi Device Internet Pooling System for Torrent Download

## Run it

You need Rust (`cargo`) and Node with pnpm.

### Quick start: one command

From the repository's root, in PowerShell:

```
.\dev.ps1
```

It builds and starts the engine (which is also the backend: the API and the
live socket on port 7700), then the page with live reload, waits until each
answers, and opens http://127.0.0.1:5173 in the browser. Edits under
`ui/src` show at once.

| Command | What runs | Opens |
|---|---|---|
| `.\dev.ps1` | The engine, and the page with live reload: for working on the page | http://127.0.0.1:5173 |
| `.\dev.ps1 -App` | The page built and served by the engine itself, exactly as it ships | http://127.0.0.1:7700 |
| `.\dev.ps1 -Preview` | The page alone, fed made-up data: no engine, nothing touches the network | http://127.0.0.1:5173/preview.html |

Options, with any of the above:

| Option | Does |
|---|---|
| `-Release` | Builds the engine optimised: slower to build, faster downloads |
| `-Downloads DIR` | Where downloads go (the folder must exist); otherwise the Downloads folder |
| `-Log FILE` | The engine records every change in FILE |
| `-NoOpen` | Does not open the browser |

- **Ctrl+C stops everything.** The engine is asked to stop first, so it
  saves each download's progress and a cut-short download resumes later.
  If either part stops on its own, the other is stopped too.
- **The first run takes a few minutes:** it installs the page's packages and
  compiles the engine. Later runs only rebuild what changed, and the
  packages are installed again only when the lockfile changes.
- **The engine uses real connections** from the moment it starts: a
  tethered phone is speed-tested once (30 to 60 MB of its data). Use
  `-Preview` to look at the page without that.
- **A port already in use** (7700 or 5173) is reported, with what probably
  holds it (another `.\dev.ps1`, `barrage` or `pnpm dev`), instead of
  starting a second copy.

### By hand

The same steps without the script:

```
pnpm --dir ui install
pnpm --dir ui build
cd engine
cargo run --bin barrage
```

After pulling a new version, run `pnpm --dir ui build` again before starting
`barrage`: the engine serves the page as last built, and an old page cannot
read what a newer engine sends.

`barrage` watches this machine's internet links and opens the dashboard at
http://127.0.0.1:7700, reachable from this machine only. `--port N` picks
another port, `--no-open` skips the browser, `--log FILE` records every change,
`--downloads DIR` names the folder downloads go to (otherwise the Downloads
folder). `cargo run --bin brg-links` shows the same links as a table in the
terminal.

The dashboard has two tabs. **Links** shows a card per connection (Wi-Fi, a
USB-tethered phone, Ethernet) with its state, delay, measured speed and
addresses, a "metered" mark when Windows calls the connection metered, how
much BARRAGE has downloaded through it since it started (its speed tests
included: 30 to 60 MB each time a phone is measured), and a notice when two
connections reach the internet through the same one. **Downloads** takes a
link to a file, which then comes down through every working connection at
once; each download shows its progress, its speed and what each connection
carried, and while it runs, each connection's share of the speed and its
connections, with how many it is wanted to hold while that changes. A tethered
phone's part comes out of its mobile data, and the phone counts 10 to 15%
more than the card shows: when a connection's part is handed to another, what
was already on its way is thrown away unread, and on a phone's slower round
trip that adds up. The faster a connection delivers,
the more of a download's connections it holds. Two cables that reach the
internet through the same connection count as one: one carries and the other
sits out, and a cable Windows calls metered is the one that sits out. A
connection that drops and comes back holds a single connection until it has
kept working for 30 seconds. Stop on a download saves what it has; Resume, or
adding its link again, carries on from there. Downloaded files carry the mark
Windows gives a browser's downloads, so SmartScreen and Office treat them as
from the internet. Across the top, both tabs show the combined speed
available, or while anything downloads the live download speed; beside them,
every change as it happens. Ctrl+C stops `barrage`; the open page says
"disconnected, retrying", keeps what it last showed, and comes back by itself
when `barrage` runs again (`--no-open` avoids a second tab). A download cut
short, by Ctrl+C or by `barrage` being closed or crashing, picks up where it
was when its link is added again, as long as the server's file is still the
same version; its progress is kept beside it in `<name>.part.json`.

Working on the page: run `barrage`, then `pnpm --dir ui dev`, which serves the
page with live reload and passes `/api` to the engine. `pnpm --dir ui test`
runs the page's tests, `cargo test` (in `engine`) the engine's.
