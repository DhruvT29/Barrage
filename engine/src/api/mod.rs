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
use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::extract::{FromRef, Path, Request, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{any, get, post};
use axum::{Json, Router, middleware};
use serde::Deserialize;
use serde_json::json;
use tokio::net::TcpListener;
use tower_http::services::{ServeDir, ServeFile};

use crate::LinkHandle;
use crate::fetch::{AddError, AddRequest, ControlError, DownloadId, Downloads};
use crate::links::watch::LinkTable;

pub use view::{
    DownloadsView, LinkView, LiveMessage, TableView, change_message, downloads_view, table_view,
};

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

/// The API alone:
///
/// - `GET /api/links`: the table as JSON.
/// - `GET /api/live`: the table and the downloads as a WebSocket stream of
///   [`LiveMessage`]s.
/// - `POST /api/downloads`: start one. The body is JSON, `{"url": string,
///   "cables"?: string[]}`, sent as `application/json`. It answers `201
///   {"id"}` (a stopped download of the URL is resumed, and its own id
///   given), `400 {"error"}` for a body or URL it cannot use, `409 {"error",
///   "id"}` for a URL already downloading, with that download's id, and `415
///   {"error"}` for anything not JSON. A form on another site can only send
///   the kinds that last refuses, and anything else from a page needs the
///   browser to ask first, which this server never answers.
/// - `GET /api/downloads`: every download, as [`DownloadsView`].
/// - `POST /api/downloads/{id}/stop` and `POST /api/downloads/{id}/resume`:
///   stop that download, its progress saved, or resume it from there. The
///   body is `{}`, as `application/json`, for the same reason as adding's.
///   They answer `200 {"id"}` once it has stopped or started again, `404
///   {"error"}` for no such download, `409 {"error"}` for one not running or
///   not stopped, and `415` or `400` as adding does.
///
/// Any other address under `/api` answers `404`. Every route answers only
/// this machine's own pages, and none may be framed ([`guard`]).
///
/// It holds a copy of the table, never the [`LinkHandle`]. A reader does not
/// keep the watcher alive, so stopping the engine ends every live stream,
/// instead of each open page keeping the engine running.
pub fn router(table: LinkTable, downloads: Downloads) -> Router {
    guarded(api(table, downloads))
}

/// What `barrage` serves: the API, as [`router`], and the built page from
/// `static_dir` at every other address, all under the same [`guard`].
pub fn app(table: LinkTable, downloads: Downloads, static_dir: PathBuf) -> Router {
    let dir = Arc::new(static_dir);
    guarded(api(table, downloads).fallback(move |request: Request| page(Arc::clone(&dir), request)))
}

/// How long the live streams get to send their close frames once the
/// server has finished, so open pages hear that the engine stopped rather
/// than see the connection break.
const CLOSE_GRACE: Duration = Duration::from_millis(250);

/// How long the downloads get to stop and save their progress when the
/// engine stops.
const SAVE_GRACE: Duration = Duration::from_secs(2);

/// Serve [`app`] on `listener` until `stop` completes, then stop the engine.
/// Downloads go to `downloads_dir`.
///
/// When `stop` fires, the downloads stop first and save their progress, for
/// at most [`SAVE_GRACE`]: with the links gone first, each would see its
/// links vanish and fail. Then the handle is dropped, while the server
/// finishes. That stops the watcher, which ends every live stream with a
/// close frame. Dropping it after the server returns works too with axum
/// 0.8.9, whose graceful shutdown does not wait for connections that became
/// WebSockets (checked on 2026-10-14 by swapping the order: the test still
/// passed). This order does not depend on that.
pub async fn serve(
    listener: TcpListener,
    links: LinkHandle,
    downloads_dir: PathBuf,
    static_dir: PathBuf,
    stop: impl Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    let downloads = Downloads::new(links.table(), downloads_dir);
    let app = app(links.table(), downloads.clone(), static_dir);
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            stop.await;
            let _ = tokio::time::timeout(SAVE_GRACE, downloads.shutdown()).await;
            drop(links);
        })
        .await
        .context("the server stopped")?;
    tokio::time::sleep(CLOSE_GRACE).await;
    Ok(())
}

/// What the routes reach. Each handler takes only the half it needs.
#[derive(Clone)]
struct Engine {
    table: LinkTable,
    downloads: Downloads,
}

impl FromRef<Engine> for LinkTable {
    fn from_ref(engine: &Engine) -> Self {
        engine.table.clone()
    }
}

impl FromRef<Engine> for Downloads {
    fn from_ref(engine: &Engine) -> Self {
        engine.downloads.clone()
    }
}

fn api(table: LinkTable, downloads: Downloads) -> Router {
    Router::new()
        .route("/api/links", get(links))
        .route("/api/live", get(live::upgrade))
        .route("/api/downloads", get(list_downloads).post(add_download))
        .route("/api/downloads/{id}/stop", post(stop_download))
        .route("/api/downloads/{id}/resume", post(resume_download))
        // Any other address under /api is a route this engine does not have,
        // a typo or one a newer page expects. Left to the page's fallback it
        // would answer 200 with index.html, which a JSON client cannot read.
        .route("/api", any(no_such_route))
        .route("/api/", any(no_such_route))
        .route("/api/{*rest}", any(no_such_route))
        .with_state(Engine { table, downloads })
}

async fn no_such_route() -> Response {
    refuse(StatusCode::NOT_FOUND, "no such API route")
}

/// A refusal, in the JSON every API route answers with.
fn refuse(status: StatusCode, error: &str) -> Response {
    (status, Json(json!({ "error": error }))).into_response()
}

/// The body of `POST /api/downloads`. A field it does not know is refused,
/// not ignored: `cable` for `cables`, ignored, would quietly download through
/// every cable.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AddBody {
    url: String,
    cables: Option<Vec<String>>,
}

async fn add_download(
    State(downloads): State<Downloads>,
    body: Result<Json<AddBody>, JsonRejection>,
) -> Response {
    let body = match body {
        Ok(Json(body)) => body,
        Err(JsonRejection::MissingJsonContentType(_)) => {
            return refuse(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "send the download as JSON, with Content-Type: application/json",
            );
        }
        Err(e) => return refuse(StatusCode::BAD_REQUEST, &e.body_text()),
    };
    let request = AddRequest {
        url: body.url,
        cables: body.cables,
    };
    match downloads.add(request).await {
        Ok(id) => (StatusCode::CREATED, Json(json!({ "id": id.as_u64() }))).into_response(),
        Err(e @ AddError::AlreadyRunning(id)) => (
            StatusCode::CONFLICT,
            Json(json!({ "error": e.to_string(), "id": id.as_u64() })),
        )
            .into_response(),
        Err(e) => refuse(StatusCode::BAD_REQUEST, &e.to_string()),
    }
}

/// The body of a stop or a resume: `{}`. There is nothing to say, but asking
/// for JSON keeps a form on another site out, as for adding.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NoFields {}

async fn stop_download(
    State(downloads): State<Downloads>,
    id: Result<Path<u64>, PathRejection>,
    body: Result<Json<NoFields>, JsonRejection>,
) -> Response {
    match asked(id, body) {
        Ok(id) => controlled(id, downloads.stop(id).await),
        Err(refused) => *refused,
    }
}

async fn resume_download(
    State(downloads): State<Downloads>,
    id: Result<Path<u64>, PathRejection>,
    body: Result<Json<NoFields>, JsonRejection>,
) -> Response {
    match asked(id, body) {
        Ok(id) => controlled(id, downloads.resume(id).await),
        Err(refused) => *refused,
    }
}

/// The download a stop or resume names, once its body is `{}` as JSON, or
/// the refusal. An id that is not a number names no download.
fn asked(
    id: Result<Path<u64>, PathRejection>,
    body: Result<Json<NoFields>, JsonRejection>,
) -> Result<DownloadId, Box<Response>> {
    let Ok(Path(id)) = id else {
        return Err(Box::new(refuse(StatusCode::NOT_FOUND, "no such download")));
    };
    match body {
        Ok(_) => Ok(DownloadId::new(id)),
        Err(JsonRejection::MissingJsonContentType(_)) => Err(Box::new(refuse(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "send {} as JSON, with Content-Type: application/json",
        ))),
        Err(e) => Err(Box::new(refuse(StatusCode::BAD_REQUEST, &e.body_text()))),
    }
}

/// What a stop or resume of download `id` answers.
fn controlled(id: DownloadId, done: Result<(), ControlError>) -> Response {
    match done {
        Ok(()) => (StatusCode::OK, Json(json!({ "id": id.as_u64() }))).into_response(),
        Err(e @ ControlError::NoSuchDownload) => refuse(StatusCode::NOT_FOUND, &e.to_string()),
        Err(e) => refuse(StatusCode::CONFLICT, &e.to_string()),
    }
}

async fn list_downloads(State(downloads): State<Downloads>) -> Json<DownloadsView> {
    Json(downloads_view(
        &downloads.status().borrow(),
        &downloads.used().borrow(),
    ))
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
