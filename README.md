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

`barrage` watches this machine's internet links and opens the dashboard at
http://127.0.0.1:7700, reachable from this machine only. `--port N` picks
another port, `--no-open` skips the browser, `--log FILE` records every change.
`cargo run --bin brg-links` shows the same links as a table in the terminal.

The dashboard shows the combined speed available across the top, a card per
connection (Wi-Fi, a USB-tethered phone, Ethernet) with its state, delay,
measured speed and addresses, a notice when two connections reach the internet
through the same one, and every change as it happens. Ctrl+C stops `barrage`;
the open page says "disconnected, retrying", keeps the last table, and comes
back by itself when `barrage` runs again (`--no-open` avoids a second tab).

Working on the page: run `barrage`, then `pnpm --dir ui dev`, which serves the
page with live reload and passes `/api` to the engine. `pnpm --dir ui test`
runs the page's tests, `cargo test` (in `engine`) the engine's.
