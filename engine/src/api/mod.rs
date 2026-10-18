//! The engine's face to the browser: what the UI reads, and the server that
//! sends it.
//!
//! The UI is a separate program in another language, so the JSON here is a
//! contract. `view` decides what a link looks like on the wire and is pure,
//! like `links::render` is for the terminal. `live` streams it, and `guard`
//! keeps every route, the page included, to this machine's own pages and out
//! of other pages' frames.

pub mod guard;
pub mod live;
pub mod view;

use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{any, get};
use axum::{Json, Router, middleware};
use tokio::net::TcpListener;
use tower_http::services::{ServeDir, ServeFile};

use crate::LinkHandle;
use crate::links::watch::LinkTable;

pub use view::{LinkView, LiveMessage, TableView, change_message, table_view};

/// The port `barrage` listens on unless told otherwise.
pub const DEFAULT_PORT: u16 = 7700;

/// Listen on `port` at this machine's loopback address, never at an address
/// another device could reach. Port 0 lets the system pick a free one.
pub async fn bind(port: u16) -> anyhow::Result<TcpListener> {
    let loopback = Ipv4Addr::LOCALHOST;
    TcpListener::bind((loopback, port)).await.with_context(|| {
        format!(
            "cannot listen on {loopback}:{port}. Is barrage already running? \
             Start it with --port and another number"
        )
    })
}

/// The API alone: `GET /api/links`, the table as JSON, and `GET /api/live`,
/// the table as a WebSocket stream of [`LiveMessage`]s. Any other address
/// under `/api` answers `404`. Every route answers only this machine's own
/// pages, and none may be framed ([`guard`]).
///
/// It holds a copy of the table, never the [`LinkHandle`]. A reader does not
/// keep the watcher alive, so stopping the engine ends every live stream,
/// instead of each open page keeping the engine running.
pub fn router(table: LinkTable) -> Router {
    guarded(api(table))
}

/// What `barrage` serves: the API, as [`router`], and the built page from
/// `static_dir` at every other address, all under the same [`guard`].
pub fn app(table: LinkTable, static_dir: PathBuf) -> Router {
    let dir = Arc::new(static_dir);
    guarded(api(table).fallback(move |request: Request| page(Arc::clone(&dir), request)))
}

/// How long the live streams get to send their close frames once the
/// server has finished, so open pages hear that the engine stopped rather
/// than see the connection break.
const CLOSE_GRACE: Duration = Duration::from_millis(250);

/// Serve [`app`] on `listener` until `stop` completes, then stop the engine.
///
/// The handle is dropped the moment `stop` fires, while the server finishes.
/// That stops the watcher, which ends every live stream with a close frame.
/// Dropping it after the server returns works too with axum 0.8.9, whose
/// graceful shutdown does not wait for connections that became WebSockets
/// (checked on 2026-10-14 by swapping the order: the test still passed).
/// This order does not depend on that.
pub async fn serve(
    listener: TcpListener,
    links: LinkHandle,
    static_dir: PathBuf,
    stop: impl Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    let app = app(links.table(), static_dir);
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            stop.await;
            drop(links);
        })
        .await
        .context("the server stopped")?;
    tokio::time::sleep(CLOSE_GRACE).await;
    Ok(())
}

fn api(table: LinkTable) -> Router {
    Router::new()
        .route("/api/links", get(links))
        .route("/api/live", get(live::upgrade))
        // Any other address under /api is a route this engine does not have,
        // a typo or one a newer page expects. Left to the page's fallback it
        // would answer 200 with index.html, which a JSON client cannot read.
        .route("/api", any(no_such_route))
        .route("/api/", any(no_such_route))
        .route("/api/{*rest}", any(no_such_route))
        .with_state(table)
}

async fn no_such_route() -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({ "error": "no such API route" })),
    )
}

fn guarded(router: Router) -> Router {
    router.layer(middleware::from_fn(guard::same_machine_only))
}

async fn links(State(table): State<LinkTable>) -> Json<TableView> {
    Json(table_view(&table.snapshot()))
}

/// Anything that is not the API: one of the page's files, the page itself
/// for any other address (it routes for itself, so a reload of one of its
/// views must find it), or, before it is built, a note saying how to build
/// it. Checked on every request, so building the page while the engine runs
/// needs only a reload.
async fn page(dir: Arc<PathBuf>, request: Request) -> Response {
    let index = dir.join("index.html");
    if !tokio::fs::try_exists(&index).await.unwrap_or(false) {
        return Html(NOT_BUILT).into_response();
    }
    let mut files = ServeDir::new(dir.as_path()).fallback(ServeFile::new(index));
    match files.try_call(request).await {
        Ok(response) => response.into_response(),
        Err(e) => {
            tracing::warn!(error = %e, "could not read the page's files");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Could not read the page's files.\n",
            )
                .into_response()
        }
    }
}

const NOT_BUILT: &str = r#"<!doctype html>
<html lang="en">
<meta charset="utf-8">
<title>BARRAGE</title>
<h1>BARRAGE is running, but its page is not built yet</h1>
<p>Build it once, from the repository's root folder:</p>
<pre>pnpm --dir ui install
pnpm --dir ui build</pre>
<p>Then reload this page. The engine does not need a restart.</p>
<p>The API is already answering: <a href="/api/links">/api/links</a>.</p>
</html>
"#;
