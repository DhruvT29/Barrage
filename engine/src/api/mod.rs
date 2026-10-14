//! The engine's face to the browser: what the UI reads, and the server that
//! sends it.
//!
//! The UI is a separate program in another language, so the JSON here is a
//! contract. `view` decides what a link looks like on the wire and is pure,
//! like `links::render` is for the terminal. `live` streams it, and `guard`
//! keeps every route to this machine's own pages.

pub mod guard;
pub mod live;
pub mod view;

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router, middleware};

use crate::links::watch::LinkTable;

pub use view::{LinkView, LiveMessage, TableView, change_message, table_view};

/// The API: `GET /api/links`, the table as JSON, and `GET /api/live`, the
/// table as a WebSocket stream of [`LiveMessage`]s. Every route answers only
/// this machine's own pages ([`guard`]).
///
/// It holds a copy of the table, never the
/// [`LinkHandle`](crate::LinkHandle). A reader does not keep the watcher
/// alive, so stopping the engine ends every live stream, instead of each
/// open page keeping the engine running.
pub fn router(table: LinkTable) -> Router {
    Router::new()
        .route("/api/links", get(links))
        .route("/api/live", get(live::upgrade))
        .layer(middleware::from_fn(guard::same_machine_only))
        .with_state(table)
}

async fn links(State(table): State<LinkTable>) -> Json<TableView> {
    Json(table_view(&table.snapshot()))
}
