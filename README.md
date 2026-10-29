# BARRAGE - **B**itTorrent **A**nd **R**ange **R**etrieval **A**cross **G**rouped **E**gress

A Multi Device Internet Pooling System for Torrent Download

## Run it

You need Rust (`cargo`) and Node with pnpm.

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
addresses, and a notice when two connections reach the internet through the
same one. **Downloads** takes a link to a file, which then comes down through
every working connection at once; each download shows its progress, its speed
and what each connection carried. A tethered phone's part comes out of its
mobile data. Stop on a download saves what it has; Resume, or adding its link
again, carries on from there. Downloaded files carry the mark Windows gives a
browser's downloads, so SmartScreen and Office treat them as from the
internet. Across the top, both tabs show the combined speed available, or
while anything downloads the live download speed; beside them, every change as
it happens. Ctrl+C stops `barrage`; the open page says "disconnected,
retrying", keeps what it last showed, and comes back by itself when `barrage`
runs again (`--no-open` avoids a second tab). A download cut short, by Ctrl+C
or by `barrage` being closed or crashing, picks up where it was when its link
is added again, as long as the server's file is still the same version; its
progress is kept beside it in `<name>.part.json`.

Working on the page: run `barrage`, then `pnpm --dir ui dev`, which serves the
page with live reload and passes `/api` to the engine. `pnpm --dir ui test`
runs the page's tests, `cargo test` (in `engine`) the engine's.
