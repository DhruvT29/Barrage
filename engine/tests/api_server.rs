//! The API server, driven by the scripted network in `fake/mod.rs`: the JSON
//! route, the live WebSocket, the page, the port, and who may use them.

mod fake;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use barrage::LinkHandle;
use barrage::api::live::Feed;
use barrage::api::{LiveMessage, app, bind, router, serve, table_view};
use barrage::links::LinkState;
use barrage::links::watch::{LinkTable, WatchConfig};
use fake::{Fake, SAMPLED_BPS, cand, ms};
use futures_util::StreamExt;
use serde_json::{Value, json};
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

/// Serve the API for `table` on a free loopback port.
async fn serve_api(table: LinkTable) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router(table)).await.unwrap() });
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

/// Frames until one passes `check`, failing the test after 3 s.
async fn frame_where(socket: &mut Socket, what: &str, check: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        assert!(Instant::now() < deadline, "waited 3 s for {what}");
        let frame = frame(socket).await;
        if check(&frame) {
            return frame;
        }
    }
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
async fn live_sends_the_table_first_then_each_change() {
    let mut steps = vec![vec![]; 20];
    steps.push(vec![cand("Wi-Fi", "192.0.2.10", true)]);
    let h = start(Fake::scripted(steps));
    let addr = serve_api(h.table()).await;
    let mut socket = connect(addr).await;

    let first = frame(&mut socket).await;
    assert_eq!(first["type"], "table");
    assert_eq!(first["table"]["links"], json!([]));

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
    frame(&mut socket).await;

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
    frame(&mut leaving).await;
    frame(&mut staying).await;

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
    let mut feed = Feed::new(h.table());
    let Some(LiveMessage::Table { .. }) = feed.next().await else {
        panic!("expected the table first");
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

/// A directory standing in for `engine/static`, removed when dropped.
struct StaticDir(PathBuf);

impl StaticDir {
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

impl Drop for StaticDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Serve the whole app, page and API, on a free loopback port.
async fn serve_app(table: LinkTable, dir: &StaticDir) -> SocketAddr {
    let listener = bind(0).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = app(table, dir.0.clone());
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
    let dir = StaticDir::new("built");
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
    let dir = StaticDir::new("unbuilt");
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
    let dir = StaticDir::new("stopping");
    let listener = bind(0).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let h = start(Fake::scripted(vec![vec![cand(
        "Wi-Fi",
        "192.0.2.10",
        true,
    )]]));
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(serve(listener, h, dir.0.clone(), async {
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
    let dir = StaticDir::new("guarded");
    dir.write("index.html", PAGE);
    let h = start(Fake::scripted(vec![vec![]]));
    let addr = serve_app(h.table(), &dir).await;

    let status = status_with_host(addr, "/", "attacker.example").await;
    assert!(status.starts_with("HTTP/1.1 403"), "{status}");
    assert_eq!(get(format!("http://{addr}/")).await, (200, PAGE.into()));
}
