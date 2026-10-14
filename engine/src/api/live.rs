//! The live WebSocket: the whole table when a page connects, then the table
//! again each time it changes, with the lines saying what changed.
//!
//! It follows the table itself, not the event channel. A speed, a public
//! address or an RTT arriving changes the table without any `LinkEvent`, and
//! the page has to show them all.

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;

use super::view::{LiveMessage, change_message, table_view};
use crate::links::Link;
use crate::links::watch::LinkTable;

/// What one client is sent, in order.
pub struct Feed {
    table: LinkTable,
    /// The table as this client last saw it, once it has seen one.
    shown: Option<Vec<Link>>,
}

impl Feed {
    pub fn new(table: LinkTable) -> Self {
        Self { table, shown: None }
    }

    /// The next message for this client: the whole table the first time,
    /// then a change each time the table moves. `None` once the watcher has
    /// stopped.
    ///
    /// Whatever changes while the client is still busy with its last message
    /// is merged into the next one, so a slow client is never sent a backlog
    /// and costs the server no queue.
    ///
    /// Cancel safe: the only wait is for the table to change, so a
    /// `select!` may drop this future without losing anything.
    pub async fn next(&mut self) -> Option<LiveMessage> {
        let Some(shown) = &self.shown else {
            let now = self.table.latest();
            let table = table_view(&now);
            self.shown = Some(now);
            return Some(LiveMessage::Table { table });
        };
        if !self.table.changed().await {
            return None;
        }
        let now = self.table.latest();
        let message = change_message(shown, &now);
        self.shown = Some(now);
        Some(message)
    }
}

/// `GET /api/live`.
pub(super) async fn upgrade(ws: WebSocketUpgrade, State(table): State<LinkTable>) -> Response {
    ws.on_upgrade(move |socket| stream(socket, Feed::new(table)))
}

async fn stream(mut socket: WebSocket, mut feed: Feed) {
    tracing::debug!("live client connected");
    loop {
        tokio::select! {
            message = feed.next() => {
                let Some(message) = message else {
                    // The engine is stopping. A proper close lets the page say
                    // so, rather than guess at a broken connection.
                    let _ = socket.send(Message::Close(None)).await;
                    tracing::debug!("live client closed: the watcher stopped");
                    return;
                };
                let text = match serde_json::to_string(&message) {
                    Ok(text) => text,
                    Err(e) => {
                        tracing::error!(error = %e, "a live message did not serialise");
                        return;
                    }
                };
                if socket.send(Message::Text(text.into())).await.is_err() {
                    tracing::debug!("live client gone");
                    return;
                }
            }
            incoming = socket.recv() => match incoming {
                // Pages send nothing; the library answers pings by itself.
                Some(Ok(Message::Close(_)) | Err(_)) | None => {
                    tracing::debug!("live client left");
                    return;
                }
                Some(Ok(_)) => {}
            },
        }
    }
}
