//! The API server, driven by the scripted network in `fake/mod.rs`: the JSON
//! routes, the live WebSocket, the page, the port, and who may use them.
//! Downloads come from the scripted file server in `serve/mod.rs`, so no
//! test here touches the internet.

mod fake;
mod serve;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use barrage::LinkHandle;
use barrage::api::live::Feed;
use barrage::api::{LiveMessage, app, bind, router, serve, table_view};
use barrage::fetch::{DownloadConfig, Downloads};
use barrage::links::LinkState;
use barrage::links::watch::{LinkTable, WatchConfig};
use fake::{Fake, SAMPLED_BPS, cand, cand_idx, ms};
use futures_util::StreamExt;
use serde_json::{Value, json};
use serve::{Answer, File, Server, content};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::{Instant, timeout};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::{self, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

fn start(net: Fake) -> LinkHandle {
    LinkHandle::start_with(Arc::new(net), WatchConfig::every(ms(50))).unwrap()
}

/// Wi-Fi at 127.0.0.1, where the scripted file server listens, kept as a
/// usable link although it is loopback.
fn wifi_to_the_file_server() -> LinkHandle {
    let wifi = cand_idx(1, "Wi-Fi", "127.0.0.1", true);
    let net = Fake::scripted(vec![vec![wifi]]).loopback_links();
    LinkHandle::start_with(Arc::new(net), WatchConfig::every(ms(20))).unwrap()
}

/// Downloads into `dir`, with short times so a test takes a second.
fn downloads_in(table: &LinkTable, dir: &Path) -> Downloads {
    let config = DownloadConfig {
        read_deadline: Duration::from_secs(5),
        find_deadline: Duration::from_secs(5),
        publish_every: ms(20),
        speed_window: ms(200),
        ..DownloadConfig::default()
    };
    Downloads::with_config(table.clone(), dir.to_path_buf(), config)
}

/// Serve the API for `table` on a free loopback port. Nothing is
/// downloaded into the folder: a test that downloads uses [`serve_with`].
async fn serve_api(table: LinkTable) -> SocketAddr {
    let downloads = downloads_in(&table, &std::env::temp_dir());
    serve_with(table, downloads).await
}

async fn serve_with(table: LinkTable, downloads: Downloads) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router(table, downloads))
            .await
            .unwrap()
    });
    addr
}

async fn connect(addr: SocketAddr) -> Socket {
    let (socket, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/api/live"))
        .await
        .unwrap();
    socket
}

/// The next message the server sends, as JSON, failing the test after 3 s.
async fn frame(socket: &mut Socket) -> Value {
    loop {
        let next = timeout(Duration::from_secs(3), socket.next())
            .await
            .expect("no frame within 3 s");
        match next.expect("the stream ended").expect("socket error") {
            Message::Text(text) => return serde_json::from_str(&text).unwrap(),
            Message::Ping(_) | Message::Pong(_) => continue,
            other => panic!("unexpected frame {other:?}"),
        }
    }
}

/// Frames until one passes `check`, failing the test after 10 s.
async fn frame_where(socket: &mut Socket, what: &str, check: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        assert!(Instant::now() < deadline, "waited 10 s for {what}");
        let frame = frame(socket).await;
        if check(&frame) {
            return frame;
        }
    }
}

/// What every client is sent first: the table, then the downloads.
async fn opening(socket: &mut Socket) -> (Value, Value) {
    let table = frame(socket).await;
    assert_eq!(table["type"], "table", "{table}");
    let downloads = frame(socket).await;
    assert_eq!(downloads["type"], "downloads", "{downloads}");
    (table, downloads)
}

async fn wait_until(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !check() {
        assert!(Instant::now() < deadline, "waited 3 s for {what}");
        tokio::time::sleep(ms(10)).await;
    }
}

fn lines(frame: &Value) -> Vec<String> {
    frame["lines"]
        .as_array()
        .expect("a change carries lines")
        .iter()
        .map(|l| l.as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn links_returns_the_table_as_json() {
    let h = start(Fake::scripted(vec![vec![cand(
        "Wi-Fi",
        "192.0.2.10",
        true,
    )]]));
    wait_until("Wi-Fi up", || {
        h.snapshot()
            .first()
            .is_some_and(|l| l.state == LinkState::Up)
    })
    .await;
    let addr = serve_api(h.table()).await;

    let response = reqwest::get(format!("http://{addr}/api/links"))
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let kind = response.headers()["content-type"].to_str().unwrap();
    assert!(kind.starts_with("application/json"), "{kind}");
    let json: Value = serde_json::from_str(&response.text().await.unwrap()).unwrap();
    assert_eq!(json["links"][0]["name"], "Wi-Fi");
    assert_eq!(json["links"][0]["state"], "up");
}

/// The script holds no links for a second, so the client is connected and
/// has its first table before Wi-Fi appears.
#[tokio::test]
async fn live_sends_the_table_then_the_downloads_then_each_change() {
    let mut steps = vec![vec![]; 20];
    steps.push(vec![cand("Wi-Fi", "192.0.2.10", true)]);
    let h = start(Fake::scripted(steps));
    let addr = serve_api(h.table()).await;
    let mut socket = connect(addr).await;

    let (table, downloads) = opening(&mut socket).await;
    assert_eq!(table["table"]["links"], json!([]));
    assert_eq!(
        downloads["downloads"],
        json!({ "live": { "bps": 0, "parts": [] }, "list": [] })
    );

    let change = frame(&mut socket).await;
    assert_eq!(change["type"], "change");
    assert_eq!(change["table"]["links"][0]["name"], "Wi-Fi");
    let lines = lines(&change);
    assert!(
        lines[0].starts_with("appeared: Wi-Fi 192.0.2.10"),
        "{lines:?}"
    );
}

/// A speed arriving changes the table but sends no `LinkEvent`, so a feed
/// built on events would never show it. This one follows the table.
#[tokio::test]
async fn a_speed_arriving_is_sent_though_no_event_fires() {
    let mut steps = vec![vec![]; 10];
    steps.push(vec![cand("Wi-Fi", "192.0.2.10", true)]);
    let h = start(Fake::scripted(steps).slow_samples(ms(300)));
    let addr = serve_api(h.table()).await;
    let mut socket = connect(addr).await;
    opening(&mut socket).await;

    let speed = frame_where(&mut socket, "the speed", |f| {
        f["table"]["links"][0]["down_bps"] == SAMPLED_BPS
    })
    .await;
    assert_eq!(speed["type"], "change");
    assert!(
        lines(&speed).contains(&"Wi-Fi 192.0.2.10: speed 40.0 Mbps".to_string()),
        "{speed}"
    );
}

/// Review Focus 2. Wi-Fi appears a second in, after one of the two tabs has
/// gone.
#[tokio::test]
async fn closing_one_client_leaves_the_other_streaming() {
    let mut steps = vec![vec![]; 20];
    steps.push(vec![cand("Wi-Fi", "192.0.2.10", true)]);
    let h = start(Fake::scripted(steps));
    let addr = serve_api(h.table()).await;
    let mut leaving = connect(addr).await;
    let mut staying = connect(addr).await;
    opening(&mut leaving).await;
    opening(&mut staying).await;

    leaving.close(None).await.unwrap();
    drop(leaving);

    let change = frame(&mut staying).await;
    assert_eq!(change["type"], "change");
    assert_eq!(change["table"]["links"][0]["name"], "Wi-Fi");
}

/// Review Focus 3. A client busy with one message (a tab in the background)
/// is not sent a queue of tables when it comes back: everything that
/// changed meanwhile arrives as one change, ending on the latest table, and
/// nothing is left waiting behind it.
#[tokio::test]
async fn a_client_that_reads_late_gets_the_latest_table_not_a_backlog() {
    let wifi = cand("Wi-Fi", "192.0.2.10", true);
    let tether = cand("Ethernet 2", "192.0.2.20", true);
    let h = start(Fake::scripted(vec![
        vec![],
        vec![wifi.clone()],
        vec![wifi, tether],
    ]));
    let downloads = downloads_in(&h.table(), &std::env::temp_dir());
    let mut feed = Feed::new(h.table(), &downloads);
    let Some(LiveMessage::Table { .. }) = feed.next().await else {
        panic!("expected the table first");
    };
    let Some(LiveMessage::Downloads { .. }) = feed.next().await else {
        panic!("expected the downloads next");
    };

    // Away while both links appear, come up, are measured and looked up.
    wait_until("both links fully known", || {
        let links = h.snapshot();
        links.len() == 2
            && links
                .iter()
                .all(|l| l.down_bps.is_some() && l.public_ip.is_some())
    })
    .await;

    let Some(LiveMessage::Change { table, lines }) = feed.next().await else {
        panic!("expected a change");
    };
    assert_eq!(table, table_view(&h.snapshot()), "not the latest table");
    assert!(
        lines.iter().any(|l| l.starts_with("appeared: Wi-Fi"))
            && lines.iter().any(|l| l.starts_with("appeared: Ethernet 2")),
        "{lines:?}"
    );
    assert!(
        timeout(ms(300), feed.next()).await.is_err(),
        "a backlog was waiting behind the latest table"
    );
}

/// Stopping the engine ends every live stream with a proper close, so the
/// page can say it is disconnected rather than wait forever.
#[tokio::test]
async fn the_stream_ends_when_the_watcher_stops() {
    let h = start(Fake::scripted(vec![vec![cand(
        "Wi-Fi",
        "192.0.2.10",
        true,
    )]]));
    let addr = serve_api(h.table()).await;
    let mut socket = connect(addr).await;
    frame(&mut socket).await;

    drop(h);
    let ended = timeout(Duration::from_secs(2), async {
        loop {
            match socket.next().await {
                Some(Ok(Message::Close(_))) => return true,
                Some(Ok(_)) => continue,
                // Ended without a close frame: the page cannot tell an engine
                // that stopped from a broken connection.
                Some(Err(_)) | None => return false,
            }
        }
    })
    .await;
    assert_eq!(
        ended,
        Ok(true),
        "no close frame within 2 s of the watcher stopping"
    );
}

/// A page on another site, open in this machine's browser, must not read
/// the links. A WebSocket gets none of the cross-site protection the browser
/// gives an ordinary request, so the server checks the Origin itself.
#[tokio::test]
async fn a_page_from_another_site_cannot_open_the_live_feed() {
    let h = start(Fake::scripted(vec![vec![]]));
    let addr = serve_api(h.table()).await;
    let mut request = format!("ws://{addr}/api/live")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("origin", "https://attacker.example".parse().unwrap());

    let refused = tokio_tungstenite::connect_async(request).await.unwrap_err();
    let tungstenite::Error::Http(response) = refused else {
        panic!("expected an HTTP refusal, got {refused:?}");
    };
    assert_eq!(response.status(), 403);
}

#[tokio::test]
async fn this_machines_own_page_can_open_it() {
    let h = start(Fake::scripted(vec![vec![]]));
    let addr = serve_api(h.table()).await;
    let mut request = format!("ws://{addr}/api/live")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("origin", format!("http://{addr}").parse().unwrap());

    let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    assert_eq!(frame(&mut socket).await["type"], "table");
}

/// DNS rebinding: a site points its own name at 127.0.0.1, and the browser
/// then treats the engine as part of that site. The Host header still
/// carries the site's name.
#[tokio::test]
async fn a_request_naming_another_host_is_refused() {
    let h = start(Fake::scripted(vec![vec![]]));
    let addr = serve_api(h.table()).await;

    let status = status_with_host(addr, "/api/links", "attacker.example").await;
    assert!(status.starts_with("HTTP/1.1 403"), "{status}");
}

/// The status line of a GET for `path` whose Host header names `host`.
/// Written by hand, since an HTTP client fills in Host from the address.
async fn status_with_host(addr: SocketAddr, path: &str, host: &str) -> String {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}:{}\r\nConnection: close\r\n\r\n",
        addr.port()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut reply = String::new();
    stream.read_to_string(&mut reply).await.unwrap();
    reply.lines().next().unwrap_or_default().to_string()
}

/// Browsers send no Origin on an `<img>`, `<script>` or frame load, or on a
/// navigation, so a page on another site can send GETs that pass the Host
/// and Origin checks. `Sec-Fetch-Site` names where those came from too.
#[tokio::test]
async fn an_image_tag_on_another_site_cannot_read_the_api() {
    let h = start(Fake::scripted(vec![vec![]]));
    let addr = serve_api(h.table()).await;

    for site in ["cross-site", "same-site"] {
        let response = reqwest::Client::new()
            .get(format!("http://{addr}/api/links"))
            .header("sec-fetch-site", site)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 403, "{site}");
    }
}

#[tokio::test]
async fn it_listens_on_loopback_only() {
    let listener = bind(0).await.unwrap();
    assert!(listener.local_addr().unwrap().ip().is_loopback());
}

/// Review Focus 4: `barrage` started twice, or the port taken by something
/// else. One line naming the port and the way out, not a panic.
#[tokio::test]
async fn a_taken_port_is_a_clear_error() {
    let first = bind(0).await.unwrap();
    let port = first.local_addr().unwrap().port();
    let err = format!("{:#}", bind(port).await.unwrap_err());
    assert!(
        err.contains(&port.to_string()) && err.contains("--port"),
        "{err}"
    );
}

/// A folder of one test's own, standing in for `engine/static` or the
/// download folder, removed when dropped.
struct Folder(PathBuf);

impl Folder {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("barrage-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn write(&self, file: &str, text: &str) {
        let path = self.0.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
}

impl Drop for Folder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Serve the whole app, page and API, on a free loopback port. Nothing is
/// downloaded.
async fn serve_app(table: LinkTable, dir: &Folder) -> SocketAddr {
    let listener = bind(0).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let downloads = downloads_in(&table, &std::env::temp_dir());
    let app = app(table, downloads, dir.0.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    addr
}

async fn get(url: String) -> (u16, String) {
    let response = reqwest::get(url).await.unwrap();
    (response.status().as_u16(), response.text().await.unwrap())
}

const PAGE: &str = "<h1>the page</h1>";

#[tokio::test]
async fn the_built_ui_is_served_with_a_fallback_to_index() {
    let dir = Folder::new("built");
    dir.write("index.html", PAGE);
    dir.write("assets/app.js", "console.log('app')");
    let h = start(Fake::scripted(vec![vec![]]));
    let addr = serve_app(h.table(), &dir).await;

    assert_eq!(get(format!("http://{addr}/")).await, (200, PAGE.into()));
    assert_eq!(
        get(format!("http://{addr}/assets/app.js")).await,
        (200, "console.log('app')".into())
    );
    // An address the page handles itself, opened directly or reloaded.
    assert_eq!(
        get(format!("http://{addr}/links")).await,
        (200, PAGE.into())
    );
    // The API still answers for itself.
    let (status, body) = get(format!("http://{addr}/api/links")).await;
    assert_eq!(status, 200);
    assert!(body.starts_with("{\"links\""), "{body}");
}

/// Review Focus 5: a fresh clone, the engine built and the page not. Building
/// it while the engine runs is enough: no restart.
#[tokio::test]
async fn an_unbuilt_ui_gets_a_page_saying_how_to_build_it() {
    let dir = Folder::new("unbuilt");
    let h = start(Fake::scripted(vec![vec![]]));
    let addr = serve_app(h.table(), &dir).await;

    let (status, body) = get(format!("http://{addr}/")).await;
    assert_eq!(status, 200);
    assert!(body.contains("pnpm") && body.contains("build"), "{body}");

    dir.write("index.html", PAGE);
    assert_eq!(get(format!("http://{addr}/")).await, (200, PAGE.into()));
}

/// What Ctrl+C does to `barrage`: every open page gets a close frame, so it
/// can say the engine stopped, and the server returns rather than waiting
/// on the pages.
#[tokio::test]
async fn stopping_closes_every_page_and_returns() {
    let dir = Folder::new("stopping");
    let listener = bind(0).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let h = start(Fake::scripted(vec![vec![cand(
        "Wi-Fi",
        "192.0.2.10",
        true,
    )]]));
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(serve(listener, h, dir.0.clone(), dir.0.clone(), async {
        let _ = stopped.await;
    }));
    let mut socket = connect(addr).await;
    frame(&mut socket).await;

    stop.send(()).unwrap();
    let closed = timeout(Duration::from_secs(2), async {
        loop {
            match socket.next().await {
                Some(Ok(Message::Close(_))) => return true,
                Some(Ok(_)) => continue,
                Some(Err(_)) | None => return false,
            }
        }
    })
    .await;
    assert_eq!(closed, Ok(true), "no close frame within 2 s of stopping");
    let returned = timeout(Duration::from_secs(2), server).await;
    assert!(
        matches!(returned, Ok(Ok(Ok(())))),
        "the server did not return within 2 s: {returned:?}"
    );
}

/// The page is under the same rule as the API: this machine's own pages
/// only, so DNS rebinding gets nothing from it either.
#[tokio::test]
async fn the_page_answers_only_this_machine_too() {
    let dir = Folder::new("guarded");
    dir.write("index.html", PAGE);
    let h = start(Fake::scripted(vec![vec![]]));
    let addr = serve_app(h.table(), &dir).await;

    let status = status_with_host(addr, "/", "attacker.example").await;
    assert!(status.starts_with("HTTP/1.1 403"), "{status}");
    assert_eq!(get(format!("http://{addr}/")).await, (200, PAGE.into()));
}

/// Another site's page must not show the dashboard inside a frame of its
/// own, where a disguised click could press the dashboard's buttons. Every
/// answer says so, refusals included.
#[tokio::test]
async fn every_answer_forbids_framing() {
    let dir = Folder::new("framing");
    dir.write("index.html", PAGE);
    let h = start(Fake::scripted(vec![vec![]]));
    let addr = serve_app(h.table(), &dir).await;
    let client = reqwest::Client::new();

    let answers = [
        ("the page", client.get(format!("http://{addr}/")), 200),
        (
            "the API",
            client.get(format!("http://{addr}/api/links")),
            200,
        ),
        (
            "a refusal",
            client
                .get(format!("http://{addr}/api/links"))
                .header("sec-fetch-site", "cross-site"),
            403,
        ),
        (
            "an unknown API route",
            client.get(format!("http://{addr}/api/link")),
            404,
        ),
    ];
    for (what, request, status) in answers {
        let response = request.send().await.unwrap();
        assert_eq!(response.status(), status, "{what}");
        let header = |name| response.headers().get(name)?.to_str().ok();
        assert_eq!(header("x-frame-options"), Some("DENY"), "{what}");
        assert_eq!(
            header("content-security-policy"),
            Some("frame-ancestors 'none'"),
            "{what}"
        );
    }
}

/// A typo, or a route a newer page expects from an older engine, must say
/// there is no such route. Answering 200 with the page leaves a JSON client
/// with a parse error instead.
#[tokio::test]
async fn an_unknown_api_route_is_404_not_the_page() {
    let dir = Folder::new("api-404");
    dir.write("index.html", PAGE);
    let h = start(Fake::scripted(vec![vec![]]));
    let addr = serve_app(h.table(), &dir).await;

    for path in ["/api/link", "/api", "/api/", "/api/links/extra"] {
        let (status, body) = get(format!("http://{addr}{path}")).await;
        assert_eq!(status, 404, "{path}");
        let json: Value = serde_json::from_str(&body).unwrap_or_else(|_| panic!("{path}: {body}"));
        assert!(json["error"].is_string(), "{path}: {body}");
    }
    // An address the page routes for itself still gets the page.
    assert_eq!(
        get(format!("http://{addr}/some/view")).await,
        (200, PAGE.into())
    );
}

/// `POST /api/downloads` with `body` as JSON: the status and the answer.
async fn post(addr: SocketAddr, body: Value) -> (u16, Value) {
    post_text(addr, &body.to_string()).await
}

/// The same, with the body exactly as written.
async fn post_text(addr: SocketAddr, body: &str) -> (u16, Value) {
    let response = reqwest::Client::new()
        .post(format!("http://{addr}/api/downloads"))
        .header("content-type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .unwrap();
    let status = response.status().as_u16();
    let text = response.text().await.unwrap();
    let json = serde_json::from_str(&text).unwrap_or_else(|_| panic!("not JSON: {text}"));
    (status, json)
}

/// `GET /api/downloads`.
async fn listed(addr: SocketAddr) -> Value {
    let (status, body) = get(format!("http://{addr}/api/downloads")).await;
    assert_eq!(status, 200, "{body}");
    serde_json::from_str(&body).unwrap()
}

/// `GET /api/downloads` until it passes `check`, failing the test after 10 s.
async fn listed_where(addr: SocketAddr, what: &str, check: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let list = listed(addr).await;
        if check(&list) {
            return list;
        }
        assert!(Instant::now() < deadline, "waited 10 s for {what}: {list}");
        tokio::time::sleep(ms(20)).await;
    }
}

const SONG: u64 = 2 << 20;

/// The whole road: a URL posted, listed, its progress on the live socket
/// with the live speed while it downloads, and the file in the folder.
#[tokio::test]
async fn a_posted_url_is_listed_and_streamed() {
    let files = Server::start().await;
    // 1 MiB a second per connection: half a second of downloading to see.
    files.at("/song.flac", Answer::File(File::new(SONG).rate(1 << 20)));
    let h = wifi_to_the_file_server();
    let dir = Folder::new("posted");
    let addr = serve_with(h.table(), downloads_in(&h.table(), &dir.0)).await;
    let mut socket = connect(addr).await;
    let (_, before) = opening(&mut socket).await;
    assert_eq!(before["downloads"]["list"], json!([]));

    let url = files.url("/song.flac").to_string();
    assert_eq!(
        post(addr, json!({ "url": url })).await,
        (201, json!({ "id": 1 }))
    );
    let list = listed(addr).await;
    assert_eq!(list["list"][0]["id"], 1);
    assert_eq!(list["list"][0]["url"], url.as_str());

    let downloading = frame_where(&mut socket, "Wi-Fi's live speed", |f| {
        f["downloads"]["list"][0]["state"] == "downloading"
            && f["downloads"]["live"]["bps"].as_u64() > Some(0)
    })
    .await;
    let live = &downloading["downloads"]["live"];
    assert_eq!(live["parts"][0]["name"], "Wi-Fi", "{live}");
    assert_eq!(live["parts"][0]["bps"], live["bps"], "{live}");

    let done = frame_where(&mut socket, "the download done", |f| {
        f["downloads"]["list"][0]["state"] == "done"
    })
    .await;
    let download = &done["downloads"]["list"][0];
    assert_eq!(download["name"], "song.flac");
    assert_eq!(
        (&download["length"], &download["done"]),
        (&json!(SONG), &json!(SONG))
    );
    assert_eq!(download["cables"][0]["name"], "Wi-Fi");
    assert_eq!(download["cables"][0]["bytes"], SONG);
    assert_eq!(done["downloads"]["live"], json!({ "bps": 0, "parts": [] }));
    assert_eq!(
        std::fs::read(dir.0.join("song.flac")).unwrap(),
        content(0..SONG)
    );

    // Once done, the same URL starts again, as a second copy.
    assert_eq!(
        post(addr, json!({ "url": url })).await,
        (201, json!({ "id": 2 }))
    );
}

/// Review Focus 3: a URL already downloading answers with the download it
/// is, rather than starting a rival. No link is up, so it waits.
#[tokio::test]
async fn the_same_url_twice_is_409_with_the_running_id() {
    let h = start(Fake::scripted(vec![vec![]]));
    let addr = serve_api(h.table()).await;
    let url = "http://example.com/files/a.iso";
    assert_eq!(
        post(addr, json!({ "url": url })).await,
        (201, json!({ "id": 1 }))
    );

    // The host's case, the default port, the fragment and spaces around it
    // do not make it another file; nor does a choice of cables.
    let again = [
        json!({ "url": url }),
        json!({ "url": "HTTP://EXAMPLE.COM:80/files/a.iso" }),
        json!({ "url": "http://example.com/files/a.iso#part-2" }),
        json!({ "url": format!("  {url}  ") }),
        json!({ "url": url, "cables": ["Wi-Fi"] }),
    ];
    for body in again {
        let (status, answer) = post(addr, body.clone()).await;
        assert_eq!(status, 409, "{body}");
        assert_eq!(answer["id"], 1, "{body}");
        let error = answer["error"].as_str().unwrap_or_default();
        assert!(error.contains("already downloading"), "{body}: {error}");
    }

    // Another path or query is another file.
    let other = "http://example.com/files/b.iso";
    assert_eq!(
        post(addr, json!({ "url": other })).await,
        (201, json!({ "id": 2 }))
    );
    let query = "http://example.com/files/a.iso?v=2";
    assert_eq!(
        post(addr, json!({ "url": query })).await,
        (201, json!({ "id": 3 }))
    );
    let list = listed(addr).await;
    let urls: Vec<&str> = list["list"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["url"].as_str().unwrap())
        .collect();
    assert_eq!(urls, [url, other, query]);
}

/// Only a download still under way blocks its URL. One that failed can be
/// tried again.
#[tokio::test]
async fn a_url_that_failed_can_be_added_again() {
    let files = Server::start().await;
    let h = wifi_to_the_file_server();
    let dir = Folder::new("failed-again");
    let addr = serve_with(h.table(), downloads_in(&h.table(), &dir.0)).await;
    let url = files.url("/missing.bin").to_string();

    assert_eq!(
        post(addr, json!({ "url": url })).await,
        (201, json!({ "id": 1 }))
    );
    listed_where(addr, "the download failed", |l| {
        l["list"][0]["state"] == "failed"
    })
    .await;
    assert_eq!(
        post(addr, json!({ "url": url })).await,
        (201, json!({ "id": 2 }))
    );
}

/// The three kinds of body an HTML form can send to another site without
/// asking first, and none at all. A page that tries must use `fetch` with
/// JSON, which sends an Origin the guard checks.
#[tokio::test]
async fn only_json_is_accepted() {
    let h = start(Fake::scripted(vec![vec![]]));
    let addr = serve_api(h.table()).await;
    let client = reqwest::Client::new();
    let url = format!("http://{addr}/api/downloads");
    let json_text = r#"{"url": "http://example.com/a.iso"}"#;

    let forms = [
        Some(("text/plain", json_text)),
        Some((
            "application/x-www-form-urlencoded",
            "url=http%3A%2F%2Fexample.com%2Fa.iso",
        )),
        Some((
            "multipart/form-data; boundary=x",
            "--x\r\nContent-Disposition: form-data; name=\"url\"\r\n\r\nhttp://example.com/a.iso\r\n--x--\r\n",
        )),
        None,
    ];
    for form in forms {
        let mut request = client.post(&url).body(form.map_or(json_text, |f| f.1));
        if let Some((kind, _)) = form {
            request = request.header("content-type", kind);
        }
        let response = request.send().await.unwrap();
        assert_eq!(response.status(), 415, "{form:?}");
        let answer: Value = serde_json::from_str(&response.text().await.unwrap()).unwrap();
        let error = answer["error"].as_str().unwrap_or_default();
        assert!(error.contains("JSON"), "{form:?}: {error}");
    }
    assert_eq!(listed(addr).await["list"], json!([]));
}

#[tokio::test]
async fn only_http_and_https_urls() {
    let h = start(Fake::scripted(vec![vec![]]));
    let addr = serve_api(h.table()).await;

    let refused = [
        ("ftp://example.com/x", "not ftp:"),
        ("javascript:alert(1)", "not javascript:"),
        ("file:///C:/Windows/win.ini", "not file:"),
        ("not a url", "\"not a url\" is not a web address"),
        ("http://", "\"http://\" is not a web address"),
        ("", "\"\" is not a web address"),
    ];
    for (url, why) in refused {
        let (status, answer) = post(addr, json!({ "url": url })).await;
        assert_eq!(status, 400, "{url}");
        let error = answer["error"].as_str().unwrap_or_default();
        assert!(error.contains(why), "{url}: {error}");
    }
    assert_eq!(listed(addr).await["list"], json!([]));
}

/// A body the engine cannot use says why, in JSON like every other refusal.
/// `cable` for `cables` is refused rather than ignored: ignored, the
/// download would quietly use every cable.
#[tokio::test]
async fn a_body_that_cannot_be_read_is_400_and_says_why() {
    let h = start(Fake::scripted(vec![vec![]]));
    let addr = serve_api(h.table()).await;

    let refused = [
        (r#"{"url": "#, "JSON"),
        (r#"{}"#, "url"),
        (r#"{"url": 7}"#, "url"),
        (
            r#"{"url": "http://example.com/a.iso", "cable": ["Wi-Fi"]}"#,
            "cable",
        ),
        (
            r#"{"url": "http://example.com/a.iso", "cables": []}"#,
            "cables",
        ),
    ];
    for (body, why) in refused {
        let (status, answer) = post_text(addr, body).await;
        assert_eq!(status, 400, "{body}");
        let error = answer["error"].as_str().unwrap_or_default();
        assert!(error.contains(why), "{body}: {error}");
    }
    assert_eq!(listed(addr).await["list"], json!([]));
}

/// The guard covers the new route: another site's page cannot start a
/// download, with or without an Origin. This machine's own page can.
#[tokio::test]
async fn another_sites_page_cannot_add_a_download() {
    let h = start(Fake::scripted(vec![vec![]]));
    let addr = serve_api(h.table()).await;
    let client = reqwest::Client::new();
    let send = |headers: Vec<(&'static str, String)>| {
        let mut request = client
            .post(format!("http://{addr}/api/downloads"))
            .header("content-type", "application/json")
            .body(r#"{"url": "http://example.com/a.iso"}"#);
        for (name, value) in headers {
            request = request.header(name, value);
        }
        request.send()
    };

    let foreign = [
        ("origin", "https://attacker.example".to_string()),
        ("origin", "http://127.0.0.1:9999".to_string()),
        ("sec-fetch-site", "cross-site".to_string()),
        ("sec-fetch-site", "same-site".to_string()),
    ];
    for header in foreign {
        let response = send(vec![header.clone()]).await.unwrap();
        assert_eq!(response.status(), 403, "{header:?}");
    }
    assert_eq!(listed(addr).await["list"], json!([]));

    let own = vec![
        ("origin", format!("http://{addr}")),
        ("sec-fetch-site", "same-origin".to_string()),
    ];
    assert_eq!(send(own).await.unwrap().status(), 201);
}

/// A download publishes on every tick, but the page is sent only what
/// changed: one waiting for a cable sends one message, not fifty a second.
#[tokio::test]
async fn a_download_that_does_not_move_sends_nothing_new() {
    let h = start(Fake::scripted(vec![vec![]]));
    let addr = serve_api(h.table()).await;
    let mut socket = connect(addr).await;
    opening(&mut socket).await;

    assert_eq!(
        post(addr, json!({ "url": "http://example.com/a.iso" })).await,
        (201, json!({ "id": 1 }))
    );
    frame_where(&mut socket, "the download waiting", |f| {
        f["downloads"]["list"][0]["state"] == "waiting"
    })
    .await;
    let next = timeout(ms(500), socket.next()).await;
    assert!(next.is_err(), "sent though nothing changed: {next:?}");
}
